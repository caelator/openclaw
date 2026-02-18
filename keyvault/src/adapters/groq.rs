//! Groq adapter — OpenAI-compatible API with rate-limit headers.

use anyhow::Result;
use async_trait::async_trait;
use super::*;

pub struct GroqAdapter { client: reqwest::Client }
impl GroqAdapter { pub fn new() -> Self { Self { client: reqwest::Client::new() } } }

#[async_trait]
impl LLMAdapter for GroqAdapter {
    fn provider_id(&self) -> &str { "groq" }
    fn display_name(&self) -> &str { "Groq" }

    async fn list_models(&self, key: &str) -> Result<Vec<ModelInfo>> {
        let resp = self.client.get("https://api.groq.com/openai/v1/models")
            .bearer_auth(key).send().await?;
        if !resp.status().is_success() { return Ok(vec![]); }
        let body: serde_json::Value = resp.json().await?;
        Ok(body["data"].as_array().map(|arr| arr.iter().filter_map(|m| {
            Some(ModelInfo {
                id: m["id"].as_str()?.to_string(),
                display_name: m["id"].as_str()?.to_string(),
                provider: "groq".to_string(),
                input_token_limit: m["context_window"].as_u64().unwrap_or(32_768),
                output_token_limit: 8_192,
                supports_generation: true, supports_embedding: false,
                is_preview: false, is_deprecated: false, deprecation_date: None,
            })
        }).collect()).unwrap_or_default())
    }

    async fn check_health(&self, key: &str) -> Result<KeyHealth> {
        let resp = self.client.get("https://api.groq.com/openai/v1/models")
            .bearer_auth(key).send().await?;
        Ok(KeyHealth { valid: resp.status().is_success(), tier: KeyTier::Free,
            quota_remaining_pct: Some(if resp.status().is_success() { 100.0 } else { 0.0 }),
            reset_at: None, error: None })
    }

    async fn generate(&self, req: &GenerateRequest, key: &str) -> Result<GenerateResponse> {
        let messages: Vec<serde_json::Value> = req.messages.iter().map(|m| {
            serde_json::json!({"role": &m.role, "content": &m.content})
        }).collect();
        let start = std::time::Instant::now();
        let resp = self.client.post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(key)
            .json(&serde_json::json!({"model": &req.model, "messages": messages,
                "max_tokens": req.max_tokens.unwrap_or(4096),
                "temperature": req.temperature.unwrap_or(0.7)}))
            .send().await?;
        let latency = start.elapsed().as_millis() as u64;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Groq failed: {}", &body[..body.len().min(500)]);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(GenerateResponse {
            text: body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string(),
            model: req.model.clone(),
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            latency_ms: latency, provider: "groq".to_string(), key_id: String::new(),
        })
    }

    fn estimate_cost(&self, model: &str, _input_tokens: u64, _output_tokens: u64) -> CostEstimate {
        CostEstimate { input_cost_usd: 0.0, output_cost_usd: 0.0, total_cost_usd: 0.0,
            model: model.to_string(), provider: "groq".to_string() }
    }
    fn parse_rate_limit_headers(&self, headers: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> {
        let get = |n: &str| -> Option<u32> { headers.get(n)?.to_str().ok()?.parse().ok() };
        Some(RateLimitInfo { rpm_limit: get("x-ratelimit-limit-requests"),
            rpm_remaining: get("x-ratelimit-remaining-requests"),
            rpd_limit: get("x-ratelimit-limit-requests-day"),
            rpd_remaining: get("x-ratelimit-remaining-requests-day"),
            ..Default::default() })
    }
    fn parse_error_response(&self, status: u16, body: &str) -> ProbeError {
        let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        ProbeError { http_status: status,
            error_type: parsed["error"]["type"].as_str().unwrap_or("unknown").to_string(),
            error_message: parsed["error"]["message"].as_str().unwrap_or(body)[..body.len().min(500)].to_string(),
            quota_metric: None, suggested_action: None, reset_time: None }
    }
}
