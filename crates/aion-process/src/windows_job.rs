use std::os::windows::io::RawHandle;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

pub(crate) struct JobObject {
    job: HANDLE,
}

unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    pub(crate) fn assign_and_resume(
        child_raw: RawHandle,
        pid: u32,
    ) -> std::result::Result<Self, String> {
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(format!(
                "CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let this = Self { job };

        let ok = unsafe { AssignProcessToJobObject(job, child_raw as HANDLE) };
        if ok == 0 {
            return Err(format!(
                "AssignProcessToJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        if let Err(error) = resume_threads(pid) {
            let _ = this.terminate();
            return Err(format!("resume failed: {error}"));
        }

        Ok(this)
    }

    pub(crate) fn terminate(&self) -> std::io::Result<()> {
        let ok = unsafe { TerminateJobObject(self.job, 1) };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.job) };
    }
}

fn resume_threads(pid: u32) -> std::result::Result<(), String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "CreateToolhelp32Snapshot failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let result = resume_threads_from_snapshot(snapshot, pid);
    unsafe { CloseHandle(snapshot) };
    result
}

fn resume_threads_from_snapshot(snapshot: HANDLE, pid: u32) -> std::result::Result<(), String> {
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;

    let mut ok = unsafe { Thread32First(snapshot, &mut entry) };
    if ok == 0 {
        return Err(format!(
            "Thread32First failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut resumed = 0_u32;
    while ok != 0 {
        if entry.th32OwnerProcessID == pid {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        ok = unsafe { Thread32Next(snapshot, &mut entry) };
    }

    if resumed == 0 {
        return Err(format!("no threads found for child process {pid}"));
    }

    Ok(())
}

fn resume_thread(thread_id: u32) -> std::result::Result<(), String> {
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(format!(
            "OpenThread({thread_id}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let resume_result = unsafe { ResumeThread(thread) };
    unsafe { CloseHandle(thread) };

    if resume_result == u32::MAX {
        return Err(format!(
            "ResumeThread({thread_id}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(())
}
