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
