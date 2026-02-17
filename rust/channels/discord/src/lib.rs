use common::config::OpenClawConfig;
use common::session::SessionStore;
use serenity::prelude::*;
use std::sync::Arc;
use tracing::{info, error};

pub mod handler;

pub async fn start(config: &OpenClawConfig, sessions: SessionStore) -> Result<(), Box<dyn std::error::Error>> {
    let discord_config = match &config.channels {
        Some(channels) => match &channels.discord {
            Some(dc) => dc,
            None => {
                info!("Discord channel not configured.");
                return Ok(());
            }
        },
        None => {
            info!("Channels not configured.");
            return Ok(());
        }
    };

    if discord_config.enabled == Some(false) {
        info!("Discord channel disabled.");
        return Ok(());
    }

    let token = match &discord_config.token {
        Some(token) => token,
        None => {
            error!("Discord token missing.");
            return Ok(());
        }
    };

    info!("Starting Discord bot...");
    
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    let handler = handler::Handler {
        sessions: sessions.clone(),
    };

    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .await?;

    if let Err(why) = client.start().await {
        error!("Client error: {:?}", why);
    }
    
    Ok(())
}
