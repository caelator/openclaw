//! Unix socket JSON-RPC server with bearer token authentication.
//!
//! Listens on ~/.openclaw/keyvault.sock for JSON-RPC 2.0 requests.
//! All communication is local-only — no TCP network exposure.
//!
//! Auth policy:
//! - Mutating methods (kv.generate, kv.admin.*) require valid bearer token
//! - Read-only methods (kv.health, kv.models) are open (for monitoring)
//! - Rate limiting is enforced per caller

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{info, warn};

use crate::adapters::{GenerateRequest, LLMAdapter};
use crate::auth::{AuthGuard, RateLimiter};
use crate::orchestrator;
use crate::pool::PoolManager;
use crate::vault::store::KeyStore;

// ── JSON-RPC Types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    params: Option<serde_json::Value>,
    id: Option<serde_json::Value>,
    /// Bearer token for authentication
    auth: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self { jsonrpc: "2.0".into(), result: Some(result), error: None, id }
    }
    fn error(id: Option<serde_json::Value>, code: i32, message: String) -> Self {
        Self { jsonrpc: "2.0".into(), result: None, error: Some(JsonRpcError { code, message }), id }
    }
    fn auth_error(id: Option<serde_json::Value>) -> Self {
        Self::error(id, -32001, "Authentication required — include valid \"auth\" field with bearer token from ~/.openclaw/keyvault.token".into())
    }
    fn rate_limited(id: Option<serde_json::Value>, retry_after_secs: u64) -> Self {
        Self::error(id, -32002, format!("Rate limited — retry after {} seconds", retry_after_secs))
    }
}

// ── Server ──────────────────────────────────────────────────────────

pub struct Server {
    socket_path: PathBuf,
    store: Arc<KeyStore>,
    pool: Arc<PoolManager>,
    adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
    auth: Arc<RwLock<AuthGuard>>,
    rate_limiter: Arc<RateLimiter>,
}

impl Server {
    pub fn new(
        socket_path: PathBuf,
        store: Arc<KeyStore>,
        pool: Arc<PoolManager>,
        adapters: Arc<HashMap<String, Box<dyn LLMAdapter>>>,
        auth: AuthGuard,
    ) -> Self {
        Self {
            socket_path,
            store,
            pool,
            adapters,
            auth: Arc::new(RwLock::new(auth)),
            // 100 requests per minute per caller (generous but prevents abuse)
            rate_limiter: Arc::new(RateLimiter::new(100, 60)),
        }
    }

    pub async fn run(&self) -> Result<()> {
        // Remove stale socket file
        if self.socket_path.exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        // Ensure parent directory exists
        if let Some(parent) = self.socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Restrict socket permissions (owner-only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.socket_path, std::fs::Permissions::from_mode(0o600))?;
        }

        info!(
            socket = %self.socket_path.display(),
            "🔑 KeyVault server listening (auth enforced)"
        );

        // ── 15-minute health pulse background timer ──
        {
            let pool = Arc::clone(&self.pool);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
                interval.tick().await; // Skip the immediate tick (no traffic yet)
                loop {
                    interval.tick().await;
                    let pulse = pool.health_pulse("gemini-3-flash-preview");
                    info!(
                        keys = pulse.keys.len(),
                        active = pulse.totals.active_keys,
                        rate_limited = pulse.totals.rate_limited_keys,
                        requests_24h = pulse.totals.requests_24h,
                        successes_24h = pulse.totals.successes_24h,
                        failures_24h = pulse.totals.failures_24h,
                        cost_24h = format!("${:.4}", pulse.totals.total_cost_24h_usd),
                        "💓 Health pulse (15-min auto)"
                    );
                }
            });
        }

        loop {
            let (stream, _) = listener.accept().await?;
            let store = Arc::clone(&self.store);
            let pool = Arc::clone(&self.pool);
            let adapters = Arc::clone(&self.adapters);
            let auth = Arc::clone(&self.auth);
            let rate_limiter = Arc::clone(&self.rate_limiter);

            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                // Bound reads to 1 MB to prevent DoS from oversized payloads
                const MAX_REQUEST_BYTES: u64 = 1_048_576;
                let bounded = reader.take(MAX_REQUEST_BYTES);
                let mut reader = BufReader::new(bounded);
                let mut line = String::new();

                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let response = handle_request(
                                &line, &store, &pool, &adapters, &auth, &rate_limiter
                            ).await;
                            let resp_json = serde_json::to_string(&response).unwrap_or_default();
                            if writer.write_all(resp_json.as_bytes()).await.is_err() { break; }
                            if writer.write_all(b"\n").await.is_err() { break; }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }
}

