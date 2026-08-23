//! The policy precedence ladder of `docs/effects.md` §1.
//!
//! Every call is evaluated through all five stages **in order**; any stage may
//! refuse or add an approval requirement, and later stages never relax an
//! earlier one:
//!
//! 1. **F / explicit deny** — class F, or a globally forbidden operation.
//!    Terminal.
//! 2. **Safety minimum** — any D/M member of the effect set below the
//!    per-resource minimum, or on `none`. Terminal.
//! 3. **Explicit operation policy** — `deny` refuses; `requires_approval`
//!    *always* requires a per-call approval **regardless of the class ceiling**;
//!    a matching `pre_authorized` scope allows. Arguments outside the bounds do
//!    **not** fall through to the class ceiling: they require an approval, or
//!    are refused if the operation declares `out_of_bounds: deny`.
//! 4. **Class ceiling** — applies **only** to operations with no explicit
//!    policy at all.
//! 5. **Quota** — a mandatory final veto over every outcome above; exhaustion
//!    refuses even an approved or pre-authorized call. Stage 5 *admits*
//!    atomically rather than reading a counter: see [`CallAdmission`].
//!
//! Two of these rules exist because review rounds found the opposite behaviour
//! in an earlier draft (`docs/review-responses/codex-004.md`, `-005.md`): a low
//! class must never bypass an explicit `requires_approval`, and an out-of-bounds
//! pre-authorization must never fall back to the ceiling. Both have their own
//! tests below, phrased so that a regression fails loudly rather than quietly
//! allowing.

use crate::manifest::AgentManifest;
use crate::safety::{meets_minimum, MinSafety, Resource};
use agentbed_protocol::dto::system_info::SafetyVector;
use agentbed_protocol::wire::{DecisionStage, EffectClass, ErrorCode};
use serde::Deserialize;

/// What a manifest says about one named operation (stage 3).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationPolicy {
    policy: PolicyKind,
    #[serde(default)]
    bounds: Option<serde_json::Value>,
    #[serde(default)]
    out_of_bounds: Option<OutOfBounds>,
}

/// The three explicit policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyKind {
    /// Refuse the operation outright.
    Deny,
    /// Always require a per-call approval.
    RequiresApproval,
    /// Allow within declared bounds.
    PreAuthorized,
}

/// What happens to arguments outside a `pre_authorized` scope.
///
/// Note what is absent: there is no variant meaning "fall back to the class
/// ceiling". The type makes the codex-005 finding unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutOfBounds {
    /// Require a per-call approval. The default.
    #[default]
    RequiresApproval,
    /// Refuse.
    Deny,
}

impl OperationPolicy {
    /// Which policy this is.
    #[must_use]
    pub fn kind(&self) -> PolicyKind {
        self.policy
    }

    /// Whether this is a pre-authorization.
    #[must_use]
    pub fn is_pre_authorized(&self) -> bool {
        self.policy == PolicyKind::PreAuthorized
    }

    /// Whether bounds were declared.
    #[must_use]
    pub fn has_bounds(&self) -> bool {
        self.bounds.is_some()
    }

    /// Out-of-bounds handling, defaulting to requiring an approval.
    #[must_use]
    pub fn out_of_bounds(&self) -> OutOfBounds {
        self.out_of_bounds.unwrap_or_default()
    }
}

/// A call as the broker computed it, ready to be judged.
///
/// Every field is broker-derived. Note in particular `effect_set`: it is the
/// *computed* set for these arguments, not the tool's static minimum, and a
/// call whose set could not be computed never reaches here — it is refused
/// upstream rather than guessed at (`docs/effects.md` §1).
#[derive(Debug, Clone)]
pub struct CallDescriptor {
    /// Operation name, e.g. `system.info`.
    pub op: &'static str,
    /// The exact computed effect set.
    pub effect_set: Vec<EffectClass>,
    /// Resources this call touches, with the class of the touch. Only D and M
    /// members are subject to the safety minimum.
    pub footprint: Vec<(Resource, EffectClass)>,
    /// Whether the operation is globally forbidden (class F): kernel,
    /// bootloader, storage layout, firewall management plane, the watchdog and
    /// its closure, Agentbed self-modification.
    pub globally_forbidden: bool,
    /// For an operation under a `pre_authorized` policy: whether these exact
    /// arguments fall inside the declared bounds. `None` for operations with no
    /// pre-authorization to be inside or outside of.
    pub within_bounds: Option<bool>,
}

