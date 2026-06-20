use crate::error::{AgentFailure, CommandFailure, InternalFailure};
use aion_protocol::events::{
    ErrorOwnership, PublicError, PublicErrorCode, PublicErrorDetail, PublicErrorDetailKey,
};
use aion_providers::{
    InvalidProviderRequestSource, ProviderFailure, ProviderFailureKind, ProviderFailurePhase,
};

pub fn agent_failure_to_public_error(error: &AgentFailure) -> PublicError {
    match error {
        AgentFailure::Provider(error) => provider_failure_to_public_error(error),
        AgentFailure::RepeatedMalformedToolCall { .. } => public_error(
            PublicErrorCode::ProviderToolCallInvalid,
            ErrorOwnership::Provider,
            vec![],
        ),
        AgentFailure::UserAborted => {
            public_error(PublicErrorCode::UserAborted, ErrorOwnership::User, vec![])
        }
        AgentFailure::ContextTooLong { .. } => public_error(
            PublicErrorCode::CompactionContextStillTooLarge,
            ErrorOwnership::User,
            vec![],
        ),
        AgentFailure::Command(CommandFailure::Failed { command }) => public_error(
            PublicErrorCode::CommandFailed,
            ErrorOwnership::User,
            vec![PublicErrorDetail::new(
                PublicErrorDetailKey::Command,
                command,
            )],
        ),
        AgentFailure::Internal(InternalFailure::Unexpected { .. }) => public_error(
            PublicErrorCode::InternalError,
            ErrorOwnership::Aionrs,
            vec![],
        ),
    }
}

fn provider_failure_to_public_error(error: &ProviderFailure) -> PublicError {
    let code = provider_failure_kind_to_public_error_code(&error.kind, error.meta.phase);
    public_error(
        code,
        provider_failure_ownership(&error.kind),
        provider_details(error),
    )
}

pub fn provider_failure_kind_to_public_error_code(
    kind: &ProviderFailureKind,
    phase: ProviderFailurePhase,
) -> PublicErrorCode {
    match kind {
        ProviderFailureKind::CredentialMissing => PublicErrorCode::ProviderCredentialMissing,
        ProviderFailureKind::AuthFailed => PublicErrorCode::ProviderAuthFailed,
        ProviderFailureKind::PermissionDenied => PublicErrorCode::ProviderPermissionDenied,
        ProviderFailureKind::BillingRequired => PublicErrorCode::ProviderBillingRequired,
        ProviderFailureKind::QuotaExceeded => PublicErrorCode::ProviderQuotaExceeded,
        ProviderFailureKind::RateLimited => PublicErrorCode::ProviderRateLimited,
        ProviderFailureKind::ModelNotFound => PublicErrorCode::ProviderModelNotFound,
        ProviderFailureKind::ModelUnsupported => PublicErrorCode::ProviderModelUnsupported,
        ProviderFailureKind::ModelUnavailable => PublicErrorCode::ProviderModelUnavailable,
        ProviderFailureKind::EndpointNotFound => PublicErrorCode::ProviderEndpointNotFound,
        ProviderFailureKind::ContextTooLarge => PublicErrorCode::ProviderContextTooLarge,
        ProviderFailureKind::ToolSchema(_) => PublicErrorCode::ProviderToolSchemaInvalid,
        ProviderFailureKind::ToolCall(_) => PublicErrorCode::ProviderToolCallInvalid,
        ProviderFailureKind::ContentBlocked => PublicErrorCode::ProviderContentBlocked,
        ProviderFailureKind::Timeout => PublicErrorCode::ProviderTimeout,
        ProviderFailureKind::Transport
            if matches!(
                phase,
                ProviderFailurePhase::AfterFirstDelta | ProviderFailurePhase::AfterToolCall
            ) =>
        {
            PublicErrorCode::ProviderStreamInterrupted
        }
        ProviderFailureKind::Transport => PublicErrorCode::ProviderTransportFailed,
        ProviderFailureKind::Server => PublicErrorCode::ProviderServerError,
        ProviderFailureKind::InvalidRequest(_) => PublicErrorCode::ProviderInvalidRequest,
        ProviderFailureKind::ResponseParse => PublicErrorCode::ProviderResponseParseFailed,
        ProviderFailureKind::EmptyResponse => PublicErrorCode::ProviderEmptyResponse,
        ProviderFailureKind::Unknown => PublicErrorCode::ProviderUnknownError,
    }
}