// ── Auth Policy ─────────────────────────────────────────────────────

/// Methods that require authentication.
fn requires_auth(method: &str) -> bool {
    match method {
        // Read-only monitoring endpoints — open
        "kv.health" | "kv.models" | "kv.activeModels" | "kv.modelRegistry" | "kv.swarmStatus" => false,
        // Everything else — authenticated
        _ => true,
    }
}

// ── Request Handling ────────────────────────────────────────────────

async fn handle_request(
    raw: &str,
    store: &KeyStore,
    pool: &PoolManager,
    adapters: &HashMap<String, Box<dyn LLMAdapter>>,
    auth: &RwLock<AuthGuard>,
    rate_limiter: &RateLimiter,
) -> JsonRpcResponse {
    let req: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => return JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e)),
    };

    // ── Auth check ──
    if requires_auth(&req.method) {
        let token = match &req.auth {
            Some(t) => t.as_str(),
            None => {
                warn!(method = %req.method, "Request rejected — no auth token");
                return JsonRpcResponse::auth_error(req.id);
            }
        };

        let valid = {
            let guard = auth.read().unwrap();
            guard.validate(token)
        };

        if !valid {
            warn!(method = %req.method, "Request rejected — invalid auth token");
            return JsonRpcResponse::auth_error(req.id);
        }
    }

    // ── Rate limiting ──
    let caller = req.params.as_ref()
        .and_then(|p| p.get("caller"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous");

    if let Err(retry_secs) = rate_limiter.check(caller) {
        warn!(caller = caller, method = %req.method, "Rate limited");
        return JsonRpcResponse::rate_limited(req.id, retry_secs);
    }

    let params = req.params.unwrap_or(serde_json::Value::Null);

    match req.method.as_str() {
        "kv.generate" => handle_generate(req.id, params, store, pool, adapters).await,
        "kv.parallelGenerate" => handle_parallel_generate(req.id, params, pool).await,
        "kv.swarmGenerate" => handle_swarm_generate(req.id, params, pool).await,
        "kv.swarmStatus" => handle_swarm_status(req.id, params, pool),
        "kv.modelRegistry" => handle_model_registry(req.id),
        "kv.activeModels" => handle_active_models(req.id, store),
        "kv.models" => handle_models(req.id, store),
        "kv.health" => handle_health(req.id, store, pool),
        "kv.usage" => handle_usage(req.id, params, store),
        "kv.admin.addKey" => handle_add_key(req.id, params, store),
        "kv.admin.removeKey" => handle_remove_key(req.id, params, store),
        "kv.admin.listKeys" => handle_list_keys(req.id, store),
        "kv.admin.rotateToken" => handle_rotate_token(req.id, auth),
        "kv.admin.syncToken" => handle_sync_token(req.id, auth),
        _ => JsonRpcResponse::error(req.id, -32601, format!("Unknown method: {}", req.method)),
    }
}

async fn handle_generate(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    store: &KeyStore,
    pool: &PoolManager,
    adapters: &HashMap<String, Box<dyn LLMAdapter>>,
) -> JsonRpcResponse {
    let gen_req: GenerateRequest = match serde_json::from_value(params.clone()) {
        Ok(r) => r,
        Err(e) => return JsonRpcResponse::error(id, -32602, format!("Invalid params: {}", e)),
    };

    let caller = params.get("caller").and_then(|v| v.as_str());
    let budget_tag = params.get("budget_tag").and_then(|v| v.as_str());
    let provider_hint = params.get("provider").and_then(|v| v.as_str());

    // Determine provider
    let provider = if let Some(p) = provider_hint {
        p.to_string()
    } else {
        // Use orchestrator to decide
        let decision = if let Some(fast) = orchestrator::fast_classify(&gen_req) {
            fast
        } else {
            // Use LLM classifier
            let google_adapter = adapters.get("google");
            match google_adapter {
                Some(adapter) => {
                    orchestrator::llm_classify(store, adapter.as_ref(), &gen_req)
                        .await
                        .unwrap_or_else(|_| orchestrator::RoutingDecision {
                            task_type: "unknown".into(),
                            complexity: "medium".into(),
                            recommended_provider: "google".into(),
                            recommended_model: "gemini-2.5-flash".into(),
                            fallback_chain: vec![],
                            estimated_tokens: 2000,
                            rationale: "Fallback default".into(),
                        })
                }
                None => return JsonRpcResponse::error(id, -32000, "No adapters available".into()),
            }
        };

        decision.recommended_provider
    };

    match pool.generate(&provider, &gen_req, caller, budget_tag).await {
        Ok(resp) => JsonRpcResponse::success(id, serde_json::to_value(resp).unwrap()),
        Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
    }
}

fn handle_models(id: Option<serde_json::Value>, store: &KeyStore) -> JsonRpcResponse {
    let db = store.db().lock().unwrap();
    let mut stmt = match db.prepare(
        "SELECT id, provider, display_name, input_token_limit, output_token_limit, is_preview, last_seen FROM model_catalog ORDER BY provider, id"
    ) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(id, -32000, e.to_string()),
    };

    let models: Vec<serde_json::Value> = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "provider": row.get::<_, String>(1)?,
            "display_name": row.get::<_, String>(2)?,
            "input_token_limit": row.get::<_, i64>(3)?,
            "output_token_limit": row.get::<_, i64>(4)?,
            "is_preview": row.get::<_, bool>(5)?,
            "last_seen": row.get::<_, String>(6)?,
        }))
    }).unwrap().filter_map(|r| r.ok()).collect();

    JsonRpcResponse::success(id, serde_json::json!({ "models": models }))
}

