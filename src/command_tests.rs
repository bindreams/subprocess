use crate::command::{Command, CommandInput};
use crate::stdio::{Direction, ResolvedStdio, Stdio};
use crate::Fd;
use std::ffi::OsString;
use std::path::Path;

fn argv(cmd: &Command) -> Vec<String> {
    match cmd.input() {
        CommandInput::Argv(v) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
        other => panic!("expected Argv, got {:?}", other),
    }
}

#[test]
fn new_is_empty() {
    let cmd = Command::new();
    assert!(matches!(cmd.input(), CommandInput::Empty));
    assert!(cmd.executable_path().is_none());
}

#[test]
fn args_sets_and_extends_argv() {
    let mut cmd = Command::new();
    cmd.args(["git", "status"]).args(["--short"]);
    assert_eq!(argv(&cmd), ["git", "status", "--short"]);
}

#[test]
fn arg_appends_one() {
    let mut cmd = Command::new();
    cmd.arg("echo").arg("hi");
    assert_eq!(argv(&cmd), ["echo", "hi"]);
}

#[test]
fn commandline_sets_string_source() {
    let mut cmd = Command::new();
    cmd.commandline(r#"git "status""#);
    match cmd.input() {
        CommandInput::CommandLine(s) => assert_eq!(s, &OsString::from(r#"git "status""#)),
        other => panic!("expected CommandLine, got {:?}", other),
    }
}

#[test]
fn commandline_then_args_switches_source_and_discards() {
    let mut cmd = Command::new();
    cmd.commandline("ignored string").args(["real", "argv"]);
    assert_eq!(argv(&cmd), ["real", "argv"]);
}

#[test]
fn args_then_commandline_switches_to_string() {
    let mut cmd = Command::new();
    cmd.args(["a", "b"]).commandline("c d");
    assert!(matches!(cmd.input(), CommandInput::CommandLine(_)));
}

#[test]
fn executable_overrides_load_path_independently_of_argv() {
    let mut cmd = Command::new();
    cmd.executable("/bin/busybox").args(["sh", "-c", "echo hi"]);
    assert_eq!(cmd.executable_path(), Some(Path::new("/bin/busybox")));
    assert_eq!(argv(&cmd), ["sh", "-c", "echo hi"]);
}

#[test]
fn stdout_shorthand_records_resolved_pipe_out() {
    let mut cmd = Command::new();
    cmd.args(["x"]);
    cmd.stdout(Stdio::pipe()).unwrap();
    let fds = cmd.fds();
    assert!(matches!(
        fds.get(&Fd::STDOUT),
        Some(ResolvedStdio::Pipe(Direction::Out))
    ));
}

#[test]
fn stdin_pipe_infers_in() {
    let mut cmd = Command::new();
    cmd.stdin(Stdio::pipe()).unwrap();
    assert!(matches!(
        cmd.fds().get(&Fd::STDIN),
        Some(ResolvedStdio::Pipe(Direction::In))
    ));
}

#[test]
fn bare_pipe_on_fd3_errs_at_attach() {
    let mut cmd = Command::new();
    assert!(cmd.fd(3, Stdio::pipe()).is_err());
}

#[test]
fn explicit_pipe_out_on_fd3_attaches() {
    let mut cmd = Command::new();
    cmd.fd(3, Stdio::pipe_out()).unwrap();
    assert!(matches!(
        cmd.fds().get(&Fd::from(3)),
        Some(ResolvedStdio::Pipe(Direction::Out))
    ));
}

#[test]
fn kill_on_drop_defaults_true_and_toggles() {
    let mut cmd = Command::new();
    assert!(cmd.kill_on_drop_flag());
    cmd.kill_on_drop(false);
    assert!(!cmd.kill_on_drop_flag());
}

#[test]
fn env_and_cwd_recorded() {
    let mut cmd = Command::new();
    cmd.env("K", "V").current_dir("/tmp");
    assert!(cmd.cwd().is_some());
    // env_ops detail is exercised end-to-end in the spawn integration tests.
}
