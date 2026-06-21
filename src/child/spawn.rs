//! std-only spawn: resolve our Stdio model onto `std::process::Command`, wire
//! the program/args via the Plan-1 quoters, and spawn through `shared_child`.

use std::collections::BTreeMap;
use std::process::Stdio as StdStdio;

use shared_child::SharedChild;

use crate::child::{Child, ParentEnd};
use crate::command::{Command, CommandInput, EnvOp};
use crate::error::Error;
use crate::identity::ProcessId;
use crate::stdio::{Direction, Fd, ResolvedStdio};

// A child-side descriptor handed to std via `Stdio::from`.
#[cfg(unix)]
type ChildEnd = std::os::unix::io::OwnedFd;
#[cfg(windows)]
type ChildEnd = std::os::windows::io::OwnedHandle;

pub(crate) fn spawn(cmd: &mut Command) -> Result<Child, Error> {
    let fds = std::mem::take(cmd.fds_mut());
    let kill_on_drop = cmd.kill_on_drop_flag();

    // Reject what the std-only backend cannot express in this plan.
    for slot in fds.keys() {
        if slot.raw() >= 3 {
            return Err(Error::Unsupported {
                op: format!("{slot}"),
                platform: std::env::consts::OS,
                detail: "arbitrary descriptors (>= 3) require the raw backend (Plan 4)".into(),
            });
        }
    }

    let mut std_cmd = build_std_command(cmd)?;

    // Resolve each std slot to a (child-side ChildEnd, optional parent end).
    // Default unset 0/1/2 to inherit. Resolve non-merge first so merge can dup.
    let mut child_ends: BTreeMap<Fd, ChildEnd> = BTreeMap::new();
    let mut parent_ends: BTreeMap<Fd, ParentEnd> = BTreeMap::new();

    for slot in [Fd::STDIN, Fd::STDOUT, Fd::STDERR] {
        let resolved = fds.get(&slot);
        if let Some(ResolvedStdio::Merge(_)) = resolved {
            continue; // second pass
        }
        let (child_end, parent) = resolve_non_merge(slot, resolved)?;
        if let Some(p) = parent {
            parent_ends.insert(slot, p);
        }
        child_ends.insert(slot, child_end);
    }
    // Second pass: merges dup an already-resolved target's child end.
    for slot in [Fd::STDIN, Fd::STDOUT, Fd::STDERR] {
        if let Some(ResolvedStdio::Merge(target)) = fds.get(&slot) {
            let src = child_ends.get(target).ok_or_else(|| Error::Unsupported {
                op: format!("merge {slot} -> {target}"),
                platform: std::env::consts::OS,
                detail: "merge target descriptor is not configured".into(),
            })?;
            child_ends.insert(slot, dup(src)?);
        }
    }

    // Hand the child ends to std (consumes them; std closes its copies on spawn).
    for (slot, end) in child_ends {
        let stdio = StdStdio::from(end);
        match slot {
            Fd::STDIN => std_cmd.stdin(stdio),
            Fd::STDOUT => std_cmd.stdout(stdio),
            _ => std_cmd.stderr(stdio),
        };
    }

    let shared = SharedChild::spawn(&mut std_cmd).map_err(Error::Io)?;
    let id = ProcessId::of(shared.id()).ok_or_else(|| {
        Error::Io(std::io::Error::other(
            "spawned child vanished before its identity could be read",
        ))
    })?;

    Ok(Child::from_parts(shared, id, parent_ends, kill_on_drop))
}

fn build_std_command(cmd: &Command) -> Result<std::process::Command, Error> {
    // Program + args via the Plan-1 quoting model.
    let mut std_cmd = match cmd.input() {
        CommandInput::Empty => return Err(Error::Io(std::io::Error::other("no program specified"))),
        CommandInput::Argv(argv) => {
            let (program, rest) = program_and_args(cmd, argv)?;
            let mut c = std::process::Command::new(program);
            c.args(rest);
            c
        }
        CommandInput::CommandLine(line) => build_from_commandline(cmd, line)?,
    };
    // Reject .bat/.cmd (BatBadBut) — only meaningful on Windows.
    reject_batch_script(&std_cmd)?;
    apply_env(&mut std_cmd, cmd.env_ops());
    if let Some(dir) = cmd.cwd() {
        std_cmd.current_dir(dir);
    }
    Ok(std_cmd)
}

// Program + the trailing args (argv mode). `executable` overrides the loaded
// file; argv[0] is the conventional program name otherwise.
fn program_and_args<'a>(
    cmd: &'a Command,
    argv: &'a [std::ffi::OsString],
) -> Result<(std::ffi::OsString, &'a [std::ffi::OsString]), Error> {
    if argv.is_empty() && cmd.executable_path().is_none() {
        return Err(Error::Io(std::io::Error::other("empty argv")));
    }
    let program = match cmd.executable_path() {
        Some(p) => p.as_os_str().to_os_string(),
        None => argv[0].clone(),
    };
    let rest = if argv.is_empty() { argv } else { &argv[1..] };
    Ok((program, rest))
}

#[cfg(unix)]
fn build_from_commandline(cmd: &Command, line: &std::ffi::OsString) -> Result<std::process::Command, Error> {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let words = crate::quote::posix::split(line.as_bytes())?;
    // `OsStringExt::from_vec` already yields an OsString; do NOT wrap it in
    // `OsString::from(..)` (that is redundant and fails type inference).
    let argv: Vec<OsString> = words.into_iter().map(OsString::from_vec).collect();
    if argv.is_empty() {
        return Err(Error::Io(std::io::Error::other("empty command line")));
    }
    let program = match cmd.executable_path() {
        Some(p) => p.as_os_str().to_os_string(),
        None => argv[0].clone(),
    };
    let mut c = std::process::Command::new(program);
    c.args(&argv[1..]);
    Ok(c)
}