fn handle_health(
    id: Option<serde_json::Value>,
    store: &KeyStore,
    pool: &PoolManager,
) -> JsonRpcResponse {
    match store.list_keys() {
        Ok(keys) => {
            let health: Vec<serde_json::Value> = keys.iter().map(|k| {
                serde_json::json!({
                    "id": k.id,
                    "provider": k.provider,
                    "role": format!("{:?}", k.role),
                    "status": format!("{:?}", k.status),
                    "last_used": k.last_used_at.map(|dt| dt.to_rfc3339()),
                    "last_health_check": k.last_health_check.map(|dt| dt.to_rfc3339()),
                })
            }).collect();

            // Include health pulse for live metrics
            let pulse = pool.health_pulse("gemini-3-flash-preview");

            JsonRpcResponse::success(id, serde_json::json!({
                "keys": health,
                "pulse": {
                    "generated_at": pulse.generated_at,
                    "totals": pulse.totals,
                    "per_key": pulse.keys,
                },
            }))
        }
        Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
    }
}

fn handle_usage(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    store: &KeyStore,
) -> JsonRpcResponse {
    let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
    let db = store.db().lock().unwrap();
    let mut stmt = match db.prepare(
        "SELECT request_id, key_id, provider, model, caller, timestamp, input_tokens, output_tokens, cost_usd, latency_ms, status
         FROM usage_log ORDER BY timestamp DESC LIMIT ?1"
    ) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(id, -32000, e.to_string()),
    };

    let usage: Vec<serde_json::Value> = stmt.query_map([limit], |row| {
        Ok(serde_json::json!({
            "request_id": row.get::<_, String>(0)?,
            "key_id": row.get::<_, String>(1)?,
            "provider": row.get::<_, String>(2)?,
            "model": row.get::<_, String>(3)?,
            "caller": row.get::<_, Option<String>>(4)?,
            "timestamp": row.get::<_, String>(5)?,
            "input_tokens": row.get::<_, i64>(6)?,
            "output_tokens": row.get::<_, i64>(7)?,
            "cost_usd": row.get::<_, f64>(8)?,
            "latency_ms": row.get::<_, i64>(9)?,
            "status": row.get::<_, String>(10)?,
        }))
    }).unwrap().filter_map(|r| r.ok()).collect();

    JsonRpcResponse::success(id, serde_json::json!({ "usage": usage }))
}

fn handle_add_key(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    store: &KeyStore,
) -> JsonRpcResponse {
    let key_id = params.get("id").and_then(|v| v.as_str());
    let provider = params.get("provider").and_then(|v| v.as_str());
    let value = params.get("value").and_then(|v| v.as_str());
    let role = params.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
    let notes = params.get("notes").and_then(|v| v.as_str());

    match (key_id, provider, value) {
        (Some(kid), Some(prov), Some(val)) => {
            let key_role = match role {
                "orchestrator" => crate::vault::store::KeyRole::Orchestrator,
                "spare" => crate::vault::store::KeyRole::Spare,
                _ => crate::vault::store::KeyRole::Worker,
            };
            match store.add_key(kid, prov, val, key_role, notes) {
                Ok(()) => JsonRpcResponse::success(id, serde_json::json!({"ok": true, "id": kid})),
                Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
            }
        }
        _ => JsonRpcResponse::error(id, -32602, "Missing required params: id, provider, value".into()),
    }
}

