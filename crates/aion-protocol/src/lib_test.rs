use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    // --- SessionMode: default mode ---

    #[test]
    fn default_mode_does_not_auto_approve_any_category() {
        let mgr = ToolApprovalManager::new();
        assert!(!mgr.is_auto_approved("info"));
        assert!(!mgr.is_auto_approved("edit"));
        assert!(!mgr.is_auto_approved("exec"));
        assert!(!mgr.is_auto_approved("mcp"));
    }

    #[test]
    fn default_mode_current_mode_string() {
        let mgr = ToolApprovalManager::new();
        assert_eq!(mgr.current_mode(), "default");
    }

    // --- SessionMode: auto_edit mode ---

    #[test]
    fn auto_edit_mode_approves_info_and_edit() {
        let mgr = ToolApprovalManager::new();
        mgr.set_mode(SessionMode::AutoEdit);
        assert!(mgr.is_auto_approved("info"));
        assert!(mgr.is_auto_approved("edit"));
    }

    #[test]
    fn auto_edit_mode_requires_approval_for_exec_and_mcp() {
        let mgr = ToolApprovalManager::new();
        mgr.set_mode(SessionMode::AutoEdit);
        assert!(!mgr.is_auto_approved("exec"));
        assert!(!mgr.is_auto_approved("mcp"));
    }

    #[test]
    fn auto_edit_mode_current_mode_string() {
        let mgr = ToolApprovalManager::new();
        mgr.set_mode(SessionMode::AutoEdit);
        assert_eq!(mgr.current_mode(), "auto_edit");
    }

    // --- SessionMode: yolo mode ---

    #[test]
    fn yolo_mode_approves_all_categories() {
        let mgr = ToolApprovalManager::new();
        mgr.set_mode(SessionMode::Yolo);
        assert!(mgr.is_auto_approved("info"));
        assert!(mgr.is_auto_approved("edit"));
        assert!(mgr.is_auto_approved("exec"));
        assert!(mgr.is_auto_approved("mcp"));
    }

    #[test]
    fn yolo_mode_current_mode_string() {
        let mgr = ToolApprovalManager::new();
        mgr.set_mode(SessionMode::Yolo);
        assert_eq!(mgr.current_mode(), "yolo");
    }

    // --- Mode switching ---

    #[test]
    fn switching_mode_changes_approval_behavior() {
        let mgr = ToolApprovalManager::new();

        // Start in default
        assert!(!mgr.is_auto_approved("edit"));

        // Switch to auto_edit
        mgr.set_mode(SessionMode::AutoEdit);
        assert!(mgr.is_auto_approved("edit"));
        assert!(!mgr.is_auto_approved("exec"));

        // Switch to yolo
        mgr.set_mode(SessionMode::Yolo);
        assert!(mgr.is_auto_approved("exec"));

        // Switch back to default
        mgr.set_mode(SessionMode::Default);
        assert!(!mgr.is_auto_approved("edit"));
        assert!(!mgr.is_auto_approved("exec"));
    }

    // --- Mode + user "always" approval coexistence ---

    #[test]
    fn user_always_approval_persists_across_mode_changes() {
        let mgr = ToolApprovalManager::new();

        // User manually approves "exec" category with "always"
        mgr.add_auto_approve("exec");
        assert!(mgr.is_auto_approved("exec"));

        // Switch to auto_edit: exec still approved via user "always"
        mgr.set_mode(SessionMode::AutoEdit);
        assert!(mgr.is_auto_approved("exec"));
        assert!(mgr.is_auto_approved("info")); // from mode

        // Switch back to default: exec still approved via user "always"
        mgr.set_mode(SessionMode::Default);
        assert!(mgr.is_auto_approved("exec"));
        assert!(!mgr.is_auto_approved("info")); // mode no longer provides this
    }
}
