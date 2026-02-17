use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawConfig {
    pub channels: Option<ChannelsConfig>,
    // Add other fields as needed
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChannelsConfig {
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    pub whatsapp: Option<WhatsAppConfig>,
    // Add other channels as needed
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WhatsAppConfig {
    pub enabled: Option<bool>,
    pub self_chat_mode: Option<bool>,
    pub allow_from: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscordConfig {
    pub enabled: Option<bool>,
    pub token: Option<String>,
    pub allow_from: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TelegramConfig {
    pub enabled: Option<bool>,
    pub bot_token: Option<String>,
    pub allow_from: Option<Vec<String>>,
    // Add other fields as needed
}
