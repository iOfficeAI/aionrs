use async_trait::async_trait;
#[cfg(test)]
use serde_json::Value;
use tokio::sync::mpsc;

use aion_types::llm::{LlmEvent, LlmRequest};

use crate::composed::ComposedProvider;
use crate::transport::{AnthropicTransport, ProviderTransport};
use crate::{LlmProvider, ProviderError};
use aion_config::compat::ProviderCompat;

pub struct AnthropicProvider {
    inner: ComposedProvider,
    api_key: String,
    base_url: String,
    compat: ProviderCompat,
    cache_enabled: bool,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, base_url: &str, compat: ProviderCompat) -> Self {
        let cache_enabled = true;
        let inner = Self::build_inner(api_key, base_url, cache_enabled, &compat);

        Self {
            inner,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            compat,
            cache_enabled,
        }
    }

    pub fn with_cache(mut self, enabled: bool) -> Self {
        self.cache_enabled = enabled;
        self.inner = Self::build_inner(&self.api_key, &self.base_url, self.cache_enabled, &self.compat);
        self
    }

    fn build_inner(api_key: &str, base_url: &str, cache_enabled: bool, compat: &ProviderCompat) -> ComposedProvider {
        let transport = ProviderTransport::Anthropic(AnthropicTransport::new(api_key, base_url, cache_enabled));
        ComposedProvider::new(transport, compat.clone())
    }

    #[cfg(test)]
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, ProviderError> {
        self.inner.build_request_body(request)
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream(&self, request: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.inner.stream(request).await
    }
}

#[cfg(test)]
#[path = "anthropic_test.rs"]
mod anthropic_test;