fn provider_failure_ownership(kind: &ProviderFailureKind) -> ErrorOwnership {
    match kind {
        ProviderFailureKind::CredentialMissing
        | ProviderFailureKind::AuthFailed
        | ProviderFailureKind::PermissionDenied
        | ProviderFailureKind::BillingRequired
        | ProviderFailureKind::QuotaExceeded
        | ProviderFailureKind::ModelNotFound
        | ProviderFailureKind::ModelUnsupported
        | ProviderFailureKind::EndpointNotFound
        | ProviderFailureKind::ContextTooLarge => ErrorOwnership::User,
        ProviderFailureKind::InvalidRequest(failure) => match failure.source {
            InvalidProviderRequestSource::AionrsRequestBuilder => ErrorOwnership::Aionrs,
            InvalidProviderRequestSource::Unknown => ErrorOwnership::User,
        },
        ProviderFailureKind::ToolSchema(_) | ProviderFailureKind::ToolCall(_) => {
            ErrorOwnership::Provider
        }
        ProviderFailureKind::RateLimited
        | ProviderFailureKind::ModelUnavailable
        | ProviderFailureKind::ContentBlocked
        | ProviderFailureKind::Timeout
        | ProviderFailureKind::Transport
        | ProviderFailureKind::Server
        | ProviderFailureKind::ResponseParse
        | ProviderFailureKind::EmptyResponse => ErrorOwnership::Provider,
        ProviderFailureKind::Unknown => ErrorOwnership::Unknown,
    }
}

fn provider_details(error: &ProviderFailure) -> Vec<PublicErrorDetail> {
    let mut details = vec![
        PublicErrorDetail::new(PublicErrorDetailKey::Provider, &error.meta.provider),
        PublicErrorDetail::new(
            PublicErrorDetailKey::Phase,
            provider_phase_detail(error.meta.phase),
        ),
    ];

    if let Some(model) = error.meta.model.as_deref() {
        details.push(PublicErrorDetail::new(PublicErrorDetailKey::Model, model));
    }
    if let Some(status) = error.meta.status {
        details.push(PublicErrorDetail::new(
            PublicErrorDetailKey::Status,
            status.to_string(),
        ));
    }
    if let Some(request_id) = error.meta.request_id.as_deref() {
        details.push(PublicErrorDetail::new(
            PublicErrorDetailKey::RequestId,
            request_id,
        ));
    }
    if let Some(raw_code) = error.meta.raw.raw_code.as_deref() {
        details.push(PublicErrorDetail::new(
            PublicErrorDetailKey::RawCode,
            raw_code,
        ));
    }
    if let Some(raw_type) = error.meta.raw.raw_type.as_deref() {
        details.push(PublicErrorDetail::new(
            PublicErrorDetailKey::RawType,
            raw_type,
        ));
    }

    details
}

fn public_error(
    code: PublicErrorCode,
    ownership: ErrorOwnership,
    details: Vec<PublicErrorDetail>,
) -> PublicError {
    PublicError {
        code,
        message: public_message(code).to_string(),
        ownership,
        details,
    }
}