#[cfg(windows)]
fn build_from_commandline(cmd: &Command, line: &std::ffi::OsString) -> Result<std::process::Command, Error> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::process::CommandExt;
    // Windows is command-line-native. CRITICAL: std::process always PREPENDS a
    // quoted form of the program to lpCommandLine and then appends raw_arg. So
    // raw_arg must be the ARGS portion only (the line MINUS its first token);
    // passing the whole line would duplicate the program token in the child's
    // argv. We split the first token off with first_token_and_rest_wide.
    //
    // NOTE: when BOTH `executable` is set AND a command line is given, the child's
    // argv[0] becomes `executable`, not the command line's first token. The raw
    // backend (Plan 4) removes this limitation; std has no stable API to set
    // lpApplicationName independently of lpCommandLine.
    // TODO(plan4): implement raw backend to support independent executable + commandline on Windows
    let wide: Vec<u16> = line.encode_wide().collect();
    let (first, rest) = crate::quote::windows::first_token_and_rest_wide(&wide)
        .ok_or_else(|| Error::Io(std::io::Error::other("empty command line")))?;
    let program = match cmd.executable_path() {
        Some(p) => p.as_os_str().to_os_string(),
        None => std::ffi::OsString::from_wide(&first),
    };
    let mut c = std::process::Command::new(program);
    c.raw_arg(std::ffi::OsString::from_wide(&rest)); // args only — program is prepended by std
    Ok(c)
}

fn reject_batch_script(std_cmd: &std::process::Command) -> Result<(), Error> {
    let prog = std::path::Path::new(std_cmd.get_program());
    if let Some(ext) = prog.extension() {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        if ext == "bat" || ext == "cmd" {
            return Err(Error::Unsupported {
                op: format!("running {}", prog.display()),
                platform: "windows",
                detail: "cmd.exe batch escaping is not implemented (CVE-2024-24576); \
                         use .commandline() to pass an explicit, pre-escaped command line"
                    .into(),
            });
        }
    }
    Ok(())
}

fn apply_env(std_cmd: &mut std::process::Command, ops: &[EnvOp]) {
    for op in ops {
        match op {
            EnvOp::Set(k, v) => {
                std_cmd.env(k, v);
            }
            EnvOp::Remove(k) => {
                std_cmd.env_remove(k);
            }
            EnvOp::Clear => {
                std_cmd.env_clear();
            }
        }
    }
}

// Resolve a non-merge slot to its child-side end + the parent's pipe end (if any).
fn resolve_non_merge(slot: Fd, r: Option<&ResolvedStdio>) -> Result<(ChildEnd, Option<ParentEnd>), Error> {
    match r {
        None | Some(ResolvedStdio::Inherit) => Ok((inherit_end(slot)?, None)),
        Some(ResolvedStdio::Null) => Ok((null_end()?, None)),
        Some(ResolvedStdio::File(f)) => Ok((file_end(f)?, None)),
        Some(ResolvedStdio::Pipe(dir)) => make_pipe(*dir),
        Some(ResolvedStdio::Merge(_)) => unreachable!("merge handled in second pass"),
    }
}

fn make_pipe(dir: Direction) -> Result<(ChildEnd, Option<ParentEnd>), Error> {
    let (reader, writer) = std::io::pipe().map_err(Error::Io)?;
    match dir {
        // Child reads: child gets the reader; parent keeps the writer.
        Direction::In => Ok((ChildEnd::from(reader), Some(ParentEnd::Writer(writer)))),
        // Child writes: child gets the writer; parent keeps the reader.
        Direction::Out => Ok((ChildEnd::from(writer), Some(ParentEnd::Reader(reader)))),
    }
}

fn dup(end: &ChildEnd) -> Result<ChildEnd, Error> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        end.as_fd().try_clone_to_owned().map_err(Error::Io)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        end.as_handle().try_clone_to_owned().map_err(Error::Io)
    }
}

fn null_end() -> Result<ChildEnd, Error> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .map_err(Error::Io)?;
    Ok(ChildEnd::from(f))
}

fn file_end(f: &std::fs::File) -> Result<ChildEnd, Error> {
    // Dup the file so the caller's File stays usable.
    let dup = f.try_clone().map_err(Error::Io)?;
    Ok(ChildEnd::from(dup))
}

fn inherit_end(slot: Fd) -> Result<ChildEnd, Error> {
    // Duplicate the parent's matching std stream. Bind the stream to a variable
    // before borrowing its descriptor (a temporary would be dropped while borrowed).
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let owned = match slot {
            Fd::STDIN => {
                let s = std::io::stdin();
                s.as_fd().try_clone_to_owned()
            }
            Fd::STDOUT => {
                let s = std::io::stdout();
                s.as_fd().try_clone_to_owned()
            }
            _ => {
                let s = std::io::stderr();
                s.as_fd().try_clone_to_owned()
            }
        };
        owned.map_err(Error::Io)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        let owned = match slot {
            Fd::STDIN => {
                let s = std::io::stdin();
                s.as_handle().try_clone_to_owned()
            }
            Fd::STDOUT => {
                let s = std::io::stdout();
                s.as_handle().try_clone_to_owned()
            }
            _ => {
                let s = std::io::stderr();
                s.as_handle().try_clone_to_owned()
            }
        };
        owned.map_err(Error::Io)
    }
}
