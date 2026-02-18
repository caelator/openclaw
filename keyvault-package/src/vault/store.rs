//! Key store — encrypted CRUD for API keys, backed by SQLite.
//!
//! Keys are encrypted before hitting disk. Decryption happens
//! in-memory only, and the plaintext is zeroized after use.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use zeroize::Zeroize;

use super::{decrypt, encrypt};

/// A key entry in the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    pub id: String,
    pub provider: String,
    pub encrypted_value: Vec<u8>,
    pub role: KeyRole,
    pub status: KeyStatus,
    pub added_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub last_health_check: Option<DateTime<Utc>>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyRole {
    Orchestrator,
    Worker,
    Spare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    RateLimited,
    Quarantined,
    Disabled,
}
/// Rolling 24hr usage aggregates for a single key.
#[derive(Debug, Clone)]
pub struct KeyUsage24h {
    pub total_requests: u64,
    pub successes: u64,
    pub failures: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// The key store backed by SQLite.
pub struct KeyStore {
    db: Mutex<Connection>,
    master_passphrase: Vec<u8>,
}

impl KeyStore {
    /// Open (or create) the key store at the given path.
    pub fn open(db_path: &Path, master_passphrase: Vec<u8>) -> Result<Self> {
        let db = Connection::open(db_path)
            .context("Failed to open keyvault database")?;

        // WAL mode for concurrent reads
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;

        // Create tables
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS keys (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                encrypted_value BLOB NOT NULL,
                role TEXT NOT NULL DEFAULT 'worker',
                status TEXT NOT NULL DEFAULT 'active',
                added_at TEXT NOT NULL,
                last_used_at TEXT,
                last_health_check TEXT,
                notes TEXT
            );

            CREATE TABLE IF NOT EXISTS probe_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT,
                probed_at TEXT NOT NULL,
                key_valid BOOLEAN,
                rpm_remaining INTEGER,
                rpd_remaining INTEGER,
                tpm_remaining INTEGER,
                error_type TEXT,
                error_message TEXT,
                reset_at TEXT,
                latency_ms INTEGER,
                FOREIGN KEY (key_id) REFERENCES keys(id)
            );

            CREATE TABLE IF NOT EXISTS usage_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT NOT NULL,
                key_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                caller TEXT,
                budget_tag TEXT,
                timestamp TEXT NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cost_usd REAL,
                latency_ms INTEGER,
                status TEXT NOT NULL,
                error_message TEXT
            );

            CREATE TABLE IF NOT EXISTS model_catalog (
                id TEXT NOT NULL,
                provider TEXT NOT NULL,
                display_name TEXT,
                input_token_limit INTEGER,
                output_token_limit INTEGER,
                supports_generation BOOLEAN,
                supports_embedding BOOLEAN,
                is_preview BOOLEAN,
                is_deprecated BOOLEAN,
                last_seen TEXT NOT NULL,
                first_seen TEXT NOT NULL,
                PRIMARY KEY (provider, id)
            );

            CREATE TABLE IF NOT EXISTS provider_daily_metrics (
                provider TEXT,
                model TEXT,
                date TEXT,
                avg_latency_ms REAL,
                p99_latency_ms REAL,
                error_rate REAL,
                total_requests INTEGER,
                total_tokens_in INTEGER,
                total_tokens_out INTEGER,
                total_cost_usd REAL,
                quota_exhaustion_count INTEGER,
                PRIMARY KEY (provider, model, date)
            );

