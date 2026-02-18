//! Auth module — self-healing bearer token authentication.
//!
//! Solves the HMAC shared-secret distribution problem:
//!
//!   Keychain (authoritative)
//!        ↓ reconcile on every boot
//!   ~/.openclaw/keyvault.token (derivative, 0600)
//!        ↓ read by clients
//!   JSON-RPC "auth" field
//!        ↓ validated with constant-time comparison
//!   Request accepted or rejected
//!
//! Self-healing properties:
//! - Token file deleted → rewritten from Keychain on boot
//! - Keychain deleted → new token generated, file rewritten
//! - Power failure mid-write → atomic rename prevents corruption
//! - Client has stale token → auto-retry after re-reading file
//! - Crash mid-rotation → Keychain is authoritative, file reconciled on restart

use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{info, warn, error};
use zeroize::Zeroize;

type HmacSha256 = Hmac<Sha256>;

const TOKEN_LEN: usize = 32; // 256 bits
const KEYCHAIN_SERVICE: &str = "ai.clawbotai.keyvault";
const KEYCHAIN_AUTH_ACCOUNT: &str = "auth-token";

/// The auth guard — holds the current valid token and validates requests.
pub struct AuthGuard {
    /// The authoritative token (from Keychain).
    token_hex: String,
    /// Path to the token file.
    token_file: PathBuf,
}

impl AuthGuard {
    /// Bootstrap the auth system. This is the main entry point.
    ///
    /// Strategy:
    /// 1. Try Keychain → if found, use it (authoritative)
    /// 2. Try file → if Keychain empty but file exists, import into Keychain
    /// 3. Neither → generate new token, store in both
    /// 4. Always reconcile: write file from Keychain (atomic rename)
    pub fn bootstrap(data_dir: &Path) -> Result<Self> {
        let token_file = data_dir.join("keyvault.token");

        // Step 1: Try Keychain (authoritative source)
        let token_hex = match load_keychain_token() {
            Ok(token) => {
                info!("🔐 Auth token loaded from macOS Keychain");
                token
            }
            Err(_) => {
                // Step 2: Try file (maybe Keychain was cleared but file survived)
                match load_file_token(&token_file) {
                    Ok(token) => {
                        warn!("Keychain empty but token file exists — re-importing to Keychain");
                        if let Err(e) = store_keychain_token(&token) {
                            error!("Failed to re-import token to Keychain: {}", e);
                        }
                        token
                    }
                    Err(_) => {
                        // Step 3: Neither exists — generate fresh token
                        info!("🔐 No auth token found — generating new 256-bit token");
                        let token = generate_token();

                        // Store in Keychain first (authoritative)
                        if let Err(e) = store_keychain_token(&token) {
                            error!("Failed to store token in Keychain: {}", e);
                            // Continue anyway — file will be the only copy
                        } else {
                            info!("Auth token stored in macOS Keychain");
                        }

                        token
                    }
                }
            }
        };

        // Step 4: Always reconcile — write file from Keychain (atomic)
        if let Err(e) = atomic_write_token_file(&token_file, &token_hex) {
            error!("Failed to write token file: {} — clients won't be able to authenticate via file", e);
        } else {
            info!(path = %token_file.display(), "Auth token file written (0600)");
        }

        Ok(Self { token_hex, token_file })
    }

    /// Validate a bearer token from a client request.
    /// Uses constant-time comparison to prevent timing attacks.
    pub fn validate(&self, candidate: &str) -> bool {
        constant_time_eq(candidate.trim(), &self.token_hex)
    }

    /// Rotate the token: generate new → Keychain → file → return new token.
    /// Old token is immediately invalidated.
    pub fn rotate(&mut self) -> Result<String> {
        let new_token = generate_token();

        // Keychain first (authoritative)
        store_keychain_token(&new_token)
            .context("Failed to store rotated token in Keychain")?;

        // Then file (derivative, atomic)
        atomic_write_token_file(&self.token_file, &new_token)
            .context("Failed to write rotated token file")?;

        // Zeroize old token
        self.token_hex.zeroize();
        self.token_hex = new_token.clone();

        info!("🔄 Auth token rotated — old token invalidated");
        Ok(new_token)
    }

    /// Force reconcile: re-read Keychain and rewrite file.
    /// Useful if file was deleted or corrupted.
    pub fn sync(&self) -> Result<()> {
        atomic_write_token_file(&self.token_file, &self.token_hex)
            .context("Failed to sync token file")?;
        info!("🔄 Token file re-synced from Keychain");
        Ok(())
    }

    /// Get the token file path (for startup messages).
    pub fn token_file_path(&self) -> &Path {
        &self.token_file
    }
}

impl Drop for AuthGuard {
    fn drop(&mut self) {
        self.token_hex.zeroize();
    }
}

// ── Token Generation ────────────────────────────────────────────────

/// Generate a cryptographically random 256-bit token, hex-encoded.
fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_LEN];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let hex = hex::encode(bytes);
    bytes.zeroize();
    hex
}

// ── Keychain Operations ─────────────────────────────────────────────

fn load_keychain_token() -> Result<String> {
    use security_framework::passwords::get_generic_password;
    let raw = get_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_AUTH_ACCOUNT)
        .map_err(|e| anyhow::anyhow!("Keychain read failed: {}", e))?;
    String::from_utf8(raw.to_vec())
        .context("Keychain token is not valid UTF-8")
}

