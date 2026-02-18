---
name: dual-model-orchestration
description: >
  Split complex development work across two AI models: an Architect model
  (Claude Opus 4.6 Thinking) for design, reasoning, and code review, and a
  Coder model (Gemini 3 Pro High) for code generation and implementation.
  Use this whenever the user asks to "use both models", "architect and code",
  or invokes `/dual-model`.
---

# Dual-Model Orchestration — Architect + Coder Pattern

## Purpose

Leverage model specialization by routing tasks to the AI best suited for them:

- **Architect** (default: Claude Opus 4.6 Thinking) — excels at reasoning, architecture, design decisions, edge-case analysis, code review, error triage
- **Coder** (default: Gemini 3 Pro High) — excels at large-scale code generation, refactoring, implementation velocity, test writing

The Architect **manages** the Coder. It decomposes work into precise implementation specs, hands off to the Coder for execution, then reviews the output.

---

## Role Configuration

> [!TIP]
> These defaults can be changed. If the user specifies different models at
> invocation time, use those instead. The skill works with any two models.

| Role          | Default Model              | Fallback           |
| ------------- | -------------------------- | ------------------ |
| **Architect** | Claude Opus 4.6 (Thinking) | Claude Sonnet 4.6  |
| **Coder**     | Gemini 3 Pro (High)        | Gemini 3 Pro (Low) |

**Which model starts?** The model active when the user invokes this skill becomes the **Architect**. If the user is already on the Architect model, proceed directly. If not, the first HARD STOP switches to the Architect.

---

## Execution Protocol

### Phase 1: Architect — Analyze & Spec (Architect Model)

The Architect receives the user's request and:

1. **Analyzes requirements** — Understand what needs to be built, modified, or fixed
2. **Designs the approach** — Architecture decisions, file structure, API contracts
3. **Writes a Code Spec** — A structured handoff document (see format below)
4. **Saves the Code Spec** to `<appDataDir>/brain/<conversation-id>/code_spec.md`
5. **Announces HARD STOP** for model switch to Coder

The Architect should be thorough but concise. The Code Spec is the Coder's **only context** — include everything needed to implement without ambiguity.

### Phase 2: HARD STOP — Switch to Coder

Use the established passphrase protocol:

```
🛑 HARD STOP: Switch to Coder Model

Next: Coder — [CODER MODEL NAME]
Task: Implement Code Spec

Action Required: Switch the model selector to [CODER MODEL NAME],
then reply with "Model Confirmed".
```

**STOP.** Wait for "Model Confirmed". Verify model identity before proceeding.

### Phase 3: Coder — Implement (Coder Model)

On activation, the Coder:

1. **Reads the Code Spec** from `<appDataDir>/brain/<conversation-id>/code_spec.md`
2. **Implements ALL items** in the spec — creates files, edits code, writes tests
3. **Runs verification** — `cargo test`, `cargo check`, `npm test`, etc. as specified
4. **Writes a Code Report** — saves to `<appDataDir>/brain/<conversation-id>/code_report.md`
5. **Announces HARD STOP** for model switch back to Architect

> [!IMPORTANT]
> The Coder must NOT deviate from the Code Spec. If the Coder encounters a
> problem that requires an architectural decision (ambiguity, missing info,
> conflicting requirements), it must:
>
> 1. Document the issue in the Code Report under "Blockers"
> 2. Implement the best-guess approach with a `// TODO(architect): [question]` marker
> 3. Continue with the rest of the spec

### Phase 4: HARD STOP — Switch to Architect

```
🛑 HARD STOP: Switch to Architect Model

Next: Architect — [ARCHITECT MODEL NAME]
Task: Review Code Report

Action Required: Switch the model selector to [ARCHITECT MODEL NAME],
then reply with "Model Confirmed".
```

### Phase 5: Architect — Review (Architect Model)

The Architect:

1. **Reads the Code Report** from `<appDataDir>/brain/<conversation-id>/code_report.md`
2. **Reviews the implementation** — checks correctness, edge cases, style
3. **Resolves blockers** — answers any questions the Coder documented
4. **Decides next action:**

   | Outcome                     | Action                                           |
   | --------------------------- | ------------------------------------------------ |
   | ✅ **Approved**             | Mark task complete, notify user                  |
   | 🔄 **Revisions needed**     | Write a new/updated Code Spec, return to Phase 2 |
   | ❌ **Fundamental redesign** | Return to Phase 1 with new approach              |

### Batch Mode (Optional)

For large tasks with multiple independent code specs, the Architect can write
**multiple Code Specs** (numbered: `code_spec_01.md`, `code_spec_02.md`, etc.)
and pre-approve them all. The Coder then implements them sequentially without
switching back for review between each. A single Code Report covers all specs.

Activate batch mode by including this header in the first Code Spec:

```markdown
> [!NOTE]
> **Batch Mode Active.** Implement specs 01 through [N] sequentially.
> File a single consolidated Code Report when all are complete.
```

---

## Code Spec Format

