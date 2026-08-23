# Gate 0 evidence — split-process spike

**Recorded:** 2026-08-23 · commit range `a4a7129..HEAD` on `claude/agentbed-gate-0-spike-lvfzfj`
**Environment:** Linux 6.18.44 x86_64, Ubuntu 24.04 container, rustc 1.94.1, running as a normal user (root in this container; the broker is *not* root-dependent at Gate 0 by design).

`docs/roadmap.md` Gate 0 exit evidence: *"docs merged after external review; spike builds and passes an RPC fuzz smoke test + the forged-request test."* This file records the second half.

## What was run

```
cargo fmt --all -- --check     clean
cargo clippy --workspace --all-targets -- -D warnings     clean
cargo test --workspace         88 tests, 0 failures
```

Per-binary:

| Test binary | Tests | Covers |
|---|---|---|
| `agentbed-protocol` (unit) | 22 | framing, strict JSON, envelope shape, digests |
| `proto/tests/jcs_conformance.rs` | 5 | RFC 8785 vectors, UTF-16 key order, ECMAScript number forms |
| `agentbed-schemas/tests/examples_validate.rs` | 10 | every shipped example validates; footprint/glob/approval-channel/out-of-bounds negatives |
| `agentbed-broker` (unit) | 25 | identity, policy ladder (all five stages), safety order, quota, adapter, host probe |
| `broker/tests/transport.rs` | 6 | socket permissions, peer credentials, per-frame fail-closed rules |
| `broker/tests/system_info.rs` | 7 | served call, binding, schema conformance, stage-3 denial, quota veto |
| **`broker/tests/forged_gateway.rs`** | **7** | **Gate 0 exit test (a)** |
| **`broker/tests/rpc_fuzz_smoke.rs`** | **1** | **Gate 0 exit test (b)** — one function, many cases (see below) |
| `gw/tests/end_to_end.rs` | 5 | gateway → broker over a real socket |

## (a) The broker, not the gateway, is the authorization authority

`broker/tests/forged_gateway.rs`. Every request arrives **on the trusted socket with valid peer credentials** — `SO_PEERCRED` passes, the uid is allowed, `0600`/`0700` permissions are satisfied. Nothing at that layer distinguishes a forged gateway from the real one, and if peer credentials were treated as authorization every case below would succeed.

| Case | Result |
|---|---|
| no credential at all | refused, `invalid_request`, no identity attributed |
| unknown credential | refused, `unauthenticated` |
| genuinely revoked credential | refused, `unauthenticated`, indistinguishable from unknown |
| valid token + asserted `agent_id` | refused at the parser (unknown field); resolves to nobody |
| valid token + supplied verdict / effect set / manifest digest / operation digest / binding | each refused at the parser |
| valid token, manifest denies the op | refused at **stage 3**, bound to the digest of the manifest the broker loaded itself |
| valid token, manifest allows (control) | **served**; the audit record shows the handler acted as the identity the *token* named |
| any refusal | discloses no hostname, kernel, adapter, safety vector, or other agent's id |

The identity-confusion case is asserted through unknown-field rejection rather than a recognized-but-ignored hint field: adding such a field to test it would create the very thing the design forbids.

## (b) RPC fuzz smoke test

`broker/tests/rpc_fuzz_smoke.rs`, one test function (the panic hook is process-global; the previous hook is restored on both paths). Asserts: **never panics · never processes a partial frame · always fails closed.**

Cases, all deterministic (seeded xorshift):

- **Malformed bodies (20):** empty, `{`, bare `null` / `[]` / string / number, valid-JSON-wrong-envelope, unsupported version, unknown op, unknown field, duplicate keys, non-interoperable integer, wrong types, embedded NUL in the request id, invalid UTF-8, 5 000 levels of nesting, a 40 KB string, trailing content after a complete document.
- **Framing abuse:** declared lengths of `u32::MAX`, `MAX_FRAME + 1`, `0x8000_0000` — each answered once, then the connection closed because the stream position is no longer knowable; truncated length prefixes (1, 2, 3 bytes) — no response, no audit record.
- **Partial frames:** a valid frame followed by a truncated one yields exactly one result and exactly one audit record.
- **Byte-at-a-time delivery:** nothing returned before the final byte, exactly one result after it, nothing further.
- **256 randomized rounds:** pure noise, lying length prefixes, bit-flipped valid requests, randomly truncated valid requests.

