//! Everything a terminal started, so that closing it ends all of it
//! (Windows).
//!
//! Terminating the process of a terminal ends that process only. Closing
//! the pseudo-console ends the processes still attached to it, but not
//! those that run without a console or with one of their own, such as a
//! program started with `Start-Process` or `start`, which would outlive
//! termx.
//!
//! So the process of a terminal is put in a job object as soon as it
//! exists, and every process it starts afterwards belongs to the job too.
//! Terminating the job ends them all at once, and the job is set to end
//! them as well when termx lets go of it, which the system does when termx
//! exits, however it exits.
//!
//! What escapes: a process started in the instant between the start of the
//! process of the terminal and its assignment to the job (a shell does not
//! start anything that early), and one started with permission to break
//! away from the job.

#![allow(unsafe_code)]

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// A job object that ends its processes when it is closed.
#[derive(Debug)]
pub(super) struct Job(OwnedHandle);

impl Job {
    /// Creates a job and puts the process `process` in it.
    ///
    /// # Errors
    ///
    /// The error of the system.
    pub(super) fn containing(process: RawHandle) -> io::Result<Self> {
        // SAFETY: no security attributes and no name are valid arguments;
        // the result is checked before it is used.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a valid handle that nothing else owns.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(std::mem::size_of_val(&limits)).map_err(io::Error::other)?;
        // SAFETY: the job handle is valid, and the buffer is a live
        // structure of the type the information class expects, of `size`
        // bytes.
        let set = unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both handles are valid for the duration of the call: the
        // job is ours, and the caller keeps the process handle open.
        if unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Terminates every process of the job.
    pub(super) fn terminate(&self) {
        // SAFETY: the handle is valid for as long as `self` lives. Failure
        // means that there is nothing left to terminate.
        let _ = unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) };
    }
}
