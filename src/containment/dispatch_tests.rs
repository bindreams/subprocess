// Unit tests for dispatch root-detection logic.
// These test `is_nested` directly — no process-env mutation required.

use super::is_nested;

/// Without the marker the process is the root (not nested).
#[test]
fn is_nested_without_marker_is_false() {
    assert!(!is_nested(false));
}

/// With the marker the process is already inside a contained tree (nested).
#[test]
fn is_nested_with_marker_is_true() {
    assert!(is_nested(true));
}
