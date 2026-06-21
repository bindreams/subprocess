//! Per-OS containment dispatch (two-phase: prepare before spawn, attach after).
//! Unix process-group branch filled by Task 3; Linux cgroup v2 by Task 4;
//! Windows Job Object by Task 5.

use crate::containment::{ContainMode, ContainRequest, Containment, Nesting};
use crate::error::Error;

/// Decided before spawn (env-marker root detection); later tasks also apply
/// pre-spawn flags / process_group / pre_exec inside `prepare`.
pub(crate) struct Prepared {
    #[allow(dead_code)] // read in #[cfg(unix)] branch of attach()
    pub mode: Option<ContainMode>,
    #[allow(dead_code)] // read in #[cfg(unix)] branch of attach()
    pub is_root: bool,
    /// Pre-created cgroup leaf (Linux only). `Some` means the child must be
    /// placed in the cgroup via the `pre_exec` closure; `None` means fall back
    /// to the process-group mechanism.
    #[cfg(target_os = "linux")]
    pub cgroup_leaf: Option<crate::containment::cgroup::CgroupLeaf>,
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
    #[cfg(target_os = "linux")]
    Cgroup(crate::containment::cgroup::CgroupLeaf),
    #[cfg(windows)]
    JobObject(crate::containment::windows::JobHandle),
    // TreeWalk(ProcessId) — Task 7.
}

// CgroupLeaf is not Debug; provide a minimal impl.
#[cfg(target_os = "linux")]
impl std::fmt::Debug for crate::containment::cgroup::CgroupLeaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CgroupLeaf").finish_non_exhaustive()
    }
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
            #[cfg(target_os = "linux")]
            Attached::Cgroup(leaf) => {
                leaf.hard_kill();
                Ok(())
            }
            #[cfg(windows)]
            Attached::JobObject(job) => {
                job.hard_kill();
                Ok(())
            }
        }
    }

    /// Send the graceful termination signal to the group (signal-only).
    pub(crate) fn terminate(&self, _child_pid: u32) -> Result<(), Error> {
        match self {
            Attached::None => Ok(()),
            #[cfg(unix)]
            Attached::ProcessGroup(pgid) => crate::containment::unix::term_group(*pgid).map_err(Error::Io),
            #[cfg(target_os = "linux")]
            Attached::Cgroup(leaf) => leaf.terminate().map_err(Error::Io),
            #[cfg(windows)]
            Attached::JobObject(_) => crate::containment::windows::terminate(_child_pid).map_err(Error::Io),
        }
    }

    /// Neutralize teardown so `detach()` leaves the tree running. For Job Objects,
    /// clears `KILL_ON_JOB_CLOSE` so the handle close does not kill the tree.
    /// No-op for mechanisms whose resource-drop does not kill (pgroup/cgroup/none).
    pub(crate) fn disarm(&self) {
        match self {
            Attached::None => {}
            #[cfg(unix)]
            Attached::ProcessGroup(_) => {} // pgroup drop doesn't kill — no-op
            #[cfg(target_os = "linux")]
            Attached::Cgroup(_) => {} // cgroup.kill is explicit — drop doesn't kill
            #[cfg(windows)]
            Attached::JobObject(job) => job.disarm(), // clear KILL_ON_JOB_CLOSE before handle drops
        }
    }
}

/// Returns `true` when the current process is the outermost contained root
/// (i.e. the env marker is absent). Pure function: takes the marker-presence
/// flag so it can be unit-tested without touching the process environment.
pub(crate) fn is_nested(marker_present: bool) -> bool {
    marker_present
}

/// Phase 1 (before spawn): env-marker root detection + pre-spawn OS setup.
pub(crate) fn prepare(std_cmd: &mut std::process::Command, req: &ContainRequest) -> Prepared {
    let mode = req.mode;
    if mode.is_none() {
        return Prepared {
            mode: None,
            is_root: false,
            #[cfg(target_os = "linux")]
            cgroup_leaf: None,
        };
    }
    let marker_present = std::env::var_os(crate::containment::NESTED_ENV).is_some();
    let is_root = !is_nested(marker_present);
    if is_root && req.nesting == Nesting::Mark {
        // Set AFTER any user env ops (env_clear) have been applied to std_cmd by
        // the spawn engine, so the marker survives env_clear (N1). `env` appends.
        std_cmd.env(crate::containment::NESTED_ENV, "1");
    }

    // Linux: session mode or (cgroup v2 + process group).
    // ContainMode::Session on Linux applies setsid INSTEAD OF process_group(0)
    // (S3 mutual exclusivity). For Strongest, always set a new process group;
    // cgroup placement is in addition (cgroup.kill is atomic, process_group lets
    // SIGTERM reach the group before kernel teardown).
    #[cfg(target_os = "linux")]
    {
        if is_root && mode.is_some() {
            if matches!(mode, Some(ContainMode::Session)) {
                // Session: setsid only — no process_group(0) (would EPERM).
                crate::containment::unix::set_session(std_cmd);
                return Prepared {
                    mode,
                    is_root,
                    cgroup_leaf: None, // cgroup not used for Session
                };
            }

            // Strongest / TreeWalk: set a new process group + try cgroup.
            crate::containment::unix::set_process_group(std_cmd);

            let leaf = crate::containment::cgroup::try_create_leaf();
            if let Some(ref l) = leaf {
                // Wire the pre_exec self-placement. The closure captures the raw
                // fd integer (Copy) — not the leaf itself (which stays in Prepared).
                // On error (e.g. EBUSY — "no internal processes" rule), the closure
                // returns Ok so the spawn proceeds and `attach` falls back to the
                // already-configured process group rather than aborting the spawn.
                // Safety: pre_exec runs post-fork, pre-exec; the function is
                // async-signal-safe (libc::write + libc::close, no alloc).
                let procs_fd = l.procs_fd();
                unsafe {
                    use std::os::unix::process::CommandExt;
                    std_cmd.pre_exec(move || {
                        let _ = crate::containment::cgroup::place_self_in_cgroup_pre_exec(procs_fd);
                        Ok(())
                    });
                }
            }
            return Prepared {
                mode,
                is_root,
                cgroup_leaf: leaf,
            };
        }
        return Prepared {
            mode,
            is_root,
            cgroup_leaf: None,
        };
    }

    // Non-Linux Unix: process group or session (mutually exclusive; S3).
    // Session mode applies setsid INSTEAD OF process_group(0): setsid makes the
    // child a session and process-group leader simultaneously, so setpgid would
    // return EPERM. Strongest on non-Linux Unix = ProcessGroup (macOS path).
    #[cfg(all(unix, not(target_os = "linux")))]
    if is_root && mode.is_some() {
        match mode {
            Some(ContainMode::Session) => crate::containment::unix::set_session(std_cmd),
            _ => crate::containment::unix::set_process_group(std_cmd),
        }
    }

    // Windows: clear handle inheritance + apply creation_flags.
    #[cfg(windows)]
    if mode.is_some() {
        crate::containment::windows::clear_std_handle_inheritance();
        if is_root {
            crate::containment::windows::set_root_flags(std_cmd);
        } else {
            crate::containment::windows::set_group_flags(std_cmd);
        }
    }

    #[allow(unreachable_code)]
    Prepared {
        mode,
        is_root,
        #[cfg(target_os = "linux")]
        cgroup_leaf: None,
    }
}

