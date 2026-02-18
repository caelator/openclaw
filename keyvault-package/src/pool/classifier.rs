//! Task complexity classifier — routes code-gen tasks to the cheapest
//! capable model via regex fast-path with model selection.
//!
//! The classifier examines the prompt text for structural signals:
//! line count requested, keywords ("refactor", "security", "algorithm"),
//! number of files mentioned, etc. This catches ~70% of cases without
//! an LLM call. For ambiguous cases the caller can use the orchestrator's
//! LLM classifier as a fallback.

use super::registry::{self, ModelSpec, TaskComplexity};

// ── Classification ──────────────────────────────────────────────────

/// Classify a code-generation prompt by complexity.
///
/// Uses structural signals in the prompt text — no LLM call needed.
/// Returns the estimated complexity level.
pub fn classify(prompt: &str) -> TaskComplexity {
    let lower = prompt.to_lowercase();
    let word_count = prompt.split_whitespace().count();

    // ── Expert signals (always check first — very specific) ──
    let expert_patterns = [
        "cross-crate", "cross crate", "architecture",
        "system design", "redesign", "refactor entire",
        "restructure", "migrate from", "rewrite the",
        "design pattern", "dependency injection",
    ];
    if expert_patterns.iter().any(|p| lower.contains(p)) {
        return TaskComplexity::Expert;
    }

    // ── For short prompts, check trivial/simple first ──
    // Short prompts with explicit trivial keywords are almost always trivial.
    if word_count <= 15 {
        let trivial_patterns = [
            "rename", "import", "use statement",
            "fix typo", "remove unused", "delete line",
            "add comma", "fix syntax", "one-line",
            "single line", "change name",
        ];
        if trivial_patterns.iter().any(|p| lower.contains(p)) {
            return TaskComplexity::Trivial;
        }

        let simple_patterns = [
            "test", "struct", "enum", "boilerplate",
            "scaffold", "template", "skeleton",
            "add field", "add method", "derive",
            "doc comment", "documentation",
        ];
        if simple_patterns.iter().any(|p| lower.contains(p)) {
            return TaskComplexity::Simple;
        }
    }

    // ── Complex signals ──
    let complex_patterns = [
        "algorithm", "security", "cryptograph", "encryption",
        "concurrent", "async", "parallel", "thread-safe",
        "thread safe", "race condition", "deadlock",
        "state machine", "parser", "lexer", "ast",
        "protocol", "serialization", "deserialization",
        "zero-copy", "unsafe", "lifetime",
        "trait object", "dynamic dispatch",
    ];
    if complex_patterns.iter().any(|p| lower.contains(p)) {
        return TaskComplexity::Complex;
    }

    // ── Count file references (multiple files = more complex) ──
    let file_extensions = [".rs ", ".ts ", ".js ", ".py ", ".go ", ".toml", ".json", ".yaml"];
    let file_refs: usize = file_extensions.iter()
        .map(|ext| lower.matches(ext).count())
        .sum();

    if file_refs >= 4 {
        return TaskComplexity::Complex;
    }

    // ── Medium signals ──
    let medium_patterns = [
        "implement", "function", "method", "refactor",
        "handler", "endpoint", "api", "route",
        "module", "component", "service",
        "error handling", "validation", "convert",
    ];
    let medium_hits: usize = medium_patterns.iter()
        .filter(|p| lower.contains(*p))
        .count();

    if medium_hits >= 2 || file_refs >= 2 {
        return TaskComplexity::Medium;
    }

    // ── Simple signals (long prompts) ──
    let simple_patterns = [
        "test", "struct", "enum", "boilerplate",
        "scaffold", "template", "skeleton",
        "add field", "add method", "derive",
        "doc comment", "documentation",
    ];
    if simple_patterns.iter().any(|p| lower.contains(p)) {
        return TaskComplexity::Simple;
    }

    // ── Trivial signals (long prompts) ──
    let trivial_patterns = [
        "rename", "import", "use statement",
        "fix typo", "remove unused", "delete line",
        "add comma", "fix syntax", "one-line",
        "single line", "change name",
    ];
    if trivial_patterns.iter().any(|p| lower.contains(p)) {
        return TaskComplexity::Trivial;
    }

    // ── Default: estimate by prompt length ──
    match word_count {
        0..=20 => TaskComplexity::Simple,
        21..=80 => TaskComplexity::Medium,
        _ => TaskComplexity::Complex,
    }
}

// ── Model Selection ─────────────────────────────────────────────────

