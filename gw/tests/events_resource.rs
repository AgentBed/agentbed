//! Gateway exposure of `agentbed://events` (repair review #5010391942).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::config::BrokerConfig;
use agentbed_broker::dispatch::Dispatcher;
use agentbed_broker::identity::TokenStore;
use agentbed_broker::manifest::ManifestStore;
use agentbed_broker::observability::StderrObserver;
use agentbed_broker::server::Server;
use agentbed_gw::{mcp, BrokerClient, Session};
use agentbed_protocol::wire::Token;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AGENT: &str = "mcp-client:gate0-reader";

fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agentbed-gw-events-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../broker/tests/manifests"
    ))
}

#[test]
fn mcp_resources_list_includes_agentbed_events() {
    let dir = scratch();
    let tokens = TokenStore::from_pairs([(
        AGENT.to_owned(),
        "agent.reader.yaml".to_owned(),
        TOKEN.to_owned(),
    )])
    .expect("tokens");
    let config = BrokerConfig {
        socket_path: dir.join("broker.sock"),
        state_dir: Some(dir.join("state")),
        read_timeout: Duration::from_secs(5),
        write_timeout: Duration::from_secs(5),
        ..BrokerConfig::default()
    };
    let dispatcher = Arc::new(Dispatcher::new(
        tokens,
        ManifestStore::new(manifest_dir()),
        Box::new(UnresolvedAdapter),
        config.state_dir.clone().expect("state_dir"),
    ));
    let mut server = Server::start(&config, dispatcher, Arc::new(StderrObserver)).expect("server");
    let mut session = Session::new(
        BrokerClient::new(&config.socket_path),
        Token::new(TOKEN),
    );

    let line = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "resources/list",
        "params": {}
    })
    .to_string();
    let response = mcp::handle_line(&mut session, &line).expect("response");
    assert!(
        response.contains("agentbed://events"),
        "expected agentbed://events resource, got: {response}"
    );
    server.shutdown(Duration::from_secs(1));
}