/// Phase 2 (after spawn, before SharedChild::new): attach the mechanism.
/// Consumes `prepared` so Linux cgroup leaf ownership transfers cleanly to
/// `Attached::Cgroup` without requiring interior mutability.
pub(crate) fn attach(child: &std::process::Child, prepared: Prepared) -> Result<(Containment, Attached), Error> {
    // Linux: session, or cgroup v2 / process group.
    #[cfg(target_os = "linux")]
    {
        if prepared.mode.is_some() {
            if prepared.is_root {
                let raw_pid = child.id();
                debug_assert!(
                    raw_pid <= i32::MAX as u32,
                    "pid {raw_pid} exceeds i32::MAX; pgid cast would truncate"
                );
                let pgid = raw_pid as i32;

                // Session mode: setsid was applied pre-spawn; no cgroup (S3).
                // pgid == sid == pid for the session leader; killpg works.
                if matches!(prepared.mode, Some(ContainMode::Session)) {
                    return Ok((Containment::Session, Attached::ProcessGroup(pgid)));
                }

                // Strongest / TreeWalk: cgroup v2 if available, else process group.
                if let Some(leaf) = prepared.cgroup_leaf {
                    // Verify placement: the pre_exec write can silently fail
                    // (EBUSY — "no internal processes" rule when the supervisor
                    // is itself an undelegated leaf). Read cgroup.procs to
                    // confirm the child's pid is actually present.
                    if leaf.contains_pid(raw_pid) {
                        return Ok((Containment::CgroupV2, Attached::Cgroup(leaf)));
                    }
                    // Placement failed — the leaf is empty; drop it (triggers
                    // rmdir). The process group set pre-spawn is the real container.
                    drop(leaf);
                    return Ok((Containment::ProcessGroup, Attached::ProcessGroup(pgid)));
                }
                // No cgroup leaf: fall back to process group (set pre-spawn).
                return Ok((Containment::ProcessGroup, Attached::ProcessGroup(pgid)));
            } else {
                // Nested: joined the ancestor's group.
                return Ok((Containment::ProcessGroup, Attached::None));
            }
        }
    }

    // Non-Linux Unix: process group or session.
    // For Session: setsid was called pre-spawn (not process_group(0)); the child
    // is a session leader with pgid == pid. Teardown via killpg is identical —
    // Attached::ProcessGroup(pgid) is reused since the pgroup == session leader's pid.
    // For Strongest (= ProcessGroup on macOS): process_group(0) was called pre-spawn.
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        if prepared.mode.is_some() {
            if prepared.is_root {
                let raw_pid = child.id();
                debug_assert!(
                    raw_pid <= i32::MAX as u32,
                    "pid {raw_pid} exceeds i32::MAX; pgid cast would truncate"
                );
                let pgid = raw_pid as i32;
                let containment = match prepared.mode {
                    Some(ContainMode::Session) => Containment::Session,
                    _ => Containment::ProcessGroup,
                };
                return Ok((containment, Attached::ProcessGroup(pgid)));
            } else {
                // Nested: joined the ancestor's group/session.
                return Ok((Containment::ProcessGroup, Attached::None));
            }
        }
    }

    // Windows: Job Object (strongest available on this OS).
    #[cfg(windows)]
    {
        if prepared.mode.is_some() && prepared.is_root {
            match crate::containment::windows::attach_job(child) {
                Ok(Some(job)) => return Ok((Containment::JobObject, Attached::JobObject(job))),
                Ok(None) => {
                    // Job assignment failed; degraded to lone-process (Task 7 wires TreeWalk).
                    return Ok((Containment::None, Attached::None));
                }
                Err(e) => return Err(Error::Containment { detail: e.to_string() }),
            }
        } else if prepared.mode.is_some() {
            // Nested: inherits the ancestor's job; no new job created.
            return Ok((Containment::JobObject, Attached::None));
        }
    }

    // Uncontained (or unsupported platform).
    let _ = (child, prepared);
    Ok((Containment::None, Attached::None))
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;
