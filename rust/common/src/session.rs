use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionEntry {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,

    #[serde(rename = "sessionFile", default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,

    #[serde(rename = "spawnedBy", default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<String>,

    #[serde(rename = "spawnDepth", default, skip_serializing_if = "Option::is_none")]
    pub spawn_depth: Option<u32>,

    #[serde(rename = "systemSent", default, skip_serializing_if = "Option::is_none")]
    pub system_sent: Option<bool>,

    #[serde(rename = "chatType", default, skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<String>,

    #[serde(rename = "providerOverride", default, skip_serializing_if = "Option::is_none")]
    pub provider_override: Option<String>,
    
    #[serde(rename = "modelOverride", default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    
    #[serde(rename = "displayName", default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    #[serde(rename = "groupId", default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,

    #[serde(rename = "groupChannel", default, skip_serializing_if = "Option::is_none")]
    pub group_channel: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SessionOrigin>,

    #[serde(rename = "deliveryContext", default, skip_serializing_if = "Option::is_none")]
    pub delivery_context: Option<DeliveryContext>,
    
    #[serde(rename = "inputTokens", default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,

    #[serde(rename = "outputTokens", default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,

    #[serde(rename = "totalTokens", default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,

    // Catch-all for other fields
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionOrigin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,

    #[serde(rename = "chatType", default, skip_serializing_if = "Option::is_none")]
    pub chat_type: Option<String>, // SessionChatType

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    #[serde(rename = "accountId", default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,

    #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Value>, // string or number
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeliveryContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    #[serde(rename = "accountId", default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,

    #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<Value>, // string or number
}

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

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
                tracing::error!("Failed to load session store from {:?}: {}", path, e);
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
        tracing::info!("Loaded {} sessions from {:?}", sessions.len(), path);
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

    pub async fn get(&self, session_key: &str) -> Option<SessionEntry> {
        let sessions = self.sessions.read().await;
        sessions.get(session_key).cloned()
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
