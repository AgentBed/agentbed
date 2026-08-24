//! L01-AC01 / L01-AC03 / L01-AC04: transaction engine integration.

use agentbed_broker::adapter::UnresolvedAdapter;
use agentbed_broker::transaction::engine::{EngineError, TransactionEngine};
use agentbed_protocol::dto::transaction::{BaseRevision, TransactionState};
use agentbed_protocol::digest::Digest;
use agentbed_protocol::wire::{ConfigFileChange, ConfigProposeParams, TxApplyParams, TxTestParams};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agb4-engine-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn base_revision() -> BaseRevision {
    BaseRevision {
        generation: Some("gen-1".to_owned()),
        etc_git_commit: "deadbeef".to_owned(),
        config_digest: Digest::from_hex(
            "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        )
        .expect("digest"),
    }
}

#[test]
fn idempotent_config_propose_returns_original_result() {
    let dir = scratch();
    let adapter = UnresolvedAdapter;
    let engine = TransactionEngine::open(&dir, &adapter).expect("open");

    let params = ConfigProposeParams {
        idempotency_key: "prop-1".try_into().expect("key"),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/configuration.nix".to_owned(),
            content: "{ }".to_owned(),
        }],
    };

    let first = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("propose");
    let second = engine
        .config_propose("agent:a", "sha256:abc", &params)
        .expect("replay");
    assert_eq!(first.tx_id, second.tx_id);
    assert_eq!(first.diff, second.diff);
}

#[test]
fn conflicting_idempotency_key_reuse_is_refused() {
    let dir = scratch();
    let adapter = UnresolvedAdapter;
    let engine = TransactionEngine::open(&dir, &adapter).expect("open");

    let key = "prop-2".try_into().expect("key");
    let params_a = ConfigProposeParams {
        idempotency_key: key.clone(),
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/a.nix".to_owned(),
            content: "a".to_owned(),
        }],
    };
    let params_b = ConfigProposeParams {
        idempotency_key: key,
        changes: vec![ConfigFileChange {
            path: "/etc/nixos/b.nix".to_owned(),
            content: "b".to_owned(),
        }],
    };

    engine
        .config_propose("agent:a", "sha256:abc", &params_a)
        .expect("first");
    let err = engine
        .config_propose("agent:a", "sha256:abc", &params_b)
        .expect_err("conflict");
    assert!(matches!(err, EngineError::IdempotencyConflict));
}

#[test]
fn moved_base_revision_refuses_apply() {
    let dir = scratch();
    let adapter = UnresolvedAdapter;
    let engine = TransactionEngine::open(&dir, &adapter).expect("open");

    let propose = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &ConfigProposeParams {
                idempotency_key: "prop-3".try_into().expect("key"),
                changes: vec![ConfigFileChange {
                    path: "/etc/nixos/configuration.nix".to_owned(),
                    content: "{ }".to_owned(),
                }],
            },
        )
        .expect("propose");

    engine
        .tx_test(
            "agent:a",
            &TxTestParams {
                tx_id: propose.tx_id.clone(),
            },
        )
        .expect("test");

    // Simulate moved base by reopening with a different adapter revision.
    struct MovedBaseAdapter;
    impl agentbed_broker::adapter::HostAdapter for MovedBaseAdapter {
        fn info(&self) -> agentbed_protocol::dto::system_info::AdapterInfo {
            UnresolvedAdapter.info()
        }
        fn safety_vector(&self) -> agentbed_protocol::dto::system_info::SafetyVector {
            UnresolvedAdapter.safety_vector()
        }
        fn safety_source(&self) -> agentbed_protocol::dto::system_info::SafetySource {
            UnresolvedAdapter.safety_source()
        }
        fn current_base_revision(&self) -> BaseRevision {
            BaseRevision {
                generation: Some("gen-2".to_owned()),
                ..base_revision()
            }
        }
    }

    let engine = TransactionEngine::open(&dir, &MovedBaseAdapter).expect("reopen");
    let err = engine
        .tx_apply(
            "agent:a",
            &TxApplyParams {
                tx_id: propose.tx_id,
                idempotency_key: "apply-1".try_into().expect("key"),
            },
        )
        .expect_err("moved base");
    assert!(matches!(err, EngineError::BaseRevisionMoved));
}

#[test]
fn happy_path_reaches_probation_without_watchdog_states() {
    let dir = scratch();
    let adapter = UnresolvedAdapter;
    let engine = TransactionEngine::open(&dir, &adapter).expect("open");

    let propose = engine
        .config_propose(
            "agent:a",
            "sha256:abc",
            &ConfigProposeParams {
                idempotency_key: "prop-4".try_into().expect("key"),
                changes: vec![ConfigFileChange {
                    path: "/etc/nixos/configuration.nix".to_owned(),
                    content: "{ }".to_owned(),
                }],
            },
        )
        .expect("propose");
    assert_eq!(propose.state, TransactionState::Proposed);

    let testing = engine
        .tx_test(
            "agent:a",
            &TxTestParams {
                tx_id: propose.tx_id.clone(),
            },
        )
        .expect("test");
    assert_eq!(testing.state, TransactionState::Testing);

    let applying = engine
        .tx_apply(
            "agent:a",
            &TxApplyParams {
                tx_id: propose.tx_id.clone(),
                idempotency_key: "apply-2".try_into().expect("key"),
            },
        )
        .expect("apply");
    assert_eq!(applying.state, TransactionState::Applying);

    let probation = engine
        .advance_to_probation("agent:a", &propose.tx_id)
        .expect("probation");
    assert_eq!(probation.state, TransactionState::Probation);

    let err = engine
        .advance_to_probation("agent:a", &propose.tx_id)
        .expect_err("no watchdog");
    assert!(matches!(err, EngineError::WatchdogAuthorityRequired));
}

#[test]
fn recovery_after_restart_preserves_state_without_invented_progress() {
    let dir = scratch();
    let adapter = UnresolvedAdapter;
    let tx_id = {
        let engine = TransactionEngine::open(&dir, &adapter).expect("open");
        let propose = engine
            .config_propose(
                "agent:a",
                "sha256:abc",
                &ConfigProposeParams {
                    idempotency_key: "prop-5".try_into().expect("key"),
                    changes: vec![ConfigFileChange {
                        path: "/etc/nixos/configuration.nix".to_owned(),
                        content: "{ }".to_owned(),
                    }],
                },
            )
            .expect("propose");
        let tx_id = propose.tx_id.clone();
        engine
            .tx_test(
                "agent:a",
                &TxTestParams {
                    tx_id: propose.tx_id,
                },
            )
            .expect("test");
        tx_id
    };

    let engine = TransactionEngine::open(&dir, &adapter).expect("reopen");
    let status = engine.tx_status(&tx_id).expect("status");
    assert_eq!(status.state, TransactionState::Testing);
}
