---
description: How to perform an 11-dimension audit of a Rust crate (per Hegemon audit plan v2)
---

# Crate Audit Workflow

Apply this framework to every crate audit request. Produces a structured per-crate report with findings classified as PASS/WARN/FAIL and prioritized P1–P3.

## Prerequisites

// turbo

1. Verify the workspace compiles: `cargo check --workspace`
   // turbo
2. Verify tests pass: `cargo test --workspace`

## Per-Crate Audit Steps

### Step 1: Read All Source Files

- Read `Cargo.toml` for dependency declarations
- Read `lib.rs` / `main.rs` for module tree
- Read every `.rs` file in the crate's `src/` directory
- Read all files in `tests/` directory (integration tests)

### Step 2: Evaluate 11 Audit Dimensions

Apply each dimension below. For each check, record a verdict: **PASS**, **WARN**, or **FAIL**.

#### Dimension 1: Structural Integrity (SI)

| Check | Description                                                                 |
| ----- | --------------------------------------------------------------------------- |
| SI-1  | `Cargo.toml` uses `version.workspace = true` and `edition.workspace = true` |
| SI-2  | All declared modules in `lib.rs` exist as files                             |
| SI-3  | No orphaned `.rs` files (exist on disk but not declared in `mod` tree)      |
| SI-4  | `pub` visibility matches intent — public API is intentional, not accidental |
| SI-5  | Dependency declarations match actual `use` statements                       |

#### Dimension 2: Logic Correctness (LC)

| Check | Description                                                                         |
| ----- | ----------------------------------------------------------------------------------- |
| LC-1  | Every `match` on an enum covers all variants (no catch-all `_` hiding missing arms) |
| LC-2  | No `unwrap()` / `expect()` in non-test code without a safety comment                |
| LC-3  | No `todo!()`, `unimplemented!()`, or `panic!()` in non-test production code         |
| LC-4  | Integer arithmetic does not silently overflow                                       |
| LC-5  | Async code does not hold locks across `.await` points                               |
| LC-6  | State machines have defined transitions — no illegal state reachable                |
| LC-7  | Boundary conditions: empty inputs, zero-length, max-length all handled              |
| LC-8  | No `unsafe` blocks without a `// SAFETY:` comment                                   |

#### Dimension 3: Error Handling (EH)

| Check | Description                                                                   |
| ----- | ----------------------------------------------------------------------------- |
| EH-1  | Custom error types implement `std::error::Error` (via `thiserror`)            |
| EH-2  | Error variants are granular enough to distinguish failure modes               |
| EH-3  | Errors propagate with context (no silent swallowing via `.ok()` or `let _ =`) |
| EH-4  | All `Result` return types are actually checked by callers                     |
| EH-5  | Error messages do not leak sensitive data                                     |

#### Dimension 4: Security Posture (SP)

| Check | Description                                                     |
| ----- | --------------------------------------------------------------- |
| SP-1  | No secret values in source code or logs                         |
| SP-2  | All user-facing string output passes through sanitization       |
| SP-3  | Cryptographic operations use vetted libraries                   |
| SP-4  | Sandbox profiles follow least-privilege                         |
| SP-5  | Frame size limits enforced on both read and write paths         |
| SP-6  | No TOCTOU races on shared files                                 |
| SP-7  | No `unsafe` code that could cause use-after-free or double-free |
| SP-8  | Timing-safe comparisons for secrets                             |
| SP-9  | Secrets zeroed on drop                                          |

#### Dimension 5: API Contract Compliance (AC)

| Check | Description                                                                |
| ----- | -------------------------------------------------------------------------- |
| AC-1  | Trait implementations fulfill all required methods with correct signatures |
| AC-2  | Public API matches documented intent                                       |
| AC-3  | Serde round-trips correctly for all public types                           |
| AC-4  | Cross-crate type usage is consistent                                       |
| AC-5  | Async trait methods are `Send + Sync` where required                       |

#### Dimension 6: Test Adequacy (TA)

| Check | Description                                                           |
| ----- | --------------------------------------------------------------------- |
| TA-1  | Every public function has ≥1 unit test (target ≥80% pub API coverage) |
| TA-2  | Happy path AND error path tested for each fallible function           |
| TA-3  | Edge cases tested: empty input, boundary values, overflow             |
| TA-4  | Tests are deterministic (no flakes)                                   |
| TA-5  | Test names clearly describe what they verify                          |
| TA-6  | Minimum test count per crate met                                      |
| TA-7  | Unit tests don't cross-crate import; integration tests do             |

#### Dimension 7: Interoperability Verification (IV)

| Check | Description                                                    |
| ----- | -------------------------------------------------------------- |
| IV-1  | Crate correctly consumes types from declared dependencies      |
| IV-2  | Events published by upstream are correctly consumed downstream |

#### Dimensions 8–11: Concurrency, Performance, Documentation, Maintainability

| Area            | Key checks                                                    |
| --------------- | ------------------------------------------------------------- |
| Concurrency     | Lock ordering, atomic ordering correctness, no data races     |
| Performance     | No blocking on hot paths, appropriate data structures         |
| Documentation   | Module-level `//!` docs, security properties documented       |
| Maintainability | Clear module hierarchy, `#[non_exhaustive]` where appropriate |

### Step 3: Write Findings Report

Create an artifact file `audit_<crate_name>.md` with:

1. Crate metadata (tier, file count, LOC, test count)
2. Per-dimension table with verdict and notes
3. Findings summary table with columns: ID, Severity (P1/P2/P3), Dimension, Description
4. Overall verdict: PASS, WARN, or FAIL

**Severity classification:**

- **P1 (Critical):** Security vulnerability, data loss risk, crash in production path
- **P2 (Major):** Logic bug, missing error handling, untested critical path
- **P3 (Minor):** Style, documentation, test gap in non-critical path

### Step 4: Update Task Tracker

Mark the crate as complete in `task.md` and proceed to the next crate.

## Report Template

```markdown
# <crate-name> — Audit Report (11 Dimensions)

**Crate:** `<crate-name>` (Tier X — Description)
**Files reviewed:** N source files, M integration tests, `Cargo.toml`
**Lines of code:** ~N (excluding tests)
**Test count:** N (X unit + Y integration)

---

## Dimension 1: Structural Integrity

| Check | Verdict        | Notes |
| ----- | -------------- | ----- |
| SI-1  | PASS/WARN/FAIL | ...   |

...

## Findings Summary

| ID        | Severity | Dimension | Description |
| --------- | -------- | --------- | ----------- |
| CRATE-001 | P1/P2/P3 | XX-N      | ...         |

**Overall verdict: PASS/WARN/FAIL**
```
