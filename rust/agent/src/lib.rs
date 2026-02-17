use common::session::SessionEntry;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, error, debug};

pub mod llm;

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: Value::Null, // TODO
        }
    }
    async fn execute(&self, args: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone)]
pub struct LLMResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[async_trait]
pub trait LLM: Send + Sync {
    async fn complete(&self, system: String, user: String, tools: Vec<ToolDefinition>) -> Result<LLMResponse, Box<dyn std::error::Error + Send + Sync>>;
}

pub struct Agent {
    session: SessionEntry,
    llm: Arc<dyn LLM>,
    tools: Vec<Arc<dyn Tool + Send + Sync>>,
}

impl Agent {
    pub fn new(session: SessionEntry, llm: Arc<dyn LLM>, tools: Vec<Arc<dyn Tool + Send + Sync>>) -> Self {
        Self { session, llm, tools }
    }

    pub async fn run(&self, input: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        info!("Agent running for session {}", self.session.session_id);
        
        let tool_definitions: Vec<ToolDefinition> = self.tools.iter().map(|t| t.definition()).collect();
        
        // 1. Retrieve context
        // Fetch inputs from session extra if available (simplified memory)
        let mut system_prompt = "You are a helpful assistant. Use tools when necessary.".to_string();
        
        if let Some(history) = self.session.extra.get("messages") {
            if let Some(msgs) = history.as_array() {
                 let context: String = msgs.iter().filter_map(|m| {
                     let role = m.get("role")?.as_str()?;
                     let content = m.get("content")?.as_str()?;
                     Some(format!("{}: {}", role, content))
                 }).collect::<Vec<_>>().join("\n");
                 
                 if !context.is_empty() {
                     system_prompt.push_str("\n\nContext:\n");
                     system_prompt.push_str(&context);
                 }
            }
        }
        
        // 2. Call LLM
        let response = self.llm.complete(system_prompt, input.to_string(), tool_definitions).await?;
        
        // 3. Tool execution
        if !response.tool_calls.is_empty() {
             for tool_call in response.tool_calls {
                 info!("Agent executing tool: {}", tool_call.name);
                 // Find tool
                 if let Some(tool) = self.tools.iter().find(|t| t.name() == tool_call.name) {
                     // Parse args
                     let args: Value = serde_json::from_str(&tool_call.arguments).unwrap_or(Value::Null);
                     match tool.execute(args).await {
                         Ok(result) => {
                             info!("Tool execution result: {:?}", result);
                             return Ok(format!("Tool {} executed. Result: {}", tool_call.name, result));
                         }
                         Err(e) => {
                             error!("Tool execution failed: {}", e);
                             return Ok(format!("Tool {} failed: {}", tool_call.name, e));
                         }
                     }
                 }
             }
        }
        
        Ok(response.content.unwrap_or_default())
    }
}

pub struct MockLLM;

#[async_trait]
impl LLM for MockLLM {
    async fn complete(&self, _system: String, user: String, _tools: Vec<ToolDefinition>) -> Result<LLMResponse, Box<dyn std::error::Error + Send + Sync>> {
        Ok(LLMResponse {
            content: Some(format!("Mock response to: {}", user)),
            tool_calls: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct WeatherTool;

    #[async_trait]
    impl Tool for WeatherTool {
        fn name(&self) -> &str {
            "weather"
        }
        fn description(&self) -> &str {
            "Get the weather for a location"
        }
        async fn execute(&self, args: Value) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            let location = args["location"].as_str().unwrap_or("unknown");
            Ok(json!({ "temperature": 25, "location": location }))
        }
    }

    struct ToolCallLLM;

    #[async_trait]
    impl LLM for ToolCallLLM {
        async fn complete(&self, _system: String, _user: String, _tools: Vec<ToolDefinition>) -> Result<LLMResponse, Box<dyn std::error::Error + Send + Sync>> {
            // Simulate LLM returning a tool call
            Ok(LLMResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "weather".to_string(),
                    arguments: json!({ "location": "Tokyo" }).to_string(),
                }],
            })
        }
    }

    #[tokio::test]
    async fn test_agent_tool_execution() {
        let session = SessionEntry {
            session_id: "test".to_string(),
            updated_at: 0,
            session_file: None,
            spawned_by: None,
            spawn_depth: None,
            system_sent: None,
            chat_type: None,
            provider_override: None,
            model_override: None,
            label: None,
            display_name: None,
            channel: None,
            group_id: None,
            subject: None,
            group_channel: None,
            origin: None,
            delivery_context: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            extra: Default::default(),
        };

        let llm = Arc::new(ToolCallLLM);
        let tools: Vec<Arc<dyn Tool + Send + Sync>> = vec![Arc::new(WeatherTool)];
        let agent = Agent::new(session, llm, tools);

        let response = agent.run("What's the weather in Tokyo?").await.unwrap();
        
        println!("Agent Response: {}", response);
        assert!(response.contains("Tool weather executed"));
        assert!(response.contains("Tokyo"));
        assert!(response.contains("25"));
    }

    #[tokio::test]
    async fn test_agent_context_retrieval() {
        let mut session = SessionEntry {
            session_id: "test_context".to_string(),
            updated_at: 0,
            session_file: None,
            spawned_by: None,
            spawn_depth: None,
            system_sent: None,
            chat_type: None,
            provider_override: None,
            model_override: None,
            label: None,
            display_name: None,
            channel: None,
            group_id: None,
            subject: None,
            group_channel: None,
            origin: None,
            delivery_context: None,
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            extra: Default::default(),
        };
        
        // Add fake history
        let history = json!([
            { "role": "user", "content": "My name is Alice." },
            { "role": "assistant", "content": "Hello Alice." }
        ]);
        session.extra.insert("messages".to_string(), history);

        // Mock LLM that asserts system prompt contains context
        struct ContextCheckLLM;
        #[async_trait]
        impl LLM for ContextCheckLLM {
            async fn complete(&self, system: String, _user: String, _tools: Vec<ToolDefinition>) -> Result<LLMResponse, Box<dyn std::error::Error + Send + Sync>> {
                assert!(system.contains("My name is Alice"));
                assert!(system.contains("Hello Alice"));
                Ok(LLMResponse { content: Some("Checked".to_string()), tool_calls: vec![] })
            }
        }

        let agent = Agent::new(session, Arc::new(ContextCheckLLM), vec![]);
        agent.run("Who am I?").await.unwrap();
    }
}
