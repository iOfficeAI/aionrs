use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aion_types::message::{Message, TokenUsage};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub cwd: String,
    pub total_usage: TokenUsage,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndex {
    pub sessions: Vec<SessionMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    /// First user message, truncated to 80 chars
    pub summary: String,
    pub message_count: usize,
}

pub struct SessionManager {
    directory: PathBuf,
    max_sessions: usize,
}

impl SessionManager {
    pub fn new(directory: PathBuf, max_sessions: usize) -> Self {
        Self {
            directory,
            max_sessions,
        }
    }

    /// Create a new session, return it
    pub fn create(&self, provider: &str, model: &str, cwd: &str, session_id: Option<&str>) -> anyhow::Result<Session> {
        std::fs::create_dir_all(&self.directory)?;

        let id = if let Some(custom_id) = session_id {
            // Validate that the ID doesn't already exist
            let index = self.load_index()?;
            if index.sessions.iter().any(|s| s.id == custom_id) {
                anyhow::bail!("Session ID '{}' already exists", custom_id);
            }
            custom_id.to_string()
        } else {
            generate_short_id()
        };
        let session = Session {
            id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            provider: provider.to_string(),
            model: model.to_string(),
            cwd: cwd.to_string(),
            total_usage: TokenUsage::default(),
            messages: Vec::new(),
        };
        self.save(&session)?;
        self.update_index(&session)?;
        self.cleanup_old()?;
        Ok(session)
    }

    /// Save current session state (called after each turn)
    pub fn save(&self, session: &Session) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let filename = format!("{}_{}.json", session.created_at.format("%Y-%m-%d"), session.id);
        let path = self.directory.join(&filename);
        let json = serde_json::to_string_pretty(session)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a session by ID (or "latest")
    pub fn load(&self, id_or_latest: &str) -> anyhow::Result<Session> {
        let index = self.load_index()?;

        let meta = if id_or_latest == "latest" {
            index
                .sessions
                .last()
                .ok_or_else(|| anyhow::anyhow!("No sessions found"))?
        } else {
            index
                .sessions
                .iter()
                .find(|s| s.id == id_or_latest)
                .ok_or_else(|| anyhow::anyhow!("Session '{}' not found", id_or_latest))?
        };

        let pattern = format!("*_{}.json", meta.id);
        let session_files: Vec<_> = glob::glob(self.directory.join(&pattern).to_string_lossy().as_ref())?
            .filter_map(|r| r.ok())
            .collect();

        let path = session_files
            .first()
            .ok_or_else(|| anyhow::anyhow!("Session file not found for '{}'", meta.id))?;

        let content = std::fs::read_to_string(path)?;
        let session: Session = serde_json::from_str(&content)?;
        Ok(session)
    }

    /// List all sessions
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let index = self.load_index()?;
        Ok(index.sessions)
    }

    fn load_index(&self) -> anyhow::Result<SessionIndex> {
        let index_path = self.directory.join("index.json");
        match std::fs::read_to_string(&index_path) {
            Ok(content) => Ok(serde_json::from_str(&content)?),
            Err(_) => Ok(SessionIndex { sessions: Vec::new() }),
        }
    }

    /// Update the session index (public, called from engine after save)
    pub fn update_index_for(&self, session: &Session) -> anyhow::Result<()> {
        self.update_index(session)
    }

    fn update_index(&self, session: &Session) -> anyhow::Result<()> {
        let mut index = self.load_index()?;

        // Extract summary from first user message
        let summary = session
            .messages
            .iter()
            .find(|m| m.role == aion_types::message::Role::User)
            .and_then(|m| {
                m.content.iter().find_map(|c| {
                    if let aion_types::message::ContentBlock::Text { text } = c {
                        Some(truncate_str(text, 80))
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let meta = SessionMeta {
            id: session.id.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            model: session.model.clone(),
            summary,
            message_count: session.messages.len(),
        };

        // Update existing or add new
        if let Some(existing) = index.sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = meta;
        } else {
            index.sessions.push(meta);
        }

        let index_path = self.directory.join("index.json");
        let json = serde_json::to_string_pretty(&index)?;
        std::fs::write(index_path, json)?;
        Ok(())
    }

    /// Remove oldest sessions beyond max_sessions
    fn cleanup_old(&self) -> anyhow::Result<()> {
        let mut index = self.load_index()?;
        if index.sessions.len() <= self.max_sessions {
            return Ok(());
        }

        // Sort by created_at, remove oldest
        index.sessions.sort_by_key(|s| s.created_at);
        let to_remove = index.sessions.len() - self.max_sessions;
        let removed: Vec<_> = index.sessions.drain(..to_remove).collect();

        // Delete session files
        for meta in &removed {
            let pattern = format!("*_{}.json", meta.id);
            if let Ok(paths) = glob::glob(self.directory.join(&pattern).to_string_lossy().as_ref()) {
                for path in paths.flatten() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        // Save updated index
        let index_path = self.directory.join("index.json");
        let json = serde_json::to_string_pretty(&index)?;
        std::fs::write(index_path, json)?;
        Ok(())
    }
}

fn generate_short_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    format!("{:06x}", nanos & 0xFFFFFF)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}

#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;
