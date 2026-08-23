//! Gateway to broker, over a real socket.
//!
//! This is the Gate 0 spike's happy path: an MCP client talks to the
//! unprivileged gateway over stdio framing, the gateway relays to the
//! privileged broker over a Unix socket, and the broker decides.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::config::BrokerConfig;
use agentbed_broker::dispatch::Dispatcher;
use agentbed_broker::identity::TokenStore;
use agentbed_broker::manifest::ManifestStore;
use agentbed_broker::observability::{ObservationSink, StderrObserver};
use agentbed_broker::server::Server;
use agentbed_gw::{mcp, BrokerClient, Session};
use agentbed_protocol::wire::Token;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AGENT: &str = "mcp-client:gate0-reader";

struct Fixture {
    server: Server,
    dir: PathBuf,
}

impl Fixture {
    fn start() -> Fixture {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("agentbed-gw-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let tokens = TokenStore::from_pairs([(
            AGENT.to_owned(),
            "agent.reader.yaml".to_owned(),
            TOKEN.to_owned(),
        )])
        .expect("token store");

        let config = BrokerConfig {
            socket_path: dir.join("broker.sock"),
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            ..BrokerConfig::default()
        };
        let dispatcher = Arc::new(Dispatcher::new(
            tokens,
            ManifestStore::new(manifest_dir()),
            Box::new(UnresolvedAdapter),
        ));
        let audit: Arc<dyn ObservationSink> = Arc::new(StderrObserver);
        let server = Server::start(&config, dispatcher, audit).expect("broker starts");
        Fixture { server, dir }
    }

    fn session(&self, token: &str) -> Session {
        Session::new(
            BrokerClient::new(self.server.socket_path()),
            Token::new(token),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.shutdown(Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The broker's own test manifests; the gateway has no manifests of its own —
/// it never sees one.
fn manifest_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../broker/tests/manifests"
    ))
}

fn call(session: &mut Session, line: &str) -> Value {
    let reply = mcp::handle_line(session, line).expect("a reply");
    serde_json::from_str(&reply).expect("valid JSON-RPC")
}

#[test]
fn initialize_then_list_then_call() {
    let fixture = Fixture::start();
    let mut session = fixture.session(TOKEN);

    let init = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string(),
    );
    assert_eq!(init["result"]["protocolVersion"], mcp::PROTOCOL_VERSION);
    assert_eq!(init["result"]["serverInfo"]["name"], "agentbed-gw");

    let listed = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
    );
    let tools = listed["result"]["tools"].as_array().expect("a tool array");
    assert_eq!(tools.len(), 1, "Gate 0 exposes exactly one tool");
    assert_eq!(tools[0]["name"], "system.info");

    let called = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"system.info","arguments":{}}})
        .to_string(),
    );
    assert_eq!(called["result"]["isError"], false);
    let info = &called["result"]["structuredContent"];
    assert_eq!(info["safety_source"], "unresolved_adapter");
    assert_eq!(info["safety"]["root_config"], "none");
    assert!(info["host"]["kernel_release"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
}

#[test]
fn a_bad_token_is_refused_by_the_broker_not_the_gateway() {
    // The gateway cannot tell this token is invalid — it holds no verifier —
    // so it relays the call and the broker refuses it. That is the split
    // working as designed, not a missing check in the gateway.
    let fixture = Fixture::start();
    let mut session = fixture.session("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz");

    let called = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"system.info","arguments":{}}})
        .to_string(),
    );
    assert_eq!(called["result"]["isError"], true);
    let text = called["result"]["content"][0]["text"]
        .as_str()
        .expect("a reason");
    assert!(text.contains("Unauthenticated"), "got: {text}");
}

#[test]
fn arguments_are_schema_checked_before_the_socket_is_touched() {
    // A fast rejection, never an authorization: the broker checks the same
    // thing again and does not care what happened here.
    let fixture = Fixture::start();
    let mut session = fixture.session(TOKEN);

    let called = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"system.info","arguments":{"verbose":true}}})
        .to_string(),
    );
    assert_eq!(called["error"]["code"], -32602, "invalid params");
}

#[test]
fn unknown_tools_and_methods_are_refused() {
    let fixture = Fixture::start();
    let mut session = fixture.session(TOKEN);

    let unknown_tool = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"shell.exec","arguments":{}}})
        .to_string(),
    );
    assert_eq!(unknown_tool["error"]["code"], -32602);

    let unknown_method = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":2,"method":"tx.apply"}).to_string(),
    );
    assert_eq!(unknown_method["error"]["code"], -32601);
}

#[test]
fn notifications_get_no_reply_and_malformed_lines_do_not_kill_the_session() {
    let fixture = Fixture::start();
    let mut session = fixture.session(TOKEN);

    assert!(
        mcp::handle_line(
            &mut session,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string()
        )
        .is_none(),
        "answering a notification is a protocol violation"
    );
    assert!(mcp::handle_line(&mut session, "").is_none());

    let malformed = call(&mut session, "{not json");
    assert_eq!(malformed["error"]["code"], -32700);

    // The session still works afterwards.
    let listed = call(
        &mut session,
        &json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}).to_string(),
    );
    assert!(listed["result"]["tools"].is_array());
}
