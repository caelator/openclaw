//! Swarm scheduler — distributes tasks across N keys × M models.
//!
//! The swarm expands to utilize all available API keys. It classifies
//! each task by complexity, assigns it the cheapest capable model,
//! spreads requests across keys via least-loaded selection (not blind
//! round-robin), and executes all tasks in parallel.
//!
//! **Failover:** If a request hits a rate limit, the swarm automatically
//! retries on a different key. If all keys for that model are exhausted,
//! it cascades UP to the next model tier.
//!
//! **Rate Tracking:** Pre-flight RPM/RPD checks prevent wasting calls
//! that would 429. The rate tracker persists in-memory across requests.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::adapters::{GenerateRequest, LLMAdapter, Message};
use crate::vault::store::{KeyStatus, KeyStore};
use super::classifier;
use super::rate_tracker::RateTracker;
use super::registry::{self, ModelSpec, TaskComplexity};

/// Maximum number of retry attempts per task (key rotation + model cascade).
const MAX_RETRIES: usize = 3;

// ── Swarm Task ──────────────────────────────────────────────────────

/// A single task to be executed by the swarm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmTask {
    /// The code-generation prompt
    pub prompt: String,
    /// Optional system prompt override
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional explicit complexity hint (skips classifier if provided)
    #[serde(default)]
    pub complexity: Option<TaskComplexity>,
    /// Optional explicit model override (skips routing if provided)
    #[serde(default)]
    pub model: Option<String>,
    /// Optional label for tracking
    #[serde(default)]
    pub label: Option<String>,
    /// Temperature (default 0.2 for code)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Max output tokens
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Result of a swarm task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmResult {
    /// The label from the original task
    pub label: Option<String>,
    /// Which model handled this task
    pub model: String,
    /// Which key was used (obfuscated)
    pub key_id: String,
    /// The detected/assigned complexity
    pub complexity: TaskComplexity,
    /// Whether the task succeeded
    pub ok: bool,
    /// The generated text (if successful)
    pub text: Option<String>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Token counts
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Latency in milliseconds
    pub latency_ms: u64,
    /// Number of retries needed
    pub retries: u32,
}

// ── Obfuscation ─────────────────────────────────────────────────────

/// Obfuscate a key ID for display: "abc123def456" → "abc1...f456"
pub fn obfuscate_key(key_id: &str) -> String {
    if key_id.len() <= 8 {
        return format!("{}...", &key_id[..key_id.len().min(4)]);
    }
    format!("{}...{}", &key_id[..4], &key_id[key_id.len()-4..])
}

// ── Rate-Aware Scheduling ───────────────────────────────────────────

/// Select the best key for a model, using the rate tracker to avoid
/// overloading any single key.
fn select_key<'a>(
    keys: &'a [String],
    model: &ModelSpec,
    tracker: &RateTracker,
    store: &KeyStore,
) -> Option<(String, String)> {
    // Use least-loaded selection (best RPM headroom)
    if let Some(key_id) = tracker.least_loaded_key(keys, model.id, model.free_rpm, model.free_rpd) {
        if let Ok(raw_key) = store.decrypt_key(key_id) {
            return Some((key_id.clone(), raw_key));
        }
    }
    None
}

// ── Single Task Execution with Failover ─────────────────────────────

