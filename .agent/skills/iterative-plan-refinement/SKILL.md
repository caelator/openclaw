---
name: iterative-plan-refinement
description: >
  Refine any plan or design document by passing it through a hierarchy of AI models,
  starting with the least capable and finishing with the most capable. Each model
  critiques and improves the output of the previous. Use this skill whenever the user
  asks to "iterate a plan through the models", "refine with all AIs", or similar.
---

# Iterative Plan Refinement Skill

## Purpose

This skill defines a **model hierarchy** and a **structured handoff protocol** for
iteratively improving any plan, design document, or specification. Each model in the
hierarchy receives the previous model's output and is instructed to:

1. Identify weaknesses, gaps, and ambiguities
2. Add depth, precision, and missing edge cases
3. Improve structure and clarity
4. Output a revised, improved version of the document

The final output — produced by the most capable model — is the authoritative version.

---

## Step 0: Dynamic Model Discovery (Required at Every Invocation)

> [!IMPORTANT]
> Before building the hierarchy, **always query the system for currently available
> models**. The list changes as models are deprecated or added. Never hardcode a
> static list — always derive the hierarchy from what is actually available.

**How to query:** Check the Antigravity model selection settings or ask the user to
confirm which models are currently available. Remove any deprecated models from the
hierarchy before proceeding.

---

## Known Model Hierarchy (as of 2026-02-18)

The following is the **current known model list**, ranked from least to most capable.
This list must be refreshed at each invocation via Step 0.

| Tier            | Model                          | Capability Profile                                        | Role in Refinement                                                                    |
| --------------- | ------------------------------ | --------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| T1 (Seed)       | **Gemini 3 Flash**             | Fast, broad coverage, lower reasoning depth               | Initial structural pass — identify obvious gaps, add missing sections                 |
| T2 (Expand)     | **Gemini 3 Pro (Low)**         | Balanced reasoning, good at structured analysis           | Deepen logic, add specificity, challenge assumptions                                  |
| T3 (Broaden)    | **GPT-OSS 12**                 | Strong general reasoning, different training distribution | Cross-paradigm review — catch blind spots from Gemini-family passes                   |
| T4 (Challenge)  | **Claude Sonnet 4.6**          | Strong adversarial reasoning, nuanced critique            | Find logical flaws, security holes, edge cases, ambiguous criteria                    |
| T5 (Deep Think) | **Claude Opus 4.6 (Thinking)** | Extended reasoning, highest Claude capability             | Deep structural critique, resolve conflicts from prior passes                         |
| T6 (Synthesize) | **Gemini 3 Pro (High)**        | Highest overall capability, best synthesis                | Synthesize all prior feedback, resolve conflicts, produce final authoritative version |

> **Deprecated / Skip:** Claude Sonnet 4.5 and Claude Sonnet 4.5 (Thinking) are
> deprecated and must be skipped if they appear in the model list.

> **Resilience:** If any tier's model is unavailable, skip it and proceed to the next.
> The hierarchy functions with as few as 2 tiers.

---

## Execution Protocol

### Step 1: Prepare the Seed Document

Before starting the hierarchy, ensure the plan exists as a saved markdown file. If it
does not exist, create a draft using the current model before beginning the hierarchy.

### Step 2: Announce and Wait for Confirmation

Before proceeding to each tier's model, you must **pause and wait for user confirmation**:

1. Announce the transition using this format:

   ```
   🛑 HARD STOP: Switch Model Now
   Next Pass: T[#] — [MODEL NAME]
   Role: [ROLE FROM HIERARCHY TABLE]

   Action Required: Switch the model selector to [MODEL NAME], then reply with "Model Confirmed".
   ```

2. **STOP.** Do not output any further text or tool calls.
3. Wait for the user to reply.
4. **Verify the Passphrase:**
   - If the user replies "Model Confirmed" (case-insensitive) -> Proceed to Step 2b.
   - If the user replies with anything else -> Do **NOT** proceed. Remind them: "Please switch the model and reply with 'Model Confirmed' to continue."

### Step 2b: Verify Model Identity

Once the "Model Confirmed" passphrase is received:

1. **Ask the model to identify itself**:

   ```
   Before we begin: what model are you? State your name and version.
   ```

2. Compare the response against the expected model for this tier:
   - If the model **matches** the expected tier -> Proceed to Step 3.
   - If the model **does not match** ->
     1. Do NOT proceed.
     2. Announce: `⚠️ Model mismatch. Expected: [MODEL]. Got: [RESPONSE].`
     3. pause and wait for "Model Confirmed" again.

### Step 3: For Each Model Tier (T1 → T6)

With the correct model confirmed, provide the following prompt:

```
You are performing a structured review and improvement of the following plan document.

Your role in this review pass is: [ROLE FROM HIERARCHY TABLE]
You are: [EXPECTED MODEL NAME] (confirm this matches your identity before proceeding)

Your task:
1. Read the entire document carefully.
2. Identify ALL weaknesses, gaps, ambiguities, missing checks, and unclear criteria.
3. Produce a REVISED, IMPROVED version of the full document addressing every issue.
4. Do NOT summarize your changes — output the complete revised document.
5. Preserve all existing content that is correct and complete.
6. Append a one-line entry to the Refinement History table at the bottom of the document.

[PASTE FULL DOCUMENT CONTENTS HERE]
```

### Step 4: Save and Continue

After each model produces its output:

- Save the revised document, overwriting the previous version
- Confirm the Refinement History table was updated before proceeding to the next tier
- Announce completion: `✅ T[#] pass complete — [MODEL NAME]. Proceeding to T[#+1].`

### Step 5: Final Delivery

After T6 (Gemini 3 Pro High) completes its pass, the document is **finalized**.
Notify the user with:

- A summary of the key improvements made at each tier
- The path to the final document
- A recommendation on whether execution can begin

---

## Refinement History Format

Each model appends one row to this table at the bottom of the refined document:

```markdown
---

## Refinement History

| Pass | Model                      | Key Changes         |
| ---- | -------------------------- | ------------------- |
| T1   | Gemini 3 Flash             | [brief description] |
| T2   | Gemini 3 Pro (Low)         | [brief description] |
| T3   | GPT-OSS 12                 | [brief description] |
| T4   | Claude Sonnet 4.6          | [brief description] |
| T5   | Claude Opus 4.6 (Thinking) | [brief description] |
| T6   | Gemini 3 Pro (High)        | [brief description] |
```

---

## When to Apply This Skill

Apply this skill automatically when the user says any of the following:

- "iterate this through the models"
- "refine the plan with all AIs"
- "run this through the hierarchy"
- "improve the plan using all models"
- "phased plan refinement"
- `/refine-plan`

---

## Notes

- Always start from the **current saved version** of the document, not a summary.
- Each model must receive the **full document text**, not a truncated version.
- The user does not need to be present during intermediate passes — only the final
  T6 output requires user review.
- This skill applies to **any plan document**, not just Hegemon audit plans.
- **Always run Step 0 (dynamic model discovery) first.** Never assume the hierarchy
  is identical to a previous session.
