//! Per-OS containment dispatch (two-phase: prepare before spawn, attach after).
//! No-op until the mechanism tasks (3-7) fill the bodies.

use crate::containment::{ContainMode, ContainRequest, Containment, Nesting};
use crate::error::Error;

/// Decided before spawn (env-marker root detection); later tasks also apply
/// pre-spawn flags / process_group / pre_exec inside `prepare`.
#[allow(dead_code)]
pub(crate) struct Prepared {
    pub mode: Option<ContainMode>,
    pub is_root: bool,
}

/// Owns the OS containment resource for a spawned child; `hard_kill`/`terminate`
/// act on the tree, `disarm` neutralizes teardown for `detach()`. `None` =
/// uncontained (lone-process semantics).
#[derive(Debug, Default)]
pub(crate) enum Attached {
    #[default]
    None,
    // Windows JobObject(..), Unix ProcessGroup(pgid)/Cgroup(..), TreeWalk(ProcessId) — later tasks.
}

#[allow(dead_code)]
impl Attached {
    /// Hard-kill the contained tree (best-effort; already-gone is success).
    pub(crate) fn hard_kill(&self) {
        match self {
            Attached::None => {}
        }
    }
    /// Send the graceful termination signal to the group (signal-only).
    pub(crate) fn terminate(&self) -> Result<(), Error> {
        match self {
            Attached::None => Ok(()),
        }
    }
    /// Neutralize teardown so `detach()` leaves the tree running (e.g. clear a
    /// Job's KILL_ON_JOB_CLOSE before its handle drops). No-op for mechanisms
    /// whose resource-drop does not kill (pgroup/cgroup/treewalk/none).
    pub(crate) fn disarm(&mut self) {
        match self {
            Attached::None => {}
        }
    }
}

/// Phase 1 (before spawn): env-marker root detection. Later tasks add the
/// pre-spawn setup (Windows creation_flags+handle hygiene; Unix process_group/
/// cgroup-pre_exec/setsid) here — WITHOUT changing this signature.
#[allow(dead_code)]
pub(crate) fn prepare(std_cmd: &mut std::process::Command, req: &ContainRequest) -> Prepared {
    let mode = req.mode;
    if mode.is_none() {
        return Prepared {
            mode: None,
            is_root: false,
        };
    }
    let is_root = std::env::var_os(crate::containment::NESTED_ENV).is_none();
    if is_root && req.nesting == Nesting::Mark {
        // Set AFTER any user env ops (env_clear) have been applied to std_cmd by
        // the spawn engine, so the marker survives env_clear (N1). `env` appends.
        std_cmd.env(crate::containment::NESTED_ENV, "1");
    }
    let _ = std_cmd; // later tasks use it for flags/process_group/pre_exec
    Prepared { mode, is_root }
}

/// Phase 2 (after spawn, before SharedChild::new): attach the mechanism. No-op
/// for now → contained requests resolve to None until Tasks 3-7 fill bodies.
pub(crate) fn attach(_child: &std::process::Child, _prepared: &Prepared) -> Result<(Containment, Attached), Error> {
    Ok((Containment::None, Attached::None))
}
