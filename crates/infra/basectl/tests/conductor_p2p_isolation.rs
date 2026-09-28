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

fn rpc_error(message: &'static str) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32000, message, None::<()>)
}

fn peer_dump() -> PeerDump {
    let mut dump = PeerDump::default();
    let peer = PeerInfo {
        peer_id: "peer-1".to_string(),
        addresses: vec!["/ip4/127.0.0.1/tcp/9000".to_string()],
        ..Default::default()
    };
    dump.total_connected = 1;
    dump.peers.insert(peer.peer_id.clone(), peer);
    dump
}

#[tokio::test]
async fn pause_fails_when_cl_disconnect_errors() -> Result<()> {
    let dump = peer_dump();
    let restore_calls = Arc::new(AtomicUsize::new(0));
    let restore_calls_for_rpc = Arc::clone(&restore_calls);
    let mut module = RpcModule::new(());
    module.register_method("opp2p_peers", move |_, _, _| {
        Ok::<_, ErrorObjectOwned>(dump.clone())
    })?;
    module.register_method("opp2p_disconnectPeer", |_, _, _| {
        Err::<(), ErrorObjectOwned>(rpc_error("disconnect failed"))
    })?;
    module.register_method("opp2p_connectPeer", move |_, _, _| {
        restore_calls_for_rpc.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(())
    })?;

    let (cl_rpc, _cl_handle) = spawn_server(module).await?;
    let result = run_pause(node(cl_rpc, None)).await;

    assert!(result.is_err(), "failed CL disconnect must not be reported as isolated");
    assert_eq!(
        restore_calls.load(Ordering::SeqCst),
        1,
        "an ambiguous CL disconnect failure should attempt to restore the current peer"
    );
    Ok(())
}

#[tokio::test]
async fn pause_fails_when_el_peer_enumeration_errors() -> Result<()> {
    let mut cl_module = RpcModule::new(());
    cl_module.register_method("opp2p_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(PeerDump::default())
    })?;
    let (cl_rpc, _cl_handle) = spawn_server(cl_module).await?;

    let mut el_module = RpcModule::new(());
    el_module.register_method("admin_peers", |_, _, _| {
        Err::<Vec<serde_json::Value>, ErrorObjectOwned>(rpc_error("admin_peers failed"))
    })?;
    let (el_rpc, _el_handle) = spawn_server(el_module).await?;

    let result = run_pause(node(cl_rpc, Some(el_rpc))).await;

    assert!(result.is_err(), "failed EL peer snapshot must not be reported as isolated");
    Ok(())
}

#[tokio::test]
async fn pause_fails_when_el_remove_peer_is_rejected() -> Result<()> {
    let mut cl_module = RpcModule::new(());
    cl_module.register_method("opp2p_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(PeerDump::default())
    })?;
    let (cl_rpc, _cl_handle) = spawn_server(cl_module).await?;

    let mut el_module = RpcModule::new(());
    el_module.register_method("admin_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(vec![json!({"enode": "enode://peer-1@127.0.0.1:30303"})])
    })?;
    el_module.register_method("admin_removePeer", |_, _, _| Ok::<_, ErrorObjectOwned>(false))?;
    // A rejected remove should leave the current peer connected, so there is no
    // current-peer restore to attempt. Previously removed peers would still be restored.
    el_module.register_method("admin_addPeer", |_, _, _| Ok::<_, ErrorObjectOwned>(true))?;
    let (el_rpc, _el_handle) = spawn_server(el_module).await?;

    let result = run_pause(node(cl_rpc, Some(el_rpc))).await;

    assert!(result.is_err(), "rejected EL peer removal must not be reported as isolated");
    Ok(())
}

#[tokio::test]
async fn pause_fails_when_el_remove_peer_rpc_errors() -> Result<()> {
    let mut cl_module = RpcModule::new(());
    cl_module.register_method("opp2p_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(PeerDump::default())
    })?;
    let (cl_rpc, _cl_handle) = spawn_server(cl_module).await?;

    let restore_calls = Arc::new(AtomicUsize::new(0));
    let restore_calls_for_rpc = Arc::clone(&restore_calls);
    let mut el_module = RpcModule::new(());
    el_module.register_method("admin_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(vec![json!({"enode": "enode://peer-1@127.0.0.1:30303"})])
    })?;
    el_module.register_method("admin_removePeer", |_, _, _| {
        Err::<bool, ErrorObjectOwned>(rpc_error("remove failed"))
    })?;
    el_module.register_method("admin_addPeer", move |_, _, _| {
        restore_calls_for_rpc.fetch_add(1, Ordering::SeqCst);
        Ok::<_, ErrorObjectOwned>(true)
    })?;
    let (el_rpc, _el_handle) = spawn_server(el_module).await?;

    let result = run_pause(node(cl_rpc, Some(el_rpc))).await;

    assert!(result.is_err(), "EL remove RPC failure must not be reported as isolated");
    assert_eq!(
        restore_calls.load(Ordering::SeqCst),
        1,
        "an ambiguous EL remove error should attempt to restore the current peer"
    );
    Ok(())
}

#[tokio::test]
async fn pause_succeeds_when_all_peer_disconnects_succeed() -> Result<()> {
    let dump = peer_dump();
    let mut cl_module = RpcModule::new(());
    cl_module.register_method("opp2p_peers", move |_, _, _| {
        Ok::<_, ErrorObjectOwned>(dump.clone())
    })?;
    cl_module.register_method("opp2p_disconnectPeer", |_, _, _| Ok::<_, ErrorObjectOwned>(()))?;
    let (cl_rpc, _cl_handle) = spawn_server(cl_module).await?;

    let mut el_module = RpcModule::new(());
    el_module.register_method("admin_peers", |_, _, _| {
        Ok::<_, ErrorObjectOwned>(vec![json!({"enode": "enode://peer-2@127.0.0.1:30303"})])
    })?;
    el_module.register_method("admin_removePeer", |_, _, _| Ok::<_, ErrorObjectOwned>(true))?;
    let (el_rpc, _el_handle) = spawn_server(el_module).await?;

    let (_, peers) = run_pause(node(cl_rpc, Some(el_rpc)))
        .await
        .map_err(anyhow::Error::msg)?;

    assert_eq!(peers.cl_addrs, vec!["/ip4/127.0.0.1/tcp/9000"]);
    assert_eq!(peers.el_enodes, vec!["enode://peer-2@127.0.0.1:30303"]);
    Ok(())
}
