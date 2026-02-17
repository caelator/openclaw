use common::session::SessionEntry;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

#[derive(Clone)]
pub struct SessionStore {
    path: PathBuf,
    sessions: Arc<RwLock<HashMap<String, SessionEntry>>>,
}

impl SessionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let sessions = if path.exists() {
            Self::load_from_file(&path).unwrap_or_else(|e| {
                error!("Failed to load session store from {:?}: {}", path, e);
                HashMap::new()
            })
        } else {
            HashMap::new()
        };

        Self {
            path,
            sessions: Arc::new(RwLock::new(sessions)),
        }
    }

    fn load_from_file(path: &Path) -> Result<HashMap<String, SessionEntry>, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let sessions: HashMap<String, SessionEntry> = serde_json::from_str(&content)?;
        info!("Loaded {} sessions from {:?}", sessions.len(), path);
        Ok(sessions)
    }

    pub async fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sessions = self.sessions.read().await;
        // serialize to string first
        let content = serde_json::to_string_pretty(&*sessions)?;
        
        // Write to temp file then rename
        // TODO: Use a proper tempfile crate or similar logic to original TS for robustness
        let tmp_path = self.path.with_extension("tmp");
        fs::write(&tmp_path, content)?;
        fs::rename(&tmp_path, &self.path)?;
        
        Ok(())
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionEntry> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    pub async fn update(&self, key: String, session: SessionEntry) -> Result<(), Box<dyn std::error::Error>> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(key, session);
        drop(sessions); // Release lock before saving
        self.save().await
    }
    
    pub async fn list(&self) -> Vec<SessionEntry> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }
}