fn public_message(code: PublicErrorCode) -> &'static str {
    match code {
        PublicErrorCode::ProviderCredentialMissing => {
            "The model provider credentials are not configured."
        }
        PublicErrorCode::ProviderAuthFailed => {
            "The model provider rejected the configured credentials."
        }
        PublicErrorCode::ProviderPermissionDenied => {
            "The model provider denied permission for this request."
        }
        PublicErrorCode::ProviderBillingRequired => {
            "The model provider requires billing to complete this request."
        }
        PublicErrorCode::ProviderQuotaExceeded => "The model provider quota has been exceeded.",
        PublicErrorCode::ProviderRateLimited => "The model provider rate-limited the request.",
        PublicErrorCode::ProviderModelNotFound => {
            "The requested model was not found by the provider."
        }
        PublicErrorCode::ProviderModelUnsupported => {
            "The requested model does not support this request."
        }
        PublicErrorCode::ProviderModelUnavailable => {
            "The requested model is currently unavailable."
        }
        PublicErrorCode::ProviderEndpointNotFound => "The model provider endpoint was not found.",
        PublicErrorCode::ProviderContextTooLarge => {
            "The request is too large for the model provider context window."
        }
        PublicErrorCode::ProviderToolSchemaInvalid => {
            "The model provider rejected the tool schema."
        }
        PublicErrorCode::ProviderToolCallInvalid => {
            "The model provider returned an invalid tool call."
        }
        PublicErrorCode::ProviderContentBlocked => {
            "The model provider blocked the requested content."
        }
        PublicErrorCode::ProviderTimeout => "The model provider request timed out.",
        PublicErrorCode::ProviderStreamInterrupted => "The model provider stream was interrupted.",
        PublicErrorCode::ProviderTransportFailed => "The model provider transport request failed.",
        PublicErrorCode::ProviderServerError => "The model provider returned a server error.",
        PublicErrorCode::ProviderInvalidRequest => "The model provider rejected the request.",
        PublicErrorCode::ProviderResponseParseFailed => {
            "The model provider response could not be parsed."
        }
        PublicErrorCode::ProviderEmptyResponse => "The model provider returned an empty response.",
        PublicErrorCode::ProviderUnknownError => "The model provider returned an unknown error.",
        PublicErrorCode::CompactionContextStillTooLarge => {
            "The conversation is still too large after compaction."
        }
        PublicErrorCode::CommandFailed => "The command failed.",
        PublicErrorCode::UserAborted => "The user aborted the operation.",
        PublicErrorCode::InternalInvariantViolation => {
            "Aionrs encountered an internal invariant violation."
        }
        PublicErrorCode::InternalError => "Aionrs encountered an internal error.",
        PublicErrorCode::ConfigInvalid
        | PublicErrorCode::ConfigProfileNotFound
        | PublicErrorCode::ConfigProviderAliasInvalid
        | PublicErrorCode::ConfigEnvMissing
        | PublicErrorCode::ConfigFileReadFailed
        | PublicErrorCode::ConfigFileParseFailed
        | PublicErrorCode::ConfigFileWriteFailed
        | PublicErrorCode::BootstrapFailed
        | PublicErrorCode::BootstrapProviderInitFailed
        | PublicErrorCode::BootstrapToolInitFailed
        | PublicErrorCode::BootstrapMcpInitFailed
        | PublicErrorCode::BootstrapMemoryInitFailed
        | PublicErrorCode::SessionCreateFailed
        | PublicErrorCode::SessionLoadFailed
        | PublicErrorCode::SessionSaveFailed
        | PublicErrorCode::SessionListFailed
        | PublicErrorCode::SessionIndexFailed
        | PublicErrorCode::SessionNotFound
        | PublicErrorCode::ToolNotFound
        | PublicErrorCode::ToolInputInvalid
        | PublicErrorCode::ToolExecutionFailed
        | PublicErrorCode::ToolTimeout
        | PublicErrorCode::ToolPermissionDenied
        | PublicErrorCode::McpServerUnavailable
        | PublicErrorCode::McpServerConfigInvalid
        | PublicErrorCode::McpProtocolError
        | PublicErrorCode::McpCapabilityDiscoveryFailed
        | PublicErrorCode::McpInvocationFailed
        | PublicErrorCode::ApprovalRejected
        | PublicErrorCode::ApprovalTimeout
        | PublicErrorCode::ApprovalChannelClosed
        | PublicErrorCode::CompactionFailed
        | PublicErrorCode::ProtocolInvalidCommand
        | PublicErrorCode::ProtocolParseFailed
        | PublicErrorCode::ProtocolClientDisconnected
        | PublicErrorCode::ProtocolStateViolation => "Aionrs encountered an error.",
    }
}

fn provider_phase_detail(phase: ProviderFailurePhase) -> &'static str {
    match phase {
        ProviderFailurePhase::BeforeFirstDelta => "before_first_delta",
        ProviderFailurePhase::AfterFirstDelta => "after_first_delta",
        ProviderFailurePhase::AfterToolCall => "after_tool_call",
    }
}

#[cfg(test)]
mod tests {
    use super::agent_failure_to_public_error;
    use crate::error::{AgentFailure, CommandFailure};
    use aion_protocol::events::{
        ErrorOwnership, PublicErrorCode, PublicErrorDetail, PublicErrorDetailKey,
    };
    use aion_providers::{
        ProviderFailurePhase, ProviderRawSignalSource, ProviderRawSignals,
        classify_provider_failure,
    };

    fn detail_value(error: &aion_protocol::events::PublicError, key: PublicErrorDetailKey) -> &str {
        error
            .details
            .iter()
            .find(|detail| detail.key == key)
            .map(|detail| detail.value.as_str())
            .expect("missing detail")
    }

