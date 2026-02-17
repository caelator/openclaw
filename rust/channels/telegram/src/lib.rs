pub mod handler;
use common::config::OpenClawConfig;
use common::session::{SessionEntry, SessionStore};
use teloxide::prelude::*;
use tracing::{info, error};


pub async fn start(config: &OpenClawConfig, sessions: SessionStore) -> Result<(), Box<dyn std::error::Error>> {
    let telegram_config = match &config.channels {
        Some(channels) => match &channels.telegram {
            Some(tg) => tg,
            None => {
                info!("Telegram channel not configured.");
                return Ok(());
            }
        },
        None => {
            info!("Channels not configured.");
            return Ok(());
        }
    };

    if telegram_config.enabled == Some(false) {
        info!("Telegram channel disabled.");
        return Ok(());
    }

    let token = match &telegram_config.bot_token {
        Some(token) => token,
        None => {
            error!("Telegram bot token missing.");
            return Ok(());
        }
    };

    // Message handler loop
    info!("Starting Telegram bot...");
    let bot = Bot::new(token);

    let sessions = sessions.clone();
    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let sessions = sessions.clone();
        async move {
            if let Err(e) = handler::handle_message(bot, msg, sessions).await {
                error!("Error handling message: {}", e);
            }
            Ok(())
        }
    }).await;

    Ok(())
}
