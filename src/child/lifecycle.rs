//! `Child` bounded waits. A submodule of `child` so it can reach `Child`'s private
//! `shared`.

use std::process::ExitStatus;
use std::time::{Duration, Instant};

use super::Child;
use crate::error::Error;

impl Child {
    /// Block up to `timeout` for the root process to exit. `Ok(Some(status))` =
    /// exited; `Ok(None)` = still running at expiry (not an error); `Err` = a wait
    /// failure. `Duration::ZERO` acts like [`try_wait`](Child::try_wait). Event-driven
    /// (no poll loop) and concurrent-safe with `kill` (shared_child pins the pid via
    /// `waitid(WNOWAIT)`). Reaps **only the root**: a contained tree's descendants have
    /// no waitable handle.
    pub fn wait_timeout(&self, timeout: Duration) -> Result<Option<ExitStatus>, Error> {
        self.shared.wait_timeout(timeout).map_err(Error::Io)
    }

    /// Like [`wait_timeout`](Child::wait_timeout) but against an absolute `deadline`
    /// (at or before now behaves like [`try_wait`](Child::try_wait)).
    pub fn wait_deadline(&self, deadline: Instant) -> Result<Option<ExitStatus>, Error> {
        self.shared.wait_deadline(deadline).map_err(Error::Io)
    }
}
