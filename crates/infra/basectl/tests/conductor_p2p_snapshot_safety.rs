use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use base_consensus_gossip::{PeerDump, PeerInfo};
use basectl_cli::{ConductorNodeConfig, PausedPeers, pause_sequencer_node};
use jsonrpsee::{
    RpcModule,
    server::{ServerBuilder, ServerHandle},
    types::ErrorObjectOwned,
};
use serde_json::json;
use tokio::sync::mpsc;
use url::Url;

async fn spawn_server(module: RpcModule<()>) -> Result<(Url, ServerHandle)> {
    let server = ServerBuilder::default().build("127.0.0.1:0").await?;
    let address = server.local_addr()?;
    let handle = server.start(module);
    Ok((Url::parse(&format!("http://{address}"))?, handle))
}

fn node(cl_rpc: Url, el_rpc: Option<Url>) -> ConductorNodeConfig {
    ConductorNodeConfig {
        name: "sequencer-0".to_string(),
        conductor_rpc: cl_rpc.clone(),
        cl_rpc,
        server_id: "sequencer-0".to_string(),
        raft_addr: "127.0.0.1:5050".to_string(),
        el_rpc,
        docker_conductor: None,
        docker_el: None,
        docker_cl: None,
        flashblocks_ws: None,
    }
}

async fn run_pause(node: ConductorNodeConfig) -> std::result::Result<(String, PausedPeers), String> {
    let (tx, mut rx) = mpsc::channel(1);
    pause_sequencer_node(node, tx).await;
    rx.recv().await.expect("pause helper should report one result")
}

fn peer_dump_with_addresses(addresses: Vec<String>) -> PeerDump {
    let mut dump = PeerDump::default();
    let peer = PeerInfo {
        peer_id: "peer-1".to_string(),
        addresses,
        ..Default::default()
    };
    dump.total_connected = 1;
    dump.peers.insert(peer.peer_id.clone(), peer);
    dump
}

#[tokio::test]
async fn pause_does_not_disconnect_cl_peer_without_reconnect_address() -> Result<()> {
    let dump = peer_dump_with_addresses(Vec::new());
    let disconnect_calls = Arc::new(AtomicUsize::new(0));
    let disconnect_calls_for_rpc = Arc::clone(&disconnect_calls);

    let mut module = RpcModule::new(());
    module.register_method("opp2p_peers", move |_, _, _| {
        Ok::<_, ErrorObjectOwned>(dump.clone())
    })?;
    module.register_method("opp2p_disconnectPeer", move |_, _, _| {
        disconnect_calls_for_rpc.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(())
    })?;

    let (cl_rpc, _cl_handle) = spawn_server(module).await?;
    let result = run_pause(node(cl_rpc, None)).await;

    assert!(
        result.is_err(),
        "a CL peer without a reconnect address must abort isolation"
    );
    assert_eq!(
        disconnect_calls.load(Ordering::SeqCst),
        0,
        "snapshot validation must fail before mutating CL peer state"
    );
    Ok(())
}

#[tokio::test]
async fn pause_does_not_mutate_cl_when_el_snapshot_is_not_reconnectable() -> Result<()> {
    let dump = peer_dump_with_addresses(vec!["/ip4/127.0.0.1/tcp/9000".to_string()]);
    let cl_disconnect_calls = Arc::new(AtomicUsize::new(0));
    let cl_disconnect_calls_for_rpc = Arc::clone(&cl_disconnect_calls);

    let mut cl_module = RpcModule::new(());
    cl_module.register_method("opp2p_peers", move |_, _, _| {
        Ok::<_, ErrorObjectOwned>(dump.clone())
    })?;
    cl_module.register_method("opp2p_disconnectPeer", move |_, _, _| {
        cl_disconnect_calls_for_rpc.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(())
    })?;
    let (cl_rpc, _cl_handle) = spawn_server(cl_module).await?;

    let el_remove_calls = Arc::new(AtomicUsize::new(0));
    let el_remove_calls_for_rpc = Arc::clone(&el_remove_calls);
    let mut el_module = RpcModule::new(());
    el_module.register_method("admin_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(vec![json!({"id": "peer-2"})])
    })?;
    el_module.register_method("admin_removePeer", move |_, _, _| {
        el_remove_calls_for_rpc.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(true)
    })?;
    let (el_rpc, _el_handle) = spawn_server(el_module).await?;

    let result = run_pause(node(cl_rpc, Some(el_rpc))).await;

    assert!(
        result.is_err(),
        "an EL peer without an enode must abort isolation"
    );
    assert_eq!(
        cl_disconnect_calls.load(Ordering::SeqCst),
        0,
        "EL snapshot validation must complete before CL peer mutation begins"
    );
    assert_eq!(
        el_remove_calls.load(Ordering::SeqCst),
        0,
        "invalid EL snapshot data must fail before removing EL peers"
    );
    Ok(())
}
