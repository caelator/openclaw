//! Anthropic Claude adapter.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

use super::{
    CostEstimate, GenerateRequest, GenerateResponse, KeyHealth, KeyTier,
    LLMAdapter, ModelInfo, ProbeError, RateLimitInfo,
};

pub struct AnthropicAdapter {
    client: reqwest::Client,
}

impl AnthropicAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LLMAdapter for AnthropicAdapter {
    fn provider_id(&self) -> &str { "anthropic" }
    fn display_name(&self) -> &str { "Anthropic Claude" }

    async fn list_models(&self, key: &str) -> Result<Vec<ModelInfo>> {
        let resp = self.client
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?;

        if !resp.status().is_success() {
            // Anthropic may not expose /models publicly; return known models.
            return Ok(Self::known_models());
        }

        let body: Value = resp.json().await?;
        let models = body["data"].as_array();
        match models {
            Some(arr) => Ok(arr.iter().filter_map(|m| {
                let id = m["id"].as_str()?;
                Some(ModelInfo {
                    id: id.to_string(),
                    display_name: m["display_name"].as_str().unwrap_or(id).to_string(),
                    provider: "anthropic".to_string(),
                    input_token_limit: 200_000,
                    output_token_limit: m["max_output_tokens"].as_u64().unwrap_or(8192),
                    supports_generation: true,
                    supports_embedding: false,
                    is_preview: id.contains("preview"),
                    is_deprecated: false,
                    deprecation_date: None,
                })
            }).collect()),
            None => Ok(Self::known_models()),
        }
    }

    async fn check_health(&self, key: &str) -> Result<KeyHealth> {
        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": "claude-sonnet-4-20250514",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .send()
            .await?;

        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let rate_limits = self.parse_rate_limit_headers(&headers);

        if status == 200 {
            let quota_pct = rate_limits.as_ref()
                .and_then(|rl| {
                    rl.rpm_remaining.zip(rl.rpm_limit)
                        .map(|(rem, lim)| (rem as f64 / lim as f64) * 100.0)
                });
            Ok(KeyHealth {
                valid: true,
                tier: KeyTier::Paid,
                quota_remaining_pct: quota_pct.or(Some(100.0)),
                reset_at: rate_limits.as_ref().and_then(|rl| rl.reset_at),
                error: None,
            })
        } else {
            let body = resp.text().await.unwrap_or_default();
            let probe_error = self.parse_error_response(status, &body);
            Ok(KeyHealth {
                valid: status != 401,
                tier: KeyTier::Paid,
                quota_remaining_pct: if status == 429 { Some(0.0) } else { Some(50.0) },
                reset_at: probe_error.reset_time,
                error: Some(probe_error),
            })
        }
    }

