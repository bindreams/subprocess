# `subprocess` — Design Spec

> Status: approved design, pre-implementation. Working crate name `subprocess` (published name TBD before first publish — `subprocess` is taken on crates.io).
> Date: 2026-06-20.
> Source of truth for the v1 implementation plan. Backed by `.tmp/claude/research/research-brief.md` (8-report research synthesis).

## 1. Thesis & goals

A unified, ergonomic, cross-platform (Windows + Linux + macOS) subprocess-management crate. Convenience *and* customizability, in the spirit of Python's `subprocess`: a trivial `.run()` path and deep, honest knobs.

The Rust ecosystem is fragmented into single-concern crates; **none** combines ergonomic spawning + process-tree containment with stable identity + privilege elevation that still pipes + PTY. This crate is that union. v1 delivers the non-elevation core; elevation (the headline differentiator) follows, tracked in `TODO.md`.

Four pillars (charter):
1. Ergonomic spawning with deep knobs (std/tokio-grade builder + raw escape hatches).
2. Process-**tree** containment with **stable-across-time** identity.
3. Cross-platform privilege **elevation that also pipes/streams** *(deferred past v1)*.
4. PTY + deadlock-free stdio as first-class concerns.

## 2. Scope

**v1 (this spec):**
- `Command`/`Child`, sync + feature-gated tokio, over a shared pure core.
- Command-input model: `executable` / `args` / `commandline` + ported quoting.
- `Fd`/`Stdio` model + deadlock-free pumping (pipe / file / null / inherit / merge). `Stdio::pty()` variant present; full PTY wiring is a fast-follow (§13).
- `ProcessId` + 3-OS stable identity + `is_alive`.
- `Process::from_id` / `parent` / `children` / foreign-process wait (incl. a new Linux `pidfd` backend).
- Tree containment (Job Object / cgroup v2 → process group / session) + reported outcome + runtime fallback.
- Hard kill-on-drop + `terminate` / `kill` / `graceful_shutdown` / `detach`.
- Error taxonomy. In-crate test-helper binaries.

**Deferred → `TODO.md` → tickets at push:** elevation (admin/root planner + per-OS effect), run-as-user, SYSTEM, de-elevation/drop, elevated broker-streaming + macOS GUI elevation + tiered capture, full psutil-style system enumeration, identity persistence (crash recovery), tree-contained async pipelines, *BSD tier, hole-crate migration + dedup, full PTY wiring, published name + ported-shlex license line.

## 3. Decision log (resolved)

| Decision | Choice |
|---|---|
| Concurrency | Both sync + async, over a **pure runtime-agnostic core**; only the effect layer forks. tokio behind a feature flag (sync pays nothing). |
| Platforms | Windows + Linux + macOS as **equals, not tiered**. OS-determined divergence → `cfg` + extension traits (compile-time). Host-variable features → runtime fallback that **reports the achieved outcome**. No BSD (deferred). |
| Platform divergence API | Hybrid: `cfg`-gated extension traits for platform-specific *inputs*; narrow runtime fallback for host-variable *behavior*, surfaced as **operation-reported outcomes** (no query-ahead capabilities bag → no TOCTOU). |
| Host fallback | Use strongest available mechanism; degrade gracefully; the operation reports what it achieved (e.g. `child.containment()`). |
| v1 scope | Core first (spawn + stdio + tree + identity). Everything else → `TODO.md`. |
| Elevation reach | Admin/root only, and after core. SYSTEM / run-as-user / de-elevation deferred. |
| Introspection | Manage-focused (spawned + foreign-by-id + tree-walk of known); full enumeration deferred. |
| Home / audience | Single **public, crates.io-grade** crate; hole's stepstool/kill-group/bridge migrate to it later. Public-quality API, no internal assumptions. |
| Drop policy | **Hard kill-on-drop** (immediate SIGKILL / `TerminateJobObject` + reap), **no built-in deadline** — choosing a graceful timeout and assuming signal-handling behavior is the user's call. Graceful teardown is explicit and convenient. `detach()` / `kill_on_drop(false)` opt-out. |
| Containment dep | Own a thin `containment` module using `windows-rs`/`nix`/cgroup-fs directly, rather than wrapping `process-wrap` — our requirements (cgroup v2 + graceful fallback + reported outcome, the suspend→assign→resume race fix, nesting detection) are specific. (Re-validate during planning.) |
| Quoting | **Port** the qodana POSIX shlex (Apache-2.0, user-authored) + thaum's `CommandLineToArgvW`-compatible Windows quoter; do **not** depend on the `shlex` crate (documented deviations; quoting is a security boundary). |
| MSRV | 1.87 (unlocks `std::io::pipe`, replacing `os_pipe`). |
| PTY | `Stdio::pty()` variant in v1; full `portable-pty`/ConPTY wiring is a fast-follow. |
| Name | Working name `subprocess`; published name chosen before publish. |