impl CallDescriptor {
    /// A read-only call touching nothing.
    #[must_use]
    pub fn read_only(op: &'static str) -> Self {
        CallDescriptor {
            op,
            effect_set: vec![EffectClass::R],
            footprint: Vec::new(),
            globally_forbidden: false,
            within_bounds: None,
        }
    }

    /// The highest class in the set, by the R < D < M < E order. F is outside
    /// the ordering and is handled at stage 1, never ranked.
    #[must_use]
    pub fn highest_class(&self) -> Option<EffectClass> {
        self.effect_set
            .iter()
            .copied()
            .filter(|c| *c != EffectClass::F)
            .max_by_key(|c| rank(*c))
    }

    fn contains_forbidden(&self) -> bool {
        self.globally_forbidden || self.effect_set.contains(&EffectClass::F)
    }
}

/// Rank in the R < D < M < E order. F is deliberately absent from the ordering.
fn rank(class: EffectClass) -> u8 {
    match class {
        EffectClass::R => 0,
        EffectClass::D => 1,
        EffectClass::M => 2,
        EffectClass::E => 3,
        // Unreachable through `highest_class`, which filters F out first. Given
        // the maximum rank so that any accidental comparison fails closed
        // rather than treating F as the weakest class.
        EffectClass::F => u8::MAX,
    }
}

/// Stage 5's admission step.
///
/// Deliberately an *action*, not a reading. An earlier version handed the
/// ladder a `calls_used` snapshot and charged the counter afterwards, which is
/// a time-of-check-to-time-of-use race: the broker serves connections on
/// concurrent threads, so two callers could both observe `limit - 1`, both be
/// allowed, and both execute — two calls against a budget of one. Passing the
/// capability to admit, rather than a number to compare, makes that shape
/// impossible to write.
///
/// Implementations must check and count under a single critical section, and
/// must refuse when accounting is unavailable.
pub trait CallAdmission {
    /// Atomically admit one call against `limit`, returning `false` when the
    /// budget is exhausted. `None` means no ceiling is declared.
    fn try_admit(&self, limit: Option<u64>) -> bool;
}

/// The outcome of evaluating the ladder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The call may proceed.
    Allow,
    /// The call is refused.
    Refuse {
        /// Wire error for the caller.
        code: ErrorCode,
        /// Which stage decided.
        stage: DecisionStage,
        /// Machine-readable reason for the ledger.
        reason: &'static str,
    },
}

impl Decision {
    fn refuse(code: ErrorCode, stage: DecisionStage, reason: &'static str) -> Self {
        Decision::Refuse {
            code,
            stage,
            reason,
        }
    }

    /// Whether the call may proceed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    /// The deciding stage, when a stage decided.
    #[must_use]
    pub fn stage(&self) -> Option<DecisionStage> {
        match self {
            Decision::Allow => None,
            Decision::Refuse { stage, .. } => Some(*stage),
        }
    }
}

/// Evaluate one call against one manifest, host vector and quota state.
///
/// Pure: no I/O, no clock, no globals. The broker's request path calls this and
/// acts on the answer; tests call it directly with hand-built inputs.
#[must_use]
pub fn evaluate(
    call: &CallDescriptor,
    manifest: &AgentManifest,
    host: &SafetyVector,
    admission: &dyn CallAdmission,
) -> Decision {
    // Stage 1 — F / globally forbidden. Terminal.
    if call.contains_forbidden() {
        return Decision::refuse(
            ErrorCode::Denied,
            DecisionStage::ForbiddenClass,
            "forbidden_class",
        );
    }

    // Stage 2 — safety minimum, uniform for D and M. Terminal.
    if let Some(reason) = failing_safety(call, manifest.min_safety(), host) {
        return Decision::refuse(ErrorCode::Denied, DecisionStage::SafetyMinimum, reason);
    }

    // Stage 3 — explicit operation policy, if any.
    let provisional = match manifest.operation_policy(call.op) {
        Some(policy) => match stage_three(policy, call) {
            Ok(()) => Ok(()),
            Err(decision) => return apply_quota_to_refusal(decision),
        },
        // Stage 4 — class ceiling, ONLY for operations with no explicit policy.
        None => stage_four(call, manifest),
    };
    if let Err(decision) = provisional {
        return apply_quota_to_refusal(decision);
    }

    // Stage 5 — mandatory final veto over every allow above, including a
    // pre-authorized one. Reached only when stages 1-4 allowed, so a refused
    // call never consumes an agent's budget.
    stage_five(manifest, admission)
}

