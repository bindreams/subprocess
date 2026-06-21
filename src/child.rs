//! The owned child handle.

use std::collections::BTreeMap;
use std::io::{PipeReader, PipeWriter};

use shared_child::SharedChild;

use crate::command::Command;
use crate::error::Error;
use crate::identity::ProcessId;
use crate::stdio::Fd;

#[path = "child/spawn.rs"]
pub(crate) mod spawn;

/// A parent-side pipe end retained for a configured descriptor.
#[derive(Debug)]
pub(crate) enum ParentEnd {
    Reader(PipeReader),
    Writer(PipeWriter),
}

/// A spawned child process the crate owns.
#[derive(Debug)]
pub struct Child {
    shared: SharedChild,
    /// Stable identity resolved immediately after spawn. `None` if identity
    /// could not be read (degenerate environment); the child is still usable
    /// for wait/kill in that case.
    id: Option<ProcessId>,
    pipes: BTreeMap<Fd, ParentEnd>,
    kill_on_drop: bool,
}

impl Child {
    pub(crate) fn from_parts(
        shared: SharedChild,
        id: Option<ProcessId>,
        pipes: BTreeMap<Fd, ParentEnd>,
        kill_on_drop: bool,
    ) -> Child {
        Child {
            shared,
            id,
            pipes,
            kill_on_drop,
        }
    }

    /// This child's stable identity, or `None` if identity could not be read
    /// at spawn time (see [`crate::identity::ProcessId`]).
    pub fn id(&self) -> Option<ProcessId> {
        self.id
    }

    /// The raw OS process id (not stable-across-time; prefer [`id`](Self::id)).
    pub fn raw_pid(&self) -> u32 {
        self.shared.id()
    }

    /// Whether the child is still running. Returns `false` if identity could
    /// not be read at spawn time (conservative — avoids a false positive).
    pub fn is_alive(&self) -> bool {
        match &self.id {
            Some(id) => id.is_alive(),
            None => false,
        }
    }

    /// Block until the child exits, returning its status.
    pub fn wait(&self) -> Result<std::process::ExitStatus, Error> {
        self.shared.wait().map_err(Error::Io)
    }

    /// Return the exit status if the child has already exited.
    pub fn try_wait(&self) -> Result<Option<std::process::ExitStatus>, Error> {
        self.shared.try_wait().map_err(Error::Io)
    }

    /// Hard-kill the process. Returns `Ok(())` if already dead.
    pub fn kill(&self) -> Result<(), Error> {
        // shared_child delegates to std::process::Child::kill, which returns
        // Ok(()) for an already-exited child on all platforms.
        self.shared.kill().map_err(Error::Io)
    }

    /// Take the parent's write end of the child's stdin pipe, if configured.
    pub fn stdin(&mut self) -> Option<PipeWriter> {
        match self.pipes.remove(&Fd::STDIN) {
            Some(ParentEnd::Writer(w)) => Some(w),
            other => {
                if let Some(e) = other {
                    self.pipes.insert(Fd::STDIN, e);
                }
                None
            }
        }
    }

    /// Take the parent's read end of the child's stdout pipe, if configured.
    pub fn stdout(&mut self) -> Option<PipeReader> {
        take_reader(&mut self.pipes, Fd::STDOUT)
    }

    /// Take the parent's read end of the child's stderr pipe, if configured.
    pub fn stderr(&mut self) -> Option<PipeReader> {
        take_reader(&mut self.pipes, Fd::STDERR)
    }

    /// Consume the handle without killing or waiting for the child (opt out of
    /// kill-on-drop, which is added by Task 6's Drop impl).
    pub fn detach(mut self) {
        self.kill_on_drop = false;
    }
}

fn take_reader(pipes: &mut BTreeMap<Fd, ParentEnd>, fd: Fd) -> Option<PipeReader> {
    match pipes.remove(&fd) {
        Some(ParentEnd::Reader(r)) => Some(r),
        other => {
            if let Some(e) = other {
                pipes.insert(fd, e);
            }
            None
        }
    }
}

impl Command {
    /// Spawn the configured command.
    pub fn spawn(&mut self) -> Result<Child, Error> {
        spawn::spawn(self)
    }
}
