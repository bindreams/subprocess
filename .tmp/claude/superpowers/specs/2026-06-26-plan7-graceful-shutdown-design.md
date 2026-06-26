# Plan 7 — Graceful-escalation trio: design

**Status:** approved 2026-06-26. Builds on Plan 5 (`Child::kill_tree`/`terminate_tree`, the `contain()` model, `Attached`) and Plan 6 (foreign `Process`, the non-reaping `wait` death-watch, `wait::kill`, identity-walk `treewalk`). Parent spec: `2026-06-20-subprocess-design.md` §5.5/§5.6/§6.4. Closes `TODO.md` "Lifecycle / graceful shutdown (from Plan 5)".

## Goal

Add cooperative-then-forced shutdown to both the owned `Child` and the foreign `Process`: a soft signal, a bounded grace period, then a hard kill — race-free for lone processes, best-effort for trees, and honest (typed `Unsupported`) where a platform has no primitive.

## Surface

Every cell is an inherent method. ✓ = acts; **U** = returns `Error::Unsupported`.

| Method | Semantics | Owned `Child` | Foreign `Process` |
|---|---|---|---|
| `terminate()` | lone soft signal (SIGTERM), signal-only | Unix ✓ / Win **U** | Unix ✓ / Win **U** |
| `graceful_shutdown(d)` | lone soft→hard escalation | Unix ✓ / Win **U** | Unix ✓ / Win **U** |
| `terminate_tree()` | tree soft signal, signal-only | all-OS ✓ *(Plan 5)* | Unix ✓ / Win **U** |
| `kill_tree()` | tree hard sweep | all-OS ✓ *(Plan 5)* | all-OS ✓ *(new)* |
| `graceful_shutdown_tree(d)` | tree soft→hard escalation | all-OS ✓ | Unix ✓ / Win **U** |

New: `Child::{terminate, graceful_shutdown, graceful_shutdown_tree}`; `Process::{terminate, graceful_shutdown, terminate_tree, kill_tree, graceful_shutdown_tree}`; the shared atom `wait::terminate(id)`.

## Two governing principles

### 1. Lone ops are precise; tree ops are best-effort sweeps

Lone `kill`/`terminate`/`graceful_shutdown` are **race-free** (identity-bound: Linux `pidfd_send_signal`, Windows pinned handle, macOS reverify-then-signal with the one documented residual) and **surface real failures** — EPERM on a live process is `Err`, already-gone is `Ok`.

Tree ops are snapshot-based identity-walks. They cannot be atomic against a forking tree and do not surface per-process failures — the existing `TreeWalk` contract (`treewalk.rs:1-11`): it misses reparented orphans and broker escapes, and swallows per-process EPERM. Foreign tree teardown reuses `treewalk::{hard_kill, terminate, descendants}` wholesale, inheriting that contract. When a caller needs a guaranteed, failure-surfacing single-process action, that is what the lone ops are for.

### 2. The tree grace-wait is non-reaping, sweep-before-reap

`graceful_shutdown_tree`:

1. soft-signal the group (`terminate_tree`);
2. `block_until_exit(root, grace)` — **non-reaping** death-watch on the root;
3. `kill_tree()` — **unconditional** hard sweep (a no-op if the tree already drained);
4. **then** reap the root (owned `Child` only) → `ExitStatus`.

Step 2 must not reap, because the `pgid` containment mechanism's `killpg(pgid)` in step 3 targets the root's pid as the group id: if the root were reaped first, that pid could be recycled and `killpg` would hit a stranger's group. The non-reaping `block_until_exit` keeps the root un-reaped through the sweep, satisfying this uniformly across all mechanisms (cgroup/Job Object don't have the reuse hazard, but pay no cost). `shared_child`'s reaping `wait_timeout` cannot be used here — that is exactly the race Plan 6 built `block_until_exit` to avoid.

The sweep is unconditional (not "only on timeout") for leak-freedom: if the root exited gracefully but a descendant lingers, the sweep still collects it; if the whole tree drained, the sweep is a harmless no-op. Because the sweep's lone backstop (`shared_child.kill`) is a no-op on an already-exited root, the reaped `ExitStatus` still reflects the *original* graceful exit (signal 15 / exit 0), not a spurious SIGKILL.

The grace-wait observes the **root's** exit as the shutdown signal (systemd's "main pid exits = stopped" model), then force-sweeps any survivors. We do not death-watch the whole group's emptiness — no portable single primitive for that exists.

## Per-method semantics

### Lone soft signal — `terminate()`

Signal-only (no wait, no reap), mirroring `terminate_tree`'s signal-only contract. Routes through the new `wait::terminate(id)`:

