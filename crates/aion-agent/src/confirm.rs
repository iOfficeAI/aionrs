use std::collections::HashSet;
use std::io::{self, BufRead, Write};

pub struct ToolConfirmer {
    auto_approve: bool,
    allow_list: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    Approved,
    Denied,
    Quit,
}

impl ToolConfirmer {
    pub fn new(auto_approve: bool, allow_list: Vec<String>) -> Self {
        Self {
            auto_approve,
            allow_list: allow_list.into_iter().collect(),
        }
    }

    /// Returns whether auto-approve is enabled
    pub fn is_auto_approve(&self) -> bool {
        self.auto_approve
    }

    /// Check if the tool needs confirmation. Returns the user's decision.
    pub fn check(&mut self, tool_name: &str, tool_input_display: &str) -> ConfirmResult {
        if self.auto_approve || self.allow_list.contains(tool_name) {
            return ConfirmResult::Approved;
        }

        eprint!(
            "\n[tool] {}({})\nAllow? [y]es / [n]o / [a]lways / [q]uit > ",
            tool_name, tool_input_display
        );
        io::stderr().flush().unwrap();

        let mut input = String::new();
        if io::stdin().lock().read_line(&mut input).is_err() {
            return ConfirmResult::Denied;
        }

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => ConfirmResult::Approved,
            "a" | "always" => {
                self.allow_list.insert(tool_name.to_string());
                ConfirmResult::Approved
            }
            "q" | "quit" => ConfirmResult::Quit,
            _ => ConfirmResult::Denied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_approve_always_allows() {
        let mut confirmer = ToolConfirmer::new(true, vec![]);
        assert_eq!(confirmer.check("Bash", "echo hello"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Read", "/tmp/file"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Write", "/tmp/out"), ConfirmResult::Approved);
    }

    #[test]
    fn test_allowlist_contains_tool() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into(), "Write".into()]);
        assert_eq!(confirmer.check("Read", "/tmp/file"), ConfirmResult::Approved);
        assert_eq!(confirmer.check("Write", "/tmp/out"), ConfirmResult::Approved);
    }

    #[test]
    fn test_allowlist_approves_even_when_auto_approve_is_false() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into()]);
        assert_eq!(confirmer.check("Read", "/some/path"), ConfirmResult::Approved);
    }
}