fn failing_safety(
    call: &CallDescriptor,
    min: &MinSafety,
    host: &SafetyVector,
) -> Option<&'static str> {
    for (resource, class) in &call.footprint {
        if !matches!(class, EffectClass::D | EffectClass::M) {
            continue;
        }
        if !meets_minimum(*resource, host, min) {
            return Some("resource_below_safety_minimum");
        }
    }
    None
}

fn stage_three(policy: &OperationPolicy, call: &CallDescriptor) -> Result<(), Decision> {
    match policy.kind() {
        PolicyKind::Deny => Err(Decision::refuse(
            ErrorCode::Denied,
            DecisionStage::OperationPolicy,
            "operation_denied_by_manifest",
        )),
        // Never bypassable by a low class: this is not gated on the effect set
        // at all (codex-004, "explicit requires_approval bypassed by class
        // ceiling"). Approvals exist at Gate 2; until then this is terminal.
        PolicyKind::RequiresApproval => Err(Decision::refuse(
            ErrorCode::ApprovalRequired,
            DecisionStage::OperationPolicy,
            "operation_requires_approval",
        )),
        PolicyKind::PreAuthorized => match call.within_bounds {
            Some(true) => Ok(()),
            // Out of bounds stays inside stage 3 (codex-005). Falling through
            // to the ceiling here would let a generous max_unapproved_class
            // silently widen a narrow pre-authorization.
            Some(false) | None => match policy.out_of_bounds() {
                OutOfBounds::RequiresApproval => Err(Decision::refuse(
                    ErrorCode::ApprovalRequired,
                    DecisionStage::OperationPolicy,
                    "arguments_outside_pre_authorized_bounds",
                )),
                OutOfBounds::Deny => Err(Decision::refuse(
                    ErrorCode::Denied,
                    DecisionStage::OperationPolicy,
                    "arguments_outside_pre_authorized_bounds",
                )),
            },
        },
    }
}

fn stage_four(call: &CallDescriptor, manifest: &AgentManifest) -> Result<(), Decision> {
    let Some(highest) = call.highest_class() else {
        // An empty effect set means the set could not be computed. Refused,
        // not guessed (`docs/effects.md` §1).
        return Err(Decision::refuse(
            ErrorCode::Denied,
            DecisionStage::ClassCeiling,
            "effect_set_not_computable",
        ));
    };
    let ceiling = manifest.max_unapproved_class().unwrap_or(EffectClass::R);
    if rank(highest) <= rank(ceiling) {
        Ok(())
    } else {
        Err(Decision::refuse(
            ErrorCode::ApprovalRequired,
            DecisionStage::ClassCeiling,
            "class_above_unapproved_ceiling",
        ))
    }
}

fn stage_five(manifest: &AgentManifest, admission: &dyn CallAdmission) -> Decision {
    if admission.try_admit(manifest.calls_per_day()) {
        Decision::Allow
    } else {
        Decision::refuse(
            ErrorCode::QuotaExhausted,
            DecisionStage::Quota,
            "call_quota_exhausted",
        )
    }
}

