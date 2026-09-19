use std::time::Duration;

use alloy_primitives::B256;
use anyhow::{Context, Result, ensure};
use base_consensus_rpc::{AdminApiClient, BaseP2PApiClient};
use jsonrpsee::{
    core::client::ClientT,
    http_client::{HttpClient, HttpClientBuilder},
    rpc_params,
};
use tokio::sync::mpsc;
use tracing::warn;
use url::Url;

use super::PausedPeers;
use crate::config::ConductorNodeConfig;

/// Timeout used when polling `admin_sequencerActive` during convergence checks.
pub const SEQUENCER_ACTIVE_RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Returns whether the consensus node reports the sequencer as active.
pub async fn fetch_sequencer_active(cl_rpc: &Url) -> Result<bool> {
    let client = HttpClientBuilder::default()
        .request_timeout(SEQUENCER_ACTIVE_RPC_TIMEOUT)
        .build(cl_rpc.as_str())
        .with_context(|| format!("building consensus admin client for {cl_rpc}"))?;
    AdminApiClient::admin_sequencer_active(&client)
        .await
        .with_context(|| format!("calling admin_sequencerActive on {cl_rpc}"))
}

/// Starts the sequencer via the consensus node's `admin_startSequencer` RPC.
pub async fn start_sequencer(cl_rpc: &Url, unsafe_head: B256) -> Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(5);

    ensure!(unsafe_head != B256::ZERO, "unsafe head must not be zero");
    let client = HttpClientBuilder::default()
        .request_timeout(TIMEOUT)
        .build(cl_rpc.as_str())
        .with_context(|| format!("building consensus admin client for {cl_rpc}"))?;
    AdminApiClient::admin_start_sequencer(&client, unsafe_head)
        .await
        .with_context(|| format!("calling admin_startSequencer on {cl_rpc}"))
}

/// Stops the sequencer via the consensus node's `admin_stopSequencer` RPC.
///
/// Returns the unsafe head hash captured at the moment the sequencer stopped.
/// A returned [`B256::ZERO`] means the sequencer was stopped but the captured
/// head is unavailable; it is not a valid restart point and must not be reused.
/// Because the RPC has already taken effect by the time this returns, surface
/// the missing head as a warning rather than an error so callers do not retry a
/// successful stop.
pub async fn stop_sequencer(cl_rpc: &Url) -> Result<B256> {
    // `admin_stopSequencer` may defer its response until the seal pipeline
    // finishes and the final unsafe head is known.
    const TIMEOUT: Duration = Duration::from_secs(60);

    let client = HttpClientBuilder::default()
        .request_timeout(TIMEOUT)
        .build(cl_rpc.as_str())
        .with_context(|| format!("building consensus admin client for {cl_rpc}"))?;
    let unsafe_head = AdminApiClient::admin_stop_sequencer(&client)
        .await
        .with_context(|| format!("calling admin_stopSequencer on {cl_rpc}"))?;
    if unsafe_head == B256::ZERO {
        warn!(
            cl_rpc = %cl_rpc,
            "admin_stopSequencer returned a zero unsafe head; sequencer stopped but the captured head is unavailable"
        );
    }
    Ok(unsafe_head)
}

/// Starts the sequencer on a single node via `admin_startSequencer`.
///
/// The `unsafe_head` hash must match the node's current engine unsafe head; the
/// server rejects mismatches and `B256::ZERO`. When op-conductor is enabled,
/// this only succeeds if the target node is the Raft leader.
pub async fn start_sequencer_node(
    node: ConductorNodeConfig,
    unsafe_head: B256,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    let outcome = start_sequencer(&node.cl_rpc, unsafe_head)
        .await
        .with_context(|| format!("starting sequencer on {} via {}", node.name, node.cl_rpc))
        .map(|()| format!("sequencer started on {} at {unsafe_head}", node.name));

    let _ = result_tx.send(outcome.map_err(|e| e.to_string())).await;
}

