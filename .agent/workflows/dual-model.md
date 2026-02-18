---
description: Use dual-model orchestration — Architect (Opus) thinks, Coder (Gemini Pro High) generates code
---

# Dual-Model Orchestration Workflow

Split complex development across two AI models for maximum effectiveness.

// turbo-all

## Steps

1. **Read the skill definition** — View `.agent/skills/dual-model-orchestration/SKILL.md` to load the full protocol
2. **Identify current model** — Determine which model is currently active
3. **If not on Architect model** — Announce HARD STOP to switch to Architect (Claude Opus 4.6 Thinking)
4. **Architect Phase** — Analyze requirements, design approach, write Code Spec to `code_spec.md`
5. **HARD STOP** — Switch to Coder (Gemini 3 Pro High), wait for "Model Confirmed"
6. **Coder Phase** — Read Code Spec, implement all items, run tests, write Code Report to `code_report.md`
7. **HARD STOP** — Switch back to Architect, wait for "Model Confirmed"
8. **Review Phase** — Architect reads Code Report, reviews implementation
9. **Decision** — Approve (done) or write revised Code Spec (loop back to step 5)

## Model Defaults

- **Architect:** Claude Opus 4.6 (Thinking) — strongest reasoning and architecture
- **Coder:** Gemini 3 Pro (High) — fastest and most accurate code generation

These can be overridden by the user at invocation time.
