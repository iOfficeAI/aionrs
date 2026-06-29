use std::io::{Error, Result};

use tokio::process::{Child, Command};

#[cfg(windows)]
use crate::windows_job::JobObject;

pub(crate) struct ChildContainment {
    #[cfg(windows)]
    job: JobObject,
}

impl ChildContainment {
    pub(crate) fn configure(command: &mut Command) {
        configure_command(command);
    }

    pub(crate) fn attach(child: &mut Child) -> Result<Self> {
        attach_child(child)
    }

    pub(crate) fn terminate(&self, child: &mut Child, child_id: Option<u32>) -> Result<()> {
        terminate_child(child, child_id, self)
    }
}

#[cfg(unix)]
fn configure_command(command: &mut Command) {
    command.process_group(0);
}

#[cfg(windows)]
fn configure_command(command: &mut Command) {
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    command.creation_flags(CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure_command(_command: &mut Command) {}

#[cfg(windows)]
fn attach_child(child: &mut Child) -> Result<ChildContainment> {
    let pid = child.id().ok_or_else(|| Error::other("spawned child has no pid"))?;
    let raw_handle = child
        .raw_handle()
        .ok_or_else(|| Error::other("spawned child has no raw handle"))?;

    let job = JobObject::assign_and_resume(raw_handle, pid)
        .map_err(|error| Error::other(format!("windows job containment failed: {error}")))?;

    Ok(ChildContainment { job })
}

#[cfg(not(windows))]
fn attach_child(_child: &mut Child) -> Result<ChildContainment> {
    Ok(ChildContainment {})
}

#[cfg(unix)]
fn terminate_child(child: &mut Child, child_id: Option<u32>, _containment: &ChildContainment) -> Result<()> {
    if let Some(target) = child_id.and_then(group_kill_target) {
        let rc = unsafe { libc::kill(target, libc::SIGKILL) };
        if rc == 0 {
            return Ok(());
        }

        let err = Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }

        return Err(err);
    }

    child.start_kill()
}

#[cfg(windows)]
fn terminate_child(_child: &mut Child, _child_id: Option<u32>, containment: &ChildContainment) -> Result<()> {
    containment.job.terminate()
}

#[cfg(not(any(unix, windows)))]
fn terminate_child(child: &mut Child, _child_id: Option<u32>, _containment: &ChildContainment) -> Result<()> {
    child.start_kill()
}

#[cfg(unix)]
fn group_kill_target(pid: u32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }

    i32::try_from(pid).ok().map(|pid| -pid)
}
