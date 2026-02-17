use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use tracing::{info, error, warn};
use common::session::{SessionEntry, SessionStore};
use uuid::Uuid;
use chrono::Utc;

pub struct Handler {
    pub sessions: SessionStore,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        }

        let chat_id = msg.channel_id;
        let session_key = format!("discord:{}", chat_id);
        
        info!("Received message from {}: {}", msg.author.name, msg.content);

        // Get or create session
        let mut session: Option<SessionEntry> = self.sessions.get(&session_key).await;
        
        if session.is_none() {
            info!("Creating new session for {}", session_key);
            let display_name = msg.author.name.clone();

            let new_session = SessionEntry {
                session_id: Uuid::new_v4().to_string(),
                updated_at: Utc::now().timestamp_millis(),
                session_file: None,
                spawned_by: None,
                spawn_depth: None,
                system_sent: None,
                chat_type: Some("direct".to_string()), // TODO: Distinguish DMs vs Guilds
                provider_override: None,
                model_override: None,
                label: Some(display_name.clone()),
                display_name: Some(display_name),
                channel: Some("discord".to_string()),
                group_id: msg.guild_id.map(|g| g.to_string()),
                subject: None,
                group_channel: None,
                origin: None, // TODO
                delivery_context: None, // TODO 
                input_tokens: Some(0),
                output_tokens: Some(0),
                total_tokens: Some(0),
                extra: std::collections::HashMap::new(),
            };
            
            if let Err(e) = self.sessions.update(session_key.clone(), new_session.clone()).await {
                error!("Failed to create session: {}", e);
            } else {
                session = Some(new_session);
            }
        }

        // Echo for now
        // Instantiate Agent
        let llm: std::sync::Arc<dyn agent::LLM> = match std::env::var("OPENAI_API_KEY") {
            Ok(key) => {
                info!("Using OpenAI Client with model gpt-4o");
                std::sync::Arc::new(agent::llm::openai::OpenAIClient::new(&key, "gpt-4o"))
            },
            Err(_) => {
                warn!("OPENAI_API_KEY not found, using MockLLM");
                std::sync::Arc::new(agent::MockLLM)
            }
        };
        let tools = vec![];

        if let Some(sess) = session {
            let agent = agent::Agent::new(sess, llm, tools);

            match agent.run(&msg.content).await {
                Ok(response) => {
                    if let Err(why) = msg.channel_id.say(&ctx.http, response).await {
                        error!("Error sending message: {:?}", why);
                    }
                }
                Err(e) => {
                    error!("Agent execution failed: {}", e);
                    if let Err(why) = msg.channel_id.say(&ctx.http, "I encountered an error.").await {
                         error!("Error sending error message: {:?}", why);
                    }
                }
            }
        }
    }

    async fn ready(&self, _: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
    }
}