The Architect writes this document for the Coder. Save to `code_spec.md` (or `code_spec_NN.md` in batch mode).

````markdown
# Code Spec — [Brief Title]

**Spec ID:** [unique id, e.g. CS-001]
**Architect:** [model name]
**Created:** [timestamp]
**Priority:** [P0/P1/P2]

## Objective

[1-3 sentences: what this code change accomplishes and why]

## Files to Modify

### [MODIFY] `path/to/file.rs`

**Function:** `function_name`
**Change:** [precise description of what to add/change/remove]
**Signature:** (if new or changed)

```rust
fn new_function(arg: Type) -> ReturnType
```
````

**Constraints:**

- [specific behavioral requirements]
- [error handling expectations]
- [performance requirements]

### [NEW] `path/to/new_file.rs`

**Purpose:** [what this file does]
**Contents:**

- [struct/function list with signatures]
- [key logic to implement]

### [DELETE] `path/to/old_file.rs`

**Reason:** [why this file is being removed]

## Test Expectations

- [ ] `test_name_1` — [what it verifies]
- [ ] `test_name_2` — [what it verifies]

## Guardrails — Do NOT

- [things the Coder must not change]
- [files/modules that are off-limits]
- [patterns to avoid]

## Verification Command

```bash
cargo test -p [package] && cargo clippy -p [package]
```

## Context (Read-Only Reference)

[Any additional context the Coder needs but should not modify — e.g., type
definitions from other crates, API contracts, etc.]

````

---

## Code Report Format

The Coder writes this document after implementing. Save to `code_report.md`.

```markdown
# Code Report — [Spec ID]

**Coder:** [model name]
**Completed:** [timestamp]
**Spec:** [link to code_spec.md]

## Summary

[1-3 sentences: what was implemented]

## Files Changed

| File | Action | Lines Changed |
|------|--------|---------------|
| `path/to/file.rs` | Modified | +25 / -10 |
| `path/to/new.rs` | Created | +80 |

## Test Results

````

[paste test output]

```

**Result:** [PASS / FAIL with details]

## Build Status

```

[paste cargo check / clippy output summary]

```

**Result:** [CLEAN / WARNINGS / ERRORS with details]

## Blockers

[List any questions, ambiguities, or decisions that need Architect input.
If none, write "None."]

## Deviations from Spec

[List any places where the implementation differs from the Code Spec.
If none, write "None — implemented exactly as specified."]
```

---

## Trigger Phrases

Activate this skill when the user says any of:

- "use both models"
- "architect and code"
- "dual model"
- "have gemini write the code"
- "thinking model + coding model"
- `/dual-model`

---

## Quick Reference

```
Architect (Opus)     Coder (Gemini Pro)     Architect (Opus)
┌──────────────┐     ┌──────────────┐      ┌──────────────┐
│ Analyze Req  │     │ Read Spec    │      │ Read Report  │
│ Design Arch  │────▶│ Write Code   │─────▶│ Review Code  │
│ Write Spec   │     │ Run Tests    │      │ Approve/Fix  │
│              │     │ Write Report │      │              │
└──────────────┘     └──────────────┘      └──────────────┘
    Phase 1      🛑     Phase 3       🛑      Phase 5
              Switch              Switch