/// Execute a single task with automatic failover:
/// 1. Try the assigned model + least-loaded key
/// 2. On rate limit → try next key for same model
/// 3. All keys exhausted → cascade to fallback model
async fn execute_with_failover(
    task: SwarmTask,
    initial_model: &'static ModelSpec,
    complexity: TaskComplexity,
    active_keys: Vec<String>,
    store: Arc<KeyStore>,
    adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    tracker: Arc<RateTracker>,
) -> SwarmResult {
    let label = task.label.clone();
    let mut current_model = initial_model;
    let mut retries: u32 = 0;
    let mut last_error = String::new();

    // Try up to MAX_RETRIES times (across keys and model cascades)
    for _attempt in 0..MAX_RETRIES {
        // Select the least-loaded key for this model
        let (key_id, raw_key) = match select_key(&active_keys, current_model, &tracker, &store) {
            Some(pair) => pair,
            None => {
                // All keys at capacity for this model — try fallback
                if let Some(fallback) = classifier::fallback_model(current_model.id) {
                    current_model = fallback;
                    retries += 1;
                    last_error = format!("All keys at capacity for {}, cascading to {}", 
                        current_model.id, fallback.id);
                    continue;
                }
                // No fallback available — all capacity exhausted
                return SwarmResult {
                    label,
                    model: current_model.id.to_string(),
                    key_id: "none".into(),
                    complexity,
                    ok: false,
                    text: None,
                    error: Some(format!("All keys at capacity, no fallback available. Last: {}", last_error)),
                    input_tokens: 0,
                    output_tokens: 0,
                    latency_ms: 0,
                    retries,
                };
            }
        };

        // Record the attempt in the rate tracker BEFORE sending
        tracker.record_request(&key_id, current_model.id);

        let gen_req = GenerateRequest {
            model: current_model.id.to_string(),
            messages: vec![Message {
                role: "user".into(),
                content: task.prompt.clone(),
            }],
            temperature: Some(task.temperature.unwrap_or(0.2)),
            max_tokens: Some(task.max_tokens
                .unwrap_or(current_model.output_token_limit as u32)),
            system_prompt: task.system_prompt.clone(),
        };

        let provider = current_model.provider;
        let adapter = match adapters.get(provider) {
            Some(a) => a,
            None => {
                return SwarmResult {
                    label,
                    model: current_model.id.to_string(),
                    key_id: obfuscate_key(&key_id),
                    complexity,
                    ok: false,
                    text: None,
                    error: Some(format!("No adapter for '{}'", provider)),
                    input_tokens: 0,
                    output_tokens: 0,
                    latency_ms: 0,
                    retries,
                };
            }
        };

        match adapter.generate(&gen_req, &raw_key).await {
            Ok(resp) => {
                // Success! Record usage.
                let cost = adapter.estimate_cost(
                    &gen_req.model, resp.input_tokens, resp.output_tokens
                );
                let request_id = uuid::Uuid::new_v4().to_string();
                let _ = store.record_usage(
                    &request_id, &key_id, provider, &gen_req.model,
                    Some("swarm"), None,
                    resp.input_tokens, resp.output_tokens,
                    cost.total_cost_usd, resp.latency_ms,
                    "success", None,
                );
                let _ = store.touch_key(&key_id);

                return SwarmResult {
                    label,
                    model: current_model.id.to_string(),
                    key_id: obfuscate_key(&key_id),
                    complexity,
                    ok: true,
                    text: Some(resp.text),
                    error: None,
                    input_tokens: resp.input_tokens,
                    output_tokens: resp.output_tokens,
                    latency_ms: resp.latency_ms,
                    retries,
                };
            }
            Err(e) => {
                let err_str = e.to_string();
                last_error = err_str.clone();

                // Record the failure
                let request_id = uuid::Uuid::new_v4().to_string();
                let _ = store.record_usage(
                    &request_id, &key_id, provider, &gen_req.model,
                    Some("swarm"), None,
                    0, 0, 0.0, 0,
                    "error", Some(&err_str[..err_str.len().min(500)]),
                );

                // Check if it's a rate limit error
                let is_rate_limit = err_str.contains("429")
                    || err_str.contains("RESOURCE_EXHAUSTED")
                    || err_str.contains("rate");

                if is_rate_limit {
                    let _ = store.update_key_status(&key_id, KeyStatus::RateLimited);
                    retries += 1;
                    // The rate tracker will now avoid this key on next iteration
                    continue;
                }

                // Non-rate-limit error — don't retry (could be auth, model not found, etc.)
                return SwarmResult {
                    label,
                    model: current_model.id.to_string(),
                    key_id: obfuscate_key(&key_id),
                    complexity,
                    ok: false,
                    text: None,
                    error: Some(err_str[..err_str.len().min(200)].to_string()),
                    input_tokens: 0,
                    output_tokens: 0,
                    latency_ms: 0,
                    retries,
                };
            }
        }
    }

    // Exhausted all retries
    SwarmResult {
        label,
        model: current_model.id.to_string(),
        key_id: "exhausted".into(),
        complexity,
        ok: false,
        text: None,
        error: Some(format!("Exhausted {} retries. Last error: {}", MAX_RETRIES, 
            last_error[..last_error.len().min(200)].to_string())),
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: 0,
        retries,
    }
}