    #[test]
    fn provider_auth_failed_maps_to_public_user_error_with_details() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "openai".to_string(),
            model: Some("gpt-4.1".to_string()),
            status: Some(401),
            raw_code: Some("invalid_api_key".to_string()),
            raw_type: Some("authentication_error".to_string()),
            message: Some("raw provider auth text must not leak".to_string()),
            request_id: Some("req_123".to_string()),
            retry_after_ms: None,
            source: ProviderRawSignalSource::Http,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderAuthFailed);
        assert_eq!(public.ownership, ErrorOwnership::User);
        assert_eq!(
            detail_value(&public, PublicErrorDetailKey::Provider),
            "openai"
        );
        assert_eq!(detail_value(&public, PublicErrorDetailKey::Status), "401");
        assert_eq!(
            detail_value(&public, PublicErrorDetailKey::Model),
            "gpt-4.1"
        );
        assert_eq!(
            detail_value(&public, PublicErrorDetailKey::RequestId),
            "req_123"
        );
        assert_eq!(
            detail_value(&public, PublicErrorDetailKey::RawCode),
            "invalid_api_key"
        );
        assert_eq!(
            detail_value(&public, PublicErrorDetailKey::RawType),
            "authentication_error"
        );
        assert!(!public.message.contains("raw provider auth text"));
    }

    #[test]
    fn provider_transport_after_first_delta_maps_to_stream_interrupted() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "anthropic".to_string(),
            model: None,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some("connection reset".to_string()),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Transport,
            phase: ProviderFailurePhase::AfterFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderStreamInterrupted);
        assert_eq!(public.ownership, ErrorOwnership::Provider);
    }

    #[test]
    fn provider_transport_before_first_delta_maps_to_transport_failed() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "anthropic".to_string(),
            model: None,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some("dns failure".to_string()),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Transport,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderTransportFailed);
        assert_eq!(public.ownership, ErrorOwnership::Provider);
    }

    #[test]
    fn provider_invalid_request_from_aionrs_request_builder_maps_to_aionrs_ownership() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "openai".to_string(),
            model: None,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some("invalid request payload".to_string()),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::RequestBuild,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderInvalidRequest);
        assert_eq!(public.ownership, ErrorOwnership::Aionrs);
    }

    #[test]
    fn provider_rate_limited_maps_to_provider_rate_limited() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "openai".to_string(),
            model: None,
            status: Some(429),
            raw_code: None,
            raw_type: None,
            message: None,
            request_id: None,
            retry_after_ms: Some(1000),
            source: ProviderRawSignalSource::Http,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderRateLimited);
        assert_eq!(public.ownership, ErrorOwnership::Provider);
    }

    #[test]
    fn provider_quota_exceeded_maps_to_provider_quota_exceeded() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "openai".to_string(),
            model: None,
            status: Some(429),
            raw_code: Some("insufficient_quota".to_string()),
            raw_type: None,
            message: Some("quota exceeded".to_string()),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Http,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderQuotaExceeded);
        assert_eq!(public.ownership, ErrorOwnership::User);
    }

    #[test]
    fn provider_response_parse_maps_to_response_parse_failed() {
        let failure = classify_provider_failure(ProviderRawSignals {
            provider: "openai".to_string(),
            model: None,
            status: None,
            raw_code: None,
            raw_type: None,
            message: Some("invalid json".to_string()),
            request_id: None,
            retry_after_ms: None,
            source: ProviderRawSignalSource::Json,
            phase: ProviderFailurePhase::BeforeFirstDelta,
        });

        let public = agent_failure_to_public_error(&AgentFailure::Provider(failure));

        assert_eq!(public.code, PublicErrorCode::ProviderResponseParseFailed);
        assert_eq!(public.ownership, ErrorOwnership::Provider);
    }

    #[test]
    fn repeated_malformed_tool_call_maps_to_provider_tool_call_invalid() {
        let public = agent_failure_to_public_error(&AgentFailure::RepeatedMalformedToolCall {
            count: 3,
            limit: 3,
        });

        assert_eq!(public.code, PublicErrorCode::ProviderToolCallInvalid);
        assert_eq!(public.ownership, ErrorOwnership::Provider);
    }

    #[test]
    fn context_too_long_maps_to_compaction_context_still_too_large() {
        let public = agent_failure_to_public_error(&AgentFailure::ContextTooLong {
            input_tokens: 101,
            limit: 100,
        });

        assert_eq!(public.code, PublicErrorCode::CompactionContextStillTooLarge);
        assert_eq!(public.ownership, ErrorOwnership::User);
    }

    #[test]
    fn command_failed_maps_to_command_failed_with_command_detail() {
        let public =
            agent_failure_to_public_error(&AgentFailure::Command(CommandFailure::Failed {
                command: "/compact".to_string(),
            }));

        assert_eq!(public.code, PublicErrorCode::CommandFailed);
        assert_eq!(public.ownership, ErrorOwnership::User);
        assert_eq!(
            public.details,
            vec![PublicErrorDetail::new(
                PublicErrorDetailKey::Command,
                "/compact"
            )]
        );
    }
}
