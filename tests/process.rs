//! Foreign `Process` integration tests. A child we spawn (and own) is treated as
//! a foreign process via its `ProcessId`; death/liveness is proven only by a real
//! exit event (control-socket EOF or the kernel exit edge), never by sleep/poll.

use std::io::{Read, Write};
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;
use common::spawn_blocker;

#[test]
fn foreign_wait_returns_when_the_process_exits() {
    let (child, mut sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("foreign process resolves");
    sock.write_all(b"x").expect("trigger child exit");
    p.wait().expect("foreign wait");
    // Prove the exit via a real event (the dead child's socket EOFs), not is_alive().
    // wait()'s block-until-exit semantics are independently pinned by the wait_timeout
    // tests below (a no-op wait would make THOSE fail deterministically).
    let mut buf = [0u8; 1];
    match sock.read(&mut buf) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("expected EOF/ConnectionReset after wait observed the exit, got {other:?}"),
    }
    let _ = child.wait();
}

#[test]
fn foreign_wait_timeout_times_out_on_a_live_process() {
    // The blocker is structurally wedged on its never-written socket, so it cannot
    // exit; wait_timeout returns Ok(false) regardless of the (short) duration.
    let (child, _sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    let exited = p.wait_timeout(Duration::from_millis(200)).expect("wait_timeout");
    assert!(
        !exited,
        "a wedged process must time out to Ok(false), got exited={exited}"
    );
    child.kill().expect("kill cleanup");
    let _ = child.wait();
}

#[test]
fn foreign_wait_timeout_observes_an_exit() {
    let (child, mut sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    sock.write_all(b"x").expect("trigger child exit");
    assert!(p.wait_timeout(Duration::from_secs(30)).expect("wait_timeout"));
    let _ = child.wait();
}

#[test]
fn foreign_wait_timeout_zero_returns_immediately_on_a_live_process() {
    // ZERO is the poll-once edge in each backend; a wedged child must yield Ok(false).
    let (child, _sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    assert!(!p.wait_timeout(Duration::ZERO).expect("wait_timeout"));
    child.kill().expect("kill cleanup");
    let _ = child.wait();
}

#[test]
fn foreign_wait_timeout_huge_duration_does_not_panic() {
    // Duration::MAX overflows Instant + Duration; the saturating deadline must make
    // it unbounded, not panic. Trigger the exit first so the wait completes.
    let (child, mut sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("resolves");
    sock.write_all(b"x").expect("trigger child exit");
    assert!(p.wait_timeout(Duration::MAX).expect("wait_timeout"));
    let _ = child.wait();
}

#[test]
fn current_and_from_id_round_trip() {
    let me = subprocess::Process::current();
    assert!(me.is_alive());
    assert_eq!(subprocess::Process::from_id(me.id()).map(|p| p.id()), Some(me.id()));
}

#[test]
fn from_pid_resolves_a_live_foreign_child_then_reports_it_dead() {
    // from_pid resolves a live foreign child to its true identity; after the child is
    // killed+reaped, that resolved Process reports !is_alive (the synchronously-correct
    // liveness check, distinct from the zombie-inclusive exists()). Death is proven by the
    // reap, never by sleep.
    let (child, _sock) = spawn_blocker();
    let p = subprocess::Process::from_pid(child.id().pid()).expect("live foreign child resolves");
    assert_eq!(p.id(), child.id(), "from_pid must resolve the child's true identity");
    assert!(p.is_alive(), "a freshly spawned foreign child is alive");
    child.kill().expect("kill");
    child.wait().expect("reap"); // synchronously confirm exit before the liveness assertion
    assert!(!p.is_alive(), "a killed+reaped foreign process must report not-alive");
}