fn handle_remove_key(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    store: &KeyStore,
) -> JsonRpcResponse {
    let key_id = params.get("id").and_then(|v| v.as_str());
    match key_id {
        Some(kid) => match store.remove_key(kid) {
            Ok(true) => JsonRpcResponse::success(id, serde_json::json!({"ok": true, "removed": kid})),
            Ok(false) => JsonRpcResponse::error(id, -32000, format!("Key '{}' not found", kid)),
            Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
        },
        None => JsonRpcResponse::error(id, -32602, "Missing required param: id".into()),
    }
}

fn handle_list_keys(
    id: Option<serde_json::Value>,
    store: &KeyStore,
) -> JsonRpcResponse {
    match store.list_keys() {
        Ok(keys) => {
            let list: Vec<serde_json::Value> = keys.iter().map(|k| {
                serde_json::json!({
                    "id": k.id,
                    "provider": k.provider,
                    "role": format!("{:?}", k.role),
                    "status": format!("{:?}", k.status),
                    "added_at": k.added_at.to_rfc3339(),
                    "notes": k.notes,
                })
            }).collect();
            JsonRpcResponse::success(id, serde_json::json!({ "keys": list }))
        }
        Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
    }
}

fn handle_rotate_token(
    id: Option<serde_json::Value>,
    auth: &RwLock<AuthGuard>,
) -> JsonRpcResponse {
    let mut guard = auth.write().unwrap();
    match guard.rotate() {
        Ok(_) => {
            info!("🔄 Auth token rotated via admin request");
            JsonRpcResponse::success(id, serde_json::json!({
                "ok": true,
                "message": "Token rotated. Clients should re-read ~/.openclaw/keyvault.token"
            }))
        }
        Err(e) => JsonRpcResponse::error(id, -32000, format!("Token rotation failed: {}", e)),
    }
}

fn handle_sync_token(
    id: Option<serde_json::Value>,
    auth: &RwLock<AuthGuard>,
) -> JsonRpcResponse {
    let guard = auth.read().unwrap();
    match guard.sync() {
        Ok(()) => {
            info!("🔄 Token file re-synced from Keychain");
            JsonRpcResponse::success(id, serde_json::json!({
                "ok": true,
                "message": "Token file re-synced from Keychain"
            }))
        }
        Err(e) => JsonRpcResponse::error(id, -32000, format!("Token sync failed: {}", e)),
    }
}

// ── Parallel Generate ───────────────────────────────────────────────

async fn handle_parallel_generate(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    pool: &PoolManager,
) -> JsonRpcResponse {
    // Parse requests: array of {provider, ...GenerateRequest fields}
    let requests_raw = match params.get("requests").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return JsonRpcResponse::error(id, -32602, "Missing 'requests' array".into()),
    };

    let caller = params.get("caller").and_then(|v| v.as_str());

    let mut requests: Vec<(String, GenerateRequest)> = Vec::new();
    for (i, raw) in requests_raw.iter().enumerate() {
        let provider = match raw.get("provider").and_then(|v| v.as_str()) {
            Some(p) => p.to_string(),
            None => return JsonRpcResponse::error(
                id, -32602,
                format!("Request {} missing 'provider' field", i),
            ),
        };

        let gen_req: GenerateRequest = match serde_json::from_value(raw.clone()) {
            Ok(r) => r,
            Err(e) => return JsonRpcResponse::error(
                id, -32602,
                format!("Request {} invalid: {}", i, e),
            ),
        };

        requests.push((provider, gen_req));
    }

    let results = pool.parallel_generate_multi(requests, caller).await;

    // Build response: array of {ok, response} or {ok, error}
    let responses: Vec<serde_json::Value> = results.into_iter().map(|r| {
        match r {
            Ok(resp) => serde_json::json!({
                "ok": true,
                "response": resp,
            }),
            Err(e) => serde_json::json!({
                "ok": false,
                "error": e.to_string(),
            }),
        }
    }).collect();

    JsonRpcResponse::success(id, serde_json::json!({ "results": responses }))
}

// ── Active Models ───────────────────────────────────────────────────

