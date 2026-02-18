//! Pool manager — selects the best key for each request.
//!
//! Implements round-robin with health awareness: skips
//! rate-limited and quarantined keys, tracks last-used time,
//! and supports parallel fan-out and swarm scheduling.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::adapters::{GenerateRequest, GenerateResponse, LLMAdapter};
use crate::vault::store::{KeyStore, KeyStatus};

pub mod registry;
pub mod classifier;
pub mod swarm;
pub mod rate_tracker;

pub struct PoolManager {
    store: Arc<KeyStore>,
    adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    /// Round-robin counters per provider.
    counters: HashMap<String, AtomicUsize>,
    /// Live rate tracker for pre-flight RPM/RPD checks.
    rate_tracker: Arc<rate_tracker::RateTracker>,
}

impl PoolManager {
    pub fn new(
        store: Arc<KeyStore>,
        adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    ) -> Self {
        let keys = store.list_keys().unwrap_or_default();
        let mut counters = HashMap::new();
        for k in &keys {
            counters
                .entry(k.provider.clone())
                .or_insert_with(|| AtomicUsize::new(0));
        }

        Self {
            store,
            adapters,
            counters,
            rate_tracker: Arc::new(rate_tracker::RateTracker::new()),
        }
    }

    /// Select the best key for a provider and execute a generation request.
    pub async fn generate(
        &self,
        provider: &str,
        req: &GenerateRequest,
        caller: Option<&str>,
        budget_tag: Option<&str>,
    ) -> Result<GenerateResponse> {
        let adapter = self.adapters.get(provider)
            .context(format!("No adapter for provider '{}'", provider))?;

        let active_keys = self.store.get_active_keys(provider)?;
        if active_keys.is_empty() {
            anyhow::bail!("No active keys available for provider '{}'", provider);
        }

        // Round-robin selection
        let counter = self.counters.get(provider)
            .context("No counter for provider")?;
        let total = active_keys.len();

        // Try all keys before giving up
        for attempt in 0..total {
            let idx = counter.fetch_add(1, Ordering::Relaxed) % total;
            let key_id = &active_keys[idx];

            let raw_key = match self.store.decrypt_key(key_id) {
                Ok(k) => k,
                Err(e) => {
                    tracing::warn!(key_id = %key_id, "Failed to decrypt: {}", e);
                    continue;
                }
            };

            // Record in rate tracker (pre-flight) so health pulse reflects all traffic
            self.rate_tracker.record_request(key_id, &req.model);

            match adapter.generate(req, &raw_key).await {
                Ok(mut resp) => {
                    resp.key_id = key_id.clone();
                    self.store.touch_key(key_id)?;

                    // Record usage
                    let cost = adapter.estimate_cost(&req.model, resp.input_tokens, resp.output_tokens);
                    let request_id = uuid::Uuid::new_v4().to_string();
                    self.store.record_usage(
                        &request_id, key_id, provider, &req.model,
                        caller, budget_tag,
                        resp.input_tokens, resp.output_tokens,
                        cost.total_cost_usd, resp.latency_ms,
                        "success", None,
                    )?;

                    return Ok(resp);
                }
                Err(e) => {
                    let err_str = e.to_string();
                    tracing::warn!(
                        key_id = %key_id,
                        attempt = attempt + 1,
                        "Generate failed: {}",
                        &err_str[..err_str.len().min(200)]
                    );

                    // If it's a rate limit error, mark the key
                    if err_str.contains("429") || err_str.contains("RESOURCE_EXHAUSTED") || err_str.contains("rate") {
                        self.store.update_key_status(key_id, KeyStatus::RateLimited)?;
                        tracing::info!(key_id = %key_id, "Marked as rate-limited, trying next key");
                    }

                    // Record the failure
                    let request_id = uuid::Uuid::new_v4().to_string();
                    self.store.record_usage(
                        &request_id, key_id, provider, &req.model,
                        caller, budget_tag,
                        0, 0, 0.0, 0,
                        "error", Some(&err_str[..err_str.len().min(500)]),
                    )?;

                    continue;
                }
            }
        }

        anyhow::bail!(
            "All {} keys for provider '{}' exhausted",
            total,
            provider
        )
    }

