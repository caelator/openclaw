//! DeepSeek adapter — OpenAI-compatible API.

use anyhow::Result;
use async_trait::async_trait;
use super::*;

pub struct DeepSeekAdapter { client: reqwest::Client }
impl DeepSeekAdapter { pub fn new() -> Self { Self { client: reqwest::Client::new() } } }

#[async_trait]
impl LLMAdapter for DeepSeekAdapter {
    fn provider_id(&self) -> &str { "deepseek" }
    fn display_name(&self) -> &str { "DeepSeek" }

    async fn list_models(&self, key: &str) -> Result<Vec<ModelInfo>> {
        let resp = self.client.get("https://api.deepseek.com/models")
            .bearer_auth(key).send().await?;
        if !resp.status().is_success() {
            return Ok(vec![
                ModelInfo { id: "deepseek-chat".into(), display_name: "DeepSeek Chat".into(),
                    provider: "deepseek".into(), input_token_limit: 64_000, output_token_limit: 8_192,
                    supports_generation: true, supports_embedding: false,
                    is_preview: false, is_deprecated: false, deprecation_date: None },
                ModelInfo { id: "deepseek-reasoner".into(), display_name: "DeepSeek Reasoner (R1)".into(),
                    provider: "deepseek".into(), input_token_limit: 64_000, output_token_limit: 8_192,
                    supports_generation: true, supports_embedding: false,
                    is_preview: false, is_deprecated: false, deprecation_date: None },
            ]);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(body["data"].as_array().map(|arr| arr.iter().filter_map(|m| {
            Some(ModelInfo {
                id: m["id"].as_str()?.to_string(), display_name: m["id"].as_str()?.to_string(),
                provider: "deepseek".to_string(), input_token_limit: 64_000, output_token_limit: 8_192,
                supports_generation: true, supports_embedding: false,
                is_preview: false, is_deprecated: false, deprecation_date: None,
            })
        }).collect()).unwrap_or_default())
    }

    async fn check_health(&self, key: &str) -> Result<KeyHealth> {
        let resp = self.client.get("https://api.deepseek.com/models")
            .bearer_auth(key).send().await?;
        Ok(KeyHealth { valid: resp.status().is_success(), tier: KeyTier::Paid,
            quota_remaining_pct: Some(if resp.status().is_success() { 100.0 } else { 0.0 }),
            reset_at: None, error: None })
    }

    async fn generate(&self, req: &GenerateRequest, key: &str) -> Result<GenerateResponse> {
        let messages: Vec<serde_json::Value> = req.messages.iter().map(|m| {
            serde_json::json!({"role": &m.role, "content": &m.content})
        }).collect();
        let start = std::time::Instant::now();
        let resp = self.client.post("https://api.deepseek.com/chat/completions")
            .bearer_auth(key)
            .json(&serde_json::json!({"model": &req.model, "messages": messages,
                "max_tokens": req.max_tokens.unwrap_or(4096)}))
            .send().await?;
        let latency = start.elapsed().as_millis() as u64;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("DeepSeek failed: {}", &body[..body.len().min(500)]);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(GenerateResponse {
            text: body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string(),
            model: req.model.clone(),
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            latency_ms: latency, provider: "deepseek".to_string(), key_id: String::new(),
        })
    }

    fn estimate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> CostEstimate {
        let (ir, or) = (0.14 / 1e6, 0.28 / 1e6); // DeepSeek is very cheap
        CostEstimate { input_cost_usd: input_tokens as f64 * ir, output_cost_usd: output_tokens as f64 * or,
            total_cost_usd: (input_tokens as f64 * ir) + (output_tokens as f64 * or),
            model: model.to_string(), provider: "deepseek".to_string() }
    }

    fn parse_rate_limit_headers(&self, _h: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> { None }
    fn parse_error_response(&self, status: u16, body: &str) -> ProbeError {
        ProbeError { http_status: status, error_type: "unknown".into(),
            error_message: body[..body.len().min(500)].to_string(),
            quota_metric: None, suggested_action: None, reset_time: None }
    }
}
