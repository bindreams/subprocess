//! Per-OS containment dispatch (two-phase: prepare before spawn, attach after).
//! Unix process-group branch filled by Task 3.

use crate::containment::{ContainMode, ContainRequest, Containment, Nesting};
use crate::error::Error;

/// Decided before spawn (env-marker root detection); later tasks also apply
/// pre-spawn flags / process_group / pre_exec inside `prepare`.
pub(crate) struct Prepared {
    #[allow(dead_code)] // read in #[cfg(unix)] branch of attach()
    pub mode: Option<ContainMode>,
    #[allow(dead_code)] // read in #[cfg(unix)] branch of attach()
    pub is_root: bool,
}

/// Owns the OS containment resource for a spawned child; `hard_kill`/`terminate`
/// act on the tree, `disarm` neutralizes teardown for `detach()`. `None` =
/// uncontained (lone-process semantics).
#[derive(Debug, Default)]
pub(crate) enum Attached {
    #[default]
    None,
    #[cfg(unix)]
    ProcessGroup(i32), // pgid (== root pid)
                       // Windows JobObject(..), Unix Cgroup(..), TreeWalk(ProcessId) — later tasks.
}

impl Attached {
    /// Hard-kill the contained tree (best-effort; already-gone is success).
    pub(crate) fn hard_kill(&self) -> Result<(), crate::error::Error> {
        match self {
            Attached::None => Ok(()),
            #[cfg(unix)]
            Attached::ProcessGroup(pgid) => {
                crate::containment::unix::kill_group(*pgid).map_err(crate::error::Error::Io)
            }
        }
    }

    /// Send the graceful termination signal to the group (signal-only).
    pub(crate) fn terminate(&self) -> Result<(), Error> {
        match self {
            Attached::None => Ok(()),
            #[cfg(unix)]
            Attached::ProcessGroup(pgid) => crate::containment::unix::term_group(*pgid).map_err(Error::Io),
        }
    }

    /// Neutralize teardown so `detach()` leaves the tree running (e.g. clear a
    /// Job's KILL_ON_JOB_CLOSE before its handle drops). No-op for mechanisms
    /// whose resource-drop does not kill (pgroup/cgroup/treewalk/none).
    #[allow(dead_code)] // used in Task 9 (tree teardown in Drop/detach)
    pub(crate) fn disarm(&mut self) {
        match self {
            Attached::None => {}
            #[cfg(unix)]
            Attached::ProcessGroup(_) => {} // pgroup drop doesn't kill — no-op
        }
    }
}

/// Phase 1 (before spawn): env-marker root detection + pre-spawn OS setup.
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
    #[cfg(unix)]
    if is_root && mode.is_some() {
        crate::containment::unix::set_process_group(std_cmd); // cgroup (Task 4) may upgrade the achieved mode
    }
    Prepared { mode, is_root }
}

/// Phase 2 (after spawn, before SharedChild::new): attach the mechanism.
pub(crate) fn attach(child: &std::process::Child, prepared: &Prepared) -> Result<(Containment, Attached), Error> {
    #[cfg(unix)]
    {
        if prepared.mode.is_some() {
            if prepared.is_root {
                // Root: we set process_group(0) pre-spawn so pgid == pid.
                let raw_pid = child.id();
                debug_assert!(
                    raw_pid <= i32::MAX as u32,
                    "pid {raw_pid} exceeds i32::MAX; pgid cast would truncate"
                );
                let pgid = raw_pid as i32;
                return Ok((Containment::ProcessGroup, Attached::ProcessGroup(pgid)));
            } else {
                // Nested: joined the ancestor's group; kill_tree falls back to direct kill.
                return Ok((Containment::ProcessGroup, Attached::None));
            }
        }
    }
    // Uncontained (or non-Unix).
    let _ = (child, prepared);
    Ok((Containment::None, Attached::None))
}
