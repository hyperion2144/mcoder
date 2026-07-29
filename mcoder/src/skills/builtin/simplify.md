---
name: simplify
description: Simplify and clean up code. Use when the user asks to refactor, clean up, simplify, or improve code readability without changing behavior.
when_to_use: "simplify", "refactor", "clean up", "improve readability", "reduce complexity"
allowed_tools:
  - read
  - edit
  - grep
  - ast_rename
  - ast_extract
  - ast_inline
---

You are simplifying code. The goal is to reduce complexity WITHOUT changing behavior.

## Target

$ARGUMENTS

## Principles

1. **Remove dead code**: Unused variables, functions, imports, commented-out code.
2. **Extract functions**: Long functions (>40 lines) should be broken into named helpers.
3. **Inline throwaway variables**: If a variable is used once, consider inlining.
4. **Simplify conditionals**:
   - Flatten nested if/else (guard clauses).
   - Remove redundant checks.
   - Use early returns.
5. **Rename for clarity**: Names should describe what, not how.
6. **Remove duplication**: If the same logic appears 3+ times, extract it.

## Steps

1. Read the target code and identify complexity hotspots.
2. Make ONE change at a time.
3. After each change, verify tests still pass.
4. Commit after each verified simplification.

## Guardrails

- NEVER change behavior. If tests break, you changed behavior — revert.
- NEVER add features during simplification.
- NEVER change public APIs.
- Don't simplify for the sake of it — only simplify what's actually complex.
- Don't add abstractions you don't need yet.
- One change per commit. Don't batch unrelated simplifications.

## Verification

- [ ] All existing tests pass
- [ ] No behavior change (same inputs → same outputs)
- [ ] Code is measurably simpler (fewer lines, lower nesting, clearer names)
- [ ] No new abstractions added unless removing duplication
