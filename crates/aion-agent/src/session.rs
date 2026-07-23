use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use aion_types::message::{ContentBlock, Message, Role, TokenUsage};

use crate::context_usage::ContextState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub cwd: String,
    pub total_usage: TokenUsage,
    #[serde(default)]
    pub context_state: ContextState,
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
        let id = if let Some(custom_id) = session_id {
            custom_id.to_string()
        } else {
            self.generate_unique_id()?
        };
        let session = Session {
            id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            provider: provider.to_string(),
            model: model.to_string(),
            cwd: cwd.to_string(),
            total_usage: TokenUsage::default(),
            context_state: ContextState::default(),
            messages: Vec::new(),
        };
        self.with_session_lock(&session.id, || {
            if self.session_exists(&session.id)? {
                anyhow::bail!("Session ID '{}' already exists", session.id);
            }
            self.save_unlocked(&session)
        })?;
        self.cleanup_old()?;
        Ok(session)
    }

    /// Save current session state (called after each turn)
    pub fn save(&self, session: &Session) -> anyhow::Result<()> {
        self.with_session_lock(&session.id, || self.save_unlocked(session))
    }

    /// Load a session by ID (or "latest")
    pub fn load(&self, id_or_latest: &str) -> anyhow::Result<Session> {
        if id_or_latest == "latest" {
            let sessions = self.list()?;
            let latest = sessions.last().ok_or_else(|| anyhow::anyhow!("No sessions found"))?;
            return self.load(&latest.id);
        }

        self.with_session_lock(id_or_latest, || {
            if let Some(session) = self.load_current(id_or_latest)? {
                return Ok(session);
            }

            if let Some(session) = self.load_legacy(id_or_latest)? {
                self.save_unlocked(&session)?;
                return Ok(session);
            }

            Err(anyhow::anyhow!("Session '{}' not found", id_or_latest))
        })
    }

    /// List all sessions
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let mut merged = HashMap::new();

        for meta in self.list_legacy()? {
            merged.insert(meta.id.clone(), meta);
        }
        for meta in self.list_current()? {
            merged.insert(meta.id.clone(), meta);
        }

        let mut sessions: Vec<_> = merged.into_values().collect();
        sessions.sort_by_key(|s| s.created_at);
        Ok(sessions)
    }

    /// Update the session index (public, called from engine after save).
    ///
    /// The new file layout does not maintain a global index as source of truth.
    /// Keep this method as a compatibility shim for existing callers/tests.
    pub fn update_index_for(&self, session: &Session) -> anyhow::Result<()> {
        self.save(session)
    }

    fn load_current(&self, session_id: &str) -> anyhow::Result<Option<Session>> {
        let path = self.state_path(session_id);
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(serde_json::from_str(&content)?)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn save_unlocked(&self, session: &Session) -> anyhow::Result<()> {
        let session_dir = self.session_dir(&session.id);
        fs::create_dir_all(&session_dir)?;
        let json = serde_json::to_string_pretty(session)?;
        write_atomic(&self.state_path(&session.id), json.as_bytes())
    }

    fn list_current(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let sessions_dir = self.sessions_dir();
        let entries = match fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };

        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let state_path = entry.path().join("state.json");
            if let Some(session) = read_session_file_if_valid(&state_path)? {
                sessions.push(meta_from_session(&session));
            }
        }

        Ok(sessions)
    }

    fn load_legacy(&self, session_id: &str) -> anyhow::Result<Option<Session>> {
        let Some(path) = self.find_legacy_session_file(session_id)? else {
            return Ok(None);
        };

        let content = fs::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    fn list_legacy(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let mut sessions = HashMap::new();
        let mut indexed_sessions = HashMap::new();

        if let Some(index) = self.load_legacy_index()? {
            for meta in index.sessions {
                indexed_sessions.insert(meta.id.clone(), meta);
            }
        }

        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };

        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.file_name() == "index.json" {
                continue;
            }
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            if let Some(session) = read_session_file_if_valid(&entry.path())? {
                let meta = indexed_sessions
                    .remove(&session.id)
                    .unwrap_or_else(|| meta_from_session(&session));
                sessions.insert(session.id.clone(), meta);
            }
        }

        Ok(sessions.into_values().collect())
    }

    fn load_legacy_index(&self) -> anyhow::Result<Option<SessionIndex>> {
        let index_path = self.directory.join("index.json");
        match fs::read_to_string(&index_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(index) => Ok(Some(index)),
                Err(error) => {
                    tracing::warn!(
                        target: "aion_agent",
                        path = %index_path.display(),
                        error = %error,
                        "skipping invalid legacy session index"
                    );
                    Ok(None)
                }
            },
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn find_legacy_session_file(&self, session_id: &str) -> anyhow::Result<Option<PathBuf>> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.file_name() == "index.json" {
                continue;
            }
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let Some(session) = read_session_file_if_valid(&entry.path())? else {
                continue;
            };
            if session.id == session_id {
                return Ok(Some(entry.path()));
            }
        }

        Ok(None)
    }

    fn session_exists(&self, session_id: &str) -> anyhow::Result<bool> {
        if self.state_path(session_id).is_file() {
            return Ok(true);
        }
        Ok(self.find_legacy_session_file(session_id)?.is_some())
    }

    fn generate_unique_id(&self) -> anyhow::Result<String> {
        for _ in 0..10 {
            let id = generate_session_id();
            if !self.session_exists(&id)? {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to generate unique session id")
    }

    fn sessions_dir(&self) -> PathBuf {
        self.directory.join("sessions")
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(encode_session_id(session_id))
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.json")
    }

    fn with_session_lock<T>(&self, session_id: &str, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let lock = session_lock(self.session_dir(session_id));
        let _guard = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("session lock poisoned for '{}'", session_id))?;
        f()
    }

    /// Remove oldest sessions beyond max_sessions.
    fn cleanup_old(&self) -> anyhow::Result<()> {
        let mut sessions = self.list_current()?;
        if sessions.len() <= self.max_sessions {
            return Ok(());
        }

        sessions.sort_by_key(|s| s.created_at);
        let to_remove = sessions.len() - self.max_sessions;
        for meta in sessions.into_iter().take(to_remove) {
            let lock = session_lock(self.session_dir(&meta.id));
            let _guard = lock
                .lock()
                .map_err(|_| anyhow::anyhow!("session lock poisoned for '{}'", meta.id))?;
            match fs::remove_dir_all(self.session_dir(&meta.id)) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }

        Ok(())
    }
}

