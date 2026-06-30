use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;

use aion_types::llm::LlmEvent;

use crate::ProviderError;

#[derive(Debug)]
pub(crate) enum StreamOutcome {
    Ok,
    FailedEmpty(ProviderError),
    FailedPartial(ProviderError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetryPolicy {
    pub max_stream_retries: u32,
    pub initial_connect: bool,
    pub can_resign: bool,
}

impl RetryPolicy {
    pub(crate) const fn new(max_stream_retries: u32, initial_connect: bool, can_resign: bool) -> Self {
        Self {
            max_stream_retries,
            initial_connect,
            can_resign,
        }
    }
}

pub(crate) async fn run_stream<Resp, SendFn, SendFut, ProcessFn, ProcessFut>(
    send: SendFn,
    process: ProcessFn,
    policy: RetryPolicy,
) -> Result<mpsc::Receiver<LlmEvent>, ProviderError>
where
    Resp: Send + 'static,
    SendFn: Fn() -> SendFut + Clone + Send + Sync + 'static,
    SendFut: Future<Output = Result<Resp, ProviderError>> + Send + 'static,
    ProcessFn: Fn(Resp, mpsc::Sender<LlmEvent>) -> ProcessFut + Clone + Send + Sync + 'static,
    ProcessFut: Future<Output = StreamOutcome> + Send + 'static,
{
    let response = if policy.initial_connect {
        crate::retry::with_initial_connect_retry(send.clone()).await?
    } else {
        send().await?
    };

    let (tx, rx) = mpsc::channel(64);

    tokio::spawn(async move {
        let mut response = response;

        match process.clone()(response, tx.clone()).await {
            StreamOutcome::Ok => {}
            StreamOutcome::FailedPartial(err) => {
                let _ = tx.send(LlmEvent::Error(err.to_string())).await;
            }
            StreamOutcome::FailedEmpty(err) => {
                if !err.is_retryable() || !policy.can_resign || policy.max_stream_retries == 0 {
                    let _ = tx.send(LlmEvent::Error(err.to_string())).await;
                    return;
                }

                let mut backoff = Duration::from_secs(1);
                let mut final_err = err;

                for attempt in 1..=policy.max_stream_retries {
                    tracing::warn!(
                        attempt,
                        max_stream_retries = policy.max_stream_retries,
                        error = %final_err,
                        "retrying stream after empty stream failure"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(15));

                    match send.clone()().await {
                        Ok(next_response) => {
                            response = next_response;
                            match process.clone()(response, tx.clone()).await {
                                StreamOutcome::Ok => return,
                                StreamOutcome::FailedPartial(err) => {
                                    let _ = tx.send(LlmEvent::Error(err.to_string())).await;
                                    return;
                                }
                                StreamOutcome::FailedEmpty(err) => {
                                    final_err = err;
                                    if !final_err.is_retryable()
                                        || !policy.can_resign
                                        || attempt == policy.max_stream_retries
                                    {
                                        let _ = tx.send(LlmEvent::Error(final_err.to_string())).await;
                                        return;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            final_err = err;
                            if !is_retryable_resend_error(&final_err) || attempt == policy.max_stream_retries {
                                let _ = tx.send(LlmEvent::Error(final_err.to_string())).await;
                                return;
                            }
                        }
                    }
                }

                let _ = tx.send(LlmEvent::Error(final_err.to_string())).await;
            }
        }
    });

    Ok(rx)
}

fn is_retryable_resend_error(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Http(_)) || error.is_retryable()
}

#[cfg(test)]
#[path = "stream_runner_test.rs"]
mod stream_runner_test;
