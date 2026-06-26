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
    pub(crate) const fn new(
        max_stream_retries: u32,
        initial_connect: bool,
        can_resign: bool,
    ) -> Self {
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
                                        let _ =
                                            tx.send(LlmEvent::Error(final_err.to_string())).await;
                                        return;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            final_err = err;
                            if !is_retryable_resend_error(&final_err)
                                || attempt == policy.max_stream_retries
                            {
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
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    async fn reqwest_builder_error() -> reqwest::Error {
        reqwest::Client::new()
            .get("http://")
            .send()
            .await
            .expect_err("invalid URL should fail before request")
    }

    #[tokio::test]
    async fn test_run_stream_retries_failed_empty_then_emits_success() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));
        let process_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        let attempt = send_count.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ProviderError>(attempt)
                    }
                }
            },
            {
                let process_count = Arc::clone(&process_count);
                move |attempt, tx| {
                    let process_count = Arc::clone(&process_count);
                    async move {
                        process_count.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            StreamOutcome::FailedEmpty(ProviderError::Connection(
                                "disconnect".into(),
                            ))
                        } else {
                            tx.send(LlmEvent::TextDelta("ok".into())).await.unwrap();
                            StreamOutcome::Ok
                        }
                    }
                }
            },
            RetryPolicy::new(2, false, true),
        )
        .await
        .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "ok"
        ));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 2);
        assert_eq!(process_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_run_stream_retries_http_error_during_resend() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));
        let process_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        let attempt = send_count.fetch_add(1, Ordering::SeqCst);
                        match attempt {
                            0 => Ok(0),
                            1 => Err(ProviderError::Http(reqwest_builder_error().await)),
                            _ => Ok(2),
                        }
                    }
                }
            },
            {
                let process_count = Arc::clone(&process_count);
                move |response, tx| {
                    let process_count = Arc::clone(&process_count);
                    async move {
                        process_count.fetch_add(1, Ordering::SeqCst);
                        if response == 0 {
                            StreamOutcome::FailedEmpty(ProviderError::Connection(
                                "disconnect".into(),
                            ))
                        } else {
                            tx.send(LlmEvent::TextDelta("ok".into())).await.unwrap();
                            StreamOutcome::Ok
                        }
                    }
                }
            },
            RetryPolicy::new(2, false, true),
        )
        .await
        .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "ok"
        ));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 3);
        assert_eq!(process_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_run_stream_stops_on_non_retryable_resend_error() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));
        let process_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        let attempt = send_count.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Ok(())
                        } else {
                            Err(ProviderError::Api {
                                status: 401,
                                message: "unauthorized".into(),
                            })
                        }
                    }
                }
            },
            {
                let process_count = Arc::clone(&process_count);
                move |(), _tx| {
                    let process_count = Arc::clone(&process_count);
                    async move {
                        process_count.fetch_add(1, Ordering::SeqCst);
                        StreamOutcome::FailedEmpty(ProviderError::Connection("disconnect".into()))
                    }
                }
            },
            RetryPolicy::new(2, false, true),
        )
        .await
        .unwrap();

        assert!(matches!(rx.recv().await, Some(LlmEvent::Error(_))));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 2);
        assert_eq!(process_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_run_stream_does_not_retry_failed_partial() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        send_count.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ProviderError>(())
                    }
                }
            },
            move |(), _tx| async move {
                StreamOutcome::FailedPartial(ProviderError::Connection("disconnect".into()))
            },
            RetryPolicy::new(2, false, true),
        )
        .await
        .unwrap();

        assert!(matches!(rx.recv().await, Some(LlmEvent::Error(_))));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_run_stream_retries_initial_connect_when_enabled() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        let attempt = send_count.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(ProviderError::Connection("connection refused".into()))
                        } else {
                            Ok(())
                        }
                    }
                }
            },
            move |(), tx| async move {
                tx.send(LlmEvent::TextDelta("connected".into()))
                    .await
                    .unwrap();
                StreamOutcome::Ok
            },
            RetryPolicy::new(2, true, true),
        )
        .await
        .unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(LlmEvent::TextDelta(text)) if text == "connected"
        ));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_run_stream_does_not_retry_initial_connect_when_disabled() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));

        let result = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        send_count.fetch_add(1, Ordering::SeqCst);
                        Err::<(), _>(ProviderError::Connection("connection refused".into()))
                    }
                }
            },
            move |(), _tx| async move { StreamOutcome::Ok },
            RetryPolicy::new(2, false, true),
        )
        .await;

        assert!(matches!(result, Err(ProviderError::Connection(_))));
        assert_eq!(send_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_run_stream_respects_can_resign_false() {
        tokio::time::pause();

        let send_count = Arc::new(AtomicU32::new(0));

        let mut rx = run_stream(
            {
                let send_count = Arc::clone(&send_count);
                move || {
                    let send_count = Arc::clone(&send_count);
                    async move {
                        send_count.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, ProviderError>(())
                    }
                }
            },
            move |(), _tx| async move {
                StreamOutcome::FailedEmpty(ProviderError::Connection("disconnect".into()))
            },
            RetryPolicy::new(2, false, false),
        )
        .await
        .unwrap();

        assert!(matches!(rx.recv().await, Some(LlmEvent::Error(_))));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 1);
    }
}
