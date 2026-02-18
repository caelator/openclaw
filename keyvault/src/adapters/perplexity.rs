//! Perplexity adapter — OpenAI-compatible with search grounding.

use anyhow::Result;
use async_trait::async_trait;
use super::*;

pub struct PerplexityAdapter { client: reqwest::Client }
impl PerplexityAdapter { pub fn new() -> Self { Self { client: reqwest::Client::new() } } }

#[async_trait]
impl LLMAdapter for PerplexityAdapter {
    fn provider_id(&self) -> &str { "perplexity" }
    fn display_name(&self) -> &str { "Perplexity" }

    async fn list_models(&self, _key: &str) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo { id: "sonar-pro".into(), display_name: "Sonar Pro".into(),
                provider: "perplexity".into(), input_token_limit: 200_000, output_token_limit: 8_192,
                supports_generation: true, supports_embedding: false,
                is_preview: false, is_deprecated: false, deprecation_date: None },
            ModelInfo { id: "sonar".into(), display_name: "Sonar".into(),
                provider: "perplexity".into(), input_token_limit: 128_000, output_token_limit: 8_192,
                supports_generation: true, supports_embedding: false,
                is_preview: false, is_deprecated: false, deprecation_date: None },
        ])
    }

    async fn check_health(&self, key: &str) -> Result<KeyHealth> {
        let resp = self.client.post("https://api.perplexity.ai/chat/completions")
            .bearer_auth(key)
            .json(&serde_json::json!({"model": "sonar", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 1}))
            .send().await?;
        Ok(KeyHealth { valid: resp.status().is_success(), tier: KeyTier::Paid,
            quota_remaining_pct: Some(if resp.status().is_success() { 100.0 } else { 0.0 }),
            reset_at: None, error: None })
    }

    async fn generate(&self, req: &GenerateRequest, key: &str) -> Result<GenerateResponse> {
        let messages: Vec<serde_json::Value> = req.messages.iter().map(|m| {
            serde_json::json!({"role": &m.role, "content": &m.content})
        }).collect();
        let start = std::time::Instant::now();
        let resp = self.client.post("https://api.perplexity.ai/chat/completions")
            .bearer_auth(key)
            .json(&serde_json::json!({"model": &req.model, "messages": messages}))
            .send().await?;
        let latency = start.elapsed().as_millis() as u64;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Perplexity failed: {}", &body[..body.len().min(500)]);
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(GenerateResponse {
            text: body["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string(),
            model: req.model.clone(),
            input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            latency_ms: latency, provider: "perplexity".to_string(), key_id: String::new(),
        })
    }

    fn estimate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> CostEstimate {
        let (ir, or) = match model {
            "sonar-pro" => (3.0 / 1e6, 15.0 / 1e6),
            _ => (1.0 / 1e6, 1.0 / 1e6),
        };
        CostEstimate { input_cost_usd: input_tokens as f64 * ir, output_cost_usd: output_tokens as f64 * or,
            total_cost_usd: (input_tokens as f64 * ir) + (output_tokens as f64 * or),
            model: model.to_string(), provider: "perplexity".to_string() }
    }

    fn parse_rate_limit_headers(&self, _h: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> { None }
    fn parse_error_response(&self, status: u16, body: &str) -> ProbeError {
        ProbeError { http_status: status, error_type: "unknown".into(),
            error_message: body[..body.len().min(500)].to_string(),
            quota_metric: None, suggested_action: None, reset_time: None }
    }
}