/// Stops the sequencer on a single node via `admin_stopSequencer`.
///
/// Returns the unsafe head hash captured at the moment the sequencer was
/// stopped, suitable for passing back into [`start_sequencer_node`] later.
pub async fn stop_sequencer_node(
    node: ConductorNodeConfig,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    let outcome = stop_sequencer(&node.cl_rpc)
        .await
        .with_context(|| format!("stopping sequencer on {} via {}", node.name, node.cl_rpc))
        .map(|head| {
            if head == B256::ZERO {
                format!("sequencer stopped on {} (unsafe head unavailable)", node.name)
            } else {
                format!("sequencer stopped on {} at {head}", node.name)
            }
        });

    let _ = result_tx.send(outcome.map_err(|e| e.to_string())).await;
}

async fn restore_peer_snapshot(
    cl_client: &HttpClient,
    cl_addrs: &[String],
    el_client: Option<&HttpClient>,
    el_enodes: &[String],
) {
    for addr in cl_addrs {
        if let Err(error) = BaseP2PApiClient::opp2p_connect_peer(cl_client, addr.clone()).await {
            warn!(peer = %addr, %error, "failed to restore CL peer after P2P isolation failure");
        }
    }

    if let Some(el_client) = el_client {
        for enode in el_enodes {
            let result: Result<bool, _> =
                ClientT::request(el_client, "admin_addPeer", rpc_params![enode]).await;
            match result {
                Ok(true) => {}
                Ok(false) => {
                    warn!(peer = %enode, "EL rejected peer restore after P2P isolation failure");
                }
                Err(error) => {
                    warn!(peer = %enode, %error, "failed to restore EL peer after P2P isolation failure");
                }
            }
        }
    }
}

/// Disconnects all p2p peers from the CL and EL of a node so that neither layer
/// can advance. Returns the saved peer addresses so they can be restored later
/// via [`unpause_sequencer_node`].
pub async fn pause_sequencer_node(
    node: ConductorNodeConfig,
    result_tx: mpsc::Sender<Result<(String, PausedPeers), String>>,
) {
    const TIMEOUT: Duration = Duration::from_secs(5);

    let outcome: anyhow::Result<(String, PausedPeers)> = async {
        let cl_client = HttpClientBuilder::default()
            .request_timeout(TIMEOUT)
            .build(node.cl_rpc.as_str())
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Snapshot every reconnect target before mutating either layer. If a peer cannot be
        // restored later, fail before disconnecting anything rather than creating partial state.
        let dump = BaseP2PApiClient::opp2p_peers(&cl_client, true)
            .await
            .map_err(|e| anyhow::anyhow!("opp2p_peers: {e}"))?;
        let mut cl_peers = Vec::with_capacity(dump.peers.len());
        for (peer_id, info) in dump.peers {
            let addr = info
                .addresses
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("connected CL peer {peer_id} has no reconnectable address"))?;
            cl_peers.push((peer_id, addr));
        }

        let (el_client, el_enodes) = if let Some(ref el_rpc) = node.el_rpc {
            let client = HttpClientBuilder::default()
                .request_timeout(TIMEOUT)
                .build(el_rpc.as_str())
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let peers: Vec<serde_json::Value> = ClientT::request(&client, "admin_peers", rpc_params![])
                .await
                .map_err(|e| anyhow::anyhow!("admin_peers: {e}"))?;
            let mut enodes = Vec::with_capacity(peers.len());
            for peer in peers {
                let enode = peer
                    .get("enode")
                    .and_then(|value| value.as_str())
                    .filter(|enode| !enode.is_empty())
                    .ok_or_else(|| anyhow::anyhow!("admin_peers returned a peer without an enode"))?;
                enodes.push(enode.to_string());
            }
            (Some(client), enodes)
        } else {
            (None, Vec::new())
        };

        let mut disconnected_cl_addrs = Vec::with_capacity(cl_peers.len());
        for (peer_id, addr) in &cl_peers {
            if let Err(error) = BaseP2PApiClient::opp2p_disconnect_peer(&cl_client, peer_id.clone()).await
            {
                // The RPC error may be ambiguous about whether the current peer was already
                // disconnected, so include it in the best-effort restore set too.
                let mut restore_addrs = disconnected_cl_addrs;
                restore_addrs.push(addr.clone());
                restore_peer_snapshot(&cl_client, &restore_addrs, None, &[]).await;
                anyhow::bail!("opp2p_disconnectPeer failed for {peer_id}: {error}");
            }
            disconnected_cl_addrs.push(addr.clone());
        }

        let cl_addrs = disconnected_cl_addrs;
        let mut removed_el_enodes = Vec::with_capacity(el_enodes.len());
        if let Some(ref client) = el_client {
            for enode in &el_enodes {
                let result: Result<bool, _> =
                    ClientT::request(client, "admin_removePeer", rpc_params![enode]).await;
                match result {
                    Ok(true) => removed_el_enodes.push(enode.clone()),
                    Ok(false) => {
                        restore_peer_snapshot(
                            &cl_client,
                            &cl_addrs,
                            Some(client),
                            &removed_el_enodes,
                        )
                        .await;
                        anyhow::bail!("admin_removePeer rejected {enode}");
                    }
                    Err(error) => {
                        // As with CL disconnects, an RPC error can be ambiguous about whether the
                        // mutation took effect. Include the current peer in the restore attempt.
                        let mut restore_enodes = removed_el_enodes;
                        restore_enodes.push(enode.clone());
                        restore_peer_snapshot(
                            &cl_client,
                            &cl_addrs,
                            Some(client),
                            &restore_enodes,
                        )
                        .await;
                        anyhow::bail!("admin_removePeer failed for {enode}: {error}");
                    }
                }
            }
        }

        let msg = format!(
            "paused {} — disconnected {} CL peer(s), {} EL peer(s)",
            node.name,
            cl_addrs.len(),
            el_enodes.len()
        );
        Ok((msg, PausedPeers { cl_addrs, el_enodes }))
    }
    .await;

    let _ = result_tx.send(outcome.map_err(|e| e.to_string())).await;
}

