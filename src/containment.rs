//! Process-tree containment. Spawn a child as a kill-group root and tear the
//! whole tree down as a unit. Mechanisms, strongest first, all *best-effort in
//! their own way* (the variant names the teardown method, not a quality grade):
//!
//! - [`Containment::CgroupV2`] (Linux): leaf cgroup + `cgroup.kill` — fork-proof.
//! - [`Containment::JobObject`] (Windows): Job + `KILL_ON_JOB_CLOSE`.
//! - [`Containment::ProcessGroup`]/[`Containment::Session`] (Unix): `killpg`.
//! - [`Containment::TreeWalk`] (all): identity-aware descendant kill at teardown.
//! - [`Containment::None`]: not contained — lone-process semantics.
//!
//! This is NOT a security sandbox: a determined child escapes every mechanism
//! (broker-spawned helpers, privilege, `setsid` out of a process group). It
//! reliably tears down *cooperative* trees and reports the achieved guarantee.

use std::fmt;

/// The teardown mechanism actually achieved for a spawned child (queried via
/// `Child::containment()`). Runtime-detected — the same binary meets hosts with
/// and without cgroup v2 / inside and outside an existing job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Containment {
    /// Linux cgroup v2 leaf + `cgroup.kill`. Fork-proof; a confined child can't leave.
    CgroupV2,
    /// Windows Job Object + `KILL_ON_JOB_CLOSE`. Kernel-enforced for direct descendants.
    JobObject,
    /// Unix process group (`setpgid`/`process_group(0)`) + `killpg`. A `setsid` child escapes.
    ProcessGroup,
    /// Unix session (`setsid`) + `killpg`. A nested-`setsid` child escapes.
    Session,
    /// Identity-aware descendant enumeration at teardown. Misses reparented orphans.
    TreeWalk,
    /// No containment — `kill`/drop act on the lone process.
    None,
}

impl fmt::Display for Containment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Containment::CgroupV2 => "cgroup v2",
            Containment::JobObject => "job object",
            Containment::ProcessGroup => "process group",
            Containment::Session => "session",
            Containment::TreeWalk => "process-tree walk",
            Containment::None => "none",
        })
    }
}

/// The teardown strategy a caller *requests* via `Command::contain_with`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContainMode {
    /// The strongest kernel container available on this host, falling back
    /// (cgroup → job → process group → …) to [`ContainMode::TreeWalk`] rather
    /// than to no containment.
    Strongest,
    /// Identity-aware process-tree walk at teardown — selectable directly (e.g.
    /// for a child known to `setsid` out of a process group).
    TreeWalk,
}

/// Whether a kill-group root marks its descendants as already-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Nesting {
    /// Mark descendants (default): nested contained spawns join THIS group.
    #[default]
    Mark,
    /// Leave descendants unmarked: a contained child's own contained spawns
    /// create their own groups (which nest inside this one on Windows).
    Opaque,
}

/// The resolved containment request carried on a `Command` (crate-internal).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ContainRequest {
    /// `None` = not contained.
    pub mode: Option<ContainMode>,
    pub nesting: Nesting,
}

impl Default for ContainRequest {
    fn default() -> ContainRequest {
        ContainRequest {
            mode: None,
            nesting: Nesting::Mark,
        }
    }
}

/// The reserved, inherited env marker for kill-group root detection. Windows
/// jobs nest but Unix process groups do not, so only the OUTERMOST `.contain()`
/// creates a group; descendants inherit this marker and join it. **Reserved and
/// load-bearing: nothing outside this crate may set it.**
#[allow(dead_code)]
pub(crate) const NESTED_ENV: &str = "__SUBPROCESS_GROUP_ROOT";

#[cfg(test)]
#[path = "containment_tests.rs"]
mod containment_tests;
