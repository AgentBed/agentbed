//! L03 RED — `/var/lib/agentbed/broker/state` class-F gap (L03-AC01).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use agentbed_adapter_nix::protected::{self, ProtectedRejectReason};
use agentbed_protocol::wire::ConfigFileChange;

#[test]
fn protected_broker_state_wal_path_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/var/lib/agentbed/broker/state/wal/records/1.json".to_owned(),
        content: "{}".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("broker state wal");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}

#[test]
fn protected_broker_state_root_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/var/lib/agentbed/broker/state/checkpoint.json".to_owned(),
        content: "{}".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("broker state root");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}

#[test]
fn protected_broker_state_alias_traversal_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/var/lib/agentbed/../agentbed/broker/state/wal/records/1.json".to_owned(),
        content: "{}".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("alias");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}

#[test]
fn protected_legacy_wal_alias_still_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/var/lib/agentbed/wal/records/1.json".to_owned(),
        content: "{}".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("legacy wal");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}

#[test]
fn protected_broker_state_content_reference_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/etc/nixos/configuration.nix".to_owned(),
        content: "{ environment.etc.\"/var/lib/agentbed/broker/state/wal\".source = ./wal; }"
            .to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("content reference");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}

#[test]
fn protected_broker_state_descendant_path_rejected_as_class_f() {
    let change = ConfigFileChange {
        path: "/var/lib/agentbed/broker/state/events/0001.json".to_owned(),
        content: "{}".to_owned(),
    };
    let err = protected::check_protected_changes(&[change]).expect_err("events descendant");
    assert_eq!(err, ProtectedRejectReason::BrokerWal);
}