    async fn generate(
        &self,
        req: &GenerateRequest,
        key: &str,
    ) -> Result<GenerateResponse> {
        let mut messages: Vec<Value> = Vec::new();
        for msg in &req.messages {
            if msg.role != "system" {
                messages.push(serde_json::json!({
                    "role": &msg.role,
                    "content": &msg.content
                }));
            }
        }

        let mut body = serde_json::json!({
            "model": &req.model,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "messages": messages,
        });

        // Use system prompt if provided
        let sys = req.system_prompt.as_deref()
            .or_else(|| req.messages.iter().find(|m| m.role == "system").map(|m| m.content.as_str()));
        if let Some(s) = sys {
            body["system"] = serde_json::json!(s);
        }
        if let Some(t) = req.temperature {
            body["temperature"] = serde_json::json!(t);
        }

        let start = Instant::now();
        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        let latency = start.elapsed().as_millis() as u64;

        let status = resp.status().as_u16();
        if status != 200 {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic generate failed ({}): {}", status, &err_body[..err_body.len().min(500)]);
        }

        let resp_body: Value = resp.json().await?;
        let text = resp_body["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = &resp_body["usage"];
        Ok(GenerateResponse {
            text,
            model: req.model.clone(),
            input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
            latency_ms: latency,
            provider: "anthropic".to_string(),
            key_id: String::new(),
        })
    }

    fn estimate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> CostEstimate {
        let (input_rate, output_rate) = match model {
            m if m.contains("opus") => (15.0 / 1_000_000.0, 75.0 / 1_000_000.0),
            m if m.contains("sonnet") => (3.0 / 1_000_000.0, 15.0 / 1_000_000.0),
            m if m.contains("haiku") => (0.25 / 1_000_000.0, 1.25 / 1_000_000.0),
            _ => (3.0 / 1_000_000.0, 15.0 / 1_000_000.0),
        };
        CostEstimate {
            input_cost_usd: input_tokens as f64 * input_rate,
            output_cost_usd: output_tokens as f64 * output_rate,
            total_cost_usd: (input_tokens as f64 * input_rate) + (output_tokens as f64 * output_rate),
            model: model.to_string(),
            provider: "anthropic".to_string(),
        }
    }

    fn parse_rate_limit_headers(&self, headers: &reqwest::header::HeaderMap) -> Option<RateLimitInfo> {
        let get_u32 = |name: &str| -> Option<u32> {
            headers.get(name)?.to_str().ok()?.parse().ok()
        };
        let get_u64 = |name: &str| -> Option<u64> {
            headers.get(name)?.to_str().ok()?.parse().ok()
        };

        let rpm_limit = get_u32("x-ratelimit-limit-requests");
        let rpm_remaining = get_u32("x-ratelimit-remaining-requests");
        let tpm_limit = get_u64("x-ratelimit-limit-tokens");
        let tpm_remaining = get_u64("x-ratelimit-remaining-tokens");

        let reset_at = headers.get("x-ratelimit-reset-requests")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        if rpm_limit.is_some() || tpm_limit.is_some() {
            Some(RateLimitInfo {
                rpm_limit,
                rpm_remaining,
                rpd_limit: None,
                rpd_remaining: None,
                tpm_limit,
                tpm_remaining,
                reset_at,
                retry_after_secs: None,
            })
        } else {
            None
        }
    }

    fn parse_error_response(&self, status: u16, body: &str) -> ProbeError {
        let parsed: Value = serde_json::from_str(body).unwrap_or_default();
        let error = &parsed["error"];

        let error_type = error["type"].as_str().unwrap_or("unknown").to_string();
        let error_message = error["message"].as_str().unwrap_or(body).to_string();

        // Parse reset time from message like "You will regain access on 2026-03-01 at 00:00 UTC."
        let reset_time = if error_message.contains("regain access on") {
            error_message
                .split("regain access on ")
                .nth(1)
                .and_then(|s| s.split('.').next())
                .and_then(|s| {
                    // "2026-03-01 at 00:00 UTC" → parse
                    let cleaned = s.replace(" at ", "T").replace(" UTC", ":00Z");
                    chrono::DateTime::parse_from_rfc3339(&cleaned).ok()
                })
                .map(|dt| dt.with_timezone(&chrono::Utc))
        } else {
            None
        };

        ProbeError {
            http_status: status,
            error_type,
            error_message: error_message[..error_message.len().min(500)].to_string(),
            quota_metric: None,
            suggested_action: if status == 429 {
                Some("Wait for rate limit reset".to_string())
            } else {
                None
            },
            reset_time,
        }
    }
}

impl AnthropicAdapter {
    fn known_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "claude-opus-4-20250514".to_string(),
                display_name: "Claude Opus 4".to_string(),
                provider: "anthropic".to_string(),
                input_token_limit: 200_000,
                output_token_limit: 32_768,
                supports_generation: true,
                supports_embedding: false,
                is_preview: false,
                is_deprecated: false,
                deprecation_date: None,
            },
            ModelInfo {
                id: "claude-sonnet-4-20250514".to_string(),
                display_name: "Claude Sonnet 4".to_string(),
                provider: "anthropic".to_string(),
                input_token_limit: 200_000,
                output_token_limit: 16_384,
                supports_generation: true,
                supports_embedding: false,
                is_preview: false,
                is_deprecated: false,
                deprecation_date: None,
            },
        ]
    }
}
