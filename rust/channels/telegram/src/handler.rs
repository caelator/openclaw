use common::session::{SessionEntry, SessionStore};
use teloxide::prelude::*;
use tracing::{info, warn, error};
use uuid::Uuid;
use chrono::Utc;

pub async fn handle_message(bot: Bot, msg: Message, sessions: SessionStore) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chat_id = msg.chat.id;
    let session_key = format!("telegram:{}", chat_id);
    
    // Get or create session
    let mut session: Option<SessionEntry> = sessions.get(&session_key).await;
    
    if session.is_none() {
        info!("Creating new session for {}", session_key);
        let user = msg.from();
        let display_name = user.map(|u| {
            let mut name = u.first_name.clone();
            if let Some(last) = &u.last_name {
                name.push_str(" ");
                name.push_str(last);
            }
            name
        }).unwrap_or_else(|| format!("Telegram User {}", chat_id));

        let new_session = SessionEntry {
            session_id: Uuid::new_v4().to_string(),
            updated_at: Utc::now().timestamp_millis(),
            session_file: None,
            spawned_by: None,
            spawn_depth: None,
            system_sent: None,
            chat_type: Some("direct".to_string()),
            provider_override: None,
            model_override: None,
            label: Some(display_name.clone()),
            display_name: Some(display_name),
            channel: Some("telegram".to_string()),
            group_id: None,
            subject: None,
            group_channel: None,
            origin: None,
            delivery_context: None, 
            input_tokens: Some(0),
            output_tokens: Some(0),
            total_tokens: Some(0),
            extra: std::collections::HashMap::new(),
            // TODO: Populate origin and delivery context correctly
        };
        
        if let Err(e) = sessions.update(session_key.clone(), new_session.clone()).await {
            error!("Failed to create session: {}", e);
        } else {
            session = Some(new_session);
        }
    }

    if let Some(text) = msg.text() {
        info!("Received message from {}: {}", chat_id, text);
        
        // Instantiate Agent (Ephemeral for now)
        // Check for OPENAI_API_KEY
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
        
        // We know session is Some here because of the logic above
        if let Some(sess) = session {
             let agent = agent::Agent::new(sess, llm, tools);
             
             match agent.run(text).await {
                 Ok(response) => {
                     bot.send_message(chat_id, response).await?;
                 }
                 Err(e) => {
                     error!("Agent execution failed: {}", e);
                     bot.send_message(chat_id, "I encountered an error processing your request.").await?;
                 }
             }
        }
    } else {
        warn!("Received non-text message from {}", chat_id);
        bot.send_message(chat_id, "I can only handle text messages for now.").await?;
    }

    Ok(())
}