- **Linux:** `pidfd_send_signal(pidfd, SIGTERM)` — identity-bound, closes the check-then-act PID-reuse race against a concurrent reap.
- **macOS:** reverify identity, then `kill(pid, SIGTERM)`; ESRCH → `Ok`, EPERM → `Err` (the same small residual window as `wait::kill`).
- **Windows:** `Unsupported { op: "graceful terminate (SIGTERM-equivalent)", platform: "windows", detail: "Windows has no per-process graceful-termination signal; for a contained child use graceful_shutdown_tree (CTRL_BREAK to the group)" }`.

Owned `Child::terminate` and foreign `Process::terminate` both delegate to `wait::terminate(self.id)`. For an owned child the pid is already pinned by `shared_child`, so the identity-bound path is belt-and-suspenders, but using one atom keeps a single SIGTERM implementation.

### Lone escalation — `graceful_shutdown(grace)`

**`Child`** (`-> Result<ExitStatus>`):

```rust
wait::terminate(self.id)?;                       // SIGTERM (Windows: Unsupported, early Err)
if let Some(status) = self.wait_timeout(grace)? { // reaping wait — lone, no killpg hazard
    return Ok(status);                            // exited within grace (signal 15 / clean)
}
self.shared.kill().map_err(Error::Io)?;          // timeout → hard SIGKILL the root
self.wait()                                      // reap → ExitStatus (signal 9)
```

**`Process`** (`-> Result<()>`): no `shared_child`, so the grace-wait is the non-reaping death-watch and there is no reap (we are not the parent):

```rust
wait::terminate(self.id)?;                                  // SIGTERM (Windows: Unsupported)
if wait::block_until_exit(self.id, Some(grace))? {
    return Ok(());                                          // exited within grace
}
wait::kill(self.id)                                         // timeout → hard SIGKILL
```

### Tree soft signal — `terminate_tree()`

**`Child`:** already exists (Plan 5) — `require_contained()?` then `Attached::terminate`, which uses the strongest mechanism (killpg / cgroup SIGTERM / Windows `CTRL_BREAK` to the spawned group / `TreeWalk` SIGTERM-walk). Unchanged.

**`Process`** (`-> Result<()>`): Unix → `treewalk::terminate(self.id)` (SIGTERM identity-walk over root + genuine descendants). Windows → `Unsupported`. The foreign case must **not** reuse `treewalk::terminate`'s Windows arm: that arm sends `CTRL_BREAK` to `root.pid()` as a process-group id, which is correct only for a child *we* spawned `CREATE_NEW_PROCESS_GROUP` — a foreign root shares no addressable group with us.

### Tree hard sweep — `kill_tree()`

**`Child`:** already exists (Plan 5). Unchanged.

**`Process`** (`-> Result<()>`): `treewalk::hard_kill(self.id)` (identity-walk `SIGKILL` / `TerminateProcess` over root + descendants), all-OS, best-effort → `Ok(())`.

### Tree escalation — `graceful_shutdown_tree(grace)`

**`Child`** (`-> Result<ExitStatus>`):

```rust
self.require_contained()?;                          // Unsupported if no actionable mechanism
self.terminate_tree()?;                             // group SIGTERM / CTRL_BREAK (signal-only)
let _ = wait::block_until_exit(self.id, Some(grace))?; // NON-reaping grace-wait on root
self.kill_tree()?;                                  // unconditional hard sweep BEFORE reap
self.wait()                                         // reap → ExitStatus
```

Works on all OSes: the Windows path uses `CTRL_BREAK` (owned group) for the soft step, `WaitForSingleObject` for the grace-wait, and `TerminateJobObject` + lone backstop for the sweep.

**`Process`** (`-> Result<()>`): Unix-only because `terminate_tree` is:

```rust
self.terminate_tree()?;                             // Windows → Unsupported (early)
let _ = wait::block_until_exit(self.id, Some(grace))?;
self.kill_tree()                                    // all-OS best-effort sweep
```

## Cross-cutting decisions

- **API shape:** inherent methods returning typed `Unsupported` on platforms without a primitive — matching the shipped `kill_tree`/`terminate_tree` precedent and the crate's "operations report the achieved outcome" rule (parent spec §3, §7). **Not** a `cfg`-gated `unix::ChildExt` extension trait. Rationale: this is platform-divergent *behavior* (operation/output), where the crate uses runtime-reported outcomes; `cfg` extension traits are reserved for platform-specific *inputs* (e.g. a Unix `uid()` builder). Matches Go (`Process.Signal(SIGTERM)` → "not supported by windows"); rejects Python's silent `terminate() == TerminateProcess` hard-kill on Windows.
- **Return types:** `Child::graceful_shutdown{,_tree}` → `Result<ExitStatus>` (the terminating signal already distinguishes graceful exit from force-kill); `Process::graceful_shutdown{,_tree}` → `Result<()>` (foreign — the kernel hands exit status only to the real parent); all `terminate`/`*_tree` signal/sweep ops → `Result<()>`.
- **Escalation proceeds past a failed/ignored soft signal.** SIGTERM returning ESRCH (already gone) is success → the wait observes exit at once. SIGTERM the target *ignores* → grace expires → SIGKILL. A soft-signal `Err` that is *not* already-gone (EPERM) aborts the escalation with that `Err` (you cannot even begin).
- **`grace` is a relative `Duration`** (Python/.NET/Go convention). `Duration::ZERO` = "signal, poll once, escalate." An overflowing `Duration` saturates to unbounded, per `block_until_exit`/`wait_timeout`.
- **Linux kernel floor:** the tree grace-wait and the foreign lone grace-wait go through `block_until_exit`, which requires pidfd (kernel ≥ 5.3) and returns `Unsupported` below — the floor Plan 6 already set. `Child::graceful_shutdown` (lone) uses `shared_child.wait_timeout`, which does **not** need pidfd; only its SIGTERM step uses `wait::terminate` (pidfd on Linux).

