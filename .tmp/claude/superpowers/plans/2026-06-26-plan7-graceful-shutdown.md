# Graceful-escalation trio (Plan 7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cooperative-then-forced shutdown (`terminate` / `graceful_shutdown` / `graceful_shutdown_tree`, plus foreign `terminate_tree`/`kill_tree`) to the owned `Child` and the foreign `Process`.

**Architecture:** A new `wait::terminate(id)` atom mirrors the existing `wait::kill` (Linux `pidfd_send_signal(SIGTERM)`, macOS reverify-then-`kill`, Windows `Unsupported`). `Child` graceful methods live in a new `src/child/graceful.rs` submodule; `Process` graceful methods in a new `src/process/graceful.rs` submodule. Lone escalation = soft signal → bounded wait → hard kill; the *tree* escalation uses the **non-reaping** `wait::block_until_exit` for the grace-wait so the hard sweep runs before the root is reaped (no `killpg`-after-reap race). Foreign tree teardown reuses the `treewalk` identity-walk.

**Tech Stack:** Rust (edition 2021, MSRV 1.87); `rustix` (Linux pidfd), `nix` (macOS/Unix signals + kqueue), `windows` (Win32); `shared_child`; in-crate `subprocess_testbin` helper.

## Global Constraints

- MSRV **1.87**, edition **2021**. `rustfmt` **max_width 120**. CI `prek` = `cargo fmt` + `cargo clippy --all-targets -- -D warnings` (lints: `clippy::all = warn`, so any warning fails CI) + external hooks. `Cargo.lock` is committed; tests run `--locked`.
- **No time-based synchronization in tests; no data races; no arbitrary loop limits.** Death/liveness is proven ONLY by a real event — control-socket EOF/`ConnectionReset` or an inspected `ExitStatus` signal — never by sleep, poll loop, or wall-clock. The escalation tests use a `SIGTERM`-ignoring child + `Duration::ZERO`, so escalation is deterministic (the child is alive at the single poll regardless of any duration).
- **`Error::Unsupported`** has fields `{ op: String, platform: &'static str, detail: String }`.
- **Return types:** `Child::graceful_shutdown{,_tree}` → `Result<ExitStatus, Error>`; `Process::graceful_shutdown{,_tree}` → `Result<(), Error>`; all `terminate` / `*_tree` signal/sweep ops → `Result<(), Error>`.
- **Lone ops are precise** (identity-bound, race-free, surface real failures: `EPERM` → `Err`, already-gone → `Ok`). **Tree ops are best-effort sweeps** (snapshot identity-walks; cannot be atomic against a forking tree; per-process failures not surfaced — the existing `TreeWalk` contract).
- **Tree grace-wait is non-reaping** (`wait::block_until_exit`), hard sweep BEFORE reap.
- **Windows:** lone `terminate`/`graceful_shutdown` and foreign `terminate_tree`/`graceful_shutdown_tree` return `Unsupported`; foreign `kill_tree` works on all platforms.
- **`grace` is a relative `Duration`**; `Duration::ZERO` = signal, poll once, escalate; an overflowing `Duration` saturates to unbounded (handled by `block_until_exit`/`wait_timeout`).
- **File org:** `foo.rs` + `foo/` submodule style (no `mod.rs`); integration tests in `tests/`, helpers in `tests/common/mod.rs`.
- **Where to run tests:** the dev host is Windows. Run `#[cfg(windows)]` + cross-platform tests on the host (`cargo test --test graceful`). Run `#[cfg(unix)]` tests on Linux via WSL:
  `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful'`
  CI covers all 6 cells (Windows/Linux/macOS × 2 arches). macOS runtime is CI-only; expect the recorded `proc_pidinfo` divergences (zombies/privileged) — these tests reap promptly and assert signals only on Unix, so they are unaffected.

---

### Task 1: `wait::terminate` atom + `Child::terminate()`

The shared SIGTERM primitive plus its first public consumer. `wait::terminate` is `pub(crate)`; it is exercised through the public `Child::terminate`.

**Files:**
- Modify: `src/wait.rs` (add the `terminate` façade)
- Modify: `src/wait/linux.rs` (add `terminate`)
- Modify: `src/wait/macos.rs` (add `terminate`)
- Modify: `src/wait/windows.rs` (add `terminate`)
- Create: `src/child/graceful.rs` (`Child::terminate`)
- Modify: `src/child.rs` (declare the `graceful` submodule)
- Create: `tests/graceful.rs` (first test)

