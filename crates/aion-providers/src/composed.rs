use async_trait::async_trait;
#[cfg(test)]
use serde_json::Value;
use tokio::sync::mpsc;

use aion_config::compat::ProviderCompat;
use aion_types::llm::{LlmEvent, LlmRequest};

use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::stream_runner::run_stream;
use crate::transport::ProviderTransport;

#[derive(Clone)]
pub(crate) struct ComposedProvider {
    transport: ProviderTransport,
    compat: ProviderCompat,
}

impl ComposedProvider {
    pub(crate) fn new(transport: ProviderTransport, compat: ProviderCompat) -> Self {
        Self { transport, compat }
    }

    #[cfg(test)]
    pub(crate) fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        let (body, _) = self.transport.project_body(request, &self.compat)?;
        Ok(body)
    }
}

#[async_trait]
impl LlmProvider for ComposedProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (body, tool_wire_shape) = self.transport.project_body(request, &self.compat)?;

        tracing::debug!(target: "aion_providers", body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "outgoing request");

        let projected_request =
            self.transport
                .build_projected_request(&request.model, body, &self.compat, tool_wire_shape)?;
        let transport = self.transport.clone();
        let send = move || {
            let transport = transport.clone();
            let request = projected_request.clone();
            async move { transport.send(request).await }
        };

        let decoder = self.transport.decoder(&self.compat);
        let process = move |response, tx| async move { decoder.process(response, &tx).await };
        let retry_policy = self.transport.retry_policy();

        run_stream(send, process, retry_policy).await
    }
}

#[cfg(test)]
#[path = "composed_test.rs"]
mod composed_test;
