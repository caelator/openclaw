---
description: How to handle complex multi-phase builds with phased checkpoints across context windows
---

# Phased Checkpoint Development

Use this workflow for any complex task that will take more than ~20 tool calls or spans multiple crates/files.

## Rules

// turbo-all

### 1. Always Create a Checkpoint Before Starting

Before writing any code, create or update TWO artifacts in `<appDataDir>/brain/<conversation-id>/`:

- **`task.md`** — A detailed checklist broken into phases, with `[ ]`, `[/]`, `[x]` markers
- **`implementation_plan.md`** — Technical plan with file-level details, grouped by component

### 2. Build in Phases, Not All at Once

- Each phase should be **independently verifiable** (can `cargo test` / `npm test` at phase boundary)
- Each phase should touch at most **2-3 crates/packages**
- Run verification (`cargo test`, `cargo clippy`, etc.) at the end of EACH phase
- Update `task.md` with `[x]` markers **as you complete items**, not at the end

### 3. Checkpoint After Every Phase

After each phase passes verification:

1. Update `task.md` with all completed items marked `[x]`
2. Note the current test count, line count, or other metrics in the task.md header
3. If the task will continue in a new context window, write a **handoff summary** at the top of `task.md`:
   - What was completed
   - What failed (if anything) and why
   - Exact next step to pick up
   - Any blockers or decisions needed

### 4. Starting a New Context Window

When picking up work from a previous session:

1. **Read `task.md` and `implementation_plan.md` FIRST** — these are your source of truth
2. Do NOT re-read files you already wrote in the previous session unless you need specific API signatures
3. Start the next unchecked `[ ]` item in `task.md`
4. Run `cargo test --workspace` (or equivalent) immediately to verify the baseline is clean

### 5. Never Lose Work

- **Verify before moving on**: Always run tests after each file/crate is complete
- **Fix forward, don't restart**: If a test fails, fix it in place rather than rewriting from scratch
- **Commit-sized chunks**: Each phase should be small enough to commit independently
- If you hit a compile error, fix it immediately — don't accumulate errors across files

### 6. Context Window Budget

- Reserve the last ~10% of your context window for verification (test runs, clippy)
- If you estimate you're >70% through your context window, finish the current phase, run verification, checkpoint, and notify the user that a new session is needed
- Better to finish one phase cleanly than to start two and finish neither
