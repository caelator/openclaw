# Code Spec — [TITLE]

**Spec ID:** CS-NNN
**Architect:** [model name]
**Created:** [YYYY-MM-DDTHH:MM:SS]
**Priority:** P0 | P1 | P2

## Objective

[What this code change accomplishes and why. 1-3 sentences.]

## Files to Modify

### [MODIFY | NEW | DELETE] `path/to/file.ext`

**Function/Struct:** `name`
**Change:** [precise description]
**Signature:** (if new or changed)

```rust
fn example(arg: Type) -> Result<ReturnType, Error>
```

**Constraints:**

- [behavioral requirements]
- [error handling]
- [performance]

## Test Expectations

- [ ] `test_name` — [what it verifies]

## Guardrails — Do NOT

- [files/modules off-limits]
- [patterns to avoid]

## Verification Command

```bash
cargo test -p [package] && cargo clippy -p [package]
```

## Context (Read-Only Reference)

[Type definitions, API contracts, or other context the Coder needs but must not modify.]