After all of it the broker still answers a valid authorized call. Panic count: 0.

## Two-process smoke run (release binaries)

```
$ agentbed-broker --socket .../broker.sock --tokens tokens.json --manifests manifests/ < /dev/null &
agentbed-broker: listening on .../broker.sock

$ AGENTBED_TOKEN=<agent token> agentbed-gw --socket .../broker.sock
  <- {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
  <- {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"system.info","arguments":{}}}
```

`system.info` returned, abridged:

```json
{"host":{"hostname":"vm","os_id":"ubuntu","os_version_id":"24.04",
         "kernel_release":"6.18.44-fc-v21","architecture":"x86_64"},
 "adapter":{"kind":"unresolved","resolved":false,"available_at_gate":1},
 "safety":{"root_config":"none","packages":"none","bootloader":"none","kernel":"none",
           "service_state":"none","plugin_data":"none","desktop_data":"none",
           "home_data":"none","external_effects":"none","recovery_requires":"oob_console"},
 "safety_source":"unresolved_adapter",
 "landlock":{"supported":false}}
```

The same gateway binary with an invalid token, against the same socket:

```
broker refused: code=Unauthenticated precedence_stage=-   isError: true
```

Broker audit lines from that run:

```
audit agent=mcp-client:gate0-reader peer_uid=0 op=system.info allowed=true  stage=- error=-               reason=authorized         req=gw-0000000000000001
audit agent=-                       peer_uid=0 op=system.info allowed=false stage=- error=Unauthenticated reason=token_not_resolved req=gw-0000000000000001
```

SIGTERM shut the broker down cleanly and removed the socket.

Note the honest readings in that output: the safety vector is `none` across the board with `safety_source: unresolved_adapter` because **no adapter ran** — that is the absence of a measurement, not a measurement of unrecoverability, and both refuse D/M steps. `landlock.supported: false` is this container's kernel/seccomp reality, reported rather than assumed; absent features degrade to deny.

## Findings recorded while building the spike

1. **`serde_json` float parsing is one ULP off correct rounding** on RFC 8785's own worked example (`333333333.33333329` → `0x41b3de4355555554`, where Rust's `str::parse`, glibc `strtod` and CPython agree on `...5555`). A digest that depends on which parser saw the frame first is not a binding, so `strict::parse` refuses fractional and exponent literals outright at Gate 0. No Gate 0 operation takes a fractional parameter; accepting them later means owning the text-to-double conversion.
2. **UTF-16 key ordering is not UTF-8 key ordering.** `serde_json::Map` is a `BTreeMap<String>`, i.e. UTF-8 byte order, which places U+1F600 *after* U+FB33; RFC 8785 places it before. The canonicalizer sorts explicitly and the conformance vector asserts that exact pair.
3. **Quota accounting conflated "no counter yet" with "poisoned lock"**, so every agent's first call read as exhausted. Found by its own unit test.
4. **The broker exited immediately under `StandardInput=null`** because its wait loop read stdin to EOF — found only by running the real binaries, not the library path. Replaced with blocked SIGTERM/SIGINT plus `sigwait`, established before any thread is spawned.
5. **A test fixture passed for the wrong reason:** the "revoked credential" case was refused as *unknown* because the token was absent from the store, and the two are deliberately indistinguishable on the wire. The store now takes full enrollments and the fixture enrols a genuinely revoked one.

## What this does not show

Gate 0 is a spike. Not demonstrated here, and not claimed: no transaction engine, no watchdog or decision log, no WAL, no approvals, no ledger hash chain or WORM anchor, no Landlock/seccomp helpers, no nftables, no connectors, no host adapter, and the broker runs unprivileged. Those are Gates 1–3 (`docs/roadmap.md`).