/// Select the best model for a task, prioritizing the cheapest option
/// that can produce valid results at the given complexity.
///
/// Strategy:
/// - Trivial/Simple → Flash-Lite (highest RPD, cheapest)
/// - Medium → Flash or 3 Flash (good balance)
/// - Complex → 3 Pro or 2.5 Pro (highest quality)
/// - Expert → 3 Pro (absolute best)
pub fn select_model(complexity: TaskComplexity) -> &'static ModelSpec {
    match complexity {
        TaskComplexity::Trivial | TaskComplexity::Simple => {
            // Use the workhorse: Flash-Lite has 1000 RPD per key
            registry::cheapest_for_complexity(complexity)
                .unwrap_or(&registry::GOOGLE_MODELS[4]) // fallback to flash-lite
        }
        TaskComplexity::Medium => {
            // Use Flash for balanced speed + quality
            registry::get_model("gemini-3-flash-preview")
                .or_else(|| registry::get_model("gemini-2.5-flash"))
                .unwrap_or(&registry::GOOGLE_MODELS[3])
        }
        TaskComplexity::Complex => {
            // Use Pro for complex logic
            registry::get_model("gemini-3-pro-preview")
                .or_else(|| registry::get_model("gemini-2.5-pro"))
                .unwrap_or(&registry::GOOGLE_MODELS[0])
        }
        TaskComplexity::Expert => {
            // Best available — 3 Pro
            registry::best_for_complexity(complexity)
                .unwrap_or(&registry::GOOGLE_MODELS[0])
        }
    }
}

/// Select a fallback model when the primary model is rate-limited.
/// Cascades UP in capability (never down, to avoid quality loss).
pub fn fallback_model(current_model_id: &str) -> Option<&'static ModelSpec> {
    // Cascade order: lite → flash → 3-flash → 2.5-pro → 3-pro
    let cascade = [
        "gemini-2.5-flash-lite",
        "gemini-2.5-flash",
        "gemini-3-flash-preview",
        "gemini-2.5-pro",
        "gemini-3-pro-preview",
    ];

    let current_pos = cascade.iter().position(|&id| id == current_model_id)?;

    // Try each model above the current one
    for &candidate_id in &cascade[current_pos + 1..] {
        if let Some(model) = registry::get_model(candidate_id) {
            if !model.deprecated {
                return Some(model);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trivial_classification() {
        assert_eq!(classify("rename the variable foo to bar"), TaskComplexity::Trivial);
        assert_eq!(classify("add import for serde::Serialize"), TaskComplexity::Trivial);
        assert_eq!(classify("fix typo in function name"), TaskComplexity::Trivial);
    }

    #[test]
    fn test_simple_classification() {
        assert_eq!(classify("write a test for the login function"), TaskComplexity::Simple);
        assert_eq!(classify("add a new struct called UserProfile"), TaskComplexity::Simple);
        assert_eq!(classify("generate boilerplate for the API client"), TaskComplexity::Simple);
    }

    #[test]
    fn test_medium_classification() {
        assert_eq!(classify("implement the handler for the /users endpoint with validation"), TaskComplexity::Medium);
        assert_eq!(classify("refactor the module to use a service pattern"), TaskComplexity::Medium);
    }

    #[test]
    fn test_complex_classification() {
        assert_eq!(classify("implement a thread-safe connection pool with async support"), TaskComplexity::Complex);
        assert_eq!(classify("write a parser for the custom DSL grammar"), TaskComplexity::Complex);
        assert_eq!(classify("implement AES-256 encryption with proper key derivation"), TaskComplexity::Complex);
    }

    #[test]
    fn test_expert_classification() {
        assert_eq!(classify("redesign the entire architecture to use event sourcing"), TaskComplexity::Expert);
        assert_eq!(classify("cross-crate refactor of the error handling system"), TaskComplexity::Expert);
    }

    #[test]
    fn test_model_selection_routes_to_cheapest() {
        let trivial_model = select_model(TaskComplexity::Trivial);
        assert!(trivial_model.free_rpd >= 1000, "Trivial should route to high-RPD model");

        let expert_model = select_model(TaskComplexity::Expert);
        assert!(expert_model.code_quality >= 4, "Expert should route to high-quality model");
    }

    #[test]
    fn test_fallback_cascades_up() {
        let fallback = fallback_model("gemini-2.5-flash-lite");
        assert!(fallback.is_some());
        assert!(fallback.unwrap().code_quality > 2, "Fallback should cascade to better model");

        let no_fallback = fallback_model("gemini-3-pro-preview");
        assert!(no_fallback.is_none(), "Best model has no fallback");
    }
}
