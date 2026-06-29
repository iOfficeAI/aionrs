use std::io::{Error, Result};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;

use crate::containment::ChildContainment;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const DEFAULT_POST_PROCESS_DRAIN: Duration = Duration::from_millis(250);

/// Runs one process and buffers its stdout/stderr while it is running.
pub struct CommandRunner {
    command: Command,
    timeout: Duration,
    post_process_drain: Duration,
}

impl CommandRunner {
    pub fn new(command: Command) -> Self {
        Self {
            command,
            timeout: DEFAULT_TIMEOUT,
            post_process_drain: DEFAULT_POST_PROCESS_DRAIN,
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn post_process_drain(mut self, drain: Duration) -> Self {
        self.post_process_drain = drain;
        self
    }

    pub async fn run(mut self) -> Result<CommandResult> {
        self.command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        ChildContainment::configure(&mut self.command);

        let mut child = self.command.spawn()?;
        let child_id = child.id();
        let containment = ChildContainment::attach(&mut child)?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));

        let stdout_reader = child
            .stdout
            .take()
            .map(|reader| read_stream(reader, Arc::clone(&stdout)));
        let stderr_reader = child
            .stderr
            .take()
            .map(|reader| read_stream(reader, Arc::clone(&stderr)));

        match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(status) => {
                let status = status?;
                let (stdout_result, stderr_result) = tokio::join!(
                    drain_reader_with_result(stdout_reader, self.post_process_drain),
                    drain_reader_with_result(stderr_reader, self.post_process_drain)
                );
                stdout_result?;
                stderr_result?;

                Ok(CommandResult {
                    exit_code: status.code(),
                    timed_out: false,
                    stdout: take_output(stdout),
                    stderr: take_output(stderr),
                })
            }
            Err(_) => {
                containment.terminate(&mut child, child_id)?;
                if let Ok(status) = tokio::time::timeout(self.post_process_drain, child.wait()).await {
                    status?;
                }
                tokio::join!(
                    drain_reader(stdout_reader, self.post_process_drain),
                    drain_reader(stderr_reader, self.post_process_drain)
                );

                Ok(CommandResult {
                    exit_code: None,
                    timed_out: true,
                    stdout: take_output(stdout),
                    stderr: take_output(stderr),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn read_stream<R>(mut reader: R, output: Arc<Mutex<Vec<u8>>>) -> JoinHandle<Result<()>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0_u8; 8192];

        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(());
            }

            output
                .lock()
                .expect("process output mutex should not be poisoned")
                .extend_from_slice(&buffer[..read]);
        }
    })
}

async fn drain_reader(reader: Option<JoinHandle<Result<()>>>, drain: Duration) {
    let _reader_result = drain_reader_with_result(reader, drain).await;
}

async fn drain_reader_with_result(reader: Option<JoinHandle<Result<()>>>, drain: Duration) -> Result<()> {
    if let Some(mut reader) = reader {
        tokio::select! {
            _ = tokio::time::sleep(drain) => {
                reader.abort();
                let _abort_join_result = reader.await;
                Ok(())
            }
            result = &mut reader => {
                result
                    .map_err(|error| Error::other(format!("process output reader failed: {error}")))?
            }
        }
    } else {
        Ok(())
    }
}

