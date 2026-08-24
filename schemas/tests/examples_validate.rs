//! Every example in `schemas/examples/` validates, every schema compiles, and
//! the negative cases the documents call out are actually rejected.
//!
//! ADR §6 promises "schema conformance examples for every initial tool ship in
//! `schemas/examples/`". A promised example that nobody validates is a
//! documentation claim, not a conformance example — so this test walks the
//! directory rather than naming files, and fails when an example is added
//! without a mapping.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use agentbed_schemas::{manifest_kind, validate, yaml_to_json, SchemaKind};
use serde_json::{json, Value};
use std::path::Path;

fn examples_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/examples"))
}

/// Map an example filename to the schema it claims to conform to.
fn is_yaml(file_name: &str) -> bool {
    Path::new(file_name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml"))
}

fn schema_for(file_name: &str) -> SchemaKind {
    match file_name {
        n if n.starts_with("tool.system.info.request") => SchemaKind::SystemInfoRequest,
        n if n.starts_with("tool.system.info.response") => SchemaKind::SystemInfoResponse,
        n if n.starts_with("tool.config.propose.request") => SchemaKind::ConfigProposeRequest,
        n if n.starts_with("tool.config.propose.response") => SchemaKind::ConfigProposeResponse,
        n if n.starts_with("tool.tx.status.request") => SchemaKind::TxStatusRequest,
        n if n.starts_with("tool.tx.status.response") => SchemaKind::TxStatusResponse,
        n if n.starts_with("tool.tx.step.response") => SchemaKind::TxStepResponse,
        n if n.starts_with("approval.") => SchemaKind::Approval,
        n if n.starts_with("ledger-record.") => SchemaKind::LedgerRecord,
        n if is_yaml(n) => {
            panic!("YAML examples are dispatched by their kind: field, not by name: {file_name}")
        }
        other => panic!("no schema mapped for example {other}; add one when adding an example"),
    }
}

#[test]
fn every_schema_compiles() {
    for kind in SchemaKind::all() {
        // A compile failure surfaces as a validation error against any input.
        let result = validate(*kind, &json!({}));
        if let Err(e) = &result {
            let text = e.to_string();
            assert!(
                !text.contains("does not compile") && !text.contains("not valid JSON"),
                "{kind:?} failed to compile: {text}"
            );
        }
    }
}

#[test]
fn every_example_validates() {
    let mut checked = 0;
    for entry in std::fs::read_dir(examples_dir()).expect("examples dir") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("file name")
            .to_owned();
        let raw = std::fs::read_to_string(&path).expect("read example");

        let (value, kind) = if is_yaml(&name) {
            let value = yaml_to_json(&raw).expect("example is valid YAML");
            let kind = manifest_kind(&value).expect("example declares a known kind");
            (value, kind)
        } else {
            let value: Value = serde_json::from_str(&raw).expect("example is valid JSON");
            (value, schema_for(&name))
        };

        validate(kind, &value).unwrap_or_else(|e| panic!("{name} does not validate: {e}"));
        checked += 1;
    }
    assert!(
        checked >= 15,
        "expected the full example set, saw {checked}"
    );
}

#[test]
fn manifests_reject_unknown_fields() {
    let mut value = yaml_to_json(
        &std::fs::read_to_string(examples_dir().join("agent.read-only.yaml")).unwrap(),
    )
    .unwrap();
    value["capabilities"]["net"]["egress_all"] = json!(true);
    assert!(validate(SchemaKind::AgentManifest, &value).is_err());
}

#[test]
fn service_control_entries_must_declare_their_footprint() {
    // effects.md §1: a tool whose D/M footprint cannot be computed is refused,
    // not guessed. The declaration is therefore required, not optional.
    let mut value = yaml_to_json(
        &std::fs::read_to_string(examples_dir().join("agent.linkedin-researcher.yaml")).unwrap(),
    )
    .unwrap();
    assert!(validate(SchemaKind::AgentManifest, &value).is_ok());

    let entry = &mut value["capabilities"]["system"]["services"]["control"][0];
    let object = entry.as_object_mut().unwrap();
    object.remove("added_effects");
    assert!(validate(SchemaKind::AgentManifest, &value).is_err());
}

#[test]
fn package_allowlist_refuses_globs_and_unsigned_repos() {
    let base = yaml_to_json(
        &std::fs::read_to_string(examples_dir().join("agent.linkedin-researcher.yaml")).unwrap(),
    )
    .unwrap();

    let mut globbed = base.clone();
    globbed["capabilities"]["system"]["packages"]["install"]["allow"][0]["name"] = json!("htop*");
    assert!(
        validate(SchemaKind::AgentManifest, &globbed).is_err(),
        "globs must be rejected"
    );

    let mut unsigned = base;
    unsigned["capabilities"]["system"]["packages"]["install"]["require_signatures"] = json!(false);
    assert!(validate(SchemaKind::AgentManifest, &unsigned).is_err());
}

#[test]
fn config_apply_requiring_approval_requires_an_approval_channel() {
    // ADR §6: `config.apply: requires_approval` requires `approvals.channel`.
    let mut value = yaml_to_json(
        &std::fs::read_to_string(examples_dir().join("agent.linkedin-researcher.yaml")).unwrap(),
    )
    .unwrap();
    value.as_object_mut().unwrap().remove("approvals");
    assert!(validate(SchemaKind::AgentManifest, &value).is_err());
}