fn meta_from_session(session: &Session) -> SessionMeta {
    let summary = session
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .and_then(|m| {
            m.content.iter().find_map(|c| {
                if let ContentBlock::Text { text } = c {
                    Some(truncate_str(text, 80))
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    SessionMeta {
        id: session.id.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        model: session.model.clone(),
        summary,
        message_count: session.messages.len(),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, bytes)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn read_session_file_if_valid(path: &Path) -> anyhow::Result<Option<Session>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };

    match serde_json::from_str(&content) {
        Ok(session) => Ok(Some(session)),
        Err(error) => {
            tracing::warn!(
                target: "aion_agent",
                path = %path.display(),
                error = %error,
                "skipping invalid session file"
            );
            Ok(None)
        }
    }
}

fn session_locks() -> &'static Mutex<HashMap<PathBuf, Weak<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_lock(path: PathBuf) -> Arc<Mutex<()>> {
    let mut locks = session_locks().lock().expect("session lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&path).and_then(Weak::upgrade) {
        return lock;
    }

    let lock = Arc::new(Mutex::new(()));
    locks.insert(path, Arc::downgrade(&lock));
    lock
}

#[cfg(test)]
fn session_lock_registry_contains(path: &Path) -> bool {
    session_locks()
        .lock()
        .expect("session lock registry poisoned")
        .contains_key(path)
}

fn encode_session_id(session_id: &str) -> String {
    let mut encoded = String::with_capacity(session_id.len());
    for byte in session_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            write!(&mut encoded, "{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

fn generate_session_id() -> String {
    Uuid::now_v7().to_string()
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