fn take_output(output: Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    output
        .lock()
        .expect("process output mutex should not be poisoned")
        .clone()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::process::Command;

    use super::CommandRunner;

    #[tokio::test]
    async fn runner_preserves_stdout_emitted_before_timeout() {
        #[cfg(windows)]
        let script = "Write-Output runner_stdout_before_timeout; Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let script = "printf 'runner_stdout_before_timeout\n'; sleep 5";

        let command = shell_command(script);
        let result = CommandRunner::new(command)
            .timeout(Duration::from_millis(1500))
            .run()
            .await
            .expect("runner should return timeout result");

        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("runner_stdout_before_timeout"),
            "stdout was: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[tokio::test]
    async fn runner_preserves_stderr_emitted_before_timeout() {
        #[cfg(windows)]
        let script = "Write-Error runner_stderr_before_timeout; Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let script = "printf 'runner_stderr_before_timeout\n' >&2; sleep 5";

        let command = shell_command(script);
        let result = CommandRunner::new(command)
            .timeout(Duration::from_millis(1500))
            .run()
            .await
            .expect("runner should return timeout result");

        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("runner_stderr_before_timeout"),
            "stderr was: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[tokio::test]
    async fn runner_returns_exit_code_and_output_for_completed_command() {
        #[cfg(windows)]
        let script = "Write-Output runner_completed_stdout; Write-Error runner_completed_stderr; exit 7";
        #[cfg(not(windows))]
        let script = "printf 'runner_completed_stdout\n'; printf 'runner_completed_stderr\n' >&2; exit 7";

        let command = shell_command(script);
        let result = CommandRunner::new(command).run().await.expect("runner should complete");

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(7));
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("runner_completed_stdout"),
            "stdout was: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("runner_completed_stderr"),
            "stderr was: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn runner_does_not_hang_when_background_process_keeps_output_pipe_open() {
        let command = shell_command("printf 'background_parent_done\n'; sleep 5 &");

        let result = tokio::time::timeout(
            Duration::from_millis(700),
            CommandRunner::new(command)
                .post_process_drain(Duration::from_millis(50))
                .run(),
        )
        .await
        .expect("runner should return before the background child closes inherited output pipes")
        .expect("runner should complete successfully");

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("background_parent_done"),
            "stdout was: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_timeout_kills_process_group() {
        let result = CommandRunner::new(shell_command("sleep 5 & echo $!; wait"))
            .timeout(Duration::from_millis(300))
            .post_process_drain(Duration::from_millis(100))
            .run()
            .await
            .expect("runner should return timeout result");

        assert!(result.timed_out);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let sleep_pid = stdout
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .expect("script should print background sleep pid");

        assert_process_exits(sleep_pid).await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn runner_timeout_kills_windows_job_descendant() {
        let script = "$p = Start-Process -FilePath powershell -ArgumentList '-NoProfile', '-Command', 'Start-Sleep -Seconds 10' -PassThru -WindowStyle Hidden; Write-Output $p.Id; Wait-Process -Id $p.Id";
        let result = CommandRunner::new(shell_command(script))
            .timeout(Duration::from_millis(5000))
            .post_process_drain(Duration::from_millis(250))
            .run()
            .await
            .expect("runner should return timeout result");

        assert!(result.timed_out);
        let stdout = String::from_utf8_lossy(&result.stdout);
        let sleep_pid = stdout
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .expect("script should print child process pid");

        assert_process_exits(sleep_pid).await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn runner_completed_command_keeps_windows_background_descendant_running() {
        let script = "$p = Start-Process -FilePath powershell -ArgumentList '-NoProfile', '-Command', 'Start-Sleep -Seconds 10' -PassThru -WindowStyle Hidden; Write-Output $p.Id";
        let result = CommandRunner::new(shell_command(script))
            .run()
            .await
            .expect("runner should complete successfully");

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        let stdout = String::from_utf8_lossy(&result.stdout);
        let sleep_pid = stdout
            .lines()
            .find_map(|line| line.trim().parse::<u32>().ok())
            .expect("script should print child process pid");

        assert!(
            process_alive(sleep_pid),
            "background process {sleep_pid} should remain alive after successful command completion"
        );
        terminate_process(sleep_pid);
    }

    #[cfg(windows)]
    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-Command", script]);
        command
    }

    #[cfg(not(windows))]
    fn shell_command(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[cfg(unix)]
    async fn assert_process_exits(pid: u32) {
        for _ in 0..20 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("process {pid} was still alive after process-group timeout kill");
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        let Ok(target) = i32::try_from(pid) else {
            return false;
        };

        let rc = unsafe { libc::kill(target, 0) };
        if rc == 0 {
            return true;
        }

        !matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH))
    }

    #[cfg(windows)]
    async fn assert_process_exits(pid: u32) {
        for _ in 0..20 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("process {pid} was still alive after job timeout kill");
    }

    #[cfg(windows)]
    fn process_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        };

        const SYNCHRONIZE: u32 = 0x0010_0000;

        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return false;
        }

        let wait_result = unsafe { WaitForSingleObject(handle, 0) };
        unsafe { CloseHandle(handle) };

        wait_result == WAIT_TIMEOUT
    }

    #[cfg(windows)]
    fn terminate_process(pid: u32) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            return;
        }

        unsafe { TerminateProcess(handle, 1) };
        unsafe { CloseHandle(handle) };
    }
}