**Interfaces:**
- Consumes: `wait/linux.rs::open_verified`, `rustix::process::{pidfd_send_signal, Signal}` (`Signal::TERM`); `nix::sys::signal::{kill, Signal::SIGTERM}`; `ProcessId`; `Child::{id, wait}`; `Error::Unsupported`.
- Produces: `crate::wait::terminate(id: ProcessId) -> Result<(), Error>`; `Child::terminate(&self) -> Result<(), Error>`.

- [ ] **Step 1: Write the failing tests** — create `tests/graceful.rs`:

```rust
//! Graceful-escalation trio integration tests (Child + Process). Death is proven only by a
//! real exit event — control-socket EOF/ConnectionReset or an inspected ExitStatus signal —
//! never by sleep, poll loop, or wall-clock. Escalation tests use a SIGTERM-ignoring child +
//! Duration::ZERO, so escalation is deterministic (the child is alive at the single poll).

#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
#[test]
fn child_terminate_sends_sigterm() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    let (child, mut sock) = common::spawn_blocker();
    child.terminate().expect("terminate sends SIGTERM");
    // Prove death by a real event: the control socket EOFs.
    let mut buf = [0u8; 1];
    match sock.read(&mut buf) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("expected EOF/ConnectionReset after SIGTERM, got {other:?}"),
    }
    // Reap and assert it died by SIGTERM (soft), NOT SIGKILL.
    let status = child.wait().expect("reap");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "control-block must die by SIGTERM, got {status:?}");
}

#[cfg(windows)]
#[test]
fn child_terminate_unsupported_on_windows() {
    let (child, _sock) = common::spawn_blocker();
    let err = child.terminate().expect_err("lone graceful terminate has no Windows primitive");
    assert!(matches!(err, subprocess::error::Error::Unsupported { .. }), "got {err:?}");
    child.kill().expect("cleanup");
    let _ = child.wait();
}
```

- [ ] **Step 2: Run to verify it fails**

Run (host): `cargo test --test graceful`
Expected: FAIL — compile error `no method named terminate found for ... Child`.

- [ ] **Step 3: Add the `wait::terminate` façade** — in `src/wait.rs`, after the `kill` façade (after line 37):

```rust
/// Send the graceful termination signal (`SIGTERM`) to the process with identity `id`,
/// identity-verified. Signal-only — does not wait or reap. Already-dead ⇒ `Ok`; a real
/// failure (no rights / `EPERM`) ⇒ `Err`. Windows has no per-process graceful signal ⇒
/// `Unsupported`.
pub(crate) fn terminate(id: ProcessId) -> Result<(), Error> {
    backend::terminate(id)
}
```

- [ ] **Step 4: Add the Linux backend** — in `src/wait/linux.rs`, after `kill` (after line 79):

```rust
pub(crate) fn terminate(id: ProcessId) -> Result<(), Error> {
    let Some(pidfd) = open_verified(id)? else { return Ok(()) };
    match pidfd_send_signal(&pidfd, Signal::TERM) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::SRCH) => Ok(()), // exited between re-verify and signal
        Err(e) => Err(Error::Io(std::io::Error::from(e))),
    }
}
```

- [ ] **Step 5: Add the macOS backend** — in `src/wait/macos.rs`, after `kill` (after line 87):

```rust
pub(crate) fn terminate(id: ProcessId) -> Result<(), Error> {
    use nix::sys::signal::{kill as nix_kill, Signal};
    use nix::unistd::Pid;
    // Re-verify identity immediately before signaling; the window to kill(2) is the same
    // irreducible best-effort window as `kill`, documented at the module head.
    if ProcessId::of(id.pid()) != Some(id) {
        return Ok(()); // gone (or recycled) => already-dead is success
    }
    debug_assert!(
        id.pid() <= i32::MAX as u32,
        "pid {} exceeds i32::MAX; signal target cast would truncate",
        id.pid()
    );
    match nix_kill(Pid::from_raw(id.pid() as i32), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(Error::Io(e.into())),
    }
}
```

- [ ] **Step 6: Add the Windows backend** — in `src/wait/windows.rs`, after `kill` (after line 88):

```rust
pub(crate) fn terminate(id: ProcessId) -> Result<(), Error> {
    let _ = id;
    Err(Error::Unsupported {
        op: "graceful terminate (SIGTERM-equivalent)".into(),
        platform: "windows",
        detail: "Windows has no per-process graceful-termination signal; for a contained \
                 child use graceful_shutdown_tree (CTRL_BREAK to the group)"
            .into(),
    })
}
```

