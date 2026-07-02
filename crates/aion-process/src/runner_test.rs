use super::*;

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

    #[tokio::test]
    async fn runner_captures_delayed_output_after_parent_shell_exits() {
        #[cfg(windows)]
        let script = "Start-Process -FilePath powershell -ArgumentList '-NoProfile', '-Command', 'Start-Sleep -Milliseconds 500; Write-Output runner_delayed_descendant_stdout' -NoNewWindow";
        #[cfg(not(windows))]
        let script = "(sleep 0.5; printf 'runner_delayed_descendant_stdout\n') &";

        let result = CommandRunner::new(shell_command(script))
            .run()
            .await
            .expect("runner should complete successfully");

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, Some(0));
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("runner_delayed_descendant_stdout"),
            "stdout was: {}",
            String::from_utf8_lossy(&result.stdout)
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
            .post_process_drain(Duration::from_millis(50))
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
