use super::ToolPolicy;

#[test]
fn unrestricted_policy_allows_every_tool() {
    assert!(ToolPolicy::Unrestricted.allows("ExecCommand"));
}

#[test]
fn allow_only_policy_matches_exact_tool_names() {
    let policy = ToolPolicy::allow_only(["Read", "team_send_message"]);

    assert!(policy.allows("Read"));
    assert!(policy.allows("team_send_message"));
    assert!(!policy.allows("Write"));
    assert!(!policy.allows("read"));
}
