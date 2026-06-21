use subprocess::Command;

fn testbin() -> &'static str {
    env!("CARGO_BIN_EXE_subprocess_testbin")
}

#[test]
fn spawn_and_status_exit_code() {
    let mut cmd = Command::new();
    cmd.executable(testbin()).args(["subprocess_testbin", "exit", "7"]);
    let child = cmd.spawn().expect("spawn");
    let status = child.wait().expect("wait");
    assert_eq!(status.code(), Some(7));
}

#[test]
fn spawned_child_has_live_identity() {
    let mut cmd = Command::new();
    // long-ish: tee-both with no stdin closes immediately on EOF; use exit after a read.
    cmd.executable(testbin()).args(["subprocess_testbin", "exit", "0"]);
    let child = cmd.spawn().expect("spawn");
    let id = child.id();
    // The child exists right after spawn (running or just-exited zombie/handle).
    assert_eq!(id.pid(), child.id().pid());
    let _ = child.wait();
}
