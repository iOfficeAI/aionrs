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
#[path = "runner_test.rs"]
mod runner_test;