## 4. Architecture

Three layers; the sync/async split is confined to the thinnest:

```
Layer 3  EFFECT (thin)        command.rs/child.rs (sync, threads)
                              tokio.rs (feature="tokio", futures)
Layer 2  SYS/EFFECTS (cfg)    sys/{windows,linux,macos}, containment/*, identity/*, wait/*
Layer 1  PURE CORE (no IO)    quote, env, error, Fd/Stdio plan, config types
```

Only Layer 3 knows sync vs async. Quoting, env composition, the stdio *plan* (a description, not the wiring), identity tokens, containment selection, and errors are shared. Sync builds never compile a runtime.

**Crate layout** (single crate; `foo.rs` + `foo/` style, no `mod.rs`; unit tests in sibling `*_tests.rs`):

```
src/
  lib.rs              re-exports + free fns: run(), run_line()
  command.rs          sync Command builder  (+ command_tests.rs)
  child.rs            sync Child handle
  tokio.rs + tokio/   async mirror              (feature = "tokio")
  process.rs          Process (foreign-by-id, tree-walk) + ProcessId
  identity.rs + identity/{windows,linux,macos}.rs
  stdio.rs + stdio/   Fd, Stdio, pipe types, deadlock-free pumping
  containment.rs + containment/{windows,linux,macos}.rs
  wait.rs + wait/{windows,linux,macos}.rs   concurrent + foreign wait
  quote.rs + quote/{posix,windows}.rs       argv split/join
  env.rs              env composition
  error.rs            thiserror taxonomy
  sys/{windows,unix,linux,macos}.rs         shared low-level primitives
  os/{unix,windows}.rs                       public cfg-gated CommandExt traits
tests/                integration tests against in-crate test-helper bins
```

**Features:** `tokio` (async layer), `pty` (portable-pty). Defaults: none. `serde` (persistence) is deferred.

**Internal deps** (public facade stays ours): `shared_child`, `nix`, `windows`(-rs), `sysinfo`, `thiserror`, `portable-pty` (feature). Internal pipes via `std::io::pipe`. cgroup v2 via filesystem (evaluate `cgroups-rs` vs thin direct impl). **Port, don't depend:** qodana POSIX shlex, thaum Windows quoter.

## 5. Public API

### 5.1 Trivial path

```rust
let out = subprocess::run(["git", "status"]).output()?;   // -> Output { status, stdout, stderr }
subprocess::run(["make", "build"]).status()?;             // inherit stdio -> ExitStatus
let txt = subprocess::run(["echo", "hi"]).read()?;        // capture stdout -> String (utf8-checked)
// string-source convenience:
let out = subprocess::run_line(r#"git "status""#).output()?;
```

`run`/`run_line` construct a `Command`; `.output()/.status()/.read()` also exist on the builder.

### 5.2 Command input model

`new()` takes no program — argv[0] is not special. The builder holds **one source of truth** in the user's format and converts only when it doesn't match the platform's native syscall form.

```rust
Command::new().args(["git", "status", "--short"])          // array source (POSIX-native)
Command::new().commandline(r#"git "status" --short"#)      // string source (Windows-native), OsString
Command::new().executable("/bin/busybox").args(["sh","-c","echo hi"])  // load file ≠ argv[0]
```

| User gave | On POSIX | On Windows |
|---|---|---|
| `.args([...])` | pass argv through | **join** → `lpCommandLine` (thaum quoter + round-trip assert) |
| `.commandline(s)` | **split** → argv (ported POSIX shlex) | pass `s` through; split first token only → `lpApplicationName` |

- `.executable(path)`: file the OS loads — `lpApplicationName` / explicit `execve` path. Decoupled from argv[0]; defaults to resolving from argv[0]/first token via PATH.
- `.args` and `.commandline` are mutually exclusive sources; last set wins (documented). No `.raw_arg` — `.commandline()` is the Windows raw escape hatch.
- `.commandline` / arg items accept `Into<OsString>` (Windows UTF-16/WTF-8, POSIX raw bytes; never lossy UTF-8).

### 5.3 Stdio model

