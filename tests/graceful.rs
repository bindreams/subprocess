//! Graceful-escalation trio integration tests (Child + Process). Death is proven only by a
//! real exit event — control-socket EOF/ConnectionReset or an inspected ExitStatus signal —
//! never by sleep, poll loop, or wall-clock. Escalation tests use a SIGTERM-ignoring child +
//! Duration::ZERO, so escalation is deterministic (the child is alive at the single poll).

#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
#[test]
fn child_terminate_sends_sigterm() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    let (child, mut sock) = common::spawn_blocker();
    child.terminate().expect("terminate sends SIGTERM");
    // Prove death by a real event: the control socket EOFs.
    let mut buf = [0u8; 1];
    match sock.read(&mut buf) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("expected EOF/ConnectionReset after SIGTERM, got {other:?}"),
    }
    // Reap and assert it died by SIGTERM (soft), NOT SIGKILL.
    let status = child.wait().expect("reap");
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "control-block must die by SIGTERM, got {status:?}"
    );
}

#[cfg(windows)]
#[test]
fn child_terminate_unsupported_on_windows() {
    let (child, _sock) = common::spawn_blocker();
    let err = child
        .terminate()
        .expect_err("lone graceful terminate has no Windows primitive");
    assert!(
        matches!(err, subprocess::error::Error::Unsupported { .. }),
        "got {err:?}"
    );
    child.kill().expect("cleanup");
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn child_graceful_shutdown_graceful_path() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // control-block dies on default-disposition SIGTERM. The long grace is the safety bound on
    // a child that exits promptly — never the synchronization; correctness is the exit signal.
    let (child, mut sock) = common::spawn_blocker();
    let status = child
        .graceful_shutdown(Duration::from_secs(30))
        .expect("graceful_shutdown");
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "graceful path must exit via SIGTERM, got {status:?}"
    );
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf); // dead — EOF
}

#[cfg(unix)]
#[test]
fn child_graceful_shutdown_escalates() {
    use std::io::Read;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    // This child installs SIG_IGN for SIGTERM, so it NEVER exits on the soft signal. With
    // Duration::ZERO the child is provably alive at the single poll → escalation to SIGKILL is
    // deterministic (no timing dependency at all). Because SIGTERM is ignored, SIGKILL is the
    // ONLY terminating signal the child can receive, so signal()==SIGKILL is unambiguous — do
    // not weaken control-block-ignore-term to honor SIGTERM or this assertion loses its meaning.
    let (child, mut sock) = common::spawn_control("control-block-ignore-term", &["R"], false);
    let status = child
        .graceful_shutdown(Duration::ZERO)
        .expect("graceful_shutdown escalates");
    assert_eq!(
        status.signal(),
        Some(libc::SIGKILL),
        "SIGTERM-ignoring child must be force-killed, got {status:?}"
    );
    let mut buf = [0u8; 1];
    let _ = sock.read(&mut buf);
}

#[cfg(windows)]
#[test]
fn child_graceful_shutdown_unsupported_on_windows() {
    use std::time::Duration;
    let (child, _sock) = common::spawn_blocker();
    let err = child
        .graceful_shutdown(Duration::from_secs(1))
        .expect_err("no Windows lone graceful");
    assert!(
        matches!(err, subprocess::error::Error::Unsupported { .. }),
        "got {err:?}"
    );
    child.kill().expect("cleanup");
    let _ = child.wait();
}