            CREATE INDEX IF NOT EXISTS idx_probe_history_key ON probe_history(key_id, probed_at);
            CREATE INDEX IF NOT EXISTS idx_usage_log_time ON usage_log(timestamp);
            CREATE INDEX IF NOT EXISTS idx_usage_log_key ON usage_log(key_id);
            ",
        )?;

        Ok(Self {
            db: Mutex::new(db),
            master_passphrase,
        })
    }

    /// Add a new key to the vault.
    pub fn add_key(
        &self,
        id: &str,
        provider: &str,
        raw_value: &str,
        role: KeyRole,
        notes: Option<&str>,
    ) -> Result<()> {
        let encrypted = encrypt(raw_value.as_bytes(), &self.master_passphrase);
        let now = Utc::now().to_rfc3339();
        let role_str = match role {
            KeyRole::Orchestrator => "orchestrator",
            KeyRole::Worker => "worker",
            KeyRole::Spare => "spare",
        };

        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT OR REPLACE INTO keys (id, provider, encrypted_value, role, status, added_at, notes)
             VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6)",
            params![id, provider, encrypted, role_str, now, notes],
        )?;

        tracing::info!(key_id = id, provider = provider, role = role_str, "Key added to vault");
        Ok(())
    }

    /// Remove a key from the vault.
    pub fn remove_key(&self, id: &str) -> Result<bool> {
        let db = self.db.lock().unwrap();
        let rows = db.execute("DELETE FROM keys WHERE id = ?1", params![id])?;
        if rows > 0 {
            tracing::info!(key_id = id, "Key removed from vault");
        }
        Ok(rows > 0)
    }

    /// Decrypt and return a key's raw value. The caller MUST zeroize this.
    pub fn decrypt_key(&self, id: &str) -> Result<String> {
        let db = self.db.lock().unwrap();
        let encrypted: Vec<u8> = db.query_row(
            "SELECT encrypted_value FROM keys WHERE id = ?1 AND status = 'active'",
            params![id],
            |row| row.get(0),
        ).context(format!("Key '{}' not found or not active", id))?;

        let mut plaintext = decrypt(&encrypted, &self.master_passphrase)?;
        let result = String::from_utf8(plaintext.clone())
            .context("Key is not valid UTF-8")?;
        plaintext.zeroize();
        Ok(result)
    }

    /// List all keys (without decrypting values).
    pub fn list_keys(&self) -> Result<Vec<KeyEntry>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id, provider, encrypted_value, role, status, added_at, last_used_at, last_health_check, notes FROM keys"
        )?;

        let entries = stmt.query_map([], |row| {
            let role_str: String = row.get(3)?;
            let status_str: String = row.get(4)?;
            Ok(KeyEntry {
                id: row.get(0)?,
                provider: row.get(1)?,
                encrypted_value: row.get(2)?,
                role: match role_str.as_str() {
                    "orchestrator" => KeyRole::Orchestrator,
                    "spare" => KeyRole::Spare,
                    _ => KeyRole::Worker,
                },
                status: match status_str.as_str() {
                    "rate_limited" => KeyStatus::RateLimited,
                    "quarantined" => KeyStatus::Quarantined,
                    "disabled" => KeyStatus::Disabled,
                    _ => KeyStatus::Active,
                },
                added_at: row.get::<_, String>(5)
                    .map(|s| DateTime::parse_from_rfc3339(&s).unwrap_or_default().with_timezone(&Utc))
                    .unwrap_or_default(),
                last_used_at: row.get::<_, Option<String>>(6)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                last_health_check: row.get::<_, Option<String>>(7)?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|dt| dt.with_timezone(&Utc)),
                notes: row.get(8)?,
            })
        })?.collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get active worker keys for a provider.
    pub fn get_active_keys(&self, provider: &str) -> Result<Vec<String>> {
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "SELECT id FROM keys WHERE provider = ?1 AND status = 'active' AND role != 'orchestrator'"
        )?;
        let ids = stmt.query_map(params![provider], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        Ok(ids)
    }

    /// Get the orchestrator key for a provider.
    pub fn get_orchestrator_key(&self, provider: &str) -> Result<Option<String>> {
        let db = self.db.lock().unwrap();
        let result = db.query_row(
            "SELECT id FROM keys WHERE provider = ?1 AND role = 'orchestrator' AND status = 'active'",
            params![provider],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update key status (e.g., mark as rate-limited or quarantined).
    pub fn update_key_status(&self, id: &str, status: KeyStatus) -> Result<()> {
        let status_str = match status {
            KeyStatus::Active => "active",
            KeyStatus::RateLimited => "rate_limited",
            KeyStatus::Quarantined => "quarantined",
            KeyStatus::Disabled => "disabled",
        };
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE keys SET status = ?1 WHERE id = ?2",
            params![status_str, id],
        )?;
        Ok(())
    }

    /// Mark a key as recently used.
    pub fn touch_key(&self, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.db.lock().unwrap();
        db.execute(
            "UPDATE keys SET last_used_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Record a usage event.
    pub fn record_usage(
        &self,
        request_id: &str,
        key_id: &str,
        provider: &str,
        model: &str,
        caller: Option<&str>,
        budget_tag: Option<&str>,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        latency_ms: u64,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO usage_log (request_id, key_id, provider, model, caller, budget_tag, timestamp, input_tokens, output_tokens, cost_usd, latency_ms, status, error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![request_id, key_id, provider, model, caller, budget_tag, now,
                    input_tokens as i64, output_tokens as i64, cost_usd, latency_ms as i64,
                    status, error_message],
        )?;
        Ok(())
    }

    /// Record a probe result.
    pub fn record_probe(
        &self,
        key_id: &str,
        provider: &str,
        model: Option<&str>,
        key_valid: bool,
        rpm_remaining: Option<u32>,
        rpd_remaining: Option<u32>,
        tpm_remaining: Option<u64>,
        error_type: Option<&str>,
        error_message: Option<&str>,
        reset_at: Option<&str>,
        latency_ms: u64,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO probe_history (key_id, provider, model, probed_at, key_valid, rpm_remaining, rpd_remaining, tpm_remaining, error_type, error_message, reset_at, latency_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![key_id, provider, model, now, key_valid,
                    rpm_remaining.map(|v| v as i32), rpd_remaining.map(|v| v as i32),
                    tpm_remaining.map(|v| v as i64),
                    error_type, error_message, reset_at, latency_ms as i64],
        )?;
        // Update health check timestamp
        let db2 = &db;
        db2.execute("UPDATE keys SET last_health_check = ?1 WHERE id = ?2", params![now, key_id])?;
        Ok(())
    }

    /// Update the model catalog with discovered models.
    pub fn update_model_catalog(
        &self,
        models: &[crate::adapters::ModelInfo],
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare(
            "INSERT INTO model_catalog (id, provider, display_name, input_token_limit, output_token_limit, supports_generation, supports_embedding, is_preview, is_deprecated, last_seen, first_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(provider, id) DO UPDATE SET
                display_name = excluded.display_name,
                input_token_limit = excluded.input_token_limit,
                output_token_limit = excluded.output_token_limit,
                supports_generation = excluded.supports_generation,
                supports_embedding = excluded.supports_embedding,
                is_preview = excluded.is_preview,
                is_deprecated = excluded.is_deprecated,
                last_seen = excluded.last_seen"
        )?;

        for m in models {
            stmt.execute(params![
                m.id, m.provider, m.display_name,
                m.input_token_limit as i64, m.output_token_limit as i64,
                m.supports_generation, m.supports_embedding,
                m.is_preview, m.is_deprecated, now
            ])?;
        }
        Ok(())
    }

    /// Get the database connection for external queries (analytics).
    pub fn db(&self) -> &Mutex<Connection> {
        &self.db
    }

    /// Alias for `list_keys` — returns all keys regardless of status.
    pub fn list_all_keys(&self) -> Result<Vec<KeyEntry>> {
        self.list_keys()
    }

    /// Query rolling 24hr usage aggregates per key.
    pub fn usage_last_24h(&self) -> Result<std::collections::HashMap<String, KeyUsage24h>> {
        let db = self.db.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();

        let mut stmt = db.prepare(
            "SELECT key_id,
                    COUNT(*) as total_requests,
                    SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) as successes,
                    SUM(CASE WHEN status != 'success' THEN 1 ELSE 0 END) as failures,
                    COALESCE(SUM(input_tokens), 0) as input_tokens,
                    COALESCE(SUM(output_tokens), 0) as output_tokens,
                    COALESCE(SUM(cost_usd), 0.0) as cost_usd
             FROM usage_log
             WHERE timestamp >= ?1
             GROUP BY key_id"
        )?;

        let mut results = std::collections::HashMap::new();
        let rows = stmt.query_map(params![cutoff], |row| {
            Ok((
                row.get::<_, String>(0)?,
                KeyUsage24h {
                    total_requests: row.get::<_, i64>(1)? as u64,
                    successes: row.get::<_, i64>(2)? as u64,
                    failures: row.get::<_, i64>(3)? as u64,
                    input_tokens: row.get::<_, i64>(4)? as u64,
                    output_tokens: row.get::<_, i64>(5)? as u64,
                    cost_usd: row.get(6)?,
                },
            ))
        })?;

        for row in rows {
            let (key_id, usage) = row?;
            results.insert(key_id, usage);
        }

        Ok(results)
    }
}

impl Drop for KeyStore {
    fn drop(&mut self) {
        // Zeroize master passphrase on drop
        self.master_passphrase.zeroize();
    }
}