- [ ] **Step 7: Create `src/child/graceful.rs`**:

```rust
//! `Child` graceful shutdown — the soft-then-hard escalation trio. A submodule of `child`
//! so it can reach `Child`'s private `shared`/`id`.

use super::Child;
use crate::error::Error;

impl Child {
    /// Send `SIGTERM` to the (lone) child — a cooperative request to exit. Signal-only: does
    /// not wait or reap. Identity-bound, so it cannot race a concurrent reap onto a recycled
    /// pid. Unix only — Windows has no per-process graceful signal and returns `Unsupported`
    /// (use [`graceful_shutdown_tree`](Child::graceful_shutdown_tree) for a contained child).
    pub fn terminate(&self) -> Result<(), Error> {
        crate::wait::terminate(self.id)
    }
}
```

- [ ] **Step 8: Declare the submodule** — in `src/child.rs`, after the `lifecycle` declaration (after line 21):

```rust
#[path = "child/graceful.rs"]
mod graceful;
```

- [ ] **Step 9: Run to verify it passes**

Run (host, Windows): `cargo test --test graceful child_terminate_unsupported_on_windows` → PASS.
Run (WSL, Linux): `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful child_terminate_sends_sigterm'` → PASS.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat: wait::terminate (SIGTERM) atom + Child::terminate (Plan 7)"
```

---

### Task 2: `Child::graceful_shutdown(grace)` + the SIGTERM-ignoring testbin mode

**Files:**
- Modify: `testbin/main.rs` (add `control-block-ignore-term` mode)
- Modify: `src/child/graceful.rs` (add `graceful_shutdown`)
- Modify: `tests/graceful.rs` (graceful + escalation + Windows tests)

**Interfaces:**
- Consumes: `crate::wait::terminate`; `Child::{wait, wait_timeout}`; `self.shared.kill()` (`shared_child`, `io::Result<()>`); `std::process::ExitStatus`; `std::time::Duration`.
- Produces: `Child::graceful_shutdown(&self, grace: Duration) -> Result<ExitStatus, Error>`.

- [ ] **Step 1: Write the failing tests** — append to `tests/graceful.rs`:

```rust
#[cfg(unix)]
#[test]
fn child_graceful_shutdown_graceful_path() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // control-block dies on default-disposition SIGTERM. The long grace is the safety bound on
    // a child that exits promptly — never the synchronization; correctness is the exit signal.
    let (child, mut sock) = common::spawn_blocker();
    let status = child.graceful_shutdown(Duration::from_secs(30)).expect("graceful_shutdown");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "graceful path must exit via SIGTERM, got {status:?}");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf); // dead — EOF
}

#[cfg(unix)]
#[test]
fn child_graceful_shutdown_escalates() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // This child installs SIG_IGN for SIGTERM, so it NEVER exits on the soft signal. With
    // Duration::ZERO the child is provably alive at the single poll → escalation to SIGKILL is
    // deterministic (no timing dependency at all). Because SIGTERM is ignored, SIGKILL is the
    // ONLY terminating signal the child can receive, so signal()==SIGKILL is unambiguous — do
    // not weaken control-block-ignore-term to honor SIGTERM or this assertion loses its meaning.
    let (child, mut sock) = common::spawn_control("control-block-ignore-term", &["R"], false);
    let status = child.graceful_shutdown(Duration::ZERO).expect("graceful_shutdown escalates");
    assert_eq!(status.signal(), Some(libc::SIGKILL), "SIGTERM-ignoring child must be force-killed, got {status:?}");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
}

