use std::collections::BTreeSet;

/// Runtime authorization policy for tools registered with an agent engine.
///
/// The policy is enforced both when tool definitions are sent to the model and
/// immediately before a requested tool is executed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ToolPolicy {
    /// Every registered tool is available.
    #[default]
    Unrestricted,
    /// Only tools whose exact names are present in the set are available.
    AllowOnly(BTreeSet<String>),
}

impl ToolPolicy {
    pub fn allow_only<I, S>(tool_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::AllowOnly(tool_names.into_iter().map(Into::into).collect())
    }

    pub fn allows(&self, tool_name: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::AllowOnly(tool_names) => tool_names.contains(tool_name),
        }
    }
}

#[cfg(test)]
#[path = "tool_policy_test.rs"]
mod tool_policy_test;