#[test]
fn out_of_bounds_policy_never_offers_a_fall_through() {
    // effects.md §1 stage 3: arguments outside a pre_authorized scope require
    // an approval or are denied — the class ceiling is not an option.
    let mut value = yaml_to_json(
        &std::fs::read_to_string(examples_dir().join("agent.linkedin-researcher.yaml")).unwrap(),
    )
    .unwrap();
    value["capabilities"]["operations"]["hubspot.request"]["out_of_bounds"] =
        json!("class_ceiling");
    assert!(validate(SchemaKind::AgentManifest, &value).is_err());
}

#[test]
fn safety_vector_pins_external_effects_to_none_and_orders_the_rest() {
    let mut vector = json!({
        "root_config": "generation", "packages": "generation", "bootloader": "none",
        "kernel": "none", "service_state": "desired_state", "plugin_data": "dedicated_snapshot",
        "desktop_data": "dedicated_snapshot", "home_data": "none",
        "external_effects": "none", "recovery_requires": "oob_console"
    });
    assert!(validate(SchemaKind::SafetyVector, &vector).is_ok());

    // external_effects is definitionally none: no host may claim otherwise.
    vector["external_effects"] = json!("generation");
    assert!(validate(SchemaKind::SafetyVector, &vector).is_err());

    // service_state has its own two-value order; a host order is not valid here.
    let mut wrong_order = vector;
    wrong_order["external_effects"] = json!("none");
    wrong_order["service_state"] = json!("generation");
    assert!(validate(SchemaKind::SafetyVector, &wrong_order).is_err());
}

#[test]
fn approvals_bind_the_effect_set_and_the_jcs_digest() {
    // Gate 0 exit condition: approval and ledger schemas bind the exact effect
    // set and the RFC 8785 canonical operation digest.
    let raw = std::fs::read_to_string(examples_dir().join("approval.config-apply.json")).unwrap();
    let base: Value = serde_json::from_str(&raw).unwrap();
    assert!(validate(SchemaKind::Approval, &base).is_ok());

    for field in [
        "effect_set",
        "canonical_operation",
        "manifest_digest",
        "nonce",
        "single_use",
    ] {
        let mut stripped = base.clone();
        stripped.as_object_mut().unwrap().remove(field);
        assert!(
            validate(SchemaKind::Approval, &stripped).is_err(),
            "an approval without {field} must not validate"
        );
    }

    // The canonicalization is named, so a different one is a different record.
    let mut re_serialized = base.clone();
    re_serialized["canonical_operation"]["canonicalization"] = json!("json");
    assert!(validate(SchemaKind::Approval, &re_serialized).is_err());

    // Single-use is not a preference.
    let mut reusable = base;
    reusable["single_use"] = json!(false);
    assert!(validate(SchemaKind::Approval, &reusable).is_err());
}

#[test]
fn ledger_records_name_the_deciding_stage() {
    let raw =
        std::fs::read_to_string(examples_dir().join("ledger-record.denied-operation-policy.json"))
            .unwrap();
    let base: Value = serde_json::from_str(&raw).unwrap();
    assert!(validate(SchemaKind::LedgerRecord, &base).is_ok());

    let mut unstaged = base.clone();
    unstaged["decision"]
        .as_object_mut()
        .unwrap()
        .remove("stage");
    assert!(validate(SchemaKind::LedgerRecord, &unstaged).is_err());

    let mut invented_stage = base;
    invented_stage["decision"]["stage"] = json!("vibes");
    assert!(validate(SchemaKind::LedgerRecord, &invented_stage).is_err());
}

#[test]
fn v2_request_schemas_reject_unknown_fields_and_missing_required_keys() {
    let propose = json!({
        "idempotency_key": "k",
        "changes": [{"path": "/etc/nixos/configuration.nix", "content": ""}]
    });
    assert!(validate(SchemaKind::ConfigProposeRequest, &propose).is_ok());

    let mut missing_key = propose.clone();
    missing_key
        .as_object_mut()
        .unwrap()
        .remove("idempotency_key");
    assert!(validate(SchemaKind::ConfigProposeRequest, &missing_key).is_err());

    let mut unknown = propose;
    unknown["extra"] = json!(1);
    assert!(validate(SchemaKind::ConfigProposeRequest, &unknown).is_err());

    let status = json!({"tx_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV"});
    assert!(validate(SchemaKind::TxStatusRequest, &status).is_ok());
    assert!(validate(SchemaKind::TxStatusRequest, &json!({})).is_err());
}

#[test]
fn v2_response_schemas_reject_malformed_states_and_digests() {
    let status = json!({
        "tx_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "state": "PROPOSED",
        "effect_set": ["D"],
    });
    assert!(validate(SchemaKind::TxStatusResponse, &status).is_ok());

    let mut bad_state = status.clone();
    bad_state["state"] = json!("INVENTED");
    assert!(validate(SchemaKind::TxStatusResponse, &bad_state).is_err());

    let step = json!({
        "tx_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "state": "TESTING",
    });
    assert!(validate(SchemaKind::TxStepResponse, &step).is_ok());

    let mut propose = json!({
        "tx_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "diff": "x",
        "test_plan": {"adapter": "nix", "steps": ["test"]},
        "affected_resources": ["root_config"],
        "base_revision": {
            "etc_git_commit": "abc",
            "config_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        }
    });
    assert!(validate(SchemaKind::ConfigProposeResponse, &propose).is_ok());
    propose["base_revision"]["config_digest"] = json!("deadbeef");
    assert!(validate(SchemaKind::ConfigProposeResponse, &propose).is_err());
}
