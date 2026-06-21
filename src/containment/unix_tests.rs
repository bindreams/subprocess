// Thin wrappers around `nix`; substantive coverage lives in the integration
// test (`unix_kill_tree_reaps_the_grandchild` in `tests/spawn_io.rs`).
// These unit tests serve as compile guards and document the ESRCH/EPERM mapping.

use super::{kill_group, term_group};

/// Killing a nonexistent pgid (ESRCH) must be a silent no-op — the goal is
/// "gone", and it already is.
#[test]
fn kill_group_ignores_esrch() {
    // PID 1 can never be a pgid we own, and pgid 0x7fff_fffe is far beyond any
    // real PID range. Use the latter to guarantee ESRCH rather than EPERM.
    kill_group(i32::MAX - 1);
    // If we reach here without panicking the ESRCH was swallowed.
}

/// term_group on a nonexistent pgid must return Ok (ESRCH → success).
#[test]
fn term_group_esrch_is_ok() {
    assert!(term_group(i32::MAX - 1).is_ok());
}
