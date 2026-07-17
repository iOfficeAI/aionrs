use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::Instrument;

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
    max_stream_retries: u32,
    initial_connect: bool,
    can_resign: bool,
    initial_http_5xx: bool,
}

impl RetryPolicy {
    pub(crate) const fn new(
        max_stream_retries: u32,
        initial_connect: bool,
        can_resign: bool,
        initial_http_5xx: bool,
    ) -> Self {
        Self {
            max_stream_retries,
            initial_connect,
            can_resign,
            initial_http_5xx,
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
    let send_initial = || {
        let send = send.clone();
        async move {
            if policy.initial_connect {
                crate::retry::with_initial_connect_retry(send).await
            } else {
                send().await
            }
        }
    };

    let response = if policy.initial_http_5xx {
        crate::retry::with_initial_http_5xx_retry(send_initial).await?
    } else {
        send_initial().await?
    };

    let (tx, rx) = mpsc::channel(64);

    let stream_span = tracing::Span::current();
    let stream_task = async move {
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

                    let resend = send.clone();
                    let resend_result = if policy.initial_http_5xx {
                        crate::retry::with_initial_http_5xx_retry(resend).await
                    } else {
                        resend().await
                    };

                    match resend_result {
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
    };
    tokio::spawn(stream_task.instrument(stream_span));

    Ok(rx)
}

fn is_retryable_resend_error(error: &ProviderError) -> bool {
    matches!(error, ProviderError::Http(_)) || error.is_retryable()
}

#[cfg(test)]
#[path = "stream_runner_test.rs"]
mod stream_runner_test;
