# Broker RPC v1 — frozen contract

**Status:** Revision 1 · 2026-08-23 · normative for `proto/`, `broker/` and `gw/`. Frozen at Gate 0: the envelope, the operation set and the operation-digest construction below do not change within protocol version 1. A change is a new version, not an edit.

This file exists because two things in the spike are load-bearing for security and must be pinned before anything is built on them: **what the wire carries**, and **exactly which bytes an operation digest covers**. `docs/effects.md` §1 requires approvals, the ledger, connectors and replay checks to bind *those exact bytes* — a binding whose construction is implicit is not a binding.

## 1. Framing

```
[u32 big-endian length][length bytes UTF-8 JSON]
```

- Maximum body: 64 KiB, compared against the declared length **before allocation**.
- Zero-length bodies are refused; the stream position is still known, so the connection survives.
- Bodies are read with `read_exact`. A short read is a truncation: the partial bytes are discarded unparsed and the connection closes.
- Nothing is ever skipped to resynchronize. When the reader cannot know where the next frame begins, it closes.

## 2. Request envelope

```json
{
  "v": 1,
  "id": "<correlation id, graphic ASCII, 1..64 bytes>",
  "op": "system.info",
  "op_version": 1,
  "auth": { "token": "<bearer token>" },
  "params": {}
}
```

`deny_unknown_fields` at every level, duplicate object keys refused at any depth, and fractional/exponent number literals refused (see §5).

**What the envelope deliberately cannot carry.** There is no field for an agent id, manifest digest, effect set, canonical bytes, operation digest, or an authorization verdict. Each is a broker *output*, derived by the broker from its own inputs; the absence of a field is what makes a forged gateway unable to assert one (`docs/threat-model.md`, boundary 2). This is a structural property, not a check: adding such a field — even an "advisory" one — breaks the contract.

- `v` — protocol version. Absent or ≠ 1 → `invalid_request`. There is no negotiation.
- `op_version` — version of the operation's own contract. Absent defaults to 1; any other value → `unsupported_operation`. Within protocol v1 every operation is at version 1, so the field exists to make a future operation revision explicit and refusable rather than silently reinterpreted.
- `auth.token` — the caller's bearer token, relayed by the gateway, verified only by the broker.

## 3. Response envelope

Exactly one of `result` or `error` is present.

```json
{ "v": 1, "id": "…", "result": {"op": "system.info", "result": {…}},
  "binding": { "agent_id": "…", "manifest_digest": "sha256:…",
               "effect_set": ["R"], "operation_digest": "sha256:…" } }
```

```json
{ "v": 1, "id": "…", "error": { "code": "denied", "stage": "operation_policy" } }
```

`code` ∈ `invalid_request` | `unauthenticated` | `denied` | `quota_exhausted` | `approval_required` | `unsupported_operation` | `internal`. `stage` is present only when a policy stage decided, and names the `docs/effects.md` §1 ladder stage.

**Errors carry no prose.** Returned text is an information-disclosure channel; reasons belong in the broker's local observability output, not on the wire.

**Unauthenticated is uniform.** An unknown token, a revoked token and an expired token all return `unauthenticated` with no stage and no detail. Distinguishing them would confirm that a credential exists, or once did.

## 4. Operation digest — frozen construction

```
digest = SHA-256( "agentbed.operation.v1\0" || JCS(canonical_input) )
```

where `canonical_input` is:

```json
{ "operation": "system.info", "operation_version": 1, "arguments": {} }
```

and `JCS` is RFC 8785 canonicalization (§5).

Frozen elements, each for a reason:

| Element | Value | Why |
|---|---|---|
| Hash | SHA-256 | One algorithm per protocol version. The digest is rendered `sha256:<hex>` so the algorithm travels with the value and a future change is a visible migration, never a silent reinterpretation of stored approvals. |
| Domain separator | `agentbed.operation.v1\0` (ASCII, trailing NUL) | An unseparated hash of canonical JSON collides across *kinds* of object: a manifest, a ledger record and an operation could otherwise hash identically given identical bytes. The NUL terminates the prefix so no separator is a prefix of another. |
| Canonical input | `{operation, operation_version, arguments}` | Exactly the operation's identity and its validated arguments. |
| `operation_version` | included | A future `system.info` v2 with the same arguments must not produce the same digest as v1. |

**Excluded, deliberately:** the request id (a correlation label the caller chooses; including it would make two identical operations digest differently), the credential (a secret must never enter a value that is logged, displayed in an approval UI, or anchored), session or connection metadata (peer uid, pid, socket path — none of it is the operation), and anything asserted by the gateway (there is nothing to exclude, since the envelope cannot carry it).

**Construction order is normative.** The broker builds `canonical_input` **only after** strict decoding, typed projection into the operation's parameter struct, and JSON Schema validation of that projection. The digest therefore covers the operation *as it will be executed*, not the bytes the caller sent. Canonical bytes and digests are **never** accepted from the gateway, and never recomputed from a re-serialization — the bytes hashed are the bytes retained.

## 5. Strict decoding

- **Duplicate object keys** are refused at any depth. A permissive parser silently keeps one; if two components keep different ones, the digest no longer identifies what was executed.
- **Non-interoperable numbers** are refused: integers outside ±(2^53−1), and *all* fractional or exponent literals. The latter because the double a JSON parser yields for a fractional literal is parser-dependent in practice — `serde_json` parses RFC 8785's own worked example `333333333.33333329` one ULP away from the correctly-rounded value that Rust's `str::parse`, glibc `strtod` and CPython agree on. A digest that depends on which parser saw the frame first is not a binding.

  Consequence, stated as a rule rather than an accident: **no floating-point values appear in any canonically-digested operation.** Quantities carry integers with declared units. Revisiting this requires the broker to own the text-to-double conversion, and is a protocol-version change.

## 6. Ownership

| Concern | Lives in | Never in |
|---|---|---|
| Framing, envelopes, typed operations, wire error/stage enums, strict decoding, digest *rendering* | `proto/` (`agentbed-protocol`) | — |
| JCS canonicalization, digest *computation*, projection, schema validation, identity, manifests, policy, quotas, observability | `broker/` | `proto/`, `gw/` |
| MCP translation, transport | `gw/` | — |

`proto/` reaches no conclusions: both processes execute it independently and neither trusts the other's result. Security semantics — what is hashed, what is authorized — live with the authority that enforces them.
