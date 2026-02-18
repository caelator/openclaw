//! Daily API poller — probes all keys across all providers.
//!
//! Runs on startup and then every 24 hours. When a request
//! fails (429, 403, etc.), it gathers all available intelligence
//! about why, when it resets, and what metrics are exhausted.

use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{self, Duration};
use tracing::{error, info, warn};

use crate::adapters::{LLMAdapter, ProbeResult};
use crate::vault::store::{KeyStore, KeyStatus};

/// Run the discovery poller in the background.
pub async fn run_poller(
    store: Arc<KeyStore>,
    adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    interval_hours: u64,
) {
    // Run immediately on startup
    info!("📡 Discovery poller starting — initial scan...");
    if let Err(e) = run_full_scan(&store, &adapters).await {
        error!("Initial scan failed: {}", e);
    }

    // Then run every `interval_hours`
    let mut interval = time::interval(Duration::from_secs(interval_hours * 3600));
    interval.tick().await; // Skip the immediate tick (we already ran)

    loop {
        interval.tick().await;
        info!("📡 Running scheduled discovery scan...");
        if let Err(e) = run_full_scan(&store, &adapters).await {
            error!("Scheduled scan failed: {}", e);
        }
    }
}

/// Execute a full scan of all keys across all providers.
pub async fn run_full_scan(
    store: &KeyStore,
    adapters: &HashMap<String, Box<dyn LLMAdapter>>,
) -> Result<()> {
    let keys = store.list_keys()?;
    let scan_start = Utc::now();

    info!(
        "Scanning {} keys across {} providers",
        keys.len(),
        adapters.len()
    );

    let mut results: Vec<ProbeResult> = Vec::new();

    for key_entry in &keys {
        let adapter = match adapters.get(&key_entry.provider) {
            Some(a) => a,
            None => {
                warn!(
                    key_id = %key_entry.id,
                    provider = %key_entry.provider,
                    "No adapter registered for provider — skipping"
                );
                continue;
            }
        };

        info!(
            key_id = %key_entry.id,
            provider = %key_entry.provider,
            role = ?key_entry.role,
            "Probing key..."
        );

        let probe_start = std::time::Instant::now();
        let raw_key = match store.decrypt_key(&key_entry.id) {
            Ok(k) => k,
            Err(e) => {
                error!(key_id = %key_entry.id, "Failed to decrypt key: {}", e);
                continue;
            }
        };

        // 1. Discover models
        let models = match adapter.list_models(&raw_key).await {
            Ok(m) => m,
            Err(e) => {
                warn!(key_id = %key_entry.id, "list_models failed: {}", e);
                vec![]
            }
        };

        // Update model catalog
        if !models.is_empty() {
            if let Err(e) = store.update_model_catalog(&models) {
                error!("Failed to update model catalog: {}", e);
            }
        }

        // 2. Check key health
        let health = match adapter.check_health(&raw_key).await {
            Ok(h) => h,
            Err(e) => {
                warn!(key_id = %key_entry.id, "check_health failed: {}", e);
                crate::adapters::KeyHealth {
                    valid: false,
                    tier: crate::adapters::KeyTier::Unknown,
                    quota_remaining_pct: None,
                    reset_at: None,
                    error: Some(crate::adapters::ProbeError {
                        http_status: 0,
                        error_type: "probe_error".into(),
                        error_message: e.to_string(),
                        quota_metric: None,
                        suggested_action: None,
                        reset_time: None,
                    }),
                }
            }
        };

        let latency = probe_start.elapsed().as_millis() as u64;

        // 3. Record probe result
        let (error_type, error_msg, reset_at_str) = match &health.error {
            Some(err) => (
                Some(err.error_type.as_str()),
                Some(err.error_message.as_str()),
                err.reset_time.map(|dt| dt.to_rfc3339()),
            ),
            None => (None, None, None),
        };

        store.record_probe(
            &key_entry.id,
            &key_entry.provider,
            None,
            health.valid,
            health.quota_remaining_pct.map(|p| p as u32),
            None, None,
            error_type,
            error_msg,
            reset_at_str.as_deref(),
            latency,
        )?;

        // 4. Update key status based on health
        if !health.valid {
            store.update_key_status(&key_entry.id, KeyStatus::Quarantined)?;
            warn!(key_id = %key_entry.id, "Key invalid — quarantined");
        } else if health.quota_remaining_pct == Some(0.0) {
            store.update_key_status(&key_entry.id, KeyStatus::RateLimited)?;
            if let Some(reset) = &health.reset_at {
                info!(
                    key_id = %key_entry.id,
                    reset_at = %reset,
                    "Key quota exhausted — rate-limited until reset"
                );
            } else {
                info!(key_id = %key_entry.id, "Key quota exhausted — rate-limited");
            }
        } else {
            // Re-activate previously rate-limited keys if quota has returned
            if key_entry.status == KeyStatus::RateLimited {
                store.update_key_status(&key_entry.id, KeyStatus::Active)?;
                info!(key_id = %key_entry.id, "Key quota restored — re-activated");
            }
        }

        // Determine new/deprecated models
        let new_models: Vec<String> = models.iter()
            .filter(|m| m.supports_generation && m.is_preview)
            .map(|m| m.id.clone())
            .collect();

        let deprecated_models: Vec<String> = models.iter()
            .filter(|m| m.is_deprecated)
            .map(|m| m.id.clone())
            .collect();

        results.push(ProbeResult {
            key_id: key_entry.id.clone(),
            provider: key_entry.provider.clone(),
            timestamp: Utc::now(),
            available_models: models,
            new_models,
            deprecated_models,
            key_health: health,
            rate_limits: None,
            latency_ms: latency,
        });

        // Drop the decrypted key (it's on the stack but let's be explicit)
        drop(raw_key);
    }

    let scan_duration = (Utc::now() - scan_start).num_seconds();

    let healthy = results.iter().filter(|r| r.key_health.valid).count();
    let exhausted = results.iter()
        .filter(|r| r.key_health.quota_remaining_pct == Some(0.0))
        .count();
    let invalid = results.iter().filter(|r| !r.key_health.valid).count();

    info!(
        total = results.len(),
        healthy = healthy,
        exhausted = exhausted,
        invalid = invalid,
        duration_secs = scan_duration,
        "📡 Discovery scan complete"
    );

    Ok(())
}