#[cfg(windows)]
#[test]
fn child_graceful_shutdown_unsupported_on_windows() {
    use std::time::Duration;
    let (child, _sock) = common::spawn_blocker();
    let err = child.graceful_shutdown(Duration::from_secs(1)).expect_err("no Windows lone graceful");
    assert!(matches!(err, subprocess::error::Error::Unsupported { .. }), "got {err:?}");
    child.kill().expect("cleanup");
    let _ = child.wait();
}
```

- [ ] **Step 2: Run to verify it fails**

Run (host): `cargo test --test graceful` → FAIL: `no method named graceful_shutdown`. (The Unix escalation test additionally needs the new testbin mode — added next.)

- [ ] **Step 3: Add the SIGTERM-ignoring testbin mode** — in `testbin/main.rs`, add a new arm before the `other =>` arm (before line 182):

```rust
        #[cfg(unix)]
        "control-block-ignore-term" => {
            // Ignore SIGTERM, then behave exactly like control-block. graceful_shutdown must
            // escalate to SIGKILL; the test asserts death by signal 9, never via a timer.
            // SAFETY: installing SIG_IGN for SIGTERM has no preconditions and is always safe.
            unsafe {
                let _ = libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            let addr = &args[2];
            let tag = args.get(3).map(String::as_str).unwrap_or("?");
            let mut sock = std::net::TcpStream::connect(addr).unwrap();
            sock.write_all(tag.as_bytes()).unwrap();
            sock.flush().unwrap();
            let mut buf = [0u8; 1];
            let _ = sock.read(&mut buf);
        }
```

- [ ] **Step 4: Add `graceful_shutdown`** — replace the `use` block at the top of `src/child/graceful.rs` and add the method inside `impl Child`:

```rust
use std::process::ExitStatus;
use std::time::Duration;

use super::Child;
use crate::error::Error;
```

Add after `terminate`:

```rust
    /// Cooperative-then-forced lone shutdown: `SIGTERM`, wait up to `grace` for the child to
    /// exit, then `SIGKILL` if it has not — reaping either way and returning its `ExitStatus`.
    /// The status's terminating signal distinguishes a graceful exit from a forced one.
    /// Escalation proceeds even if the child ignores `SIGTERM`. Unix only; Windows returns
    /// `Unsupported`. `grace` is relative; `Duration::ZERO` signals, polls once, then escalates.
    pub fn graceful_shutdown(&self, grace: Duration) -> Result<ExitStatus, Error> {
        crate::wait::terminate(self.id)?; // SIGTERM (Windows: Unsupported, early return)
        if let Some(status) = self.wait_timeout(grace)? {
            return Ok(status); // exited within grace — reaped; a lone wait has no killpg hazard
        }
        self.shared.kill().map_err(Error::Io)?; // timeout → hard SIGKILL the root
        self.wait() // reap → ExitStatus (SIGKILL)
    }
```

- [ ] **Step 5: Run to verify it passes**

Run (host, Windows): `cargo test --test graceful child_graceful_shutdown_unsupported_on_windows` → PASS.
Run (WSL, Linux): `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful child_graceful_shutdown'` → 2 passed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat: Child::graceful_shutdown lone escalation + SIGTERM-ignoring testbin mode (Plan 7)"
```

---

### Task 3: `Child::graceful_shutdown_tree(grace)`

**Files:**
- Modify: `src/child/graceful.rs` (add `graceful_shutdown_tree`)
- Modify: `tests/common/mod.rs` (add the `spawn_grandchild` helper)
- Modify: `tests/graceful.rs` (tree teardown + graceful + escalation tests)

**Interfaces:**
- Consumes: `Child::{require_contained, terminate_tree, kill_tree, wait, id}`; `crate::wait::block_until_exit(id, Option<Duration>) -> Result<bool, Error>`.
- Produces: `Child::graceful_shutdown_tree(&self, grace: Duration) -> Result<ExitStatus, Error>`; `common::spawn_grandchild(contain: bool) -> (subprocess::Child, Vec<std::net::TcpStream>)`.

- [ ] **Step 1: Add the `spawn_grandchild` test helper** — in `tests/common/mod.rs`, append:

```rust
/// Spawn the `spawn-grandchild` helper (root tag "R" + one grandchild tag "G"), optionally
/// contained, and return the owned `Child` plus BOTH accepted sockets (the two tag reads prove
/// the 2-level tree is alive). The tree dies — and both sockets EOF — only when the whole tree
/// is torn down, so callers prove teardown by reading EOF on both, never by a timer.
pub fn spawn_grandchild(contain: bool) -> (subprocess::Child, Vec<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    let mut cmd = subprocess::Command::new();
    cmd.executable(testbin())
        .args(["subprocess_testbin", "spawn-grandchild", addr.as_str()]);
    if contain {
        cmd.contain();
    }
    let child = cmd.spawn().expect("spawn grandchild tree");
    let mut socks = Vec::new();
    for _ in 0..2 {
        let (mut s, _) = listener.accept().expect("accept");
        let mut tag = [0u8; 1];
        s.read_exact(&mut tag).expect("read tag");
        socks.push(s);
    }
    (child, socks)
}
```

- [ ] **Step 2: Write the failing tests** — append to `tests/graceful.rs`:

```rust
#[test]
fn child_graceful_shutdown_tree_tears_down_tree() {
    use std::io::Read;
    use std::time::Duration;
    // A contained 2-level tree (root R + grandchild G). The group's graceful signal
    // (SIGTERM / CTRL_BREAK) plus the hard sweep tear down BOTH; both sockets EOF. All OSes.
    let (child, mut socks) = common::spawn_grandchild(true);
    child.graceful_shutdown_tree(Duration::from_secs(30)).expect("tree graceful");
    for (i, s) in socks.iter_mut().enumerate() {
        let mut buf = [0u8; 1];
        match s.read(&mut buf) {
            Ok(0) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("tree member {i} not torn down: {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn child_graceful_shutdown_tree_graceful_root_sigterm() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // A contained control-block root that honors SIGTERM: the group signal makes it exit;
    // the root's reaped status is SIGTERM (15), not escalated.
    let (child, mut sock) = common::spawn_control("control-block", &["R"], true);
    let status = child.graceful_shutdown_tree(Duration::from_secs(30)).expect("tree graceful");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "root must exit via SIGTERM, got {status:?}");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
}

#[cfg(unix)]
#[test]
fn child_graceful_shutdown_tree_escalates() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // A contained SIGTERM-ignoring root: the group SIGTERM is ignored, so with Duration::ZERO
    // the root is provably alive at the poll and the hard sweep (kill_tree) SIGKILLs it. SIGKILL
    // is the only terminating signal it can receive (SIGTERM ignored), so the assertion is
    // unambiguous.
    let (child, mut sock) = common::spawn_control("control-block-ignore-term", &["R"], true);
    let status = child.graceful_shutdown_tree(Duration::ZERO).expect("tree escalates");
    assert_eq!(status.signal(), Some(libc::SIGKILL), "ignored SIGTERM must escalate to SIGKILL, got {status:?}");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
}
```

- [ ] **Step 3: Run to verify it fails**

Run (host): `cargo test --test graceful child_graceful_shutdown_tree_tears_down_tree` → FAIL: `no method named graceful_shutdown_tree`.

- [ ] **Step 4: Add `graceful_shutdown_tree`** — in `src/child/graceful.rs`, add inside `impl Child` after `graceful_shutdown`:

```rust
    /// Cooperative-then-forced shutdown of the contained tree: send the group its graceful
    /// signal (`SIGTERM` via `killpg`/cgroup, or `CTRL_BREAK` to the job/console group), wait
    /// up to `grace` for the **root** to exit, then hard-sweep any survivors and reap the root.
    /// Returns the root's `ExitStatus`. Requires an actionable containment mechanism (errors
    /// `Unsupported` otherwise — use [`graceful_shutdown`](Child::graceful_shutdown) for a lone
    /// child). Works on all platforms.
    ///
    /// The grace-wait is **non-reaping** (watches the root's exit without collecting it), so the
    /// subsequent hard sweep runs while the root's pid — and thus the `killpg` group id — is
    /// still valid; reaping first could let `killpg` hit a recycled group. The sweep is
    /// unconditional but a no-op once the tree has drained, so a graceful exit's status is
    /// preserved (the lone backstop no-ops on the already-dead root).
    pub fn graceful_shutdown_tree(&self, grace: Duration) -> Result<ExitStatus, Error> {
        // Fail fast before sending any signal. terminate_tree/kill_tree re-check this guard
        // internally; the redundancy is intentional so an uncontained child errors up front.
        self.require_contained()?;
        self.terminate_tree()?; // group SIGTERM / CTRL_BREAK (signal-only)
        let _ = crate::wait::block_until_exit(self.id, Some(grace))?; // NON-reaping grace-wait on root
        self.kill_tree()?; // unconditional hard sweep BEFORE reap (no-op if already drained)
        self.wait() // reap → ExitStatus
    }
```

- [ ] **Step 5: Run to verify it passes**

Run (host, Windows): `cargo test --test graceful child_graceful_shutdown_tree_tears_down_tree` → PASS.
Run (WSL, Linux): `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful child_graceful_shutdown_tree'` → 3 passed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat: Child::graceful_shutdown_tree (non-reaping grace-wait, sweep-before-reap) (Plan 7)"
```

---

### Task 4: `Process::terminate()` + `Process::graceful_shutdown(grace)` (foreign lone)

**Files:**
- Create: `src/process/graceful.rs` (`Process::{terminate, graceful_shutdown}`)
- Modify: `src/process.rs` (declare the `graceful` submodule)
- Modify: `tests/graceful.rs` (foreign lone tests)

**Interfaces:**
- Consumes: `crate::wait::{terminate, block_until_exit, kill}`; `Process`'s private `id` field; `std::time::Duration`.
- Produces: `Process::terminate(&self) -> Result<(), Error>`; `Process::graceful_shutdown(&self, grace: Duration) -> Result<(), Error>`.

- [ ] **Step 1: Write the failing tests** — append to `tests/graceful.rs`. (These spawn an owned `Child`, take it foreign via `Process::from_pid`, act by identity, then reap the OWNED handle to inspect the signal.)

```rust
#[cfg(unix)]
#[test]
fn process_terminate_sends_sigterm() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    let (child, mut sock) = common::spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    p.terminate().expect("foreign terminate");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf); // dead — EOF
    let status = child.wait().expect("reap");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "got {status:?}");
}

