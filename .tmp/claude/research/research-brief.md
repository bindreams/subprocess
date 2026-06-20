# `subprocess` — Research Brief

A unified, ergonomic, cross-platform (Windows + POSIX) subprocess-management crate.
Convenience + customizability, like Python's `subprocess`: a trivial `.run()` path AND deep configurability.

> Status: pre-brainstorm source of truth. Synthesized from 8 research reports (local prior-art mining of `stepstool`/`thaum`/`hole`, psutil identity model, Rust ecosystem survey, POSIX + Windows elevation deep-dives, and cross-platform tree/lifecycle/PTY mechanics).

---

## 1. Landscape & gaps

The Rust subprocess ecosystem is **fragmented into single-concern crates**. No crate unifies all four pillars this crate targets:

1. Ergonomic spawning with deep knobs (`std`/`tokio`-grade builder + raw escape hatches).
2. Process-**tree** containment with **stable-across-time** identity.
3. Cross-platform privilege **elevation that also pipes/streams** (every existing crate fails this).
4. PTY + deadlock-free stdio as first-class concerns.

What exists, and where it stops:

| Concern | Best-in-class crate(s) | Stops at |
| --- | --- | --- |
| Spawn (sync) | `std::process::Command` | no async, no groups/tree-kill, `&mut self` wait/kill, PID-reuse races, BatBadBut quoting |
| Spawn (async) | `tokio::process`, `async-process` | best-effort reaping, child survives drop, no tree mgmt |
| Concurrent wait/kill | `shared_child` (`waitid(WNOWAIT)`) | just the wait/kill race — clean dep |
| Tree containment | `process-wrap` (Job/pgroup/KillOnDrop over std+tokio), `processkit` (adds cgroup v2), `kill_tree` | deliberately NOT a unified facade; composition ordering is the caller's problem |
| Pipelines / ergonomics | `duct` (`cmd!`, `.pipe()`), `subprocess` (`Exec`/`Pipeline`, deadlock-free `Communicator`) | both **sync-only** |
| Enumeration / introspection | `sysinfo`, `procfs`, `remoteprocess` | reads state, never spawns |
| Elevation + streaming | `deelevate` (Windows-only, PTY-bridge) | **Windows only** |
| Elevation (no streaming) | `elevated-command` (xplat), `runas`, `sudo` | explicitly NO piping/streaming/capture |
| PTY | `portable-pty` (ConPTY + openpty) | de-facto layer — depend on it |

**The gap = the thesis.** No crate is "duct's ergonomics + tokio + tree-kill + stable IDs + cross-platform elevation-with-piping" in one. `deelevate` proves elevation-with-streaming is achievable (and *how*: a PTY/IPC bridge across the privilege boundary) but only on Windows. That bridge, generalized cross-platform, is the headline differentiator.

**The two structural facts that shape everything:**

- **Elevation is structurally asymmetric.** POSIX elevates the **child** (a launcher prefix; parent stays unprivileged; stdio inherits naturally). Windows elevates the **whole process up front** via the Shell/AppInfo service (`ShellExecuteEx("runas")`), which **cannot carry stdio**. "Elevate one child from an unprivileged parent and keep talking to it over pipes" is natural on POSIX and **impossible on Windows UAC without a broker**.
- **The decision unifies; the effect does not.** The abandoned `hole` `privilege` module proved a pure, side-effect-free planner (`Host::plan(target) -> Transition`) cross-tests beautifully on every OS — but the *spawn/effect* layer must be platform-specific, and a single `Transition` enum that names both platforms' moves leaks at runtime (POSIX-only and Windows-only variants rejected by the wrong effect layer). Design around this, don't fight it.

---

## 2. What to reuse from prior art (concrete)

### `stepstool` (shipped elevation primitives) — verdict: reuse 3 ideas, discard the structure
A de-scoped bag of primitives, NOT a unified API (no `Command` type, no spawn path, no stdio). Its lib.rs header is the most important artifact: it lists what was *deliberately removed* because the author "could not come up with a unified interface" — `self_elevate`, faithful arg quoting, linked-token queries, all de-elevation. That removal list is the new crate's actual charter.

- **Reuse:** `preserve_env_arg(vars) -> "--preserve-env=A,B,C"` as a *single owner* of that literal (prevents drift between call sites). `prime_sudo()` / `prime_sudo_with(sudo, has_tty)` with TTY-gating built in (probe `sudo -n true`, else interactive `sudo -v` only if TTY, else error). Error taxonomy `{SudoNotFound, AuthFailed, NoTty}`.
- **Discard:** the structure (4 standalone fns) — it's exactly what failed.

