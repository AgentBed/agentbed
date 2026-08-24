//! L01-AC07: sub-second R-class read performance on a deterministic fixture.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    unused_imports,
    clippy::uninlined_format_args
)]

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::events::{EventLog, EventRecord};
use agentbed_broker::transaction::engine::TransactionEngine;
use agentbed_protocol::wire::{
    ConfigFileChange, ConfigProposeParams, IdempotencyKey, TransactionId, TxStatusParams,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const TX_COUNT: usize = 100;
const EVENT_COUNT: usize = 500;

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-perf-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

#[test]
fn r_class_reads_complete_under_one_second_on_fixture() {
    let dir = scratch();
    let engine = TransactionEngine::open(&dir, UnresolvedAdapter).expect("open");
    let mut tx_ids = Vec::with_capacity(TX_COUNT);

    for i in 0..TX_COUNT {
        let propose = engine
            .config_propose(
                "agent:perf",
                "sha256:perf",
                &ConfigProposeParams {
                    idempotency_key: IdempotencyKey::new(format!("perf-{i}")).expect("key"),
                    changes: vec![ConfigFileChange {
                        path: "/etc/nixos/configuration.nix".to_owned(),
                        content: format!("{{ n = {i}; }}"),
                    }],
                },
            )
            .expect("propose");
        tx_ids.push(propose.tx_id);
    }

    let events_dir = dir.join("events");
    let log = EventLog::open(&events_dir).expect("events");
    for i in 0..EVENT_COUNT {
        log.append(EventRecord {
            kind: "fixture".to_owned(),
            payload: format!("{{\"i\":{i}}}"),
        })
        .expect("append");
    }

    let start = Instant::now();
    for tx_id in &tx_ids {
        let _ = engine.tx_status(tx_id).expect("status");
    }
    let cursor = log.latest_cursor().expect("cursor");
    let _ = log.replay(&cursor).expect("replay");
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_secs() < 1,
        "R-class reads took {:?}, expected < 1s",
        elapsed
    );
}
