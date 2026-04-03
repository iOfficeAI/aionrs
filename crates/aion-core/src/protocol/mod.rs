pub mod commands;
pub mod events;
pub mod reader;
pub mod writer;

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// Result of a tool approval request
pub enum ToolApprovalResult {
    Approved,
    Denied { reason: String },
}

/// Manages pending tool approval requests using oneshot channels
pub struct ToolApprovalManager {
    /// Pending approvals: call_id -> oneshot sender
    pending: Mutex<HashMap<String, oneshot::Sender<ToolApprovalResult>>>,
    /// Session-level auto-approve set (by tool category)
    auto_approved: Mutex<HashSet<String>>,
}

impl ToolApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            auto_approved: Mutex::new(HashSet::new()),
        }
    }

    /// Request approval for a tool call. Returns a oneshot receiver that
    /// resolves when the client responds.
    pub fn request_approval(&self, call_id: &str) -> oneshot::Receiver<ToolApprovalResult> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .insert(call_id.to_string(), tx);
        rx
    }

    /// Called when client sends tool_approve/tool_deny
    pub fn resolve(&self, call_id: &str, result: ToolApprovalResult) {
        if let Some(tx) = self.pending.lock().unwrap().remove(call_id) {
            let _ = tx.send(result);
        }
    }

    /// Check if a category is session-auto-approved
    pub fn is_auto_approved(&self, category: &str) -> bool {
        self.auto_approved.lock().unwrap().contains(category)
    }

    /// Drop the pending sender for a call_id without resolving.
    /// The receiver will get a channel-closed error, simulating client disconnect.
    pub fn drop_pending(&self, call_id: &str) {
        self.pending.lock().unwrap().remove(call_id);
    }

    /// Add category to session auto-approve list
    pub fn add_auto_approve(&self, category: &str) {
        self.auto_approved
            .lock()
            .unwrap()
            .insert(category.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_request_then_approve() {
        let mgr = ToolApprovalManager::new();
        let rx = mgr.request_approval("call-1");
        mgr.resolve("call-1", ToolApprovalResult::Approved);
        assert!(matches!(rx.await.unwrap(), ToolApprovalResult::Approved));
    }

    #[tokio::test]
    async fn test_request_then_deny() {
        let mgr = ToolApprovalManager::new();
        let rx = mgr.request_approval("call-2");
        mgr.resolve(
            "call-2",
            ToolApprovalResult::Denied {
                reason: "not allowed".into(),
            },
        );
        match rx.await.unwrap() {
            ToolApprovalResult::Denied { reason } => assert_eq!(reason, "not allowed"),
            _ => panic!("expected Denied"),
        }
    }

    #[tokio::test]
    async fn test_resolve_unknown_call_id_is_noop() {
        let mgr = ToolApprovalManager::new();
        // Should not panic
        mgr.resolve("nonexistent", ToolApprovalResult::Approved);
    }

    #[tokio::test]
    async fn test_dropped_sender_errors() {
        let mgr = ToolApprovalManager::new();
        let rx = mgr.request_approval("call-3");
        // Drop the manager without resolving — sender is dropped when pending map drops
        drop(mgr);
        assert!(rx.await.is_err());
    }

    #[test]
    fn test_auto_approve_category() {
        let mgr = ToolApprovalManager::new();
        assert!(!mgr.is_auto_approved("exec"));
        mgr.add_auto_approve("exec");
        assert!(mgr.is_auto_approved("exec"));
        assert!(!mgr.is_auto_approved("edit"));
    }

    #[tokio::test]
    async fn test_multiple_pending_approvals() {
        let mgr = ToolApprovalManager::new();
        let rx1 = mgr.request_approval("a");
        let rx2 = mgr.request_approval("b");

        mgr.resolve("b", ToolApprovalResult::Approved);
        mgr.resolve(
            "a",
            ToolApprovalResult::Denied {
                reason: "no".into(),
            },
        );

        assert!(matches!(rx2.await.unwrap(), ToolApprovalResult::Approved));
        match rx1.await.unwrap() {
            ToolApprovalResult::Denied { reason } => assert_eq!(reason, "no"),
            _ => panic!("expected Denied"),
        }
    }
}
