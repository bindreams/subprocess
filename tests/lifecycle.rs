use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use subprocess::Command;

fn testbin() -> &'static str {
    env!("CARGO_BIN_EXE_subprocess_testbin")
}

/// Spawn `mode <addr> [extra...]` as a single control child; return
/// `(child, its_control_socket)`. The child connects, writes a 1-byte tag, then
/// blocks; the accepted+read socket proves it is alive. `contain` selects `.contain()`.
fn spawn_control(mode: &str, extra: &[&str], contain: bool) -> (subprocess::Child, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind control listener");
    let addr = listener.local_addr().unwrap().to_string();
    let mut argv: Vec<String> = vec!["subprocess_testbin".into(), mode.into(), addr];
    argv.extend(extra.iter().map(|s| s.to_string()));
    let mut cmd = Command::new();
    cmd.executable(testbin()).args(&argv);
    if contain {
        cmd.contain();
    }
    let child = cmd.spawn().expect("spawn control child");
    let (mut sock, _) = listener.accept().expect("accept control conn");
    let mut tag = [0u8; 1];
    sock.read_exact(&mut tag).expect("read control tag");
    (child, sock)
}

#[test]
fn wait_timeout_returns_none_while_running() {
    // `control-block` blocks on sock.read forever; the test never writes, so the
    // child cannot exit. wait_timeout bounds a never-completing event -> Ok(None),
    // independent of the duration. (Aliveness is guaranteed STRUCTURALLY by the
    // never-written socket; the None result is meaningful only paired with the
    // companion `wait_timeout_returns_some_after_exit`, which pins the Some path.)
    let (child, _sock) = spawn_control("control-block", &["R"], false);
    let r = child.wait_timeout(Duration::from_millis(200)).expect("wait_timeout");
    assert!(r.is_none(), "still-running child must time out to None, got {r:?}");
    child.kill().expect("kill cleanup");
    let _ = child.wait();
}

#[test]
fn wait_timeout_returns_some_after_exit() {
    // Writing a byte makes the child's sock.read return -> it exits(0). A generous
    // wait_timeout then observes the exit; the bound is only a failure backstop.
    let (child, mut sock) = spawn_control("control-block", &["R"], false);
    sock.write_all(b"x").expect("trigger child exit");
    let r = child.wait_timeout(Duration::from_secs(30)).expect("wait_timeout");
    assert!(
        matches!(r, Some(s) if s.success()),
        "exited child must be observed, got {r:?}"
    );
}

#[test]
fn wait_timeout_zero_returns_none_while_running() {
    // Duration::ZERO is the try_wait path: it must return immediately with None for a
    // still-running (structurally wedged) child, never block or false-positive Some.
    let (child, _sock) = spawn_control("control-block", &["R"], false);
    let r = child.wait_timeout(Duration::ZERO).expect("wait_timeout");
    assert!(r.is_none(), "ZERO timeout on a live child must be None, got {r:?}");
    child.kill().expect("kill cleanup");
    let _ = child.wait();
}

#[test]
fn wait_deadline_past_returns_none_while_running() {
    // A deadline at/before now is the try_wait path. Aliveness is STRUCTURAL: the child
    // is blocked on an un-written control socket and cannot exit, so None is determined
    // by structure, not by the deadline value (meaningful paired with the Some test below).
    let (child, _sock) = spawn_control("control-block", &["R"], false);
    let r = child.wait_deadline(Instant::now()).expect("wait_deadline");
    assert!(r.is_none(), "live child past a past deadline must be None, got {r:?}");
    child.kill().expect("kill cleanup");
    let _ = child.wait();
}

#[test]
fn wait_deadline_past_returns_some_after_exit() {
    // After the child exits and is reaped, a deadline at/before now (== try_wait) returns
    // the cached Some — proving the past-deadline path is try_wait, not a block.
    let (child, mut sock) = spawn_control("control-block", &["R"], false);
    sock.write_all(b"x").expect("trigger child exit");
    child.wait().expect("reap"); // deterministically confirm exit + reap first
    let r = child.wait_deadline(Instant::now()).expect("wait_deadline");
    assert!(
        matches!(r, Some(s) if s.success()),
        "reaped child past a past deadline must be Some, got {r:?}"
    );
}

#[test]
fn concurrent_wait_timeout_and_kill_is_safe() {
    // wait_timeout in one thread, kill in another, sharing &Child (requires Child:
    // Sync, asserted below). shared_child makes this race-free (waitid WNOWAIT pins
    // the pid). The kill guarantees death, so wait_timeout observes Some —
    // deterministic, no is_alive/sleep. The 30s is only a backstop against a hang.
    let (child, _sock) = spawn_control("control-block", &["R"], false);
    std::thread::scope(|s| {
        let waiter = s.spawn(|| child.wait_timeout(Duration::from_secs(30)));
        child.kill().expect("kill");
        let result = waiter.join().expect("join waiter");
        assert!(
            matches!(result, Ok(Some(_))),
            "concurrent wait_timeout must observe the kill, got {result:?}"
        );
    });
}

#[test]
fn child_is_send_and_sync() {
    // Load-bearing for `concurrent_wait_timeout_and_kill_is_safe` (shares &Child across
    // threads). Compile-time invariant guard.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<subprocess::Child>();
}

#[test]
fn tree_ops_on_uncontained_child_are_unsupported() {
    let mut cmd = Command::new();
    cmd.executable(testbin()).args(["subprocess_testbin", "exit", "0"]);
    let child = cmd.spawn().expect("spawn");
    assert_eq!(child.containment(), subprocess::Containment::None);
    for r in [child.kill_tree(), child.terminate_tree()] {
        assert!(
            matches!(r, Err(subprocess::error::Error::Unsupported { .. })),
            "uncontained _tree op must be Unsupported, got {r:?}"
        );
    }
    let _ = child.wait();
}

#[test]
fn nested_member_kill_tree_is_unsupported_end_to_end() {
    // Spawn the reporter CONTAINED so it inherits NESTED_ENV; its crate-spawned grandchild
    // is therefore a nested member (Attached::Delegated), whose kill_tree() must be
    // Unsupported — proving the full prepare->attach->require_contained chain for a REAL
    // nested member, not just a hand-built Prepared.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    let mut cmd = Command::new();
    cmd.executable(testbin())
        .args(["subprocess_testbin", "report-nested-kill-tree", &addr])
        .contain();
    let child = cmd.spawn().expect("spawn reporter");
    let (mut sock, _) = listener.accept().expect("accept");
    let mut tag = [0u8; 1];
    sock.read_exact(&mut tag).expect("read report");
    assert_eq!(&tag, b"U", "a nested member's kill_tree() must be Unsupported");
    let _ = child.wait();
}