#[cfg(unix)]
#[test]
fn process_graceful_shutdown_graceful_path() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    let (child, mut sock) = common::spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    p.graceful_shutdown(Duration::from_secs(30)).expect("foreign graceful");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
    let status = child.wait().expect("reap"); // owned handle reaps; confirm SIGTERM (graceful)
    assert_eq!(status.signal(), Some(libc::SIGTERM), "got {status:?}");
}

#[cfg(unix)]
#[test]
fn process_graceful_shutdown_escalates() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // SIGTERM is ignored → SIGKILL is the only terminating signal the child can receive, so the
    // reaped status is unambiguously SIGKILL (do not weaken control-block-ignore-term).
    let (child, mut sock) = common::spawn_control("control-block-ignore-term", &["R"], false);
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    p.graceful_shutdown(Duration::ZERO).expect("foreign escalates");
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
    let status = child.wait().expect("reap");
    assert_eq!(status.signal(), Some(libc::SIGKILL), "got {status:?}");
}

#[cfg(windows)]
#[test]
fn process_lone_graceful_unsupported_on_windows() {
    use std::time::Duration;
    let (child, _sock) = common::spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    assert!(matches!(p.terminate(), Err(subprocess::error::Error::Unsupported { .. })));
    assert!(matches!(
        p.graceful_shutdown(Duration::from_secs(1)),
        Err(subprocess::error::Error::Unsupported { .. })
    ));
    child.kill().expect("cleanup");
    let _ = child.wait();
}
```

- [ ] **Step 2: Run to verify it fails**

Run (host): `cargo test --test graceful process_lone_graceful_unsupported_on_windows` → FAIL: `no method named terminate found for ... Process`.

- [ ] **Step 3: Create `src/process/graceful.rs`**:

```rust
//! Foreign `Process` graceful shutdown — the soft-then-hard escalation trio over a process we
//! do not own (no stdio, no reap). A submodule of `process` so it can reach the private `id`.
//! Lone ops are identity-bound and surface real failures; tree ops are best-effort identity-
//! walk sweeps (the `TreeWalk` contract).

