use super::*;

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
                            StreamOutcome::FailedEmpty(ProviderError::Connection("disconnect".into()))
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
                            StreamOutcome::FailedEmpty(ProviderError::Connection("disconnect".into()))
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
            move |(), _tx| async move { StreamOutcome::FailedPartial(ProviderError::Connection("disconnect".into())) },
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
                tx.send(LlmEvent::TextDelta("connected".into())).await.unwrap();
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
            move |(), _tx| async move { StreamOutcome::FailedEmpty(ProviderError::Connection("disconnect".into())) },
            RetryPolicy::new(2, false, false),
        )
        .await
        .unwrap();

        assert!(matches!(rx.recv().await, Some(LlmEvent::Error(_))));
        assert!(rx.recv().await.is_none());
        assert_eq!(send_count.load(Ordering::SeqCst), 1);
    }
}
