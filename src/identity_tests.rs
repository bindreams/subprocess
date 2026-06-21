use super::{ProcessId, RawPid, StartToken};
use std::collections::HashSet;

// Build a ProcessId directly from parts (descendant module can access the
// private fields and the private StartToken). No public constructor exists yet.
fn id(pid: RawPid, tok: u64) -> ProcessId {
    ProcessId {
        pid,
        start: StartToken::from_raw(tok),
    }
}

#[test]
fn equal_when_pid_and_token_match() {
    assert_eq!(id(42, 1000), id(42, 1000));
}

#[test]
fn differ_when_pid_differs() {
    assert_ne!(id(42, 1000), id(43, 1000));
}

#[test]
fn differ_when_token_differs_same_pid() {
    // The PID-reuse case: same pid, different start token => different process.
    assert_ne!(id(42, 1000), id(42, 2000));
}

#[test]
fn hash_is_consistent_with_eq() {
    let mut set = HashSet::new();
    set.insert(id(7, 9));
    assert!(set.contains(&id(7, 9)));
    assert!(!set.contains(&id(7, 10)));
}

#[test]
fn is_copy_and_exposes_pid() {
    let a = id(5, 1);
    let b = a; // Copy: `a` remains usable below.
    assert_eq!(a.pid(), 5);
    assert_eq!(b.pid(), 5);
}
