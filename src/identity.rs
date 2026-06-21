//! Stable-across-time process identity.
//!
//! A bare PID is unsafe: the OS recycles PIDs, so the same number can name a
//! different process minutes later. [`ProcessId`] pairs the PID with a raw
//! kernel *start token* — a per-process value fixed at creation — so equality
//! distinguishes "the same process" from "a reused PID".
//!
//! The token is the RAW kernel value (Windows creation `FILETIME`, Linux
//! `/proc` `starttime` jiffies, macOS `proc_bsdinfo` start µs), compared
//! exactly. It is deliberately NOT a wall-clock time: deriving wall-clock from
//! boot time drifts under NTP and would silently break `Eq`/`Hash`. The
//! human-facing wall-clock lives in `created_at()` (Task 2), allowed to drift
//! and never used for identity.

/// A process identifier as the OS knows it (matches `std::process::id`).
pub type RawPid = u32;

/// A raw, per-process kernel start value. Opaque: its only meaning is identity
/// (exact equality). Interpreted into a wall-clock time only by `created_at`.
// Not constructed outside tests until Task 2 wires the per-OS backends; the
// allow is removed there once `from_raw`/`raw` have real callers.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct StartToken(u64);

#[allow(dead_code)]
impl StartToken {
    fn from_raw(v: u64) -> StartToken {
        StartToken(v)
    }

    fn raw(self) -> u64 {
        self.0
    }
}

/// A process identity that stays unique across time: `(pid, start_token)`.
/// `Eq`/`Hash` are over the pair, so a recycled PID never compares equal to the
/// original process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessId {
    pid: RawPid,
    start: StartToken,
}

impl ProcessId {
    /// The raw OS process id. NOTE: a bare PID is not unique across time — use
    /// the whole `ProcessId` for identity, comparison, and map keys.
    pub fn pid(&self) -> RawPid {
        self.pid
    }
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod identity_tests;
