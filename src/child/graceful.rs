//! `Child` graceful shutdown — the soft-then-hard escalation trio. A submodule of `child`
//! so it can reach `Child`'s private `shared`/`id`.

use std::process::ExitStatus;
use std::time::Duration;

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
}
