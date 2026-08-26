# AGB-8 — L03 Watchdog decision authority and durable local protocol

**Issue:** AGB-8 · parent AGB-1 · GitHub #12
**Workflow:** `workflow:guarded`
**Baseline:** `01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c` (`origin/main`, verified 2026-08-25)
**Branch:** `agent/agb-8/l03-watchdog-decision-authority`
**PR:** https://github.com/AgentBed/agentbed/pull/24

The immutable PR ref and gate tracker (GitHub #12) are authoritative for the final candidate SHA after push; this document records repaired history and verification evidence without self-referencing a not-yet-published remote head.

## PLAN / RED / GREEN evidence

| Phase | SHA | Artifact |
|---|---|---|
| PLAN (sealed) | `57d17c2364784bdd6609ce6042c60b79bac6be13` | `plans/AGB-8/PLAN.md` |
| RED (initial) | `58ec7da309e435f456b800e14703d4e8536fb24a` | `plans/AGB-8/red-evidence.txt` + L03 RED tests/fixtures |
| GREEN (implementation) | `999a63f11850ed8077aa2e314be9ed1d15959c34` | initial production sources |
| GREEN (clippy repair) | `8072b68c3b02b64415a6e8330c2ba58e6806b1c8` | workspace `-D warnings` compliance |
| GREEN (initial RESULT) | `522c6bcc67a8406e89f7f18cab06367273206314` | initial `RESULT.md` |
| Review RED (scenario verification) | `94301f8b0aeefb0560df87caecba8271ec81ac16` | `plans/AGB-8/review-red-evidence.txt` + `watchdogd/tests/l03_review_repair.rs` |
| Review GREEN (production repair) | `b79ba1bec7d0b2b257a211f6470825b47d21a347` | production fixes for scenario findings F1–F11, F14, F15 |
| PLAN (fencing-safety ratification) | `83821e57aefbddd039d3c31b169a01285909ec12` | amended `plans/AGB-8/PLAN.md` (L-P decision `6179e45d`) |
| RED (fencing-safety) | `5b5d20798eee2eddd5f9e3110365dbd9945419c8` | `plans/AGB-8/fencing-safety-red-evidence.txt` + `watchdogd/tests/fencing_seam.rs`; `fencing_fixture.rs` deleted |
| GREEN (fencing-safety) | `02267647dc784a7ef96ca5eb4f65b26c39a2c162` | `WorkerGroupTag`, `UnavailableProcessGroupFencer`, sealed 9-step fence ordering; no real signaling |
| RED (constructor safety) | `dffd5bdbdece9c9a9e1a7753e76623a2c50b2e33` | `plans/AGB-8/fencing-constructor-red-evidence.txt` + additive `fencing_seam.rs` tests |
| GREEN (constructor safety) | `1a510e2360cec73426f2f1853875d60c9f66ee91` | removed `from_trusted_i32`; public constructors consume `WorkerGroupTag` only |
| **Rejected remote PR head** | `7c4e2b76f9ec68d910de650ebd73c31b1b084e8b` | last pushed head before bounded closure repair |
| RED (bounded closure G1–G3) | `fba4c7816b57d57be0bad76645c2faad05116181` | `watchdogd/tests/l03_scenario_round2.rs` (**12** tests) + `plans/AGB-8/scenario-round2-red-evidence.txt` |
| GREEN (G2/G3 durability) | `a04a4252015c190d36cb392157760cf7dcc6419e` | safe-mode latch on durability failures; same-directory temps; ambiguous temp refusal |
| RED (G3 sequencing micro) | `35a52f9db1f84a6b06e05290494cd86b278e5e92` | two additive sequencing oracle tests in `l03_scenario_round2.rs` (**14** total) |
| GREEN (G3 sequencing) | `25379c25bc6335c5dce59f703a0189332fe59f66` | dir-fsync before readback; safe-mode marker readback after parent fsync |
| GREEN (B1 topology G1) | `113b53341302a330162c1acee27f614c8927cde5` | substantive `ProductionTopologyProbe` in `topology.rs` |
| GREEN (B1 fidelity repair) | `6186d7a9136b0e0891701806e1c0bfc6d8346fe7` | protected-path metadata, unique mount lookups, shared evaluators |
| GREEN (B1 final fidelity) | `f572496e6ce1e3262d9cef3edbc7215b03267f74` | mountinfo whitelist escapes, `symlink_metadata` layout inspection, probe cleanup |
| **Rejected remote PR head (round 2)** | `f5532bf6ce7dd22261f5acd95d42113d22287838` | last pushed head before frozen G3 round-3 repair |
| RED (G3 round-3 read-dir ambiguity) | `87e722234cf0bf70a74814eeea6c8d9dbe9d17f5` | `plans/AGB-8/scenario-round3-read-dir-red-evidence.txt` + `g3_unreadable_parent_dir_temp_residue_is_ambiguous` in `l03_scenario_round2.rs` (**15** total) |
| GREEN (G3 round-3 fail-closed) | `721267e50017c65704f3d4ace0b61c1e9d5c81b7` | `ambiguous_temp_residue` in `durability_store.rs`: `read_dir` and per-entry errors map to ambiguity |

Accepted initial RED tests/fixtures/evidence remain byte-identical to `58ec7da` except superseded fencing paths. Fencing-safety RED oracle semantics preserved through `5b5d207`. Constructor-safety RED at `dffd5bdb` adds two additive `fencing_seam` tests (10 total).

**Constructor-safety repair:** removed `WorkerGroupTag::from_trusted_i32` and all production `.expect("trusted worker_group_tag")` paths. `SessionBind::new`, `LocalRequest::request_lease_renewal`, and `LocalRequest::heartbeat` now require a validated `WorkerGroupTag`; test/broker call sites use `try_from_raw` via `valid_worker_group_tag` fixture helper.

## Scenario verification (rejected head) and closure mapping

Independent scenario verifier run `00951621-3f57-4a94-ab48-5c13bebf5381` at exact rejected SHA `7c4e2b76f9ec68d910de650ebd73c31b1b084e8b` returned **NEEDS_FIXES** with three blockers:

| Blocker | Finding at `7c4e2b7` | Closure evidence |
|---|---|---|
| **G1 — production topology proof** | `ProductionTopologyProbe` was a stub; no mount/device/ownership/durability proof | RED `fba4c781` → GREEN `113b533`, `6186d7a`, `f572496`; `l03_scenario_round2` G1 4/4; topology lib unit tests 19/19 |
| **G2 — durability failures not latching safe mode** | post-write fsync/rename/dir-fsync failures returned typed errors without in-memory safe-mode latch | GREEN `a04a425` (`latch_safe_mode_best_effort`); `l03_scenario_round2` G2 4/4 |
| **G3 — cross-directory temp + ambiguity** | epoch/safe-mode temps under store root; ambiguous legacy temps not refused; durability order wrong; `read_dir` failure treated as nonblocking | GREEN `a04a425` (same-target-dir temps, ambiguity refusal); micro-RED `35a52f9` → GREEN `25379c2` (dir-fsync before readback, marker readback); round-3 RED `87e7222` → GREEN `721267e` (`read_dir` and per-entry enumeration errors fail closed); `l03_scenario_round2` G3 **7/7** |

Independent scenario verifier run `623845d0-c905-4add-a25a-4156162decb3` initially returned **APPROVED** at `f5532bf6ce7dd22261f5acd95d42113d22287838`, but its own follow-up identified `read_dir` failure as nonblocking. Coordinator reproduced parent mode `0300` (write+execute, no read) where directory enumeration failed `EACCES` while same-directory `O_EXCL` temp creation succeeded and stale temp residue remained — superseding approval for frozen G3 only; other G1/G2/fencing findings from that run remain valid evidence.

Full closure matrix: `l03_scenario_round2` **15/15** at accepted local implementation head (G2 4/4 + G3 7/7 = **11/11**).

## Critical fencing incident — root cause and safe design

**Root cause:** Prior GREEN introduced `ProductionProcessGroupFencer` with `libc::kill(-pgid, …)` and a spawned `fencing_fixture.rs` test that could signal `kill(-1)` on shared developer/login hosts.

**Ratified safe design (L-P `6179e45d`):**

- Library-only `watchdogd` — no daemon, no live signaling in L03.
- `WorkerGroupTag` (`u32` newtype) on the wire for opaque correlation only; never passed to syscalls.
- `UnavailableProcessGroupFencer` in production: `FenceError::Unavailable`, `group_alive` resolves ambiguity toward still-alive; authority fails closed with no `BEGIN_*`.
- `ProcessGroupFence::signal` has no caller-supplied pgid/target.
- Hermetic ordering via injected fakes: Term → bounded_wait(Term) → AfterTerm consumed → [if alive: Kill → bounded_wait(Kill) → AfterKill absent] → zero jobs → recovery authority.
- Real process termination deferred to a later daemon-owned cgroup-v2 lane.

## Scenario verification review repair — addressed findings

| Finding | Repair |
|---|---|
| **F1** | Decision log append fails closed without `set_len` truncation (`read_model.rs`) |
| **F2** | Runtime safe-mode latch persists `state/safe-mode.json` (`durability_store.rs`, `core.rs`) |
| **F3** | Epoch replacement uses unique `O_EXCL` temps in target parent dir; legacy `.tmp-epoch` ambiguity refused; stale epoch below high-water refused (not clamped); dir-fsync before readback |
| **F4** | Production topology verifier (`topology.rs`, `ProductionTopologyProbe`) — see current behavior below |
| **F5** | `UnavailableProcessGroupFencer: ProcessGroupFence`; fence wait failure latches safe mode |
| **F6** | Arming refused while any transaction remains armed |
| **F7** | Moved base and expired arming deadline rechecked at decision time |
| **F8** | Accepted-stream `SO_PEERCRED` via `StreamPeerAuth` during `SessionBind`; session capability derived at bind; post-bind requests validate stored capability and monotonic counter (not continuous peer-credential re-derivation); socket read/write timeouts |
| **F9** | Arm epoch must meet durable high-water authority, not session-only |
| **F10** | Decision log reader rejects decreasing epoch |
| **F11** | Append uses `O_NOFOLLOW` and post-write `Durability` fsync seam |
| **F14** | Additive manifest checks validated (not ignored) |
| **F15** | Late lease renewal refused; corrupt log at decision enters safe mode |

## Current production behavior (accepted local head)

**Topology (`ProductionTopologyProbe`):** before arming, proves the supplied path is the exact sealed mount at `/var/lib/agentbed/watchdog` using unique `/proc/self/mountinfo` evidence (validated `major:minor`, whitelisted octal escapes only, absolute mount points). Rejects lexical traversal and `symlink_metadata` symlink components on the store path and every protected domain (`/`, `/nix`, `/nix/store`, broker state, rollback). Requires each protected path to exist with actual `symlink_metadata().dev()`; compares mount ID and `st_dev` against the store (no mountinfo device fabrication). Inspects known existing subdirs/files via `symlink_metadata` only (`NotFound` = optional absent); enforces root-owned dir `0700`, regular file modes, `nlink==1`. Proves writable same-directory atomic replacement: O_EXCL temp → write → file fsync → rename → parent dir fsync → readback → verified cleanup (RAII on error paths). Ordinary temp directories without an exact mount entry receive `OrdinaryDirectoryFallback`.

**Durability / safe mode:** epoch and safe-mode markers use unique same-target-directory `O_EXCL` temps; ambiguous legacy temp residue is refused — both initial `read_dir(parent)` errors and per-entry enumeration errors in `ambiguous_temp_residue` map to ambiguity (fail closed), not silent skip; durability order is file fsync → atomic rename → parent dir fsync → readback. Post-write durability failures call `latch_safe_mode_best_effort()` (in-memory latch + best-effort durable marker persist) while preserving the original typed `RpcError`/`Durability` response.

## Scope delivered

Hermetic L03 watchdog decision authority: durable append-only decision log and epoch high-water store; fail-closed safe-mode and external-floor handling; injected topology/durability/process-group/job/invariant interfaces; framed authenticated local RPC (`SessionBind` with `worker_group_tag` → `SessionEstablished` → five production request types); broker narrow client stub (wire DTOs only); Nix protected-resource rejection for `/var/lib/agentbed/broker/state`; `UnavailableProcessGroupFencer` (no syscall fencing); production topology probe; `fencing_seam.rs` (10 tests); closure scenario round 2 (15 tests); failure matrix (54 tests); review repair suite (19 tests); topology lib unit tests (19).

Production paths:

- `watchdogd/src/{core,durability_store,error,fencing,interfaces,peercred,read_model,session,topology,worker_group_tag,lib}.rs`
- `watchdogd/src/rpc/{mod,protocol,server}.rs`
- `broker/src/watchdog/{mod,client}.rs`, `broker/src/lib.rs`, `broker/Cargo.toml`
- `adapters/nix/src/protected.rs`
- `watchdogd/Cargo.toml`, `Cargo.lock`

**Deleted:** `watchdogd/tests/fencing_fixture.rs` (dangerous real-signal fixture).

## Acceptance traceability

| AC | Evidence |
|---|---|
| **L03-AC01** | `watchdogd/src/topology.rs` (`ProductionTopologyProbe`); `watchdogd/src/interfaces.rs`; `adapters/nix/src/protected.rs`; `watchdogd/tests/l03_failure_matrix.rs` topology matrix; `watchdogd/tests/l03_scenario_round2.rs` G1 (4); topology lib tests (19); `adapters/nix/tests/l03_protected_broker_state.rs` (6 tests) |
| **L03-AC02** | `watchdogd/src/read_model.rs`, `watchdogd/src/core.rs`; `broker/src/watchdog/client.rs`; matrix AC02 + `broker/tests/l03_watchdog_client.rs`; review F1/F10/F11 |
| **L03-AC03** | `watchdogd/src/core.rs`, `watchdogd/src/durability_store.rs`, `watchdogd/src/read_model.rs`; epoch/safe-mode/fsync/rename/readback matrix; `l03_scenario_round2` G2/G3 (**11**); round-3 `g3_unreadable_parent_dir_temp_residue_is_ambiguous`; review F2/F3/F9 |
| **L03-AC04** | `watchdogd/src/rpc/{protocol,server}.rs`, `watchdogd/src/session.rs`, `watchdogd/src/peercred.rs`, `watchdogd/src/worker_group_tag.rs`; `broker/src/watchdog/client.rs`; frame codec, stream peercred, session bootstrap, counter/capability binding, socket permissions, unix round-trip; review F8 |
| **L03-AC05** | `watchdogd/src/core.rs` arming validation; matrix AC05; review F6/F7/F14 |
| **L03-AC06** | `watchdogd/src/core.rs` authority selection; broker WAL safe-mode tests; review F15 |
| **L03-AC07** | `watchdogd/src/session.rs`, `watchdogd/src/core.rs` lease/heartbeat with `worker_group_tag`; matrix AC07; review F15 late renewal |
| **L03-AC08** | `watchdogd/src/fencing.rs` (`UnavailableProcessGroupFencer`); `watchdogd/tests/fencing_seam.rs` (10/10); matrix AC08 + review F5; no spawned fixture; `watchdogd/src/**` contains no `libc::kill`/`waitpid`/`killpg`/`sigqueue` |
| **L03-AC09** | `watchdogd/tests/l03_failure_matrix.rs` (54) + `watchdogd/tests/l03_review_repair.rs` (19) + `watchdogd/tests/l03_scenario_round2.rs` (15) + `watchdogd/tests/fencing_seam.rs` (10) |
| **L03-AC10** | `watchdogd/src/interfaces.rs` + test fakes under `watchdogd/tests/common/`; PLAN §1 assumption 4 — no hostile-root boundary claimed |
| **L03-AC11** | This `RESULT.md`, bounded closure RED/GREEN chain above, verification commands below (bare/unpiped, exit 0) |
| **L03-AC12** | No L04/L05/live install/OOB/credentials/router changes; hermetic tests only; no merge |

## Verification commands (bare, unpiped, local head before push)

```text
# safety static scan (read-only)
watchdogd/tests/fencing_fixture.rs absent
no libc::kill / libc::waitpid / killpg / sigqueue / from_trusted_i32 in watchdogd/src/**

cargo test -p agentbed-watchdogd --lib                                    (19 expected)
cargo test -p agentbed-watchdogd --test l03_scenario_round2               (15 expected)
cargo test -p agentbed-watchdogd --test fencing_seam                      (10 expected)
cargo test -p agentbed-watchdogd --test l03_failure_matrix                (54 expected)
cargo test -p agentbed-watchdogd --test l03_review_repair                 (19 expected)
cargo test -p agentbed-broker --test l03_watchdog_client                  (9 expected)
cargo test -p agentbed-adapter-nix --test l03_protected_broker_state              (6 expected)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
git diff --check 01a4bf8c8de2a5cb4544bf74af9bb819c29adf1c..HEAD
```

DCO sign-off present on every commit `01a4bf8..HEAD`.

## Hard non-goals held (L03-AC12)

No L04 commit/recovery orchestration; no actual promotion/revert execution; no L05 OOB implementation; no live watchdog/systemd install/start; no live NixOS/profile/boot/network/firewall/Proxmox mutation; **no deployment, activation, or live host mutation occurred in this lane**; no credentials/keys/external epoch store; no Gate 2+; no router/reconciler/repository-setting changes; no merge.

## Residual gaps (explicit)

- **No live sealed-mount proof or provisioning** — topology verifier is production startup behavior; hermetic tests use temp paths and injected fakes. No claim that `/var/lib/agentbed/watchdog` exists as a dedicated mount on any developer host.
- **No hostile-root security boundary** — process independence and transaction self-protection only; malicious root can alter watchdog files; OOB observer is the backstop (H-02/H-05).
- **Real process termination** is not implemented in L03. Production L03 refuses recovery authority when fencing is unavailable; daemon-owned cgroup-v2 fencing is deferred to a later lane.
- **No OOB floor/signing** — external epoch floor is injected for hermetic mismatch tests only.
- **Temp-name uniqueness** uses `SystemTime` nanos in helper paths — adequate for hermetic L03 but not claimed as full live readiness hardening; follow-up hardening may adopt stronger entropy without changing authority semantics.
- No live dedicated-mount provisioning, systemd daemon, NixOS VM, power-loss, spare-node chaos, or OOB mirror evidence — deferred to later authorized lanes (L04–L08).
- Broker transaction engine does not yet wire full watchdog RPC orchestration (L04 scope).
- Gate 1 remains open; this lane delivers L03 hermetic proof only (PLAN §9).