```

## Notes

- The Architect should always run on the model with the strongest reasoning
- The Coder should always run on the model with the fastest/most accurate code generation
- If a task is simple enough to not need orchestration, don't use this skill — just do it on the current model
- This skill composes with `phased-build` — each phase in a phased build can use the dual-model pattern
- Code Spec quality is the bottleneck — a vague spec produces bad code. The Architect must be precise.

---

## Parallel Code Generation with KeyVault

> [!TIP]
> For pure code generation (no file browsing or command execution needed),
> the Architect can stay on Opus and delegate to ALL available models
> simultaneously via KeyVault's `kv.parallelGenerate` API — no manual
> model switching needed.

### When to Use Parallel Mode

| Scenario            | Mode            | Example                                             |
| ------------------- | --------------- | --------------------------------------------------- |
| Critical algorithm  | **Competitive** | Same spec → Gemini + Claude + GPT → pick best       |
| Multi-file refactor | **Division**    | File A → Gemini, File B → Claude, File C → DeepSeek |
| Simple single-file  | **Sequential**  | Use standard dual-model handoff                     |

### Competitive Mode (Best-of-N)

Send the same Code Spec to N models. The Architect reviews all outputs and
selects the best implementation.

**Steps:**

1. Architect writes Code Spec as normal
2. Architect queries `kv.activeModels` to discover available providers/models
3. Architect calls `kv.parallelGenerate` with the same prompt to N models
4. Architect reviews all N responses, selects best, and applies the code

### Division of Labor Mode

Send different Code Specs to different models simultaneously.

**Steps:**

1. Architect writes N Code Specs, one per component/file
2. Architect calls `kv.parallelGenerate` routing each spec to a different model
3. Architect integrates all outputs, resolves conflicts, applies code

### API Reference

#### `kv.activeModels` (read-only, no auth)

Returns all currently-active providers, their keys, and available models.

```json
{ "jsonrpc": "2.0", "method": "kv.activeModels", "id": 1 }
```

Response:

```json
{
  "providers": [
    {"provider": "google", "active_keys": 10, "models": [...]},
    {"provider": "anthropic", "active_keys": 1, "models": [...]}
  ],
  "total_active_keys": 11
}
```

#### `kv.parallelGenerate` (auth required)

Fan out code generation across multiple providers simultaneously.

```json
{
  "jsonrpc": "2.0",
  "method": "kv.parallelGenerate",
  "auth": "<bearer token>",
  "params": {
    "caller": "dual-model-orchestration",
    "requests": [
      {
        "provider": "google",
        "model": "gemini-3-pro-exp-0312",
        "messages": [{ "role": "user", "content": "...code spec..." }],
        "system_prompt": "You are a Rust expert. Implement exactly as specified.",
        "temperature": 0.2,
        "max_tokens": 8192
      },
      {
        "provider": "anthropic",
        "model": "claude-sonnet-4-20250514",
        "messages": [{ "role": "user", "content": "...same or different spec..." }],
        "system_prompt": "You are a Rust expert. Implement exactly as specified.",
        "temperature": 0.2,
        "max_tokens": 8192
      }
    ]
  },
  "id": 2
}
```

Response:

```json
{
  "results": [
    {"ok": true, "response": {"text": "...", "model": "...", "provider": "google", ...}},
    {"ok": true, "response": {"text": "...", "model": "...", "provider": "anthropic", ...}}
  ]
}
```

> [!WARNING]
> **Cost awareness.** Competitive mode multiplies API costs by N. Always
> check `kv.activeModels` first and confirm the user is comfortable with
> the number of parallel requests before sending.

---

## Swarm Mode — Automatic Task Distribution

> [!TIP]
> For multi-task workloads (e.g., implementing 5 files), use **Swarm Mode**
> to automatically classify each task by complexity and route it to the
> **cheapest model that produces valid results** — all for free.

### How Swarm Mode Works

```
Tasks → Classifier → Model Selection → Key Assignment → Parallel Exec
  ┌─────────┐   ┌─────────────┐   ┌────────────────┐   ┌──────────┐
  │ Task 1   │──→│ Trivial     │──→│ Flash-Lite K1  │──→│          │
  │ Task 2   │──→│ Medium      │──→│ 3 Flash    K2  │──→│ Results  │
  │ Task 3   │──→│ Complex     │──→│ 3 Pro      K3  │──→│          │
  │ Task 4   │──→│ Simple      │──→│ Flash-Lite K4  │──→│          │
  └─────────┘   └─────────────┘   └────────────────┘   └──────────┘
```

### Model Tiers (Google AI Studio Free)

| Complexity     | Model                    | RPD/Key | Quality    |
| -------------- | ------------------------ | ------- | ---------- |
| Trivial/Simple | `gemini-2.5-flash-lite`  | 1,000   | ⭐⭐       |
| Medium         | `gemini-3-flash-preview` | 250     | ⭐⭐⭐⭐   |
| Complex/Expert | `gemini-3-pro-preview`   | 100     | ⭐⭐⭐⭐⭐ |

### `kv.swarmGenerate` (auth required)

```json
{
  "jsonrpc": "2.0",
  "method": "kv.swarmGenerate",
  "auth": "<bearer token>",
  "params": {
    "tasks": [
      {
        "prompt": "Add derive(Serialize) to the Config struct",
        "label": "config-derive",
        "complexity": "trivial"
      },
      {
        "prompt": "Implement the connection pool with async retry logic",
        "label": "conn-pool",
        "system_prompt": "You are a Rust expert.",
        "temperature": 0.1,
        "max_tokens": 16384
      }
    ]
  },
  "id": 3
}
```

Response:

```json
{
  "results": [
    {
      "label": "config-derive",
      "model": "gemini-2.5-flash-lite",
      "complexity": "trivial",
      "ok": true,
      "text": "..."
    },
    {
      "label": "conn-pool",
      "model": "gemini-3-pro-preview",
      "complexity": "complex",
      "ok": true,
      "text": "..."
    }
  ],
  "summary": { "total": 2, "succeeded": 2, "failed": 0 }
}
```

### `kv.modelRegistry` (read-only, no auth)

Query all available models with their specs:

```json
{ "jsonrpc": "2.0", "method": "kv.modelRegistry", "id": 4 }
```

### Swarm Fields

| Field           | Required | Description                                              |
| --------------- | -------- | -------------------------------------------------------- |
| `prompt`        | ✅       | The code-gen prompt                                      |
| `label`         | Optional | Tracking label for results                               |
| `complexity`    | Optional | Override: `trivial`/`simple`/`medium`/`complex`/`expert` |
| `model`         | Optional | Override: exact model ID                                 |
| `system_prompt` | Optional | Custom system prompt                                     |
| `temperature`   | Optional | Default: 0.2                                             |
| `max_tokens`    | Optional | Default: model's max output                              |
