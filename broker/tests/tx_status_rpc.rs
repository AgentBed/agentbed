//! L01-AC05: `tx.status` wired to durable state over RPC v2.

mod support;

use agentbed_broker::config::BrokerConfig;
use agentbed_broker::dispatch::Dispatcher;
use agentbed_broker::identity::{Enrollment, TokenStore};
use agentbed_broker::manifest::ManifestStore;
use agentbed_broker::observability::CollectingObserver;
use agentbed_broker::server::Server;
use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_protocol::wire::{ErrorCode, OperationResult};
use agentbed_protocol::PROTOCOL_VERSION_V2;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use support::{read_response, send_frame, TOKEN_A};

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-rpc-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn start_with_state(dir: &PathBuf) -> Server {
    let tokens = TokenStore::from_enrollments([Enrollment {
        agent_id: "mcp-client:gate0-reader".to_owned(),
        manifest_ref: "agent.reader.yaml".to_owned(),
        token: TOKEN_A.to_owned(),
        revoked: false,
        expires_at_unix: None,
    }])
    .expect("tokens");

    let mut config = BrokerConfig {
        socket_path: dir.join("broker.sock"),
        state_dir: Some(dir.join("state")),
        manifest_dir: Some(support::manifest_dir()),
        ..BrokerConfig::default()
    };
    config.read_timeout = std::time::Duration::from_secs(5);
    config.write_timeout = std::time::Duration::from_secs(5);

    let dispatcher = Arc::new(Dispatcher::new(
        tokens,
        ManifestStore::new(support::manifest_dir()),
        Box::new(UnresolvedAdapter),
        config.state_dir.clone().expect("state_dir"),
    ));
    let observer = Arc::new(CollectingObserver::default());
    Server::start(&config, dispatcher, observer).expect("server")
}

#[test]
fn tx_status_unknown_id_is_denied_without_sensitive_prose() {
    let dir = scratch();
    let server = start_with_state(&dir);
    let mut stream = UnixStream::connect(dir.join("broker.sock")).expect("connect");
    let body = format!(
        r#"{{"v":2,"id":"01J-status","op":"tx.status","auth":{{"token":"{TOKEN_A}"}},"params":{{"tx_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}}}}"#
    );
    send_frame(&mut stream, body.as_bytes());
    let response = read_response(&mut stream).expect("response");
    assert_eq!(response.v, PROTOCOL_VERSION_V2);
    assert!(response.result.is_none());
    let err = response.error.expect("error");
    assert_eq!(err.code, ErrorCode::Denied);
    assert!(err.stage.is_some());
    server.shutdown(std::time::Duration::from_secs(1));
}

#[test]
fn tx_status_returns_durable_state_after_propose() {
    let dir = scratch();
    let server = start_with_state(&dir);
    let mut stream = UnixStream::connect(dir.join("broker.sock")).expect("connect");

    let propose = format!(
        r#"{{"v":2,"id":"01J-propose","op":"config.propose","auth":{{"token":"{TOKEN_A}"}},"params":{{"idempotency_key":"k1","changes":[{{"path":"/etc/nixos/configuration.nix","content":"{{}}"}}]}}}}"#
    );
    send_frame(&mut stream, propose.as_bytes());
    let propose_resp = read_response(&mut stream).expect("propose response");
    let tx_id = match propose_resp.result {
        Some(OperationResult::ConfigPropose(result)) => result.tx_id,
        other => panic!("expected config.propose result, got {other:?}"),
    };

    let status = format!(
        r#"{{"v":2,"id":"01J-status2","op":"tx.status","auth":{{"token":"{TOKEN_A}"}},"params":{{"tx_id":"{tx_id}"}}}}"#
    );
    send_frame(&mut stream, status.as_bytes());
    let status_resp = read_response(&mut stream).expect("status response");
    match status_resp.result {
        Some(OperationResult::TxStatus(result)) => {
            assert_eq!(result.tx_id, tx_id);
            assert_eq!(
                result.state,
                agentbed_protocol::dto::transaction::TransactionState::Proposed
            );
        }
        other => panic!("expected tx.status result, got {other:?}"),
    }
    server.shutdown(std::time::Duration::from_secs(1));
}

use std::os::unix::net::UnixStream;