    /// Fan out N requests across N different keys in parallel (single provider).
    ///
    /// Each request is assigned a different key via round-robin.
    /// Uses the provider's `LLMAdapter` trait rather than raw HTTP calls.
    pub async fn parallel_generate(
        &self,
        provider: &str,
        requests: Vec<GenerateRequest>,
        caller: Option<&str>,
    ) -> Vec<Result<GenerateResponse>> {
        let _adapter = match self.adapters.get(provider) {
            Some(a) => a,
            None => {
                return requests.iter()
                    .map(|_| Err(anyhow::anyhow!("No adapter for '{}'", provider)))
                    .collect();
            }
        };

        let active_keys = match self.store.get_active_keys(provider) {
            Ok(k) => k,
            Err(e) => {
                return requests.iter()
                    .map(|_| Err(anyhow::anyhow!("Failed to get keys: {}", e)))
                    .collect();
            }
        };

        if active_keys.is_empty() {
            return requests.iter()
                .map(|_| Err(anyhow::anyhow!("No active keys for '{}'", provider)))
                .collect();
        }

        let caller_owned = caller.map(|s| s.to_string());
        let provider_owned = provider.to_string();
        let store = Arc::clone(&self.store);
        let adapters = Arc::clone(&self.adapters);

        let mut handles = Vec::new();
        for (i, req) in requests.into_iter().enumerate() {
            let key_id = active_keys[i % active_keys.len()].clone();
            let raw_key = match self.store.decrypt_key(&key_id) {
                Ok(k) => k,
                Err(e) => {
                    handles.push(tokio::spawn(async move {
                        Err::<GenerateResponse, anyhow::Error>(e)
                    }));
                    continue;
                }
            };

            let store = Arc::clone(&store);
            let adapters = Arc::clone(&adapters);
            let provider = provider_owned.clone();
            let caller = caller_owned.clone();

            handles.push(tokio::spawn(async move {
                let adapter = adapters.get(&provider)
                    .context(format!("No adapter for '{}'", &provider))?;

                match adapter.generate(&req, &raw_key).await {
                    Ok(mut resp) => {
                        resp.key_id = key_id.clone();
                        store.touch_key(&key_id)?;

                        let cost = adapter.estimate_cost(&req.model, resp.input_tokens, resp.output_tokens);
                        let request_id = uuid::Uuid::new_v4().to_string();
                        store.record_usage(
                            &request_id, &key_id, &provider, &req.model,
                            caller.as_deref(), None,
                            resp.input_tokens, resp.output_tokens,
                            cost.total_cost_usd, resp.latency_ms,
                            "success", None,
                        )?;

                        Ok(resp)
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("429") || err_str.contains("RESOURCE_EXHAUSTED") || err_str.contains("rate") {
                            let _ = store.update_key_status(&key_id, KeyStatus::RateLimited);
                        }

                        let request_id = uuid::Uuid::new_v4().to_string();
                        let _ = store.record_usage(
                            &request_id, &key_id, &provider, &req.model,
                            caller.as_deref(), None,
                            0, 0, 0.0, 0,
                            "error", Some(&err_str[..err_str.len().min(500)]),
                        );

                        Err(anyhow::anyhow!("Generate failed on {}: {}", &provider, &err_str[..err_str.len().min(200)]))
                    }
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap_or_else(|e| Err(anyhow::anyhow!("Task panicked: {}", e))));
        }
        results
    }

    /// Fan out requests across MULTIPLE providers in parallel.
    ///
    /// Each `(provider, request)` pair is routed to the correct adapter and
    /// assigned an active key from that provider's pool. All requests execute
    /// concurrently. This enables "competitive" mode (same prompt to N models)
    /// and "division of labor" mode (different prompts to different models).
    pub async fn parallel_generate_multi(
        &self,
        requests: Vec<(String, GenerateRequest)>,
        caller: Option<&str>,
    ) -> Vec<Result<GenerateResponse>> {
        let caller_owned = caller.map(|s| s.to_string());
        let store = Arc::clone(&self.store);
        let adapters = Arc::clone(&self.adapters);

        // Pre-fetch active keys for each provider to avoid repeated DB calls
        let mut provider_keys: HashMap<String, Vec<String>> = HashMap::new();
        let mut provider_counters: HashMap<String, usize> = HashMap::new();

        for (provider, _) in &requests {
            if !provider_keys.contains_key(provider) {
                let keys = self.store.get_active_keys(provider).unwrap_or_default();
                provider_keys.insert(provider.clone(), keys);
                provider_counters.insert(provider.clone(), 0);
            }
        }

        let mut handles = Vec::new();
        for (provider, req) in requests.into_iter() {
            let keys = match provider_keys.get(&provider) {
                Some(k) if !k.is_empty() => k,
                _ => {
                    let prov = provider.clone();
                    handles.push(tokio::spawn(async move {
                        Err::<GenerateResponse, anyhow::Error>(
                            anyhow::anyhow!("No active keys for provider '{}'", prov)
                        )
                    }));
                    continue;
                }
            };

            // Round-robin key selection within this provider
            let counter = provider_counters.get_mut(&provider).unwrap();
            let key_id = keys[*counter % keys.len()].clone();
            *counter += 1;

            let raw_key = match self.store.decrypt_key(&key_id) {
                Ok(k) => k,
                Err(e) => {
                    handles.push(tokio::spawn(async move {
                        Err::<GenerateResponse, anyhow::Error>(e)
                    }));
                    continue;
                }
            };

            let store = Arc::clone(&store);
            let adapters = Arc::clone(&adapters);
            let caller = caller_owned.clone();

            handles.push(tokio::spawn(async move {
                let adapter = adapters.get(&provider)
                    .context(format!("No adapter for '{}'", &provider))?;

                match adapter.generate(&req, &raw_key).await {
                    Ok(mut resp) => {
                        resp.key_id = key_id.clone();
                        store.touch_key(&key_id)?;

                        let cost = adapter.estimate_cost(&req.model, resp.input_tokens, resp.output_tokens);
                        let request_id = uuid::Uuid::new_v4().to_string();
                        store.record_usage(
                            &request_id, &key_id, &provider, &req.model,
                            caller.as_deref(), None,
                            resp.input_tokens, resp.output_tokens,
                            cost.total_cost_usd, resp.latency_ms,
                            "success", None,
                        )?;

                        Ok(resp)
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("429") || err_str.contains("RESOURCE_EXHAUSTED") || err_str.contains("rate") {
                            let _ = store.update_key_status(&key_id, KeyStatus::RateLimited);
                        }

                        let request_id = uuid::Uuid::new_v4().to_string();
                        let _ = store.record_usage(
                            &request_id, &key_id, &provider, &req.model,
                            caller.as_deref(), None,
                            0, 0, 0.0, 0,
                            "error", Some(&err_str[..err_str.len().min(500)]),
                        );

                        Err(anyhow::anyhow!("Generate failed on {}/{}: {}", &provider, &req.model, &err_str[..err_str.len().min(200)]))
                    }
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await.unwrap_or_else(|e| Err(anyhow::anyhow!("Task panicked: {}", e))));
        }
        results
    }

    /// Swarm mode: classify tasks by complexity, route to cheapest
    /// capable model, distribute across all available keys using
    /// rate-aware selection, and execute with automatic failover.
    pub async fn swarm_generate(
        &self,
        tasks: Vec<swarm::SwarmTask>,
    ) -> Vec<swarm::SwarmResult> {
        swarm::swarm_generate(
            tasks,
            Arc::clone(&self.store),
            Arc::clone(&self.adapters),
            Arc::clone(&self.rate_tracker),
        ).await
    }

    /// Generate an API health pulse showing all keys with usage metrics.
    pub fn health_pulse(&self, default_model: &str) -> swarm::HealthPulse {
        swarm::generate_health_pulse(&self.store, &self.rate_tracker, default_model)
    }

    /// Get the rate tracker for snapshot/dashboard access.
    pub fn rate_tracker(&self) -> &rate_tracker::RateTracker {
        &self.rate_tracker
    }
}
