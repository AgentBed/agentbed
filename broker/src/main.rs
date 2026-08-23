//! Broker entry point.
//!
//! Gate 0 runs as a normal user with the socket under a `0700` directory;
//! systemd socket activation, root, and the split from the gateway's
//! `DynamicUser` come with Gate 1.

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::config::BrokerConfig;
use agentbed_broker::dispatch::Dispatcher;
use agentbed_broker::identity::TokenStore;
use agentbed_broker::manifest::ManifestStore;
use agentbed_broker::observability::{ObservationSink, StderrObserver};
use agentbed_broker::server::Server;
use agentbed_broker::signals;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("agentbed-broker: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    // Before any thread exists, so every thread inherits the block and the
    // signal is handled in exactly one place.
    signals::block_termination_signals()?;

    let mut config = BrokerConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => config.socket_path = PathBuf::from(next_value(&mut args, &arg)?),
            "--tokens" => {
                config.token_store_path = Some(PathBuf::from(next_value(&mut args, &arg)?));
            }
            "--manifests" => {
                config.manifest_dir = Some(PathBuf::from(next_value(&mut args, &arg)?));
            }
            "--allow-peer-uid" => {
                let raw = next_value(&mut args, &arg)?;
                let uid: u32 = raw.parse().map_err(|_| format!("invalid uid: {raw}"))?;
                config.allowed_peer_uids.push(uid);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}\n\n{USAGE}")),
        }
    }

    let token_path = config
        .token_store_path
        .clone()
        .ok_or_else(|| format!("--tokens is required\n\n{USAGE}"))?;
    let tokens = TokenStore::load(&token_path)?;
    if tokens.is_empty() {
        // A broker that can authenticate nobody should say so at startup
        // rather than refusing every call at runtime for an unclear reason.
        return Err("token store is empty: no agent could authenticate".to_owned());
    }

    let manifest_dir = config
        .manifest_dir
        .clone()
        .ok_or_else(|| format!("--manifests is required\n\n{USAGE}"))?;

    let observer: Arc<dyn ObservationSink> = Arc::new(StderrObserver);
    let dispatcher = Arc::new(Dispatcher::new(
        tokens,
        ManifestStore::new(manifest_dir),
        // Gate 0 resolves no host adapter, so every resource reports `none`
        // and every D/M step would be refused. The Nix adapter lands at Gate 1.
        Box::new(UnresolvedAdapter),
    ));
    let mut server = Server::start(&config, dispatcher, observer).map_err(|e| e.to_string())?;
    eprintln!(
        "agentbed-broker: listening on {}",
        server.socket_path().display()
    );

    signals::wait_for_termination();
    eprintln!("agentbed-broker: shutting down");
    server.shutdown(Duration::from_secs(5));
    Ok(())
}

const USAGE: &str = "usage: agentbed-broker --tokens <file> --manifests <dir> \
[--socket <path>] [--allow-peer-uid <uid>]";

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}
