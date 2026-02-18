//! Hegemon KeyVault — Secure AI API key pool daemon.
//!
//! Runs as a system service, listening on a Unix socket for JSON-RPC
//! requests. Manages encrypted API keys, intelligent routing, daily
//! discovery, and usage tracking.
//!
//! Security:
//! - Keys encrypted at rest (AES-256-GCM + Argon2id)
//! - Master key in macOS Keychain
//! - Bearer token auth on all mutating requests
//! - Token file with 0600 permissions (self-healing)
//! - Per-caller rate limiting
//! - Keys never cross socket boundary

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, error};

mod adapters;
mod auth;
mod discovery;
mod orchestrator;
mod pool;
mod server;
mod vault;

use adapters::LLMAdapter;
use vault::store::KeyStore;

/// Configuration loaded from args or defaults.
struct Config {
    data_dir: PathBuf,
    db_path: PathBuf,
    socket_path: PathBuf,
    discovery_interval_hours: u64,
}

impl Config {
    fn from_env() -> Self {
        let home = dirs::home_dir().expect("Cannot determine home directory");
        let data_dir = home.join(".openclaw");

        Self {
            db_path: data_dir.join("keyvault.db"),
            socket_path: data_dir.join("keyvault.sock"),
            discovery_interval_hours: 24,
            data_dir,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (structured logs)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "keyvault=info".into()),
        )
        .with_target(false)
        .init();

    info!("🔑 Hegemon KeyVault v{}", env!("CARGO_PKG_VERSION"));
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let config = Config::from_env();

    // Ensure data directory exists
    std::fs::create_dir_all(&config.data_dir)?;

    // ── Auth Bootstrap ──────────────────────────────────────────────
    // Self-healing: Keychain → file reconciliation on every boot.
    // If token file was deleted, it's rewritten from Keychain.
    // If Keychain was cleared, new token generated and written to both.
    let auth_guard = auth::AuthGuard::bootstrap(&config.data_dir)
        .context("Failed to bootstrap auth system")?;

    info!(
        token_file = %auth_guard.token_file_path().display(),
        "🔐 Auth system ready (bearer token required for mutating methods)"
    );

    // ── Master Key ──────────────────────────────────────────────────
    let master_passphrase = std::env::var("KEYVAULT_MASTER_KEY")
        .unwrap_or_else(|_| {
            match get_keychain_passphrase() {
                Ok(pass) => pass,
                Err(_) => {
                    info!("No master key found — generating new one");
                    let key = uuid::Uuid::new_v4().to_string();
                    if let Err(e) = set_keychain_passphrase(&key) {
                        error!("Failed to store master key in Keychain: {}", e);
                    } else {
                        info!("Master key stored in macOS Keychain");
                    }
                    key
                }
            }
        });

    // ── Key Store ───────────────────────────────────────────────────
    let store = Arc::new(
        KeyStore::open(&config.db_path, master_passphrase.into_bytes())
            .context("Failed to open key store")?
    );

    // Report vault status (no seeding — keys added via kv.admin.addKey)
    let key_count = store.list_keys().map(|k| k.len()).unwrap_or(0);
    if key_count == 0 {
        info!("📦 Vault is empty — add keys via: kv.admin.addKey");
        info!("   Example: {{\"jsonrpc\":\"2.0\",\"method\":\"kv.admin.addKey\",\"params\":{{\"id\":\"google-01\",\"provider\":\"google\",\"value\":\"AIza...\",\"role\":\"worker\"}},\"auth\":\"<token>\",\"id\":1}}");
    } else {
        info!("📦 Vault contains {} key(s)", key_count);
    }

    // ── Provider Adapters ───────────────────────────────────────────
    let mut adapter_map: HashMap<String, Box<dyn LLMAdapter>> = HashMap::new();
    adapter_map.insert("google".into(), Box::new(adapters::google::GoogleAdapter::new()));
    adapter_map.insert("anthropic".into(), Box::new(adapters::anthropic::AnthropicAdapter::new()));
    adapter_map.insert("openai".into(), Box::new(adapters::openai::OpenAIAdapter::new()));
    adapter_map.insert("groq".into(), Box::new(adapters::groq::GroqAdapter::new()));
    adapter_map.insert("deepseek".into(), Box::new(adapters::deepseek::DeepSeekAdapter::new()));
    adapter_map.insert("perplexity".into(), Box::new(adapters::perplexity::PerplexityAdapter::new()));

    let adapters = Arc::new(adapter_map);

    // ── Pool Manager ────────────────────────────────────────────────
    let pool = Arc::new(pool::PoolManager::new(
        Arc::clone(&store),
        Arc::clone(&adapters),
    ));

    // ── Discovery Poller ────────────────────────────────────────────
    let poller_store = Arc::clone(&store);
    let poller_adapters = Arc::clone(&adapters);
    tokio::spawn(async move {
        discovery::poller::run_poller(
            poller_store,
            poller_adapters,
            config.discovery_interval_hours,
        ).await;
    });

    // ── JSON-RPC Server ─────────────────────────────────────────────
    let srv = server::Server::new(
        config.socket_path,
        Arc::clone(&store),
        Arc::clone(&pool),
        Arc::clone(&adapters),
        auth_guard,
    );

    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("KeyVault daemon ready — all systems operational");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    srv.run().await?;

    Ok(())
}

/// Try to get the master passphrase from macOS Keychain.
fn get_keychain_passphrase() -> Result<String> {
    use security_framework::passwords::get_generic_password;
    let pass = get_generic_password("ai.clawbotai.keyvault", "master-key")
        .map_err(|e| anyhow::anyhow!("Keychain error: {}", e))?;
    String::from_utf8(pass.to_vec())
        .context("Keychain passphrase is not valid UTF-8")
}

/// Store the master passphrase in macOS Keychain.
fn set_keychain_passphrase(passphrase: &str) -> Result<()> {
    use security_framework::passwords::set_generic_password;
    set_generic_password("ai.clawbotai.keyvault", "master-key", passphrase.as_bytes())
        .map_err(|e| anyhow::anyhow!("Keychain error: {}", e))
}