fn store_keychain_token(token: &str) -> Result<()> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(KEYCHAIN_SERVICE, KEYCHAIN_AUTH_ACCOUNT, token.as_bytes())
        .map_err(|e| anyhow::anyhow!("Keychain write failed: {}", e))
}

// ── File Operations (Atomic) ────────────────────────────────────────

/// Write token to file using atomic rename.
///
/// Strategy:
///   1. Write to .token.tmp
///   2. fsync() the file
///   3. rename .token.tmp → .token (atomic on POSIX)
///
/// If power fails during step 1 or 2, the old .token is still intact.
/// If power fails during step 3, rename() is atomic — either old or new.
fn atomic_write_token_file(path: &Path, token: &str) -> Result<()> {
    let tmp_path = path.with_extension("token.tmp");

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Write to temp file
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)
            .context("Failed to create temp token file")?;

        // Set permissions BEFORE writing content (defense in depth)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }

        file.write_all(token.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?; // fsync — flush to disk
    }

    // Atomic rename
    fs::rename(&tmp_path, path)
        .context("Atomic rename failed")?;

    // Also set permissions on the final file (belt + suspenders)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Read token from file.
fn load_file_token(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)
        .context("Failed to read token file")?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("Token file is empty");
    }
    // Validate it looks like a hex token
    if trimmed.len() != TOKEN_LEN * 2 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("Token file contains invalid data (expected 64 hex chars)");
    }
    Ok(trimmed)
}

// ── Constant-Time Comparison ────────────────────────────────────────

/// Compare two strings in constant time to prevent timing attacks.
/// Uses HMAC-SHA256 with a fixed key — if both inputs produce the
/// same HMAC, they're equal. This is constant-time by construction.
fn constant_time_eq(a: &str, b: &str) -> bool {
    // Use HMAC with a fixed key to do constant-time comparison.
    // This avoids the need for the subtle crate while still being
    // timing-attack resistant.
    let key = b"keyvault-constant-time-comparison-key";
    let mut mac_a = HmacSha256::new_from_slice(key).unwrap();
    let mut mac_b = HmacSha256::new_from_slice(key).unwrap();

    mac_a.update(a.as_bytes());
    mac_b.update(b.as_bytes());

    let result_a = mac_a.finalize().into_bytes();
    let result_b = mac_b.finalize().into_bytes();

    // Compare the two HMAC outputs — this is constant time because
    // we're comparing fixed-length byte arrays of the same size
    result_a == result_b
}

// ── Per-Caller Rate Limiting ────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Simple sliding-window rate limiter per caller identity.
pub struct RateLimiter {
    /// Map of caller → list of request timestamps
    windows: Mutex<HashMap<String, Vec<Instant>>>,
    /// Maximum requests per window
    max_requests: usize,
    /// Window duration in seconds
    window_secs: u64,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_secs: u64) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            max_requests,
            window_secs,
        }
    }

    /// Check if a caller is allowed to make a request.
    /// Returns Ok(()) if allowed, Err with remaining seconds if rate-limited.
    pub fn check(&self, caller: &str) -> Result<(), u64> {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.window_secs);

        let timestamps = windows.entry(caller.to_string()).or_default();

        // Remove expired timestamps
        timestamps.retain(|t| now.duration_since(*t) < window);

        if timestamps.len() >= self.max_requests {
            // Find when the oldest request in the window expires
            let oldest = timestamps.first().unwrap();
            let remaining = window.as_secs() - now.duration_since(*oldest).as_secs();
            return Err(remaining);
        }

        timestamps.push(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token_length() {
        let token = generate_token();
        assert_eq!(token.len(), 64); // 32 bytes = 64 hex chars
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_constant_time_eq_matching() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn test_constant_time_eq_not_matching() {
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("abc123", ""));
    }

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new(3, 60);
        assert!(limiter.check("client-1").is_ok());
        assert!(limiter.check("client-1").is_ok());
        assert!(limiter.check("client-1").is_ok());
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiter::new(2, 60);
        assert!(limiter.check("client-1").is_ok());
        assert!(limiter.check("client-1").is_ok());
        assert!(limiter.check("client-1").is_err());
    }

    #[test]
    fn test_rate_limiter_per_caller() {
        let limiter = RateLimiter::new(1, 60);
        assert!(limiter.check("client-1").is_ok());
        assert!(limiter.check("client-1").is_err());
        // Different caller has its own window
        assert!(limiter.check("client-2").is_ok());
    }

    #[test]
    fn test_atomic_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.token");
        let token = generate_token();

        atomic_write_token_file(&path, &token).unwrap();
        let loaded = load_file_token(&path).unwrap();
        assert_eq!(loaded, token);

        // Check permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::metadata(&path).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn test_load_file_token_rejects_invalid() {
        let dir = tempfile::tempdir().unwrap();

        // Empty file
        let path = dir.path().join("empty.token");
        fs::write(&path, "").unwrap();
        assert!(load_file_token(&path).is_err());

        // Wrong length
        let path2 = dir.path().join("short.token");
        fs::write(&path2, "abc123").unwrap();
        assert!(load_file_token(&path2).is_err());

        // Non-hex
        let path3 = dir.path().join("nonhex.token");
        fs::write(&path3, "g".repeat(64)).unwrap();
        assert!(load_file_token(&path3).is_err());
    }
}
