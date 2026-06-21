//! Unix process-group containment: the root becomes a process-group leader
//! (`process_group(0)`, pgid == pid), and the whole group is signalled with
//! `killpg`. cgroup v2 (Task 4) preempts this when available; this is the
//! universal Unix fallback and the macOS path.
//! Parent-side signals use `nix` (not hand-rolled `libc`); see Global Constraints.

use std::io;

use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;

/// Apply pre-spawn group setup to `std_cmd` (root spawns only).
pub(crate) fn set_process_group(std_cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    std_cmd.process_group(0); // leader: pgid == pid
}

/// Hard-kill the whole process group. ESRCH (gone) is success; EPERM falls
/// back to the leader directly (pgid == pid for a process-group root).
pub(crate) fn kill_group(pgid: i32) -> io::Result<()> {
    match killpg(Pid::from_raw(pgid), Signal::SIGKILL) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(nix::errno::Errno::EPERM) => kill_direct(pgid),
        Err(e) => Err(io::Error::from(e)),
    }
}

/// SIGTERM the group; ESRCH (gone) is success; EPERM falls back to the leader
/// directly (valid because pgid == pid — the sudo-wrapper case).
pub(crate) fn term_group(pgid: i32) -> io::Result<()> {
    match killpg(Pid::from_raw(pgid), Signal::SIGTERM) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(nix::errno::Errno::EPERM) => term_direct(pgid),
        Err(e) => Err(io::Error::from(e)),
    }
}

fn kill_direct(pid: i32) -> io::Result<()> {
    match kill(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(io::Error::from(e)),
    }
}

fn term_direct(pid: i32) -> io::Result<()> {
    match kill(Pid::from_raw(pid), Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(e) => Err(io::Error::from(e)),
    }
}

#[cfg(test)]
#[path = "unix_tests.rs"]
mod unix_tests;