### `hole` `xtask/src/privilege` (recovered, abandoned PR #456) — verdict: reuse the architecture wholesale
The real unified-ish attempt. This is where the design lessons live.

- **Reuse — the pure decision core:** `Host { elevated, invoking_user, is_ci, has_tty, strategy }` detected once per-OS; `Host::plan(target: Privilege) -> Transition` is a **side-effect-free** function exhaustively unit-tested on every OS. Lift this pattern as the policy layer.
- **Reuse — the Windows de-elevation/piping sequence verbatim** (the 9-step minefield, each step encodes a real bug): resolve+assert child PE is 64-bit (WOW64 silently drops a 32-bit child's std handles); 3 inheritable pipes + clear `HANDLE_FLAG_INHERIT` on parent ends; explicit UTF-16 env block (NULL inherits the *elevated* env, dropping overrides); `CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT`; verify child IL dropped below High *before* resume (terminate if not); assign to `KILL_ON_JOB_CLOSE` Job; `ResumeThread` must return prev-suspend-count == 1 (race-free proof `CREATE_SUSPENDED` survived seclogon); **close the Job BEFORE joining relay threads** (the #197 grandchild-pipe-hang lesson).
- **Reuse — POSIX drop-to-user discipline:** resolve groups in the *parent* (`getgrouplist` NGROUPS-grow loop, no arbitrary cap); `pre_exec` does ONLY `setgroups -> setgid -> setuid` in that exact order, no allocation.
- **Discard:** a single `Transition` enum spanning both platforms — surface the asymmetry as a runtime capability query instead.

### `hole` `crates/hole/src/setup.rs` (production GUI elevation) — verdict: reuse the fallbacks + error taxonomy
The only path with macOS support and cancellation handling.

- **Reuse:** the **file-based output channel** for UAC (`ShellExecuteEx` can't pipe → pass `--log-dir` to a co-operating child, read the log back; `tempdir.keep()` on failure for support). macOS `osascript "do shell script ... with administrator privileges"` backend + "User canceled" detection. Error taxonomy `SetupError { Cancelled, ExitCode { code, output, log_path }, Io, Windows }`. Map `ERROR_CANCELLED` (HRESULT `0x800704C7`) → first-class user-declined.
- **Reuse caveat (security):** osascript admin mode preserves the unprivileged parent's `HOME`, so root helpers write user-home dirs as root — needs ownership-repair awareness.

### `thaum` (shell impl over raw syscalls) — verdict: reuse the stdio/PTY core, discard elevation expectations
Does NOT use `std::process::Command`; implements `CommandEx`/`ChildEx` over `posix_spawnp`/`CreateProcessW` for full fd-table control. Its most reusable asset.

- **Reuse — the `Fd` enum as the stdio abstraction:** `CommandEx.fds: HashMap<i32, Fd>` where `Fd = Pipe | InputPipe | Pty | File`, keyed by fd number, supporting **fds beyond 0/1/2**. Generalizes redirection/pipelining/PTY uniformly. (Add explicit `Inherit` + `Null` variants — thaum represents Inherit by absence and opens NUL ad hoc.)
- **Reuse — concurrent drain + wait:** `drain_child_pipes` (stdout on current thread, stderr on a scoped thread); `take_pipes_and_waiter()` split-borrow so pipes drain while wait runs. The ConPTY-specific `drain_and_wait_conpty` (ConPTY pipes never EOF until console closes).
- **Reuse — full ConPTY incl. mixed stdout/stderr PTY** (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` whitelist + selective `STARTF_USESTDHANDLES`; HPCON + input-pipe write-end lifetime kept until exit or you get `STATUS_CONTROL_C_EXIT`), and ConPTY output scrubbing (`clean_conpty_output` strips CSI/OSC/2-byte VT + CR).
- **Reuse — the round-trip-through-real-`CommandLineToArgvW` debug-assert** for the argv quoter. Explicit PATH/PATHEXT resolution (MSYS2 vs cmd.exe modes).
- **Discard:** "impersonation" (it's argv[0] shell-name mimicry, NOT privilege). Zero elevation, zero job-control/tree-kill here. The `lpReserved2` CRT-fd-table trick for fds 3+ is MSVC-CRT-only — document, don't rely on for arbitrary executables.

### `hole/kill-group` — verdict: this IS the seed of the tree-kill module
The closest prior art to the tree charter; lift `grouped_child.rs` largely intact.

- **Reuse — Windows tree-kill:** Job Object + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, root spawned `CREATE_SUSPENDED`, assigned to job, then resumed — to defeat the **spawn-then-assign race** (a fast-forking child can place a grandchild outside the job before assignment). Resume by walking a **Toolhelp32 thread snapshot** (std/tokio already closed the `CreateProcess` thread handle).
- **Reuse — Unix tree-kill:** `process_group(0)` then `kill(-pgid, SIGKILL)`; graceful `SIGTERM` to `-pgid` with `ESRCH=ok`, `EPERM` → signal-leader-directly fallback (handles sudo-wrapped children).
- **Reuse — root auto-detection via inherited env marker** (`KILL_GROUP_NESTED`): Windows jobs nest, Unix pgids do NOT, so exactly the outermost spawn creates the group. Pick a **stable marker name from day one** and honor legacy names forever (skew across published binaries reintroduces the orphan bug).
- **Reuse — Windows stdio handle hygiene:** clear `HANDLE_FLAG_INHERIT` on the parent's std handles before spawn (else an orphan inherits host pipes and the reader never EOFs → runtime drop hangs forever). `unsafe impl Send` justification for stored Windows HANDLEs in async.
- **Reuse — graceful Windows signal:** `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)` (needs `CREATE_NEW_PROCESS_GROUP`; CTRL_C can't target a group).

### `hole/bridge` + `hole/relaunch` + `handle-holders` — verdict: this IS the identity + foreign-wait module
- **Reuse — unique-across-time id:** `(pid, start_time_unix_ms)` with `START_TIME_TOLERANCE_MS = 2000` reuse guard, all 3 platforms (Windows `GetProcessTimes`, macOS `proc_pidinfo(PROC_PIDTBSDINFO)`, Linux `/proc/<pid>/stat` field 22 + btime + `_SC_CLK_TCK`). Cross-platform `kill_pid(pid)` with already-dead error-code idempotency (`0x80070057`/`87`, `0x80070005`, `ESRCH` → Ok).
- **Reuse — atomic persisted records:** `PluginRecord { pid, start_time_unix_ms }`, `NamedTempFile -> sync_all -> persist` (atomic rename), `serde(deny_unknown_fields)` + `SCHEMA_VERSION`, fail-soft-to-None load.
- **Reuse — race-free foreign-process exit wait:** `ArmedWait::arm(pid).wait()` (arm-while-alive-then-block). Windows `OpenProcess(SYNCHRONIZE) + WaitForSingleObject`; macOS `kqueue EVFILT_PROC/NOTE_EXIT`. **Gap: no Linux backend** — must add `pidfd_open` + poll.
- **Reuse — `PidSink` hook:** synchronous post-spawn `Fn(u32)` fired *before any `.await`* so identity is recorded before anything can go wrong. (Or just return the durable identity from `spawn()`.)
- **Discard:** the per-crate ad-hoc `OwnedHandle` re-rolls — standardize on `std`/`windows-rs` `OwnedHandle`/`OwnedFd`.

---

## 3. Elevation deep-dive (the headline feature)

### 3.1 The unified-interface principle
Model elevation as a **declarative attribute of the spawn**, resolved against detected ambient privilege by a **pure planner** — never as imperative `sudo()`/`runas()` calls, and never as "run this shell pipeline as root."

```
Privilege::{ Inherit, Elevated, AsUser{creds}, System }   // declared on the builder
        │
   Host::detect()  →  Host::plan(Privilege) → Transition   // PURE, cross-tested on all OS
        │
   per-OS effect layer  (rejects wrong-platform Transition variants at runtime)
```

This makes the redirection/pipeline gotchas *impossible to express by accident*: the privileged unit is one process, not a shell string.

### 3.2 The piping problem — the crux the prior crate failed at
On POSIX `sudo cmd > /root/file` fails because the **shell** opens `/root/file` as the unprivileged user *before* sudo runs. `sudo a | b` runs only `a` as root. On Windows, `ShellExecuteEx("runas")` has **no `STARTUPINFO`, no `bInheritHandles`, no std-handle fields at all** — and AppInfo (session 0), not your process, is the creator, and **handles can't cross sessions**.

**Solution strategy (cross-platform):**
1. **Never shell out a pipeline string and hope.** Expose a `Command`-like builder where the privileged child is one process.
2. **Re-exec-self model** as the cleanest path for "elevated I/O" (write root-owned files, build a real pipeline): the privileged side is *real Rust running as root* (it opens the files), which sidesteps every shell-redirection gotcha. Spawn the same binary with a private marker arg via `/proc/self/exe`; guard against re-elevation loops.
3. **Explicit write-as-root primitives** (`write_file_elevated(path, bytes)`) implemented via re-exec, falling back to `tee`/`dd` when shelling out.
4. **Capture as a degrading capability** (see 3.5).

### 3.3 POSIX elevation
- **Mental model:** every helper (`sudo`/`doas`/`pkexec`/`run0`/`su`) is a **program launcher** — elevates one `execve`'d process + children; the shell stays unprivileged. **stdio inherits by default** (the POSIX advantage), so `sudo_command.output()` and pipes work — *except* PTY-allocating helpers (`run0`, `sudo` under I/O logging / `use_pty`) which change EOF/signal/line-buffering. `run0` auto-detects: all-three-TTYs → PTY, else pipe passthrough.
- **Backends (runtime-detected, ordered, overridable):** CLI prefer `run0` (systemd ≥256) > `sudo` > `doas`; GUI prefer `pkexec` > `run0`; macOS `sudo` (CLI) + `osascript`/helper (GUI). Detect by probing PATH + features (polkit agent, `$DISPLAY`/`$WAYLAND_DISPLAY`, systemd version) at runtime, not `cfg!`.
- **Auth-strategy enum:** `Interactive(TTY)` / `Stdin(-S)` / `Askpass(-A + SUDO_ASKPASS)` / `NonInteractive(-n, NOPASSWD-or-fail)` / `Gui(polkit / macOS dialog)`. Fail fast with a typed `NoTtyNoAskpass` instead of hanging. A blind `sudo -v` with no TTY hangs/fails (notably macOS under IDE runners).
- **Env (a security boundary):** default to **clean**; require an explicit allowlist; hard-deny/warn on `LD_PRELOAD`/`LD_LIBRARY_PATH`/`DYLD_*`/`PYTHONPATH`/`PERL5LIB` (CVE class). Own the sudo argv shaping: `--preserve-env=LIST`, the `env KEY=VALUE ... program args` ordering (env treats trailing assignments as args), `secure_path`-defeating PATH/HOME, and **no `--` on macOS BSD env**.
- **Detection:** `geteuid()==0` necessary-but-not-sufficient (Linux capabilities grant root powers without euid 0; setuid binaries have ruid≠euid). Expose strict `is_elevated()` and an optional capability-aware `has_effective_root_powers()`.
- **Dropping (if offered):** `setgroups/initgroups -> setresgid -> setresuid` (clear saved-set-ids), then **verify by attempting to regain and asserting failure**; propagate every error (`setuid` can fail even as root and silently leave you privileged). `seteuid` = temporary, label clearly. Offer a least-privilege escape hatch (request `CAP_NET_BIND_SERVICE` via file/ambient caps / `setpriv`).
- **macOS:** GUI elevation cannot inherit stdio (same shape as Windows). CLI sudo inherits normally. GUI = `osascript` dialog (one-shot) or `SMAppService`/`SMJobBless` helper + XPC. Programmatic password passing blocked since Big Sur.

### 3.4 Windows elevation
- **Tiers (each maps to a different mechanism + stdio strategy):** Medium (default filtered token) → **High / "Elevated" admin** (the linked full token, via UAC) → **System** (`NT AUTHORITY\SYSTEM`, IL `0x4000` > High `0x3000`; **NOT reachable by UAC consent** — needs SeDebug/SeImpersonate token duplication, a service, or a SYSTEM scheduled task).
- **`CreateProcess*` cannot elevate.** Against a `requireAdministrator` EXE they return `ERROR_ELEVATION_REQUIRED (740)`. Only `ShellExecuteEx("runas")` → AppInfo → UAC consent → full-token child.
- **Two backends:**
  - **Same-session fast path** (already-have-a-token, or run-as-user): `CreateProcessAsUser` / `CreateProcessWithTokenW` / `CreateProcessWithLogonW` with normal inheritable-pipe redirection. Privilege-aware fallback chain on `ERROR_PRIVILEGE_NOT_HELD (1314)`: `WithTokenW` (SeImpersonate) → `AsUser` (SeIncreaseQuota/SeAssignPrimaryToken) → `WithLogonW` (no priv).
  - **Cross-boundary path** (UAC-elevated or System): bundled **elevated helper/broker EXE** launched via `ShellExecuteEx("runas")`, stdio bridged over **named pipes** (string names cross the session boundary; raw handles don't).
- **Detection (two different questions):** `TokenElevationType` (birth state: Default=1 / Full=2 / Limited=3) for `is_elevated()`/`can_elevate()`; `TokenIntegrityLevel` (actual) via RID **range-compare** (not equality) for `integrity_level()`.
- **Quoting (BatBadBut / CVE-2024-24576, CVSS 10):** always emit an explicit quoted `lpApplicationName`; build the command line per `CommandLineToArgvW` rules for EXEs; **special-case `.bat`/`.cmd` with cmd.exe escaping (caret + `%` neutralization) or refuse.** `CreateProcess` silently runs batch via `cmd.exe /c`.

### 3.5 The unified piping/streaming strategy (the elevated-helper / broker pattern)
This is the architecture `deelevate`/`gsudo`/PsExec/Microsoft `sudo.exe` all converge on, generalized:

- **Non-elevated parent** ("renderer") creates a control + data channel and launches a small bundled **elevated helper** ("host") via the platform's elevation entry point, passing channel addresses on the helper's command line.
- **Helper** (running elevated) `CreateProcess`es the real target with stdio redirected to the channel ends (`STARTF_USESTDHANDLES`), and pumps bytes back.
- **Transport sub-modes:** raw byte mode (simple capture) **or** ConPTY/PTY mode (full interactive TTY, color, line-editing) when the caller requests a tty — unifying with the cross-platform PTY abstraction.
- **Windows channel = named pipes**, secured by default: `\\.\pipe\ProtectedPrefix\Administrators\...` namespace, `FILE_FLAG_FIRST_PIPE_INSTANCE`, restrictive SDDL (High-IL/Admins only), verify peer via `GetNamedPipeClientProcessId` + token IL (avoid PsExec-style hijack, TRA-2020-68).
- **POSIX channel = inherited pipes** (CLI helpers inherit stdio directly — often no broker needed); **macOS GUI = file/helper channel** (no stdio inheritance through the GUI path).
- **Capture as an explicit, degrading capability**, with the in-effect mode queryable:
  1. **Inherit** — always works.
  2. **True pipes** — POSIX CLI always; Windows only when already-elevated with a linked token (de-elevate path) OR via the broker.
  3. **File-based fallback** — UAC `ShellExecuteEx` with no co-operating broker (the production `setup.rs` pattern).
- **Security note (Microsoft's own warning):** a Medium-IL process can drive the elevated one over a shared channel — the broker must **constrain** what the unelevated side can send, not blindly proxy.

---

## 4. Process identity & tree management

### 4.1 Unique-across-time identity (psutil is the gold standard)
psutil never trusts a bare PID. Identity = the tuple `(pid, create_time)`, with `Eq`/`Hash` defined purely over it, to defeat **PID reuse**.

- **Critical rule:** the identity component must be a **monotonic, clock-stable** kernel value, **NOT wall-clock**. On Linux/macOS/NetBSD, boot_time/btime drifts under NTP, so wall-clock `create_time` changes over a process's lifetime and silently breaks `Eq`/`Hash` (psutil issue #2526). Keep a **separate, lazy, human-facing `created_at()`** wall-clock that may shift.

- **Canonical identity formula per OS (use the RAW kernel value as the token):**
  - **Linux:** `(pid, /proc/<pid>/stat field 22 starttime_jiffies as u64)` — raw jiffies, perfectly stable; do NOT divide by `SC_CLK_TCK` for identity. Parse with `rfind(')')` (comm may contain spaces and `)`).
  - **Windows:** `(pid, GetProcessTimes ftCreate as u64)` — 100-ns FILETIME, high-res and clock-stable; the **strongest** token. `NtQuerySystemInformation` fallback for AccessDenied system procs (psutil uses `fast_only=True` to skip the slow fallback in bulk enumeration).
  - **macOS/BSD:** `(pid, kinfo_proc p_starttime as i64 usec)` via sysctl — weaker (wall-clock based on FreeBSD/OpenBSD); accept the documented caveat.

- **Type:** `ProcessId { pid: RawPid, start_token: StartToken }` with derived `Eq`/`Hash`/`Clone`/`Copy`. ALL comparison/hashing/caching/"is this still the same process" keys off this — never the bare PID. `spawn()` returns it (or fires a synchronous `on_spawn(pid)` hook before any await). On Windows, additionally **retain the process HANDLE** to pin identity. `hole`'s `(pid, start_time_unix_ms)` + `START_TIME_TOLERANCE_MS` is the working implementation to lift; promote to a first-class type with exact-match where possible (Windows FILETIME) and tolerance only where required (clock-drifty OSes).

- **Liveness:** `is_alive()` re-reads the start_token and compares — not "does PID exist." Cache a `gone`/`reused` flag so a known-dead process is never re-queried (psutil `_pids_reused`).

### 4.2 Tree management
- **Containment = a kernel container chosen at spawn, killed atomically** (NOT best-effort PID walking — walking races reuse/respawn):
  - **Windows:** Job Object + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Spawn root `CREATE_SUSPENDED`, assign, resume (defeats spawn-then-assign race). Children auto-join; escape only via `CREATE_BREAKAWAY_FROM_JOB` + `JOB_OBJECT_LIMIT_BREAKAWAY_OK`. Prefer `TerminateJobObject` over relying on handle-close (avoids runtime-drop ordering race).
  - **Linux:** cgroup v2 (`cgroup.kill` — a child cannot fork out) where available/permitted; fall back to process groups + `PR_SET_PDEATHSIG` (weaker).
  - **macOS/BSD:** process group / session (`setpgid`/`setsid` in `pre_exec`, `killpg`). Best-effort: a child that `setsid`s itself escapes; no subreaper/cgroup/pidfd equivalent — document honestly.
- **Nesting asymmetry:** Windows jobs nest; Unix pgids do NOT. Root-vs-nested detection via inherited env marker (kill-group pattern) decides who creates the group.
- **parent()/children():** build a single ppid snapshot, then guard every link with the **start-time ordering invariant** (`parent.start_token_time <= child.start_token_time`) to reject recycled-PID impostors. Do NOT cache ppid on Unix (reparenting to init/subreaper changes it); caching OK on Windows.
- **Tree kill (graceful escalation):** `terminate` (SIGTERM to group / `CTRL_BREAK_EVENT` on Windows console apps) → bounded wait to a caller deadline → `kill` (SIGKILL / `TerminateJobObject`). The deadline is a **legitimate** timeout (awaiting child exit, an external event) — the one place a numeric timeout is correct. Mirror psutil `wait_procs(timeout) -> (gone, alive)` and bound per-process wait to limit the reuse window.

### 4.3 Lifecycle / reaping / signaling
- **Always reap** (Unix zombies hold PID slots). Guarantee reaping in `Drop` and in kill-on-drop: **signal AND wait/reap** (avoid the tokio #2685 leak class).
- **Concurrent wait/kill:** build on the `shared_child` model (`waitid(WNOWAIT)` keeps the PID pinned so a concurrent signal is safe). Provide native `wait_timeout(Duration)` / `wait_deadline(Instant)`. Do NOT expose `&mut self` wait/kill.
- **Linux race-free signaling:** prefer **pidfd** (`pidfd_open` via raw syscall; `pidfd_send_signal` can't mis-target; pidfd is `poll`/`epoll`-able for exit — `EPOLLIN` zombie, `EPOLLHUP` reaped). `CLONE_PIDFD` for race-free-at-spawn. Integrate into the async reactor instead of a global SIGCHLD handler.
- **macOS:** `kqueue EVFILT_PROC/NOTE_EXIT` for exit notification.
- **Parent-death auto-kill (Linux):** `PR_SET_PDEATHSIG` in `pre_exec`, then immediately re-check `getppid()` and self-exit if already orphaned (fires on parent *thread* death; cleared on fork and set-uid exec; silent if parent already dead). Prefer container-based containment which has none of these edges.
- **Kill-by-identity AND kill-by-Child:** the recovery path needs raw `(pid, start_token)` kill with already-dead error mapping; the owning path needs `Child`-based kill.

---

## 5. Stdio / piping / PTY

- **Core abstraction = per-fd table** (`thaum`'s `Fd`): `HashMap<i32, Fd>` with `Fd = Inherit | Null | Pipe | InputPipe | Pty | File`, supporting fds beyond 0/1/2 (`exec 3>file`). Generalizes redirection, pipelines, PTY uniformly vs std's 3 fixed `Stdio` slots.
- **Deadlock-free I/O is mandatory.** Finite pipe buffers (~64 KiB) → writing stdin without concurrently draining stdout/stderr hangs. Pump each stream on its own thread (sync) or futures (async). `wait_with_output`'s trick (close stdin before waiting) is the default. Reuse `subprocess::Communicator`'s 3-way design / `thaum`'s `drain_child_pipes` + `take_pipes_and_waiter` split-borrow. **Always close parent-side copies of child pipe ends right after spawn** (rust #98209: never-EOF hang).
- **Inheritance asymmetry:** POSIX = `O_CLOEXEC` set atomically (`pipe2`); macOS `pipe()` does NOT set `O_CLOEXEC` (set `FD_CLOEXEC` explicitly or parent ends leak → reader never EOFs). Windows = `bInheritHandles=TRUE` inherits **ALL** inheritable handles → scope with `STARTUPINFOEX` + `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` to exactly the intended handles (prevents cross-child pipe leaks → hangs).
- **PTY = `portable-pty`** (ConPTY on Windows ≥1809, openpty on Unix), behind a **feature flag** (most uses don't need it). ConPTY quirks: output pipes never EOF until `ClosePseudoConsole` (drain concurrently with wait/close); close can deadlock single-threaded; output carries VT/CSI/OSC + CRLF (scrub if you want clean bytes); keep the input-pipe write-end alive until exit. Mixed stdout/stderr-PTY needs the HANDLE_LIST whitelist (`thaum` has it working).
- **Redirection primitives** mapping cleanly per platform: `null()` → `/dev/null`/`NUL`; append (`O_APPEND`/`FILE_APPEND_DATA`); explicit `2>&1` merge via `dup2`/`DuplicateHandle` (so callers don't reach for shell strings).
- **Argv quoting / PATH resolution** owned by the crate (one tested quoter with the `CommandLineToArgvW` round-trip debug-assert; explicit PATH/PATHEXT). Default to safe quoting; expose a `raw_arg` escape hatch, documented.
- **Pipelines:** mirror `duct`'s `Expression`/`.pipe()` + `subprocess`'s `Exec | Exec`, but **tree-contained and async** — the literal gap in the ecosystem.

---

## 6. Recommended crate dependencies

Per the user's "add deps over hand-rolling" preference. **Depend, don't reinvent.**

| Crate | Why |
| --- | --- |
| `process-wrap` | Composable per-platform containment (JobObject / ProcessGroup / KillOnDrop / CreationFlags) over std AND tokio — the prime build-on seam; the crate provides the unified facade it omits. |
| `shared_child` | Concurrent wait/kill via `waitid(WNOWAIT)` / Windows PID-pinning — solves the `&mut self` race without hand-rolling waitid-vs-waitpid. |
| `portable-pty` | De-facto cross-platform PTY (ConPTY + openpty); behind a feature flag. |
| `sysinfo` | Cross-platform enumeration + `start_time()` for stable IDs and tree-walk fallback (it reads; we spawn). |
| `nix` | POSIX syscalls: `setsid`/`setpgid`/`killpg`/`kill`/`waitpid`/`waitid`, `Signal`, pty helpers, kqueue. |
| `windows` (windows-rs) | Win32: Job Objects, `ShellExecuteEx`, `CreateProcess*`, token info, ConPTY, named pipes. |
| `win32job` | Safe wrapper over the Job Object dance, if going lower-level than process-wrap. |
| `interprocess` | Cross-process control channel: named pipe (Windows) / UDS (Unix); tokio async — for the elevated broker. |
| `tempfile` | Atomic persisted identity records (`NamedTempFile -> persist`) + UAC log-dir channel. |
| `serde` + `serde_json` | Schema-versioned identity persistence + broker payloads. |
| `thiserror` | The rich error taxonomy. |
| `std::io::pipe` (MSRV 1.87+) | Internal pipe plumbing — **prefer over `os_pipe`** (now redundant). |
| `caps` (libcap, optional) | Capability-aware `has_effective_root_powers()` + least-privilege requests on Linux. |
| `deelevate` (Windows, evaluate) | Reference / possible direct dep for Windows de-elevation-with-streaming. |

**Avoid:** `os_pipe` (superseded by `std::io::pipe`); `command-group` (deprecated → `process-wrap`); re-rolling `OwnedHandle`/`OwnedFd` (use std/windows-rs); reimplementing introspection/unwinding (defer to `remoteprocess`/`procfs`). Vet `whoami` versions (RUSTSEC-2024-0020).

**Gaps to build (no good dep):** Linux `pidfd`-based foreign-process wait (extend `relaunch.rs`); the cross-platform elevation-with-piping broker (generalize `deelevate`); the unified facade itself.

---

## 7. Proposed high-level API sketch

A trivial path AND deep knobs — the Python `subprocess` philosophy.

```rust
// ---- Trivial path -------------------------------------------------
let out = subprocess::run("git", ["status"]).output()?;         // capture
subprocess::run("make", ["build"]).status()?;                   // inherit stdio
let txt = subprocess::run("echo", ["hi"]).read()?;              // String

// ---- Builder with deep knobs -------------------------------------
let child = Command::new("server")
    .args(["--port", "8080"])
    .env_clear().env("PATH", "/usr/bin")                        // explicit, clean env
    .current_dir("/srv")
    .stdout(Fd::Pipe).stderr(Fd::pipe_merge_to(1))             // 2>&1 via dup, not shell
    .fd(3, Fd::File(log_file))                                  // arbitrary fd
    .tree(TreeMode::Contain)                                    // Job/cgroup/pgroup
    .kill_on_drop(KillOnDrop::TreeGraceful { deadline })
    .privilege(Privilege::Elevated)                            // declarative
    .auth(AuthStrategy::Gui)                                    // POSIX backends
    .spawn()?;                                                  // -> (Child, ProcessId)

let id: ProcessId = child.id();          // (pid, start_token), unique across time
child.is_alive();                        // re-reads start_token
let status = child.wait_timeout(dur)?;   // shared_child model; no &mut needed for kill
child.terminate()?;                      // graceful (SIGTERM / CTRL_BREAK)
child.kill_tree(deadline).await?;        // escalate to SIGKILL / TerminateJobObject

// ---- Capture, deadlock-free -------------------------------------
let cap = child.communicate(Some(stdin_bytes))?;   // 3-way pump
println!("mode in effect: {:?}", cap.capture_mode); // Inherit | Pipe | FileFallback

// ---- Elevation conveniences -------------------------------------
subprocess::write_file_elevated("/etc/app.conf", bytes)?;  // re-exec-self, no shell redirect
let lvl: PrivilegeLevel = subprocess::privilege_level();   // is_elevated + integrity tier
let caps = subprocess::capabilities();                     // runtime: has_tty, has_linked_token, backends

// ---- Pipelines (tree-contained, async) --------------------------
Pipeline::new()
    .add(Command::new("producer"))
    .pipe(Command::new("grep").arg("x"))
    .tree(TreeMode::Contain)
    .run().await?;

// ---- Tree / identity / foreign process --------------------------
let p = Process::from_id(saved_id)?;     // None if reused (start_token mismatch)
for c in p.children(Recursive::Yes) { ... }   // start-time-ordering guarded
ArmedWait::arm(some_pid)?.wait()?;        // race-free foreign exit (pidfd/kqueue/handle)
```

**Layering:** pure planner (`Host::plan`, 100% xplat-testable) → platform effect backends behind a runtime capability query → ergonomic facade. Sync and async share the planner/identity/quoting/decision core; only the spawn/IO effect layer forks.

---

## 8. Key risks

1. **Elevation effect-layer leakage.** The unified type names both platforms' moves but each backend rejects wrong-platform variants at runtime — the abstraction leaks. Mitigate: surface asymmetry as a queryable capability, keep the pure decision core small and exhaustively cross-tested.
2. **Windows "elevate one child + keep pipes" is impossible without a shipped broker.** Requires bundling/generating + signing a helper EXE — a packaging and supply-chain burden, and a security-sensitive attack surface (pipe hijack, Medium-IL drives elevated). Mitigate: secured pipe namespace, peer verification, constrained proxy.
3. **POSIX-behind-sudo children are not force-killable.** sudo ≥1.9.14 pty/monitor mode won't relay SIGKILL; pipes can stay open forever (the live unsolved `dev-console` problem). The API must express graceful-only termination and let callers bound/abandon never-EOF readers (drain-then-abandon).
4. **Identity is only probabilistically unique on Linux/macOS.** Windows FILETIME is effectively unique; Linux jiffies precision and macOS clock-drift make it strong-but-racy. Document the guarantee precisely; never claim perfection.
5. **macOS is a genuinely weaker platform** (no pidfd, no subreaper, no cgroup, GUI elevation can't inherit stdio, reaping the group leader reclaims the PGID). Honest degradation, not pretend-parity.
6. **Quoting/injection (BatBadBut).** Any path that shells out (elevation helpers, `.bat`/`.cmd`, osascript) re-opens the hole. Per-interpreter escaping from day one; explicit, documented `raw_arg`.
7. **ConPTY + handle-inheritance footguns** (never-EOF, close deadlock, cross-child handle leaks, 32-bit WOW64 std-handle drop). Each is a real shipped bug in the prior art — reuse the battle-tested sequences verbatim rather than rediscovering.
8. **Env-marker compatibility forever.** The inherited tree-root marker name must be stable from day one and honor legacy names, or version-skewed binaries reintroduce the orphan bug.
9. **Scope sprawl.** The charter touches spawning, tree mgmt, identity, elevation, PTY, IPC, persistence — risk of never shipping. Needs explicit phase/scope boundaries (an open question for the user).
