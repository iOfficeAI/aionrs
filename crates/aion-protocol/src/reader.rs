use std::io;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::commands::ProtocolCommand;

/// Reads JSON Lines from stdin in a background task.
/// Returns a channel receiver for parsed commands.
pub fn spawn_stdin_reader() -> mpsc::UnboundedReceiver<ProtocolCommand> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF - client closed stdin
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<ProtocolCommand>(trimmed) {
                        Ok(cmd) => {
                            if tx.send(cmd).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::debug!(target: "aion_protocol", error = %e, "invalid protocol command");
                        }
                    }
                }
                Err(e) if is_transient_stdin_read_error(&e) => {
                    sleep(Duration::from_millis(10)).await;
                }
                Err(e) => {
                    tracing::debug!(target: "aion_protocol", error = %e, "stdin read error");
                    break;
                }
            }
        }
    });

    rx
}

fn is_transient_stdin_read_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    #[test]
    fn retries_transient_stdin_read_errors() {
        assert!(is_transient_stdin_read_error(&Error::from(
            ErrorKind::WouldBlock
        )));
        assert!(is_transient_stdin_read_error(&Error::from(
            ErrorKind::Interrupted
        )));
    }

    #[test]
    fn does_not_retry_terminal_stdin_read_errors() {
        assert!(!is_transient_stdin_read_error(&Error::from(
            ErrorKind::BrokenPipe
        )));
        assert!(!is_transient_stdin_read_error(&Error::from(
            ErrorKind::UnexpectedEof
        )));
    }
}