use std::time::Duration;

use super::Process;
use crate::error::Error;

impl Process {
    /// Send `SIGTERM` to the foreign process — a cooperative request to exit. Signal-only.
    /// Identity-bound (Linux `pidfd_send_signal`; macOS reverify-then-`kill`). Already-dead ⇒
    /// `Ok`; a real failure (`EPERM`) ⇒ `Err`. Unix only; Windows returns `Unsupported`.
    pub fn terminate(&self) -> Result<(), Error> {
        crate::wait::terminate(self.id)
    }

    /// Cooperative-then-forced lone shutdown of the foreign process: `SIGTERM`, wait up to
    /// `grace` for it to exit, then `SIGKILL` if it has not. No `ExitStatus` — the kernel hands
    /// exit status only to the real parent. Escalation proceeds even if `SIGTERM` is ignored.
    /// Unix only; Windows returns `Unsupported`. `grace` is relative; `ZERO` signals, polls
    /// once, then escalates.
    pub fn graceful_shutdown(&self, grace: Duration) -> Result<(), Error> {
        crate::wait::terminate(self.id)?; // SIGTERM (Windows: Unsupported)
        if crate::wait::block_until_exit(self.id, Some(grace))? {
            return Ok(()); // exited within grace
        }
        crate::wait::kill(self.id) // timeout → hard SIGKILL (no reap — not the parent)
    }
}
```

- [ ] **Step 4: Declare the submodule** — in `src/process.rs`, after the imports (after line 10, `use crate::identity::{ProcessId, RawPid};`):

```rust
#[path = "process/graceful.rs"]
mod graceful;
```

- [ ] **Step 5: Run to verify it passes**

Run (host, Windows): `cargo test --test graceful process_lone_graceful_unsupported_on_windows` → PASS.
Run (WSL, Linux): `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful process_terminate process_graceful_shutdown'` → 3 passed.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
git add -A
git commit -m "feat: Process::terminate + graceful_shutdown (foreign lone escalation) (Plan 7)"
```

---

### Task 5: `Process::{terminate_tree, kill_tree, graceful_shutdown_tree}` (foreign tree)

**Files:**
- Modify: `src/process/graceful.rs` (add the three tree methods)
- Modify: `tests/graceful.rs` (foreign tree tests)

**Interfaces:**
- Consumes: `crate::containment::treewalk::{terminate(root) -> Result<(), Error>, hard_kill(root)}`; `crate::wait::block_until_exit`; `common::spawn_grandchild` (Task 3).
- Produces: `Process::terminate_tree(&self) -> Result<(), Error>`; `Process::kill_tree(&self) -> Result<(), Error>`; `Process::graceful_shutdown_tree(&self, grace: Duration) -> Result<(), Error>`.

- [ ] **Step 1: Write the failing tests** — append to `tests/graceful.rs`:

```rust
#[test]
fn process_kill_tree_tears_down_tree() {
    use std::io::Read;
    // An UNcontained 2-level tree (root R + grandchild G). Take the root foreign and kill_tree
    // it: the identity-walk (snapshot-then-kill) reaches both. Both sockets EOF. All OSes.
    let (child, mut socks) = common::spawn_grandchild(false);
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    p.kill_tree().expect("kill_tree");
    for (i, s) in socks.iter_mut().enumerate() {
        let mut buf = [0u8; 1];
        match s.read(&mut buf) {
            Ok(0) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("tree member {i} not torn down: {other:?}"),
        }
    }
    let _ = child.wait(); // reap the owned root (grandchild is reaped by init)
}

