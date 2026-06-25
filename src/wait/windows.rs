//! Windows death-watch + kill. `OpenProcess` returns a HANDLE that pins the kernel
//! object, so a reused pid cannot fool it; we re-verify the start_token once at open.
//! No reaping concept on Windows.

use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

use crate::error::Error;
use crate::identity::ProcessId;

fn close(handle: HANDLE) {
    // Match identity/windows.rs: a failed CloseHandle of an owned handle is a contract
    // violation, asserted in debug.
    let closed = unsafe { CloseHandle(handle) };
    debug_assert!(closed.is_ok(), "CloseHandle of an owned process handle should not fail");
}

pub(crate) fn block_until_exit(id: ProcessId, deadline: Option<Option<Instant>>) -> Result<bool, Error> {
    // SAFETY: OpenProcess tolerates a dead/invalid pid (returns Err); the handle is
    // closed on every return path below.
    let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, id.pid()) }
    {
        Ok(h) => h,
        // gone / unopenable => exited (an access-denied live process reads as exited here,
        // mirroring ProcessId::is_alive, which also treats open-failure as not-running).
        Err(_) => return Ok(true),
    };
    if !id.exists() {
        close(handle);
        return Ok(true); // recycled before open
    }
    let ms: u32 = match crate::wait::remaining(deadline) {
        None => INFINITE,
        Some(d) => d.as_millis().min((INFINITE - 1) as u128) as u32,
    };
    // SAFETY: `handle` is a live process handle held for the wait's duration.
    let waited = unsafe { WaitForSingleObject(handle, ms) };
    close(handle);
    if waited == WAIT_OBJECT_0 {
        Ok(true)
    } else if waited == WAIT_TIMEOUT {
        Ok(false)
    } else {
        Err(Error::Io(std::io::Error::last_os_error()))
    }
}

#[allow(dead_code)] // foreign kill is wired in Task 3; the backend lands with the wait primitive.
pub(crate) fn kill(id: ProcessId) -> Result<(), Error> {
    if ProcessId::of(id.pid()) != Some(id) {
        return Ok(()); // gone / recycled => already-dead is success
    }
    // SAFETY: OpenProcess tolerates an invalid pid; the handle is closed on success.
    let handle = match unsafe { OpenProcess(PROCESS_TERMINATE, false, id.pid()) } {
        Ok(h) => h,
        // Couldn't open for terminate: if the process is gone it's success; if it still
        // exists we genuinely lack rights (access-denied) => surface it.
        Err(e) => {
            return if id.exists() { Err(Error::Io(e.into())) } else { Ok(()) };
        }
    };
    // SAFETY: handle is live; close on every path.
    let res = unsafe { TerminateProcess(handle, 1) };
    close(handle);
    match res {
        Ok(()) => Ok(()),
        Err(e) => {
            if id.exists() {
                Err(Error::Io(e.into()))
            } else {
                Ok(())
            }
        }
    }
}