/// A refusal from stages 1–4 is already terminal; the quota veto cannot turn it
/// into an allow, so the earlier stage keeps the attribution.
fn apply_quota_to_refusal(decision: Decision) -> Decision {
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::load_agent_manifest;
    use agentbed_protocol::dto::system_info::{
        DataSafety, ExternalEffectsSafety, HostSafety, RecoveryRequires, ServiceStateSafety,
    };
    use std::io::Write as _;

    /// Build a manifest by writing YAML to a temp file and loading it through
    /// the real loader — so these tests exercise the schema and the semantic
    /// checks too, not a hand-built struct that skips them.
    fn manifest(capabilities: &str) -> AgentManifest {
        let dir = std::env::temp_dir().join(format!(
            "agentbed-policy-test-{}-{:p}",
            std::process::id(),
            capabilities
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("agent.yaml");
        let yaml = format!(
            "kind: agent\nversion: 1\nname: policy-fixture\nruntime: mcp-client\n\
             identity:\n  mcp_client_id: mcp-client:policy-fixture\n  owner: lp\n\
             capabilities:\n{capabilities}"
        );
        let mut file = std::fs::File::create(&path).expect("write manifest");
        file.write_all(yaml.as_bytes()).expect("write manifest");
        let loaded = load_agent_manifest(&path).expect("manifest loads");
        let _ = std::fs::remove_dir_all(&dir);
        loaded
    }

    fn generous_host() -> SafetyVector {
        SafetyVector {
            root_config: HostSafety::Generation,
            packages: HostSafety::Generation,
            bootloader: HostSafety::None,
            kernel: HostSafety::None,
            service_state: ServiceStateSafety::DesiredState,
            plugin_data: DataSafety::DedicatedSnapshot,
            desktop_data: DataSafety::DedicatedSnapshot,
            home_data: DataSafety::DedicatedSnapshot,
            external_effects: ExternalEffectsSafety::None,
            recovery_requires: RecoveryRequires::RemoteReboot,
        }
    }

    /// An admission that always succeeds — for the stage 1-4 tests, which must
    /// not depend on quota at all.
    struct AlwaysAdmits;

    impl CallAdmission for AlwaysAdmits {
        fn try_admit(&self, _limit: Option<u64>) -> bool {
            true
        }
    }

    /// A counting admission with the same semantics as the real ledger, so the
    /// stage-5 tests exercise the contract rather than a stub that always says
    /// no.
    #[derive(Default)]
    struct CountingAdmission {
        used: std::cell::Cell<u64>,
    }

    impl CountingAdmission {
        fn with_used(used: u64) -> Self {
            CountingAdmission {
                used: std::cell::Cell::new(used),
            }
        }
    }

    impl CallAdmission for CountingAdmission {
        fn try_admit(&self, limit: Option<u64>) -> bool {
            if let Some(limit) = limit {
                if self.used.get() >= limit {
                    return false;
                }
            }
            self.used.set(self.used.get().saturating_add(1));
            true
        }
    }

    /// An admission that records whether stage 5 was reached at all.
    #[derive(Default)]
    struct RecordingAdmission {
        consulted: std::cell::Cell<bool>,
    }

    impl CallAdmission for RecordingAdmission {
        fn try_admit(&self, _limit: Option<u64>) -> bool {
            self.consulted.set(true);
            true
        }
    }

    fn fresh() -> AlwaysAdmits {
        AlwaysAdmits
    }

    fn config_apply() -> CallDescriptor {
        CallDescriptor {
            op: "config.apply",
            effect_set: vec![EffectClass::D],
            footprint: vec![(Resource::RootConfig, EffectClass::D)],
            globally_forbidden: false,
            within_bounds: None,
        }
    }

    #[test]
    fn stage1_forbidden_class_beats_every_permission() {
        // The manifest is as permissive as a manifest can be; F is still F.
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: E\n  operations:\n    kernel.patch:\n      policy: pre_authorized\n      bounds: {}\n",
        );
        let call = CallDescriptor {
            op: "kernel.patch",
            effect_set: vec![EffectClass::D],
            footprint: vec![],
            globally_forbidden: true,
            within_bounds: Some(true),
        };
        let decision = evaluate(&call, &manifest, &generous_host(), &fresh());
        assert_eq!(decision.stage(), Some(DecisionStage::ForbiddenClass));
    }

    #[test]
    fn stage2_safety_minimum_refuses_before_any_permission_is_consulted() {
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: M\n  operations:\n    config.apply:\n      policy: pre_authorized\n      bounds: {}\n",
        );
        let mut host = generous_host();
        host.root_config = HostSafety::None;

        let mut call = config_apply();
        call.within_bounds = Some(true);

        let decision = evaluate(&call, &manifest, &host, &fresh());
        assert_eq!(
            decision.stage(),
            Some(DecisionStage::SafetyMinimum),
            "a pre-authorized D step on a none resource must still be refused"
        );
    }

    #[test]
    fn stage3_requires_approval_is_not_bypassed_by_a_low_class() {
        // codex-004's authorization bug: D <= max_unapproved_class M, so a
        // ceiling-first ladder would have allowed this.
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: M\n  operations:\n    config.apply:\n      policy: requires_approval\n",
        );
        let decision = evaluate(&config_apply(), &manifest, &generous_host(), &fresh());
        assert_eq!(
            decision,
            Decision::Refuse {
                code: ErrorCode::ApprovalRequired,
                stage: DecisionStage::OperationPolicy,
                reason: "operation_requires_approval",
            }
        );
    }

    #[test]
    fn stage3_pre_authorization_overrides_a_lower_class_ceiling() {
        // A scoped E pre-authorization deliberately outranks the ceiling:
        // stage 3 runs before stage 4.
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: M\n  operations:\n    hubspot.request:\n      policy: pre_authorized\n      bounds: {}\n",
        );
        let call = CallDescriptor {
            op: "hubspot.request",
            effect_set: vec![EffectClass::E],
            footprint: vec![],
            globally_forbidden: false,
            within_bounds: Some(true),
        };
        assert_eq!(
            evaluate(&call, &manifest, &generous_host(), &fresh()),
            Decision::Allow
        );
    }

    #[test]
    fn stage3_out_of_bounds_never_falls_through_to_the_ceiling() {
        // codex-005's finding. The ceiling here is E, so a fall-through would
        // ALLOW an out-of-bounds call — which is exactly what must not happen.
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: E\n  operations:\n    hubspot.request:\n      policy: pre_authorized\n      bounds: {}\n",
        );
        let call = CallDescriptor {
            op: "hubspot.request",
            effect_set: vec![EffectClass::E],
            footprint: vec![],
            globally_forbidden: false,
            within_bounds: Some(false),
        };
        let decision = evaluate(&call, &manifest, &generous_host(), &fresh());
        assert_eq!(
            decision,
            Decision::Refuse {
                code: ErrorCode::ApprovalRequired,
                stage: DecisionStage::OperationPolicy,
                reason: "arguments_outside_pre_authorized_bounds",
            }
        );
    }

    #[test]
    fn stage3_out_of_bounds_deny_refuses_outright() {
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: E\n  operations:\n    hubspot.request:\n      policy: pre_authorized\n      bounds: {}\n      out_of_bounds: deny\n",
        );
        let call = CallDescriptor {
            op: "hubspot.request",
            effect_set: vec![EffectClass::E],
            footprint: vec![],
            globally_forbidden: false,
            within_bounds: Some(false),
        };
        let decision = evaluate(&call, &manifest, &generous_host(), &fresh());
        assert_eq!(decision.stage(), Some(DecisionStage::OperationPolicy));
        assert!(matches!(
            decision,
            Decision::Refuse {
                code: ErrorCode::Denied,
                ..
            }
        ));
    }

    #[test]
    fn stage4_applies_only_without_an_explicit_policy() {
        let manifest = manifest("  risk:\n    max_unapproved_class: R\n");
        assert_eq!(
            evaluate(
                &CallDescriptor::read_only("system.info"),
                &manifest,
                &generous_host(),
                &fresh()
            ),
            Decision::Allow
        );

        let decision = evaluate(&config_apply(), &manifest, &generous_host(), &fresh());
        assert_eq!(
            decision,
            Decision::Refuse {
                code: ErrorCode::ApprovalRequired,
                stage: DecisionStage::ClassCeiling,
                reason: "class_above_unapproved_ceiling",
            }
        );
    }

    #[test]
    fn stage4_refuses_a_call_whose_effect_set_could_not_be_computed() {
        let manifest = manifest("  risk:\n    max_unapproved_class: E\n");
        let call = CallDescriptor {
            op: "mystery.tool",
            effect_set: vec![],
            footprint: vec![],
            globally_forbidden: false,
            within_bounds: None,
        };
        assert_eq!(
            evaluate(&call, &manifest, &generous_host(), &fresh()).stage(),
            Some(DecisionStage::ClassCeiling)
        );
    }

    #[test]
    fn stage5_vetoes_both_ceiling_allows_and_pre_authorized_calls() {
        let by_ceiling =
            manifest("  risk:\n    max_unapproved_class: R\n  quotas:\n    calls_per_day: 2\n");
        let call = CallDescriptor::read_only("system.info");
        // One admission object across the calls: each allow consumes budget, so
        // the third call is refused by the same ladder that allowed the first
        // two — the counter is not a parameter the test can fake past.
        let admission = CountingAdmission::default();
        assert_eq!(
            evaluate(&call, &by_ceiling, &generous_host(), &admission),
            Decision::Allow
        );
        assert_eq!(
            evaluate(&call, &by_ceiling, &generous_host(), &admission),
            Decision::Allow
        );
        let exhausted = evaluate(&call, &by_ceiling, &generous_host(), &admission);
        assert_eq!(
            exhausted,
            Decision::Refuse {
                code: ErrorCode::QuotaExhausted,
                stage: DecisionStage::Quota,
                reason: "call_quota_exhausted",
            }
        );

        // "Quota exhaustion refuses even an approved or pre-authorized call."
        let pre_authorized = manifest(
            "  risk:\n    max_unapproved_class: R\n  quotas:\n    calls_per_day: 1\n  operations:\n    hubspot.request:\n      policy: pre_authorized\n      bounds: {}\n",
        );
        let scoped = CallDescriptor {
            op: "hubspot.request",
            effect_set: vec![EffectClass::E],
            footprint: vec![],
            globally_forbidden: false,
            within_bounds: Some(true),
        };
        assert_eq!(
            evaluate(
                &scoped,
                &pre_authorized,
                &generous_host(),
                &CountingAdmission::with_used(1),
            )
            .stage(),
            Some(DecisionStage::Quota)
        );
    }

    #[test]
    fn a_refused_call_never_consumes_quota() {
        // Stage 5 is reached only after stages 1-4 allow. Otherwise a hostile
        // caller could drain another agent's budget with calls that were always
        // going to fail.
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: M\n  quotas:\n    calls_per_day: 10\n  operations:\n    config.apply:\n      policy: deny\n",
        );
        let admission = RecordingAdmission::default();
        let decision = evaluate(&config_apply(), &manifest, &generous_host(), &admission);
        assert_eq!(decision.stage(), Some(DecisionStage::OperationPolicy));
        assert!(
            !admission.consulted.get(),
            "a stage-3 refusal must not reach the quota counter"
        );
    }

    #[test]
    fn an_earlier_refusal_keeps_its_attribution_when_quota_is_also_exhausted() {
        let manifest = manifest(
            "  risk:\n    max_unapproved_class: M\n  quotas:\n    calls_per_day: 0\n  operations:\n    config.apply:\n      policy: deny\n",
        );
        let decision = evaluate(
            &config_apply(),
            &manifest,
            &generous_host(),
            &CountingAdmission::with_used(99),
        );
        assert_eq!(
            decision.stage(),
            Some(DecisionStage::OperationPolicy),
            "stages 1-3 are terminal; the quota veto must not relabel them"
        );
    }

    #[test]
    fn an_unbounded_pre_authorization_is_refused_at_load_time() {
        let dir = std::env::temp_dir().join(format!("agentbed-unbounded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("agent.yaml");
        std::fs::write(
            &path,
            "kind: agent\nversion: 1\nname: unbounded\nruntime: mcp-client\n\
             identity:\n  mcp_client_id: mcp-client:unbounded\n  owner: lp\n\
             capabilities:\n  operations:\n    hubspot.request:\n      policy: pre_authorized\n",
        )
        .expect("write manifest");
        assert!(
            load_agent_manifest(&path).is_err(),
            "an unbounded scope must not compile"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