```rust
pub struct Fd(RawFd);                 // RawFd = i32; non-negative, validated
impl Fd { pub const STDIN: Fd = Fd(0); pub const STDOUT: Fd = Fd(1); pub const STDERR: Fd = Fd(2); }
impl From<i32> for Fd {}              // bare int at call sites; Display: "stdout"/"fd 3"

// Stdio = redirection target
Stdio::{ inherit(), null(), pipe(), pipe_in(), pipe_out(), from_file(File), merge(Fd), pty(/*feat*/) }
```

```rust
.fd(Fd::STDOUT, Stdio::pipe())        // general form
.fd(3, Stdio::pipe_out())             // bare int + explicit direction for fds ≥ 3
.stdout(Stdio::pipe())                // shorthand == .fd(Fd::STDOUT, ...)
.stderr(Stdio::merge(Fd::STDOUT))     // 2>&1 via dup, not a shell string
.fd(3, Stdio::from_file(log))         // exec 3>log
```

Direction: `Stdio::pipe()` infers from the descriptor (`STDIN` → child-reads; `STDOUT`/`STDERR` → child-writes); `pipe_in()`/`pipe_out()` override (required for `Fd(n)`, n ≥ 3, which is ambiguous).

**Platform note (divergence in action):** `Fd(n)` for n ≥ 3 is fully supported on POSIX; on Windows it works only for MSVC-CRT children (`lpReserved2`) and otherwise returns `Unsupported` rather than silently dropping the handle. Std handles (0–2) and inheritable handles work everywhere.

### 5.4 Command builder method groups

Program/args (`args`, `commandline`, `executable`); env (`env`, `envs`, `env_clear`, `env_remove`); `current_dir`; stdio (`stdin`/`stdout`/`stderr`/`fd`); tree (`contain()` strongest available / `contain_with(mode)`); drop (`kill_on_drop(bool)`, default on); finalizers (`spawn() -> Result<Child>`, `output()`, `status()`, `read()`). Platform knobs via `os::unix::CommandExt` (`uid`, `gid`, `pre_exec`, `process_group`) / `os::windows::CommandExt` (`creation_flags`, later `integrity_level`).

### 5.5 Child

```rust
child.id() -> ProcessId;            child.is_alive() -> bool;       // re-reads start_token
child.containment() -> Containment; // what was ACTUALLY achieved on this host
child.stdin()/stdout()/stderr() -> Option<...>;   child.pipe(Fd) -> Option<...>;  // arbitrary fds
child.communicate(Some(input)) -> Result<Output>; // deadlock-free 3-way pump
child.wait()/try_wait()/wait_timeout(d)/wait_deadline(at);          // shared_child; no &mut
child.terminate()?;                // graceful: SIGTERM | CTRL_BREAK_EVENT
child.kill()?;                     // hard: SIGKILL | TerminateJobObject
child.graceful_shutdown(deadline)?;// terminate -> wait(deadline) -> kill (user picks the timeout)
child.detach();                    // leave running; cancels kill-on-drop
child.kill_tree()/terminate_tree();// act on the contained group when .contain() was set
```

### 5.6 Identity & foreign processes

```rust
pub struct ProcessId { /* pid + raw monotonic start_token */ }   // Copy, Eq, Hash
id.pid() -> RawPid;     id.created_at() -> Option<SystemTime>;    // lazy wall-clock; NOT identity

Process::from_id(saved) -> Option<Process>;   // None if reused or gone
Process::from_pid(pid) -> Option<Process>;    Process::current() -> Process;
p.id()/is_alive();   p.parent() -> Option<Process>;
p.children(Recursive::Yes) -> Vec<Process>;   // snapshot; start-time-ordering guarded
p.terminate()?/kill()?/wait()?;               // foreign exit via pidfd | kqueue | handle
```

