//! Model registry — static metadata for all known LLM models.
//!
//! Every model we can route to is described here: its capabilities,
//! rate limits on the free tier, quality rating, and what kinds of
//! tasks it excels at. The swarm scheduler and complexity classifier
//! use this to pick the cheapest model that can handle each task.

use serde::{Deserialize, Serialize};

// ── Enums ───────────────────────────────────────────────────────────

/// The pricing tier of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Free,
    Paid,
    Enterprise,
}

/// What kind of code-generation task a model is good at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Simple renames, imports, single-line fixes
    Trivial,
    /// Write a test, add a struct, boilerplate generation
    Simple,
    /// Implement a function, refactor a module
    Medium,
    /// Algorithm design, security-critical code, complex logic
    Complex,
    /// Architecture, cross-crate refactors, system design
    Expert,
}

/// Task complexity level (maps 1:1 with TaskKind for routing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskComplexity {
    Trivial = 0,
    Simple = 1,
    Medium = 2,
    Complex = 3,
    Expert = 4,
}

// ── Model Spec ──────────────────────────────────────────────────────

/// Static specification of an LLM model's capabilities and limits.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    /// Model identifier for API calls (e.g., "gemini-3-pro-preview")
    pub id: &'static str,
    /// Provider identifier (e.g., "google")
    pub provider: &'static str,
    /// Human-readable name
    pub display_name: &'static str,
    /// Pricing tier
    pub tier: ModelTier,
    /// Code generation quality rating (1-5)
    pub code_quality: u8,
    /// Whether the model supports "thinking" / chain-of-thought
    pub supports_thinking: bool,
    /// Maximum input tokens (context window)
    pub input_token_limit: u64,
    /// Maximum output tokens
    pub output_token_limit: u64,
    /// Free-tier requests per minute (per key)
    pub free_rpm: u16,
    /// Free-tier requests per day (per key)
    pub free_rpd: u32,
    /// Free-tier tokens per minute (per key)
    pub free_tpm: u64,
    /// Minimum task complexity this model is appropriate for
    pub min_complexity: TaskComplexity,
    /// Whether this model is deprecated / retiring
    pub deprecated: bool,
}

// ── Static Registry ─────────────────────────────────────────────────

/// All known Google AI Studio models for code generation.
/// Ordered from highest quality to lowest (for preference ranking).
pub static GOOGLE_MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "gemini-3-pro-preview",
        provider: "google",
        display_name: "Gemini 3 Pro Preview",
        tier: ModelTier::Free,
        code_quality: 5,
        supports_thinking: true,
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        free_rpm: 10,
        free_rpd: 100,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Complex,
        deprecated: false,
    },
    ModelSpec {
        id: "gemini-3-flash-preview",
        provider: "google",
        display_name: "Gemini 3 Flash Preview",
        tier: ModelTier::Free,
        code_quality: 4,
        supports_thinking: true,
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        free_rpm: 10,
        free_rpd: 250,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Medium,
        deprecated: false,
    },
    ModelSpec {
        id: "gemini-2.5-pro",
        provider: "google",
        display_name: "Gemini 2.5 Pro",
        tier: ModelTier::Free,
        code_quality: 4,
        supports_thinking: true,
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        free_rpm: 5,
        free_rpd: 100,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Complex,
        deprecated: false,
    },
    ModelSpec {
        id: "gemini-2.5-flash",
        provider: "google",
        display_name: "Gemini 2.5 Flash",
        tier: ModelTier::Free,
        code_quality: 3,
        supports_thinking: true,
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        free_rpm: 10,
        free_rpd: 250,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Simple,
        deprecated: false,
    },
    ModelSpec {
        id: "gemini-2.5-flash-lite",
        provider: "google",
        display_name: "Gemini 2.5 Flash-Lite",
        tier: ModelTier::Free,
        code_quality: 2,
        supports_thinking: true,
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        free_rpm: 15,
        free_rpd: 1_000,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Trivial,
        deprecated: false,
    },
    ModelSpec {
        id: "gemini-2.0-flash",
        provider: "google",
        display_name: "Gemini 2.0 Flash",
        tier: ModelTier::Free,
        code_quality: 2,
        supports_thinking: false,
        input_token_limit: 1_048_576,
        output_token_limit: 8_192,
        free_rpm: 15,
        free_rpd: 1_500,
        free_tpm: 250_000,
        min_complexity: TaskComplexity::Trivial,
        deprecated: true, // Retiring March 31, 2026
    },
];

// ── Registry API ────────────────────────────────────────────────────

/// Look up a model spec by its API identifier.
pub fn get_model(id: &str) -> Option<&'static ModelSpec> {
    GOOGLE_MODELS.iter().find(|m| m.id == id)
}

/// Get all non-deprecated models sorted by code quality (descending).
pub fn best_models() -> Vec<&'static ModelSpec> {
    let mut models: Vec<_> = GOOGLE_MODELS.iter().filter(|m| !m.deprecated).collect();
    models.sort_by(|a, b| b.code_quality.cmp(&a.code_quality));
    models
}

/// Get the cheapest (highest RPD, lowest quality) model that can handle
/// the given complexity level.
pub fn cheapest_for_complexity(complexity: TaskComplexity) -> Option<&'static ModelSpec> {
    let mut candidates: Vec<_> = GOOGLE_MODELS
        .iter()
        .filter(|m| !m.deprecated && m.min_complexity <= complexity)
        .collect();

    // Sort by: cheapest first (highest RPD = most available), then lowest quality
    candidates.sort_by(|a, b| {
        b.free_rpd.cmp(&a.free_rpd)
            .then(a.code_quality.cmp(&b.code_quality))
    });

    candidates.first().copied()
}

/// Get the best model that can handle the given complexity.
pub fn best_for_complexity(complexity: TaskComplexity) -> Option<&'static ModelSpec> {
    let mut candidates: Vec<_> = GOOGLE_MODELS
        .iter()
        .filter(|m| !m.deprecated && m.min_complexity <= complexity)
        .collect();

    // Sort by: highest quality first
    candidates.sort_by(|a, b| b.code_quality.cmp(&a.code_quality));

    candidates.first().copied()
}

/// Get all models suitable for a given complexity level, ordered from
/// cheapest to most expensive.
pub fn models_for_complexity(complexity: TaskComplexity) -> Vec<&'static ModelSpec> {
    let mut candidates: Vec<_> = GOOGLE_MODELS
        .iter()
        .filter(|m| !m.deprecated && m.min_complexity <= complexity)
        .collect();

    // Cheapest first: highest RPD, lowest quality
    candidates.sort_by(|a, b| {
        b.free_rpd.cmp(&a.free_rpd)
            .then(a.code_quality.cmp(&b.code_quality))
    });

    candidates
}

/// Compute aggregate free-tier capacity across N keys for a model.
pub fn aggregate_capacity(model: &ModelSpec, num_keys: usize) -> (u32, u64, u64) {
    let total_rpm = model.free_rpm as u32 * num_keys as u32;
    let total_rpd = model.free_rpd as u64 * num_keys as u64;
    let total_tpm = model.free_tpm * num_keys as u64;
    (total_rpm, total_rpd, total_tpm)
}