// ── High-Level API ──────────────────────────────────────────────────

/// Swarm entry point: classify, schedule with rate awareness, execute
/// with automatic failover.
pub async fn swarm_generate(
    tasks: Vec<SwarmTask>,
    store: Arc<KeyStore>,
    adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    tracker: Arc<RateTracker>,
) -> Vec<SwarmResult> {
    let provider = "google";
    let active_keys = store.get_active_keys(provider).unwrap_or_default();

    if active_keys.is_empty() {
        return tasks.into_iter().map(|t| SwarmResult {
            label: t.label,
            model: "none".into(),
            key_id: "none".into(),
            complexity: t.complexity.unwrap_or(TaskComplexity::Medium),
            ok: false,
            text: None,
            error: Some(format!("No active keys for provider '{}'", provider)),
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0,
            retries: 0,
        }).collect();
    }

    let mut handles = Vec::new();

    for task in tasks {
        let complexity = task.complexity
            .unwrap_or_else(|| classifier::classify(&task.prompt));

        let model = if let Some(ref model_id) = task.model {
            registry::get_model(model_id).unwrap_or_else(|| classifier::select_model(complexity))
        } else {
            classifier::select_model(complexity)
        };

        let store = Arc::clone(&store);
        let adapters = Arc::clone(&adapters);
        let tracker = Arc::clone(&tracker);
        let keys = active_keys.clone();

        handles.push(tokio::spawn(async move {
            execute_with_failover(task, model, complexity, keys, store, adapters, tracker).await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.unwrap_or_else(|e| SwarmResult {
            label: None,
            model: "unknown".into(),
            key_id: "unknown".into(),
            complexity: TaskComplexity::Medium,
            ok: false,
            text: None,
            error: Some(format!("Task panicked: {}", e)),
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0,
            retries: 0,
        }));
    }
    results
}

// ── Health Pulse ────────────────────────────────────────────────────

/// Health pulse for a single key — rolling 24hr metrics.
#[derive(Debug, Clone, Serialize)]
pub struct KeyHealthPulse {
    /// Obfuscated key ID
    pub key_id: String,
    /// Key status
    pub status: String,
    /// Current RPM (live, from rate tracker)
    pub live_rpm: u32,
    /// Current RPD (live, from rate tracker)
    pub live_rpd: u32,
    /// RPM limit for the model
    pub rpm_limit: u16,
    /// RPD limit for the model
    pub rpd_limit: u32,
    /// Usage % of RPM
    pub rpm_utilization_pct: f32,
    /// Usage % of RPD
    pub rpd_utilization_pct: f32,
    /// Total requests in last 24hrs (from SQLite)
    pub requests_24h: u64,
    /// Successful requests in last 24hrs
    pub successes_24h: u64,
    /// Failed requests in last 24hrs
    pub failures_24h: u64,
    /// Total input tokens in last 24hrs
    pub input_tokens_24h: u64,
    /// Total output tokens in last 24hrs
    pub output_tokens_24h: u64,
    /// Total cost in last 24hrs (USD)
    pub cost_24h_usd: f64,
}

/// Full health pulse response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthPulse {
    /// Timestamp of this pulse
    pub generated_at: String,
    /// Per-key health metrics
    pub keys: Vec<KeyHealthPulse>,
    /// Aggregate totals
    pub totals: PulseTotals,
}

/// Aggregate totals across all keys.
#[derive(Debug, Clone, Serialize)]
pub struct PulseTotals {
    pub total_keys: usize,
    pub active_keys: usize,
    pub rate_limited_keys: usize,
    pub requests_24h: u64,
    pub successes_24h: u64,
    pub failures_24h: u64,
    pub total_input_tokens_24h: u64,
    pub total_output_tokens_24h: u64,
    pub total_cost_24h_usd: f64,
}

/// Generate a health pulse by combining live rate tracker data with
/// 24hr rolling usage from SQLite.
pub fn generate_health_pulse(
    store: &KeyStore,
    tracker: &RateTracker,
    default_model: &str,
) -> HealthPulse {
    let model_spec = registry::get_model(default_model)
        .unwrap_or(&registry::GOOGLE_MODELS[1]); // default to 3-flash

    // Get all keys (not just active) for full visibility
    let all_keys = store.list_all_keys().unwrap_or_default();
    let rate_snapshot = tracker.snapshot();

    // Build a lookup from the snapshot
    let mut rate_map: HashMap<String, (u32, u32)> = HashMap::new();
    for snap in &rate_snapshot {
        let entry = rate_map.entry(snap.key_id.clone()).or_insert((0, 0));
        entry.0 = entry.0.max(snap.current_rpm); // max RPM across models
        entry.1 += snap.current_rpd; // sum RPD across models
    }

    // Query 24hr usage from SQLite per key
    let usage_24h = store.usage_last_24h().unwrap_or_default();

    let mut key_pulses = Vec::new();
    let mut totals = PulseTotals {
        total_keys: all_keys.len(),
        active_keys: 0,
        rate_limited_keys: 0,
        requests_24h: 0,
        successes_24h: 0,
        failures_24h: 0,
        total_input_tokens_24h: 0,
        total_output_tokens_24h: 0,
        total_cost_24h_usd: 0.0,
    };

    for key in &all_keys {
        let key_id = &key.id;
        let status_str = format!("{:?}", key.status);

        match key.status {
            KeyStatus::Active => totals.active_keys += 1,
            KeyStatus::RateLimited => totals.rate_limited_keys += 1,
            _ => {}
        }

        let (live_rpm, live_rpd) = rate_map.get(key_id).copied().unwrap_or((0, 0));

        // Look up 24hr usage for this key
        let usage = usage_24h.get(key_id);
        let requests_24h = usage.map(|u| u.total_requests).unwrap_or(0);
        let successes_24h = usage.map(|u| u.successes).unwrap_or(0);
        let failures_24h = usage.map(|u| u.failures).unwrap_or(0);
        let input_tokens_24h = usage.map(|u| u.input_tokens).unwrap_or(0);
        let output_tokens_24h = usage.map(|u| u.output_tokens).unwrap_or(0);
        let cost_24h = usage.map(|u| u.cost_usd).unwrap_or(0.0);

        totals.requests_24h += requests_24h;
        totals.successes_24h += successes_24h;
        totals.failures_24h += failures_24h;
        totals.total_input_tokens_24h += input_tokens_24h;
        totals.total_output_tokens_24h += output_tokens_24h;
        totals.total_cost_24h_usd += cost_24h;

        let rpm_util = if model_spec.free_rpm > 0 {
            (live_rpm as f32 / model_spec.free_rpm as f32) * 100.0
        } else { 0.0 };

        let rpd_util = if model_spec.free_rpd > 0 {
            (live_rpd as f32 / model_spec.free_rpd as f32) * 100.0
        } else { 0.0 };

        key_pulses.push(KeyHealthPulse {
            key_id: obfuscate_key(key_id),
            status: status_str,
            live_rpm,
            live_rpd,
            rpm_limit: model_spec.free_rpm,
            rpd_limit: model_spec.free_rpd,
            rpm_utilization_pct: (rpm_util * 10.0).round() / 10.0,
            rpd_utilization_pct: (rpd_util * 10.0).round() / 10.0,
            requests_24h,
            successes_24h,
            failures_24h,
            input_tokens_24h,
            output_tokens_24h,
            cost_24h_usd: (cost_24h * 10000.0).round() / 10000.0,
        });
    }

    HealthPulse {
        generated_at: chrono::Utc::now().to_rfc3339(),
        keys: key_pulses,
        totals,
    }
}
