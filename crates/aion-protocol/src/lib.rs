// JSON stream protocol for host ↔ agent communication.
// Contains: events (agent→host), commands (host→agent), approval manager.

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
    pending: Mutex<HashMap<String, oneshot::Sender<ToolApprovalResult>>>,
    auto_approved: Mutex<HashSet<String>>,
}

impl ToolApprovalManager {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            auto_approved: Mutex::new(HashSet::new()),
        }
    }

    pub fn request_approval(&self, call_id: &str) -> oneshot::Receiver<ToolApprovalResult> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(call_id.to_string(), tx);
        rx
    }

    pub fn resolve(&self, call_id: &str, result: ToolApprovalResult) {
        if let Some(tx) = self.pending.lock().unwrap().remove(call_id) {
            let _ = tx.send(result);
        }
    }

    pub fn is_auto_approved(&self, category: &str) -> bool {
        self.auto_approved.lock().unwrap().contains(category)
    }

    pub fn drop_pending(&self, call_id: &str) {
        self.pending.lock().unwrap().remove(call_id);
    }

    pub fn add_auto_approve(&self, category: &str) {
        self.auto_approved.lock().unwrap().insert(category.to_string());
    }
}

impl Default for ToolApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}
