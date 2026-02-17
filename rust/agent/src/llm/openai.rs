use crate::{LLM, LLMResponse, ToolCall, ToolDefinition};
use async_trait::async_trait;
use async_openai::{
    types::{CreateChatCompletionRequestArgs, ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs, ChatCompletionRequestMessage, ChatCompletionTool, ChatCompletionToolType, ChatCompletionToolArgs, FunctionObjectArgs},
    Client,
};
use tracing::{info, error};
use std::sync::Arc;
use serde_json::Value; // Keep usage if needed, but ToolDefinition is used now

pub struct OpenAIClient {
    client: Client<async_openai::config::OpenAIConfig>,
    model: String,
}

impl OpenAIClient {
    pub fn new(api_key: &str, model: &str) -> Self {
        let config = async_openai::config::OpenAIConfig::new().with_api_key(api_key);
        let client = Client::with_config(config);
        Self {
            client,
            model: model.to_string(),
        }
    }
}

#[async_trait]
impl LLM for OpenAIClient {
    async fn complete(&self, system: String, user: String, tools: Vec<ToolDefinition>) -> Result<LLMResponse, Box<dyn std::error::Error + Send + Sync>> {
        let messages: Vec<ChatCompletionRequestMessage> = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(system)
                .build()?
                .into(),
            ChatCompletionRequestUserMessageArgs::default()
                .content(user)
                .build()?
                .into(),
        ];

        let mut request_builder = CreateChatCompletionRequestArgs::default();
        request_builder.model(&self.model).messages(messages);

        if !tools.is_empty() {
             let openai_tools: Vec<ChatCompletionTool> = tools.iter().map(|t| {
                 ChatCompletionToolArgs::default()
                    .r#type(ChatCompletionToolType::Function)
                    .function(
                        FunctionObjectArgs::default()
                            .name(&t.name)
                            .description(&t.description)
                            // TODO: Add schema to Tool trait
                            // .parameters(...)
                            .build().unwrap()
                    )
                    .build().unwrap()
             }).collect();
             request_builder.tools(openai_tools);
        }

        let request = request_builder.build()?;
        let response = self.client.chat().create(request).await?;

        if let Some(choice) = response.choices.first() {
            let tool_calls = if let Some(calls) = &choice.message.tool_calls {
                calls.iter().map(|c| ToolCall {
                    id: c.id.clone(),
                    name: c.function.name.clone(),
                    arguments: c.function.arguments.clone(),
                }).collect()
            } else {
                vec![]
            };

            return Ok(LLMResponse {
                content: choice.message.content.clone(),
                tool_calls,
            });
        }

        Ok(LLMResponse { content: None, tool_calls: vec![] })
    }
}
