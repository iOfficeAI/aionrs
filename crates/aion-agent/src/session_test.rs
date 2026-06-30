use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use aion_types::message::{ContentBlock, Message, Role};
    use tempfile::tempdir;

    #[test]
    fn test_create_session() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let result = manager.create("openai", "gpt-4", "/tmp", None);
        assert!(result.is_ok());

        let session = result.unwrap();
        assert_eq!(session.provider, "openai");
        assert_eq!(session.model, "gpt-4");
        assert_eq!(session.cwd, "/tmp");
        assert!(session.messages.is_empty());
    }

    #[test]
    fn test_save_and_load_session() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let session = manager.create("anthropic", "claude-3", "/home", None).unwrap();
        let loaded = manager.load(&session.id).unwrap();

        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.model, "claude-3");
        assert_eq!(loaded.cwd, "/home");
    }

    #[test]
    fn test_load_nonexistent_returns_error() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let result = manager.load("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_sessions_empty() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let sessions = manager.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_sorted_by_time() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let s1 = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        let s2 = manager.create("anthropic", "claude-3", "/home", None).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 2);

        let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&s1.id.as_str()));
        assert!(ids.contains(&s2.id.as_str()));
    }

    #[test]
    fn test_update_index() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();

        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        );
        session.messages.push(msg);

        manager.update_index_for(&session).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].summary, "hello");
        assert_eq!(list[0].message_count, 1);
    }

    #[test]
    fn test_cleanup_old_sessions() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 2);

        let _s1 = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        let _s2 = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        let _s3 = manager.create("openai", "gpt-4", "/tmp", None).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_session_id_uniqueness() {
        let id1 = generate_short_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let id2 = generate_short_id();
        assert_ne!(id1, id2);
    }
}
