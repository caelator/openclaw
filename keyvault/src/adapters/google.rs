//! Google Gemini adapter — supports all Gemini models via the
//! generativelanguage REST API.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

use super::{
    CostEstimate, GenerateRequest, GenerateResponse, KeyHealth, KeyTier,
    LLMAdapter, ModelInfo, ProbeError, RateLimitInfo,
};

pub struct GoogleAdapter {
    client: reqwest::Client,
}

impl GoogleAdapter {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LLMAdapter for GoogleAdapter {
    fn provider_id(&self) -> &str {
        "google"
    }

    fn display_name(&self) -> &str {
        "Google Gemini"
    }

    async fn list_models(&self, key: &str) -> Result<Vec<ModelInfo>> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models?key={}",
            key
        );
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Google list_models failed ({}): {}", status, &body[..body.len().min(500)]);
        }
        let body: Value = resp.json().await?;
        let models = body["models"]
            .as_array()
            .context("No models array in response")?;

        Ok(models
            .iter()
            .filter_map(|m| {
                let id = m["name"].as_str()?.strip_prefix("models/")?;
                let methods = m["supportedGenerationMethods"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                Some(ModelInfo {
                    id: id.to_string(),
                    display_name: m["displayName"]
                        .as_str()
                        .unwrap_or(id)
                        .to_string(),
                    provider: "google".to_string(),
                    input_token_limit: m["inputTokenLimit"].as_u64().unwrap_or(0),
                    output_token_limit: m["outputTokenLimit"].as_u64().unwrap_or(0),
                    supports_generation: methods.iter().any(|m| m == "generateContent"),
                    supports_embedding: methods.iter().any(|m| m == "embedContent"),
                    is_preview: id.contains("preview") || id.contains("exp"),
                    is_deprecated: false,
                    deprecation_date: None,
                })
            })
            .collect())
    }

    async fn check_health(&self, key: &str) -> Result<KeyHealth> {
        // Minimal probe: list models (free, no tokens consumed).
        // If that succeeds, try a 1-token generation to check quota.
        let models_result = self.list_models(key).await;
        if let Err(e) = &models_result {
            let msg = e.to_string();
            return Ok(KeyHealth {
                valid: false,
                tier: KeyTier::Unknown,
                quota_remaining_pct: None,
                reset_at: None,
                error: Some(ProbeError {
                    http_status: 0,
                    error_type: "connection_error".to_string(),
                    error_message: msg,
                    quota_metric: None,
                    suggested_action: None,
                    reset_time: None,
                }),
            });
        }

        // Try a minimal generate to check quota
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash-lite:generateContent?key={}",
            key
        );
        let probe_body = serde_json::json!({
            "contents": [{"parts": [{"text": "hi"}]}],
            "generationConfig": {"maxOutputTokens": 1}
        });

        let resp = self.client.post(&url).json(&probe_body).send().await?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let rate_limits = self.parse_rate_limit_headers(&headers);

        if status == 200 {
            Ok(KeyHealth {
                valid: true,
                tier: if rate_limits.is_some() { KeyTier::Paid } else { KeyTier::Free },
                quota_remaining_pct: Some(100.0),
                reset_at: None,
                error: None,
            })
        } else {
            let body = resp.text().await.unwrap_or_default();
            let probe_error = self.parse_error_response(status, &body);
            let has_quota = status != 429;

            Ok(KeyHealth {
                valid: status != 401 && status != 403,
                tier: KeyTier::Free,
                quota_remaining_pct: if has_quota { Some(50.0) } else { Some(0.0) },
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
        let model = &req.model;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model, key
        );

        // Build contents
        let mut contents: Vec<Value> = Vec::new();
        for msg in &req.messages {
            let role = match msg.role.as_str() {
                "user" => "user",
                "assistant" => "model",
                _ => "user",
            };
            contents.push(serde_json::json!({
                "role": role,
                "parts": [{"text": &msg.content}]
            }));
        }

        let mut body = serde_json::json!({ "contents": contents });
        if let Some(sys) = &req.system_prompt {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": sys}]
            });
        }

        let mut gen_config = serde_json::json!({});
        if let Some(t) = req.temperature {
            gen_config["temperature"] = serde_json::json!(t);
        }
        if let Some(m) = req.max_tokens {
            gen_config["maxOutputTokens"] = serde_json::json!(m);
        }
        body["generationConfig"] = gen_config;

        let start = Instant::now();
        let resp = self.client.post(&url).json(&body).send().await?;
        let latency = start.elapsed().as_millis() as u64;

        let status = resp.status().as_u16();
        if status != 200 {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Google generate failed ({}): {}",
                status,
                &err_body[..err_body.len().min(500)]
            );
        }

        let resp_body: Value = resp.json().await?;
        let text = resp_body["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = &resp_body["usageMetadata"];
        let input_tokens = usage["promptTokenCount"].as_u64().unwrap_or(0);
        let output_tokens = usage["candidatesTokenCount"].as_u64().unwrap_or(0);

        Ok(GenerateResponse {
            text,
            model: model.clone(),
            input_tokens,
            output_tokens,
            latency_ms: latency,
            provider: "google".to_string(),
            key_id: String::new(), // Filled by pool manager
        })
    }

    fn estimate_cost(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> CostEstimate {
        // Free tier = $0. Paid tier pricing varies by model.
        // For now, assume free tier.
        let (input_rate, output_rate) = match model {
            m if m.contains("2.5-pro") => (1.25 / 1_000_000.0, 10.0 / 1_000_000.0),
            m if m.contains("2.5-flash") && m.contains("lite") => (0.0, 0.0),
            m if m.contains("2.5-flash") => (0.15 / 1_000_000.0, 0.60 / 1_000_000.0),
            m if m.contains("2.0-flash") && m.contains("lite") => (0.0, 0.0),
            m if m.contains("2.0-flash") => (0.10 / 1_000_000.0, 0.40 / 1_000_000.0),
            m if m.contains("3-pro") => (1.25 / 1_000_000.0, 10.0 / 1_000_000.0),
            m if m.contains("3-flash") => (0.15 / 1_000_000.0, 0.60 / 1_000_000.0),
            _ => (0.0, 0.0),
        };
        let input_cost = input_tokens as f64 * input_rate;
        let output_cost = output_tokens as f64 * output_rate;
        CostEstimate {
            input_cost_usd: input_cost,
            output_cost_usd: output_cost,
            total_cost_usd: input_cost + output_cost,
            model: model.to_string(),
            provider: "google".to_string(),
        }
    }

    fn parse_rate_limit_headers(
        &self,
        _headers: &reqwest::header::HeaderMap,
    ) -> Option<RateLimitInfo> {
        // Google doesn't return standard rate limit headers on free tier.
        // On paid tier, they appear in x-ratelimit-* headers.
        None
    }

    fn parse_error_response(&self, status: u16, body: &str) -> ProbeError {
        let parsed: Value = serde_json::from_str(body).unwrap_or_default();
        let error = &parsed["error"];

        let error_type = error["status"]
            .as_str()
            .unwrap_or("UNKNOWN")
            .to_string();

        let error_message = error["message"]
            .as_str()
            .unwrap_or(body)
            .to_string();

        // Try to extract quota metric from error message
        let quota_metric = if error_message.contains("Quota exceeded for metric:") {
            error_message
                .split("Quota exceeded for metric:")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .map(|s| s.trim().to_string())
        } else {
            None
        };

        let suggested_action = if status == 429 {
            Some("Wait for quota reset or switch to another key/project".to_string())
        } else if status == 403 {
            Some("Enable billing or check API key permissions".to_string())
        } else {
            None
        };

        ProbeError {
            http_status: status,
            error_type,
            error_message: error_message[..error_message.len().min(500)].to_string(),
            quota_metric,
            suggested_action,
            reset_time: None, // Google doesn't provide explicit reset time
        }
    }
}