/// Reconnects the CL and EL peers that were saved by [`pause_sequencer_node`],
/// allowing the node to resume syncing to tip.
pub async fn unpause_sequencer_node(
    node: ConductorNodeConfig,
    peers: PausedPeers,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    const TIMEOUT: Duration = Duration::from_secs(5);

    let outcome: anyhow::Result<String> = async {
        let cl_client = HttpClientBuilder::default()
            .request_timeout(TIMEOUT)
            .build(node.cl_rpc.as_str())
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let mut cl_ok = 0usize;
        for addr in &peers.cl_addrs {
            if BaseP2PApiClient::opp2p_connect_peer(&cl_client, addr.clone()).await.is_ok() {
                cl_ok += 1;
            }
        }

        let mut el_ok = 0usize;
        if let Some(ref el_rpc) = node.el_rpc {
            let el_client = HttpClientBuilder::default()
                .request_timeout(TIMEOUT)
                .build(el_rpc.as_str())
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            for enode in &peers.el_enodes {
                let r: Result<bool, _> =
                    ClientT::request(&el_client, "admin_addPeer", rpc_params![enode]).await;
                if matches!(r, Ok(true)) {
                    el_ok += 1;
                }
            }
        }

        if cl_ok != peers.cl_addrs.len() || (node.el_rpc.is_some() && el_ok != peers.el_enodes.len()) {
            anyhow::bail!(
                "unpaused {} — reconnected {cl_ok}/{} CL peer(s), {el_ok}/{} EL peer(s); saved peers kept for retry",
                node.name,
                peers.cl_addrs.len(),
                peers.el_enodes.len()
            );
        }

        Ok(format!(
            "unpaused {} — reconnected {cl_ok}/{} CL peer(s), {el_ok}/{} EL peer(s)",
            node.name,
            peers.cl_addrs.len(),
            peers.el_enodes.len()
        ))
    }
    .await;

    let _ = result_tx.send(outcome.map_err(|e| e.to_string())).await;
}
