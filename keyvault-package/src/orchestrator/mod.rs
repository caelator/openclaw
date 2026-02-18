//! Orchestrator brain — uses a dedicated Gemini key to classify
//! incoming tasks and route to the optimal model/provider.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::adapters::{GenerateRequest, LLMAdapter, Message};
use crate::vault::store::KeyStore;

/// The routing decision made by the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub task_type: String,
    pub complexity: String,
    pub recommended_provider: String,
    pub recommended_model: String,
    pub fallback_chain: Vec<(String, String)>, // (provider, model) pairs
    pub estimated_tokens: u64,
    pub rationale: String,
}

/// Fast-path classifier — regex-based, no LLM call.
/// Returns a routing decision for simple cases.
pub fn fast_classify(req: &GenerateRequest) -> Option<RoutingDecision> {
    let content = req.messages.last()
        .map(|m| m.content.to_lowercase())
        .unwrap_or_default();

    // Simple heuristics
    if content.len() < 50 {
        return Some(RoutingDecision {
            task_type: "chat".to_string(),
            complexity: "low".to_string(),
            recommended_provider: "google".to_string(),
            recommended_model: "gemini-2.5-flash-lite".to_string(),
            fallback_chain: vec![
                ("google".to_string(), "gemini-2.0-flash".to_string()),
            ],
            estimated_tokens: 100,
            rationale: "Short message — flash-lite is sufficient".to_string(),
        });
    }

    if content.contains("```") || content.contains("write code") || content.contains("implement")
        || content.contains("function") || content.contains("class ") || content.contains("fn ") {
        return Some(RoutingDecision {
            task_type: "code_gen".to_string(),
            complexity: if content.len() > 2000 { "high" } else { "medium" }.to_string(),
            recommended_provider: "google".to_string(),
            recommended_model: if content.len() > 2000 {
                "gemini-2.5-pro".to_string()
            } else {
                "gemini-2.5-flash".to_string()
            },
            fallback_chain: vec![
                ("google".to_string(), "gemini-3-flash-preview".to_string()),
                ("anthropic".to_string(), "claude-sonnet-4-20250514".to_string()),
            ],
            estimated_tokens: (content.len() as u64 / 4) + 2000,
            rationale: "Code generation detected".to_string(),
        });
    }

    if content.contains("analyze") || content.contains("review") || content.contains("explain")
        || content.contains("why") || content.contains("how does") {
        return Some(RoutingDecision {
            task_type: "analysis".to_string(),
            complexity: "medium".to_string(),
            recommended_provider: "google".to_string(),
            recommended_model: "gemini-2.5-flash".to_string(),
            fallback_chain: vec![
                ("google".to_string(), "gemini-3-flash-preview".to_string()),
            ],
            estimated_tokens: (content.len() as u64 / 4) + 1000,
            rationale: "Analysis/reasoning task".to_string(),
        });
    }

    None // Needs LLM classification
}

/// Use the orchestrator brain (dedicated flash-lite key) to classify a task.
pub async fn llm_classify(
    store: &KeyStore,
    adapter: &dyn LLMAdapter,
    req: &GenerateRequest,
) -> Result<RoutingDecision> {
    let orchestrator_key_id = store.get_orchestrator_key("google")?
        .ok_or_else(|| anyhow::anyhow!("No orchestrator key configured"))?;
    let raw_key = store.decrypt_key(&orchestrator_key_id)?;

    // Build a classification prompt (compressed — only first/last 200 chars)
    let content_summary = req.messages.last()
        .map(|m| {
            let c = &m.content;
            if c.len() > 400 {
                format!("{}...{}", &c[..200], &c[c.len()-200..])
            } else {
                c.clone()
            }
        })
        .unwrap_or_default();

    let classify_req = GenerateRequest {
        model: "gemini-2.5-flash-lite".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: format!(
                r#"Classify this AI request and recommend the best model. Respond ONLY in JSON.

Request summary: "{}"

Available models:
- gemini-3-flash-preview (fastest, search)
- gemini-2.5-pro (deep reasoning)
- gemini-2.5-flash (hybrid reasoning)
- gemini-2.5-flash-lite (cheapest)
- gemini-2.0-flash (balanced)
- claude-sonnet-4 (code quality)

JSON schema:
{{"task_type":"code_gen|analysis|chat|search|creative","complexity":"low|medium|high","recommended_provider":"google|anthropic","recommended_model":"...","fallback_chain":[["provider","model"]],"estimated_tokens":N,"rationale":"..."}}"#,
                content_summary
            ),
        }],
        temperature: Some(0.0),
        max_tokens: Some(300),
        system_prompt: Some("You are a task classifier. Output only valid JSON.".to_string()),
    };

    let resp = adapter.generate(&classify_req, &raw_key).await?;

    // Parse the JSON response
    let decision: RoutingDecision = serde_json::from_str(&resp.text)
        .or_else(|_| {
            // Try to extract JSON from markdown code block
            let json_str = resp.text
                .trim()
                .strip_prefix("```json")
                .or_else(|| resp.text.trim().strip_prefix("```"))
                .unwrap_or(&resp.text)
                .strip_suffix("```")
                .unwrap_or(&resp.text)
                .trim();
            serde_json::from_str(json_str)
        })
        .unwrap_or_else(|_| {
            // Fallback: default routing
            RoutingDecision {
                task_type: "unknown".to_string(),
                complexity: "medium".to_string(),
                recommended_provider: "google".to_string(),
                recommended_model: "gemini-2.5-flash".to_string(),
                fallback_chain: vec![
                    ("google".to_string(), "gemini-3-flash-preview".to_string()),
                ],
                estimated_tokens: 2000,
                rationale: format!("LLM classification failed, defaulting. Raw: {}", &resp.text[..resp.text.len().min(100)]),
            }
        });

    tracing::info!(
        task_type = %decision.task_type,
        complexity = %decision.complexity,
        model = %decision.recommended_model,
        rationale = %decision.rationale,
        "🧠 Orchestrator routing decision"
    );

    Ok(decision)
}