Foreign `Process` exposes lifecycle/identity/tree, no stdio (we don't own its pipes).

### 5.7 Supporting types

`Output { status, stdout, stderr }`; `ExitStatus` (mirrors std — non-zero is `Ok`, not `Err`); `Containment { CgroupV2, JobObject, ProcessGroup, Session, None }`; `Recursive`.

## 6. Behaviors

### 6.1 Stdio pumping (deadlock-free)

Finite pipe buffers force concurrent pumping. Sync: one thread per active stream (thaum `drain_child_pipes` + `take_pipes_and_waiter` split-borrow — pipes drain while `wait` runs). Async: concurrent futures. `communicate(input)` is the 3-way pump; `output()`/`read()` build on it. **Close parent-side child-pipe ends immediately after spawn** (rust#98209 never-EOF hang). Inheritance hygiene: `pipe2(O_CLOEXEC)` (Linux); explicit `FD_CLOEXEC` (macOS `pipe()` omits it); `STARTUPINFOEX` + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` scoping + clear `HANDLE_FLAG_INHERIT` on parent ends (Windows).

### 6.2 Tree containment (strongest available, runtime fallback, reported)

- **Windows:** Job Object + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`; spawn `CREATE_SUSPENDED` → assign → resume (defeats spawn-then-assign race; resume by walking a Toolhelp32 *thread* snapshot since std/tokio close the `CreateProcess` thread handle); `TerminateJobObject` for kills.
- **Linux:** cgroup v2 (`cgroup.kill` — child cannot fork out) when mounted + delegated; else process group (`setsid`/`setpgid` in `pre_exec`) + `killpg`.
- **macOS:** process group / session + `killpg` (honestly weaker — a self-`setsid` child escapes).
- Reported via `child.containment()`. Nesting asymmetry (Windows jobs nest, Unix pgids don't) handled by an inherited env marker (`__SUBPROCESS_GROUP_ROOT`, stable; honor legacy names forever once published).

### 6.3 Identity, wait, reaping

- `ProcessId = (pid, raw monotonic start_token)`. Tokens: Linux `/proc/<pid>/stat` field 22 raw jiffies (parse via `rfind(')')`); Windows `GetProcessTimes` 100 ns FILETIME (exact); macOS `proc_pidinfo` `p_starttime` µs. Exact compare where high-res; tolerance window only where forced. `created_at()` is a separate lazy wall-clock that may drift; never identity.
- Wait via `shared_child` (`waitid(WNOWAIT)` pins the pid → concurrent kill safe): `wait`/`try_wait`/`wait_timeout`/`wait_deadline`, none `&mut`.
- **Always reap**; Drop and kill-on-drop *signal AND reap* (avoids tokio#2685 leak class).
- Foreign exit wait: arm-while-alive-then-block — Windows `OpenProcess(SYNCHRONIZE)`+`WaitForSingleObject`; macOS kqueue `NOTE_EXIT`; **Linux `pidfd_open`+poll (new backend)**. Linux signaling prefers `pidfd_send_signal`; fallback `kill(pid)` guarded by start_token re-check.

### 6.4 Drop / kill

Drop = immediate **hard kill** (SIGKILL / `TerminateJobObject`) of the contained tree (or lone child) + reap; no deadline. `kill()` hard; `terminate()` graceful; `graceful_shutdown(deadline)` = terminate → `wait(deadline)` → kill. Opt-out: `detach()` / `kill_on_drop(false)`. Kill by owned `Child` and by raw `ProcessId` (already-dead errors `ESRCH`/`87`/`0x80070005`/`0x80070057` → `Ok`).

## 7. Platform divergence strategy

- **Compile-time (`cfg`):** all OS-determined divergence — which mechanisms exist, identity-token kind, integrity levels, PTY kind. Platform-specific *inputs* live in `cfg`-gated extension traits (std idiom; cannot be named on the wrong OS).
- **Runtime (narrow):** host-variable features only — cgroup v2 presence+permission, `pidfd` (kernel ≥5.3), ConPTY (Win10 1809+). Probe on first use; degrade gracefully; **the operation reports what it achieved** (no query-ahead capabilities bag → no TOCTOU). Elevation-era ambient state (deferred) is the same pattern.

## 8. Quoting (port plan)

- `quote/posix.rs`: port of the qodana `shlex` (`split` + `join`/`quote` + `QuoteError{pos, kind}` where `kind: QuoteErrorKind`). **Byte-oriented** (`&[u8]` / `OsStr::as_bytes()` → `OsString::from_vec`): lossless, NUL-safe, encoding-agnostic — must not become `&str`-based. Strict IEEE 1003.1-2024 §2.2 with the three documented deviations (CR-as-whitespace, `#` not a comment, no expansion). Port the ~600 lines of test vectors as the oracle.
- `quote/windows.rs`: thaum's `CommandLineToArgvW`-compatible join + round-trip debug-assert; split via OS `CommandLineToArgvW` (or ported) for first-token → `lpApplicationName`. Special-case `.bat`/`.cmd` (cmd.exe escaping) or refuse — BatBadBut/CVE-2024-24576.
- License: Apache-2.0, user-authored; keep attribution header/NOTICE; settle the crate license line before publish (TODO).

## 9. Error model

One `thiserror` enum with `source` chaining: `ProgramNotFound`, `PermissionDenied`, `Spawn`, `Io`, `Quote(QuoteError{pos, kind})` (typed `QuoteErrorKind`, not a stringly-typed message), `Containment`, `Identity`, `Unsupported{ op, platform, detail }`. Non-zero exit is `Ok(ExitStatus)`, never an error.

## 10. Dependencies & MSRV

Internal: `shared_child`, `nix`, `windows`(-rs), `sysinfo`, `thiserror`, `portable-pty` (feature `pty`); cgroup v2 via fs (`cgroups-rs` vs thin direct — decide in planning); `std::io::pipe` (not `os_pipe`). Port (not depend): qodana shlex, thaum Windows quoter. **MSRV 1.87.** Avoid: `os_pipe`, `command-group`, re-rolled `OwnedHandle`/`OwnedFd`. Vet `whoami` (RUSTSEC-2024-0020) if ever needed.

## 11. Testing (TDD)

- Pure core: 100% unit-testable on every OS, table-driven; port the qodana shlex corpus + the `CommandLineToArgvW` round-trip debug-assert as the quoting oracle.
- Effect layers: integration tests against small in-crate **test-helper binaries** (echo-argv, dump-env, read-stdin, emit-N-bytes-to-{out,err}, exit-with-code, spawn-children, ignore-SIGTERM) — à la thaum `test-tools`.
- **No skip-on-missing** (fail loudly; opt out via explicit `cgroup`/`admin` markers; CI provisions). **No time-based synchronization** — sync on real primitives (pipe EOF, exit, pidfd/handle); the only legitimate timeout bounds a genuine child-exit wait. No reliance on data races.

## 12. Prior-art reuse map

- **stepstool:** salvage `preserve_env_arg` single-owner, TTY-gated `prime_sudo`, `{SudoNotFound,AuthFailed,NoTty}` taxonomy *(elevation era)*. Its "deliberately removed" list = the charter.
- **hole xtask/src/privilege (recovered):** pure `Host::plan()` planner + verbatim 9-step Windows de-elevation/piping sequence + POSIX drop discipline *(elevation era)*.
- **thaum:** `Fd`-table stdio model, concurrent drain+wait (`drain_child_pipes`, `take_pipes_and_waiter`), ConPTY (incl. mixed out/err + output scrubbing), `CommandLineToArgvW` round-trip quoter + PATH/PATHEXT resolution. (No elevation/tree-kill there.)
- **hole kill-group:** seed of tree-kill — `grouped_child.rs` (Job + `KILL_ON_JOB_CLOSE`, suspend→assign→Toolhelp32-resume, pgid + EPERM fallback, env-marker nesting, handle hygiene, CTRL_BREAK).
- **hole bridge/relaunch + handle-holders:** identity `(pid,start_token)` 3-OS backends + tolerance guard + already-dead idempotency; `ArmedWait` foreign exit (build the Linux pidfd backend); standardize on std/windows-rs `OwnedHandle`/`OwnedFd`.
- **psutil (external):** `(pid, start_token)` identity with `Eq`/`Hash`, re-read liveness, start-time-ordering-guarded tree, `oneshot` snapshot batching, `wait_procs(timeout)->(gone,alive)`. Use the RAW token (issue #2526), never wall-clock.

## 13. Key risks

1. **Containment is only best-effort on macOS** (no cgroup/subreaper/pidfd; self-`setsid` escapes) — honest, queryable degradation, not pretend-parity.
2. **Identity is probabilistically unique on Linux/macOS** (jiffies precision / clock drift); Windows FILETIME is effectively unique. Document the guarantee precisely.
3. **Quoting/injection (BatBadBut)** — per-interpreter escaping from day one; explicit, documented escape hatch (`commandline`).
4. **ConPTY + handle-inheritance footguns** (never-EOF, close deadlock, cross-child handle leaks, 32-bit WOW64 std-handle drop) — reuse battle-tested sequences verbatim *(PTY fast-follow)*.
5. **Env-marker compatibility forever** — pick the stable name now, honor legacy once published.
6. **Scope sprawl** — strict v1 boundary; everything else in `TODO.md`.

## 14. Open items

Tracked in repo-root `TODO.md`; converted to tickets at push. Git repo + first commit (this spec, `TODO.md`, Cargo skeleton) happen at scaffolding (first implementation step).