fn handle_active_models(
    id: Option<serde_json::Value>,
    store: &KeyStore,
) -> JsonRpcResponse {
    // List all keys grouped by provider, with active model info
    let keys = match store.list_keys() {
        Ok(k) => k,
        Err(e) => return JsonRpcResponse::error(id, -32000, e.to_string()),
    };

    // Group active keys by provider
    let mut providers: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for key in &keys {
        if key.status == crate::vault::store::KeyStatus::Active {
            providers.entry(key.provider.clone()).or_default().push(
                serde_json::json!({
                    "key_id": key.id,
                    "role": format!("{:?}", key.role),
                })
            );
        }
    }

    // Get models from catalog
    let db = store.db().lock().unwrap();
    let mut stmt = match db.prepare(
        "SELECT id, provider, display_name, input_token_limit, output_token_limit \
         FROM model_catalog WHERE is_deprecated = 0 ORDER BY provider, id"
    ) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(id, -32000, e.to_string()),
    };

    let mut models_by_provider: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    let _ = stmt.query_map([], |row| {
        let model_id: String = row.get(0)?;
        let prov: String = row.get(1)?;
        let display: String = row.get(2)?;
        let input_limit: i64 = row.get(3)?;
        let output_limit: i64 = row.get(4)?;

        models_by_provider.entry(prov.clone()).or_default().push(
            serde_json::json!({
                "model_id": model_id,
                "display_name": display,
                "input_token_limit": input_limit,
                "output_token_limit": output_limit,
            })
        );
        Ok(())
    });

    // Combine: only providers with active keys
    let result: Vec<serde_json::Value> = providers.iter().map(|(provider, active_keys)| {
        serde_json::json!({
            "provider": provider,
            "active_keys": active_keys.len(),
            "keys": active_keys,
            "models": models_by_provider.get(provider).unwrap_or(&vec![]),
        })
    }).collect();

    JsonRpcResponse::success(id, serde_json::json!({
        "providers": result,
        "total_active_keys": keys.iter().filter(|k| k.status == crate::vault::store::KeyStatus::Active).count(),
    }))
}

// ── Swarm Generate ──────────────────────────────────────────────────

async fn handle_swarm_generate(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    pool: &PoolManager,
) -> JsonRpcResponse {
    use crate::pool::swarm::SwarmTask;

    // Parse tasks array
    let tasks_raw = match params.get("tasks").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return JsonRpcResponse::error(id, -32602, "Missing 'tasks' array".into()),
    };

    let mut tasks: Vec<SwarmTask> = Vec::new();
    for (i, raw) in tasks_raw.iter().enumerate() {
        match serde_json::from_value::<SwarmTask>(raw.clone()) {
            Ok(t) => tasks.push(t),
            Err(e) => return JsonRpcResponse::error(
                id, -32602,
                format!("Task {} invalid: {}", i, e),
            ),
        }
    }

    let results = pool.swarm_generate(tasks).await;

    let responses: Vec<serde_json::Value> = results.into_iter().map(|r| {
        serde_json::json!({
            "label": r.label,
            "model": r.model,
            "key_id": r.key_id,
            "complexity": r.complexity,
            "ok": r.ok,
            "text": r.text,
            "error": r.error,
            "input_tokens": r.input_tokens,
            "output_tokens": r.output_tokens,
            "latency_ms": r.latency_ms,
        })
    }).collect();

    // Summary stats
    let total = responses.len();
    let succeeded = responses.iter().filter(|r| r["ok"].as_bool() == Some(true)).count();
    let failed = total - succeeded;

    JsonRpcResponse::success(id, serde_json::json!({
        "results": responses,
        "summary": {
            "total": total,
            "succeeded": succeeded,
            "failed": failed,
        }
    }))
}

// ── Model Registry ──────────────────────────────────────────────────

fn handle_model_registry(id: Option<serde_json::Value>) -> JsonRpcResponse {
    use crate::pool::registry;

    let models: Vec<serde_json::Value> = registry::GOOGLE_MODELS.iter().map(|m| {
        serde_json::json!({
            "id": m.id,
            "provider": m.provider,
            "display_name": m.display_name,
            "tier": format!("{:?}", m.tier),
            "code_quality": m.code_quality,
            "supports_thinking": m.supports_thinking,
            "input_token_limit": m.input_token_limit,
            "output_token_limit": m.output_token_limit,
            "free_rpm": m.free_rpm,
            "free_rpd": m.free_rpd,
            "free_tpm": m.free_tpm,
            "min_complexity": m.min_complexity,
            "deprecated": m.deprecated,
        })
    }).collect();

    JsonRpcResponse::success(id, serde_json::json!({
        "models": models,
        "total": models.len(),
    }))
}

// ── Swarm Status / Health Pulse ─────────────────────────────────────

fn handle_swarm_status(
    id: Option<serde_json::Value>,
    params: serde_json::Value,
    pool: &PoolManager,
) -> JsonRpcResponse {
    // Optional model parameter for RPM/RPD limits (defaults to gemini-3-flash-preview)
    let default_model = params.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini-3-flash-preview");

    let pulse = pool.health_pulse(default_model);

    JsonRpcResponse::success(id, serde_json::json!({
        "generated_at": pulse.generated_at,
        "keys": pulse.keys,
        "totals": pulse.totals,
    }))
}
