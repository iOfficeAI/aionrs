use aion_agent::engine::AgentError as EngineAgentError;
use aion_agent::error::AgentError;

#[test]
fn engine_reexports_agent_error_for_existing_callers() {
    let err: EngineAgentError = AgentError::UserAborted;

    assert!(matches!(err, AgentError::UserAborted));
}
