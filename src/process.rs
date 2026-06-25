//! A foreign process referenced by stable identity. Wraps a `ProcessId` (never a bare
//! pid) and exposes lifecycle / identity / tree — NO stdio (we don't own its pipes).
//! Every operation re-verifies identity. `wait()` is a death-watch yielding no
//! `ExitStatus` (the kernel hands exit status only to the real parent — contrast
//! `Child::wait`).

use std::time::Duration;

use crate::error::Error;
use crate::identity::{ProcessId, RawPid};

/// Whether a tree query descends recursively or returns only direct children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recursive {
    /// Only direct children (one level).
    No,
    /// All descendants (the whole subtree).
    Yes,
}

/// A handle to a process identified by `(pid, start_token)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Process {
    id: ProcessId,
}

impl Process {
    /// Resolve a foreign process by a saved identity. `None` if that exact identity is
    /// gone or the pid was recycled.
    pub fn from_id(id: ProcessId) -> Option<Process> {
        (ProcessId::of(id.pid()) == Some(id)).then_some(Process { id })
    }

    /// Resolve the process currently holding `pid`. `None` if no live process has it.
    pub fn from_pid(pid: RawPid) -> Option<Process> {
        ProcessId::of(pid).map(|id| Process { id })
    }

    /// This process's own handle. Infallible.
    pub fn current() -> Process {
        Process {
            id: ProcessId::current(),
        }
    }

    /// The stable identity (`(pid, start_token)`).
    pub fn id(&self) -> ProcessId {
        self.id
    }

    /// Whether the process is still running (zombie-exclusive; see [`ProcessId::is_alive`]).
    pub fn is_alive(&self) -> bool {
        self.id.is_alive()
    }

    /// Block until the process exits. Death-watch — yields no `ExitStatus` (we are not its
    /// parent). `Err` only on a wait failure (incl. `Unsupported` on Linux < 5.3). Non-reaping.
    pub fn wait(&self) -> Result<(), Error> {
        let exited = crate::wait::block_until_exit(self.id, None)?;
        debug_assert!(exited, "an unbounded wait can only return on exit");
        Ok(())
    }

    /// Block up to `timeout` for the process to exit. `Ok(true)` = exited; `Ok(false)` =
    /// still alive at expiry. `Duration::ZERO` polls once.
    pub fn wait_timeout(&self, timeout: Duration) -> Result<bool, Error> {
        crate::wait::block_until_exit(self.id, Some(timeout))
    }

    /// The parent process, by identity. **Best-effort and possibly wrong on recycle:**
    /// resolved from a `(pid, ppid)` host snapshot that carries no parent start-token, so
    /// if the real parent exits and `ppid` is recycled in the resolution window, this can
    /// return a DIFFERENT (impostor) process. Use only as a hint, not an identity guarantee.
    pub fn parent(&self) -> Option<Process> {
        let parents = crate::containment::enumerate::process_parents();
        let ppid = parents
            .iter()
            .find(|&&(pid, _)| pid == self.id.pid())
            .map(|&(_, ppid)| ppid)?;
        ProcessId::of(ppid).map(|id| Process { id })
    }

    /// The process's children. `Recursive::No` = direct children; `Recursive::Yes` = the
    /// whole subtree. Identity-guarded against pid-reuse by the tree-walk token rule (a
    /// candidate is kept only if its start token orders at-or-after this process). Snapshot;
    /// best-effort.
    pub fn children(&self, recursive: Recursive) -> Vec<Process> {
        let parents = crate::containment::enumerate::process_parents();
        let ids = match recursive {
            Recursive::No => crate::containment::treewalk::children_of(self.id, &parents),
            Recursive::Yes => crate::containment::treewalk::descendants(self.id, &parents),
        };
        ids.into_iter().map(|id| Process { id }).collect()
    }

    /// Hard-kill the process by identity (`SIGKILL` / `TerminateProcess`). Already-dead ⇒
    /// `Ok`; a real failure (no rights / `EPERM` / access-denied on a live process) ⇒ `Err`.
    /// **Race-freedom is OS-dependent:** Linux uses an identity-bound `pidfd_send_signal`
    /// (atomic, zero pid-reuse race) and Windows pins the kernel object via its handle; macOS
    /// has no pidfd, so it re-verifies identity immediately before `kill(2)` with a small
    /// irreducible residual window — best-effort there, like the existing tree teardown.
    pub fn kill(&self) -> Result<(), Error> {
        crate::wait::kill(self.id)
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod process_tests;