## Reuse map

| Need | Reused from |
|---|---|
| non-reaping grace-wait | `wait::block_until_exit` (Plan 6) |
| lone hard kill (race-free) | `wait::kill` (Plan 6) |
| foreign tree SIGTERM-walk | `treewalk::terminate` (Unix arm) |
| foreign tree hard sweep | `treewalk::hard_kill` |
| descendant enumeration | `treewalk::descendants` |
| owned group soft/hard | `Attached::{terminate, hard_kill}` (Plan 5) |
| owned lone reaping wait | `Child::wait_timeout`/`wait`, `shared_child.kill` (Plan 5) |
| child identity for grace-wait | `Child::id() -> ProcessId` (Plan 6) |

## File structure

- `src/wait.rs`: add `pub(crate) fn terminate(id: ProcessId) -> Result<(), Error>` façade (mirrors `kill`).
- `src/wait/linux.rs`: `terminate` via `pidfd_send_signal(&pidfd, Signal::TERM)` reusing `open_verified`.
- `src/wait/macos.rs`: `terminate` via reverify + `nix kill(pid, SIGTERM)` (mirrors its `kill`).
- `src/wait/windows.rs`: `terminate` → `Unsupported`.
- `src/child/graceful.rs` (new submodule of `child`, like `child/lifecycle.rs`): `Child::{terminate, graceful_shutdown, graceful_shutdown_tree}`.
- `src/process/graceful.rs` (new submodule of `process`): `Process::{terminate, graceful_shutdown, terminate_tree, kill_tree, graceful_shutdown_tree}`. (Promote `process.rs` to a `process/` directory module, mirroring `child/`.)
- `testbin/main.rs`: new Unix-only `control-block-ignore-term` mode — `libc::signal(SIGTERM, SIG_IGN)` then connect/tag/block, for the escalation tests.
- `tests/`: graceful tests for `Child` and `Process` (lone graceful, lone escalation, tree graceful, tree escalation, Windows `Unsupported`).

## Test strategy

No time-based synchronization. Death is proven only by a real exit event — control-socket EOF/`ConnectionReset` or an inspected `ExitStatus`/reap — never by sleep, poll loop, or wall-clock.

- **Lone graceful path:** `control-block` child (dies on default-disposition SIGTERM) + a long grace (e.g. 30 s, as the Plan 6 `wait_timeout` "observes-an-exit" tests use). Assert exit by **signal 15** on Unix (proves SIGTERM, not SIGKILL); socket EOFs. The long grace is the safety bound on a child that exits promptly — never the synchronization.
- **Lone escalation path:** `control-block-ignore-term` child + a tiny grace. Assert exit by **signal 9** (escalated). Deterministic and flake-free: the child *never* honors SIGTERM, so it always escalates regardless of grace length — correctness comes from the exit signal, not the clock.
- **Tree graceful / escalation:** the `spawn-grandchild` helpers (owned contained tree; foreign tree via a spawned-then-taken-foreign root). Assert the whole tree EOFs; for the owned contained case assert the root's `ExitStatus`.
- **Windows divergence:** assert `terminate`/`graceful_shutdown` (lone) and `Process::terminate_tree`/`graceful_shutdown_tree` (foreign soft-tree) return `Error::Unsupported`; assert `Process::kill_tree` tears down a foreign tree.
- macOS: expect the per-OS divergences already recorded (zombie/privileged-process `proc_pidinfo` gaps) — scope macOS-divergent assertions per-OS, budget a CI round-trip.

## Out of scope (deferred to `TODO.md`)

- A Windows per-process graceful path (WM_CLOSE to GUI windows / console `CTRL_C` choreography) — none is reliable for an arbitrary console child; `Unsupported` is the honest answer.
- Surfacing per-descendant failures from tree sweeps (would require a structured multi-error result; the best-effort `TreeWalk` contract stands).
- Waiting for whole-tree emptiness rather than root exit as the grace signal (no portable primitive).
