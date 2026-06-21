//! The `Command` builder. This plan implements only the input surface
//! (executable / args / commandline); stdio, env, containment, and spawning
//! are added by later plans.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::stdio::{Fd, ResolvedStdio, Stdio};

/// A process to be configured and (later) spawned.
#[derive(Debug)]
pub struct Command {
    // Read syntactically by input()/executable_path(), so the fields themselves
    // are not flagged; the accessors (consumed only by the next plan) carry the
    // allow below.
    input: CommandInput,
    executable: Option<PathBuf>,
    fds: BTreeMap<Fd, ResolvedStdio>,
    env_ops: Vec<EnvOp>,
    cwd: Option<PathBuf>,
    kill_on_drop: bool,
}

/// An environment variable operation, recorded in order.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum EnvOp {
    Set(OsString, OsString),
    Remove(OsString),
    Clear,
}

impl Default for Command {
    fn default() -> Command {
        Command {
            input: CommandInput::Empty,
            executable: None,
            fds: BTreeMap::new(),
            env_ops: Vec::new(),
            cwd: None,
            kill_on_drop: true,
        }
    }
}

/// The argument source of truth. `Argv` and `CommandLine` are mutually
/// exclusive — the last one set wins.
// The CommandLine field is read by tests and the resolution plan, not the lib target yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) enum CommandInput {
    #[default]
    Empty,
    Argv(Vec<OsString>),
    CommandLine(OsString),
}

impl Command {
    /// A fresh command with no arguments. argv is not special: set it via
    /// [`Command::args`]/[`Command::arg`] or [`Command::commandline`].
    pub fn new() -> Command {
        Command::default()
    }

    /// Append one argument, switching to argv mode if a command line was set.
    pub fn arg<S: Into<OsString>>(&mut self, a: S) -> &mut Command {
        match &mut self.input {
            CommandInput::Argv(v) => v.push(a.into()),
            _ => self.input = CommandInput::Argv(vec![a.into()]),
        }
        self
    }

    /// Append several arguments, switching to argv mode if a command line was set.
    pub fn args<I, S>(&mut self, args: I) -> &mut Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let items = args.into_iter().map(Into::into);
        match &mut self.input {
            CommandInput::Argv(v) => v.extend(items),
            _ => self.input = CommandInput::Argv(items.collect()),
        }
        self
    }

    /// Set the argument source to a single command-line string (Windows-native
    /// form). Discards any previously set argv.
    pub fn commandline<S: Into<OsString>>(&mut self, line: S) -> &mut Command {
        self.input = CommandInput::CommandLine(line.into());
        self
    }

    /// Override the executable file that the OS loads, independently of argv[0]
    /// (e.g. load `/bin/busybox` while argv[0] is `sh`).
    pub fn executable<P: Into<PathBuf>>(&mut self, path: P) -> &mut Command {
        self.executable = Some(path.into());
        self
    }

    // Consumed by the resolution plan; unused by the lib target in this plan.
    #[allow(dead_code)]
    pub(crate) fn input(&self) -> &CommandInput {
        &self.input
    }

    #[allow(dead_code)]
    pub(crate) fn executable_path(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    /// Wire descriptor `slot` to `target`. Errors now if the target's direction
    /// is ambiguous for `slot` (a bare `pipe()` on a descriptor >= 3).
    pub fn fd(&mut self, slot: impl Into<Fd>, target: Stdio) -> Result<&mut Command, Error> {
        let slot = slot.into();
        let resolved = target.resolve(slot)?;
        self.fds.insert(slot, resolved);
        Ok(self)
    }

    pub fn stdin(&mut self, target: Stdio) -> Result<&mut Command, Error> {
        self.fd(Fd::STDIN, target)
    }

    pub fn stdout(&mut self, target: Stdio) -> Result<&mut Command, Error> {
        self.fd(Fd::STDOUT, target)
    }

    pub fn stderr(&mut self, target: Stdio) -> Result<&mut Command, Error> {
        self.fd(Fd::STDERR, target)
    }

    pub fn env(&mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> &mut Command {
        self.env_ops.push(EnvOp::Set(k.into(), v.into()));
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Command
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        for (k, v) in vars {
            self.env_ops.push(EnvOp::Set(k.into(), v.into()));
        }
        self
    }

    pub fn env_remove(&mut self, k: impl Into<OsString>) -> &mut Command {
        self.env_ops.push(EnvOp::Remove(k.into()));
        self
    }

    pub fn env_clear(&mut self) -> &mut Command {
        self.env_ops.push(EnvOp::Clear);
        self
    }

    pub fn current_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Command {
        self.cwd = Some(dir.into());
        self
    }

    pub fn kill_on_drop(&mut self, yes: bool) -> &mut Command {
        self.kill_on_drop = yes;
        self
    }

    // ---- crate-internal accessors for the spawn engine (Task 4) -------------
    // `fds` is read only by command_tests; spawn uses `fds_mut` (std::mem::take).
    #[allow(dead_code)]
    pub(crate) fn fds(&self) -> &BTreeMap<Fd, ResolvedStdio> {
        &self.fds
    }

    #[allow(dead_code)]
    pub(crate) fn fds_mut(&mut self) -> &mut BTreeMap<Fd, ResolvedStdio> {
        &mut self.fds
    }

    #[allow(dead_code)]
    pub(crate) fn env_ops(&self) -> &[EnvOp] {
        &self.env_ops
    }

    #[allow(dead_code)]
    pub(crate) fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    #[allow(dead_code)]
    pub(crate) fn kill_on_drop_flag(&self) -> bool {
        self.kill_on_drop
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod command_tests;