#[cfg(unix)]
#[test]
fn process_graceful_shutdown_tree_tears_down_tree() {
    use std::io::Read;
    use std::time::Duration;
    let (child, mut socks) = common::spawn_grandchild(false);
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    p.graceful_shutdown_tree(Duration::from_secs(30)).expect("foreign tree graceful");
    for (i, s) in socks.iter_mut().enumerate() {
        let mut buf = [0u8; 1];
        match s.read(&mut buf) {
            Ok(0) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("tree member {i} not torn down: {other:?}"),
        }
    }
    let _ = child.wait();
}

#[cfg(windows)]
#[test]
fn process_soft_tree_unsupported_on_windows() {
    use std::time::Duration;
    let (child, _sock) = common::spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    assert!(matches!(p.terminate_tree(), Err(subprocess::error::Error::Unsupported { .. })));
    assert!(matches!(
        p.graceful_shutdown_tree(Duration::from_secs(1)),
        Err(subprocess::error::Error::Unsupported { .. })
    ));
    child.kill().expect("cleanup");
    let _ = child.wait();
}
```

- [ ] **Step 2: Run to verify it fails**

Run (host): `cargo test --test graceful process_kill_tree_tears_down_tree` → FAIL: `no method named kill_tree found for ... Process`.

- [ ] **Step 3: Add the three tree methods** — in `src/process/graceful.rs`, add inside `impl Process` after `graceful_shutdown`:

```rust
    /// Best-effort hard sweep of the foreign process's tree: an identity-walk that re-verifies
    /// each `(pid, ppid)` before `SIGKILL`/`TerminateProcess`, root then descendants. Cannot be
    /// atomic against a forking tree and does not surface per-process failures — the `TreeWalk`
    /// contract. All platforms. For a guaranteed, failure-surfacing single-process kill use
    /// [`kill`](Process::kill).
    pub fn kill_tree(&self) -> Result<(), Error> {
        crate::containment::treewalk::hard_kill(self.id);
        Ok(())
    }

    /// Best-effort graceful (`SIGTERM`) sweep of the foreign process's tree (identity-walk, root
    /// then descendants). Signal-only. Unix only: Windows has no per-process graceful signal and
    /// a foreign process shares no addressable group with us, so this returns `Unsupported`
    /// there (use [`kill_tree`](Process::kill_tree) for a hard sweep).
    pub fn terminate_tree(&self) -> Result<(), Error> {
        #[cfg(unix)]
        {
            crate::containment::treewalk::terminate(self.id)
        }
        #[cfg(windows)]
        {
            let _ = self.id;
            Err(Error::Unsupported {
                op: "foreign tree graceful terminate".into(),
                platform: "windows",
                detail: "Windows has no per-process graceful signal, and a foreign process \
                         shares no addressable process group with us; use kill_tree for a hard \
                         identity-walk sweep"
                    .into(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.id;
            Ok(())
        }
    }

    /// Cooperative-then-forced shutdown of the foreign process's tree: `SIGTERM`-walk, wait up
    /// to `grace` for the **root** to exit, then a hard identity-walk sweep. Best-effort (the
    /// `TreeWalk` contract); no `ExitStatus`. Unix only (Windows `terminate_tree` is
    /// `Unsupported`).
    pub fn graceful_shutdown_tree(&self, grace: Duration) -> Result<(), Error> {
        self.terminate_tree()?; // SIGTERM-walk (Windows: Unsupported, early return)
        let _ = crate::wait::block_until_exit(self.id, Some(grace))?; // non-reaping grace-wait on root
        self.kill_tree() // hard identity-walk sweep
    }
```

- [ ] **Step 4: Run to verify it passes**

Run (host, Windows): `cargo test --test graceful process_kill_tree_tears_down_tree process_soft_tree_unsupported_on_windows` → 2 passed.
Run (WSL, Linux): `MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --test graceful process_kill_tree process_graceful_shutdown_tree'` → 2 passed.

- [ ] **Step 5: Full suite + lint + commit**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --test graceful           # host (Windows): windows + cross-platform tests
git add -A
git commit -m "feat: Process foreign tree teardown — kill_tree/terminate_tree/graceful_shutdown_tree (Plan 7)"
```

Then run the full crate suite on Linux via WSL to confirm no regressions:
`MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/Users/bindreams/src/subprocess && CARGO_TARGET_DIR=/tmp/sp-target cargo test --locked'`

- [ ] **Step 6: Update `TODO.md`** — mark the "Lifecycle / graceful shutdown (from Plan 5)" item done (it is now implemented), and commit:

```bash
git add TODO.md
git commit -m "docs: mark graceful-escalation trio done (Plan 7)"
```

---

## Self-Review

**1. Spec coverage:**
- Surface matrix (5 methods × Child/Process) → Tasks 1–5 cover every cell; Windows `Unsupported` arms tested in Tasks 1, 2, 4, 5.
- Principle 1 (precise lone / best-effort tree) → lone via `wait::{terminate,kill,block_until_exit}` (Tasks 1,2,4); tree via `treewalk::{terminate,hard_kill}` (Tasks 3,5).
- Principle 2 (non-reaping sweep-before-reap) → `Child::graceful_shutdown_tree` (Task 3) and `Process::graceful_shutdown_tree` (Task 5) both use `block_until_exit` then sweep then (Child) reap.
- Escalation past ignored SIGTERM → `control-block-ignore-term` mode + `Duration::ZERO` tests (Tasks 2,3,4).
- Return types: `Child::graceful_shutdown{,_tree}` → `ExitStatus`; `Process::graceful_shutdown{,_tree}` → `()`; signal/sweep ops → `()`. Matches Global Constraints.
- Reuse map: every reused symbol (`open_verified`, `block_until_exit`, `treewalk::{terminate,hard_kill}`, `Attached` via `terminate_tree`/`kill_tree`, `shared_child.kill`, `Child::id`) is consumed as named.

**2. Placeholder scan:** none — every step has complete code and exact commands.

**3. Type consistency:** `wait::terminate(ProcessId) -> Result<(), Error>` defined in Task 1, consumed in Tasks 2,4. `block_until_exit(ProcessId, Option<Duration>) -> Result<bool, Error>` used with `Some(grace)` in Tasks 3,5. `Child::wait_timeout(Duration) -> Result<Option<ExitStatus>, Error>` consumed in Task 2. `treewalk::terminate(ProcessId) -> Result<(), Error>` and `treewalk::hard_kill(ProcessId)` consumed in Task 5. `common::spawn_grandchild(bool) -> (Child, Vec<TcpStream>)` defined in Task 3, consumed in Task 5. Signatures are consistent across tasks.
