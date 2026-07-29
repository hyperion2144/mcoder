---
name: debug
description: Systematic debugging workflow. Use when the user reports a bug, test failure, or unexpected behavior. Follows reproduce → minimize → hypothesize → instrument → fix → regression-test loop.
when_to_use: "debug", "bug", "error", "crash", "test failure", "not working", "unexpected behavior"
allowed_tools:
  - read
  - bash
  - edit
  - grep
  - graph_query
---

You are debugging an issue. Follow the disciplined diagnosis loop.

## Problem

$ARGUMENTS

## Diagnosis Loop

1. **REPRODUCE**: Confirm you can reproduce the bug.
   - Run the exact steps or command that triggers it.
   - If you can't reproduce, ask for more context (OS, version, steps).

2. **MINIMIZE**: Reduce the reproduction to the smallest case.
   - Strip unrelated code paths.
   - Isolate the failing input.

3. **HYPOTHESIZE**: Form a hypothesis about the root cause.
   - State it explicitly: "I believe the bug is in X because Y."
   - Don't skip this step — guessing leads to wasted effort.

4. **INSTRUMENT**: Add logging or use a debugger to verify the hypothesis.
   - Read the relevant code first (use `read` / `grep` / `graph_query`).
   - Add minimal `println!`/`console.log`/`print()` at the suspected location.
   - Re-run the reproduction.

5. **FIX**: Apply the minimal fix that addresses the root cause.
   - Don't fix symptoms. Fix the cause.
   - Don't refactor surrounding code — that's a separate task.

6. **REGRESSION TEST**: Write a test that would have caught this bug.
   - Run the test against the OLD code to confirm it fails.
   - Run the test against the NEW code to confirm it passes.

## Guardrails

- NEVER fix without reproducing first.
- NEVER fix without a stated hypothesis.
- NEVER fix symptoms — find the root cause.
- Don't clean up surrounding code. A bug fix is not a refactor.
- Don't add error handling for impossible states.
- If the fix doesn't work, revert and form a new hypothesis. Don't pile on changes.

## Verification

- [ ] Bug reproduced before fix
- [ ] Root cause identified (not just symptoms)
- [ ] Minimal fix applied
- [ ] Regression test added
- [ ] Regression test fails on old code, passes on new code
- [ ] No new test failures introduced
