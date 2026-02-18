//! Universal LLM adapter trait — provider-agnostic interface.
//!
//! Any LLM provider (Google, Anthropic, OpenAI, local, etc.) implements
//! this trait. The pool manager calls adapters; adapters never see other
//! adapters or the vault.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Re-export for convenience.
pub mod google;
pub mod anthropic;
pub mod openai;
pub mod groq;
pub mod deepseek;
pub mod perplexity;

// ── Core Types ──────────────────────────────────────────────────────

/// A provider-agnostic generation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
}

/// A provider-agnostic generation response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResponse {
    pub text: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
    pub provider: String,
    pub key_id: String,
}

/// Information about a model discovered from a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub input_token_limit: u64,
    pub output_token_limit: u64,
    pub supports_generation: bool,
    pub supports_embedding: bool,
    pub is_preview: bool,
    pub is_deprecated: bool,
    pub deprecation_date: Option<String>,
}

/// Health status of a single API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyHealth {
    pub valid: bool,
    pub tier: KeyTier,
    pub quota_remaining_pct: Option<f64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub error: Option<ProbeError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyTier {
    Free,
    Paid,
    Enterprise,
    Unknown,
}

/// Rate limit information extracted from response headers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub rpm_limit: Option<u32>,
    pub rpm_remaining: Option<u32>,
    pub rpd_limit: Option<u32>,
    pub rpd_remaining: Option<u32>,
    pub tpm_limit: Option<u64>,
    pub tpm_remaining: Option<u64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub retry_after_secs: Option<u32>,
}

/// Detailed error information from a probe or failed request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeError {
    pub http_status: u16,
    pub error_type: String,
    pub error_message: String,
    pub quota_metric: Option<String>,
    pub suggested_action: Option<String>,
    pub reset_time: Option<DateTime<Utc>>,
}

/// Cost estimate for a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
    pub model: String,
    pub provider: String,
}

/// Complete probe result from the daily scanner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub key_id: String,
    pub provider: String,
    pub timestamp: DateTime<Utc>,
    pub available_models: Vec<ModelInfo>,
    pub new_models: Vec<String>,
    pub deprecated_models: Vec<String>,
    pub key_health: KeyHealth,
    pub rate_limits: Option<RateLimitInfo>,
    pub latency_ms: u64,
}

// ── Adapter Trait ───────────────────────────────────────────────────

/// The universal LLM adapter trait.
///
/// Every provider implements this. Adding a new LLM = implementing this
/// trait in a new file, then registering it in config.
#[async_trait]
pub trait LLMAdapter: Send + Sync {
    /// Unique provider identifier (e.g., "google", "anthropic").
    fn provider_id(&self) -> &str;

    /// Human-readable provider name.
    fn display_name(&self) -> &str;

    // ── Discovery ──

    /// List all models available through this provider.
    async fn list_models(&self, key: &str) -> Result<Vec<ModelInfo>>;

    /// Check if a key is valid and what remains of its quota.
    async fn check_health(&self, key: &str) -> Result<KeyHealth>;

    // ── Execution ──

    /// Send a generation request using the provided key.
    /// The key is decrypted by the vault and passed here — the adapter
    /// must NOT store, log, or cache the key.
    async fn generate(
        &self,
        req: &GenerateRequest,
        key: &str,
    ) -> Result<GenerateResponse>;

    // ── Cost ──

    /// Estimate cost for a request (before sending).
    fn estimate_cost(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> CostEstimate;

    // ── Response Parsing ──

    /// Extract rate-limit info from response headers.
    fn parse_rate_limit_headers(
        &self,
        headers: &reqwest::header::HeaderMap,
    ) -> Option<RateLimitInfo>;

    /// Parse an error response body into structured error info.
    fn parse_error_response(
        &self,
        status: u16,
        body: &str,
    ) -> ProbeError;
}
