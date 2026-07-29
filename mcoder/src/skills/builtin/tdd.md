---
name: tdd
description: Test-Driven Development workflow. Use when implementing new features, fixing bugs, or whenever tests should drive the design. Writes a failing test first, then minimal implementation, then refactors.
when_to_use: "implement a feature", "write tests", "fix a bug with tests", "TDD"
allowed_tools:
  - read
  - write
  - edit
  - bash
  - grep
---

You are following the TDD (Test-Driven Development) workflow.

## Input

$ARGUMENTS

## Core Process

1. **RED**: Understand the requirement and write a failing test that captures the expected behavior.
   - Run the test to confirm it fails for the right reason (not a syntax error).
2. **GREEN**: Write the minimal implementation to make the test pass.
   - Don't add extra logic — just enough to pass.
3. **REFACTOR**: Improve the code while keeping the test green.
   - Rename variables, extract functions, remove duplication.
4. **VERIFY**: Run the full test suite to ensure no regressions.
5. **COMMIT**: Commit after each green test.

## Guardrails

- ALWAYS write the test BEFORE the implementation.
- NEVER skip the RED step — confirm the test fails first.
- Write ONE test at a time. Don't batch.
- Commit after each RED→GREEN cycle, not at the end.
- If the test passes immediately, your test is wrong — delete it and rewrite.
- Don't over-engineer. Write the minimum code to pass the current test.

## Verification Checklist

- [ ] Test fails before implementation (RED confirmed)
- [ ] Test passes after implementation (GREEN confirmed)
- [ ] No other tests broken
- [ ] Code refactored for clarity
- [ ] Committed with clear message
