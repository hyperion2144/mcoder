---
name: review
description: Review code changes for quality, bugs, security, and best practices. Use when the user asks to review code, check a PR, or audit changes.
when_to_use: "review", "code review", "check PR", "audit", "inspect changes"
allowed_tools:
  - read
  - bash
  - grep
---

You are reviewing code.

## Target

$ARGUMENTS

## Steps

1. Identify the changes to review:
   - If target is a file path, read that file
   - If target is "staged" or empty, run `git diff --staged`
   - If target is a commit hash, run `git show <hash>`
   - If target is a branch, run `git diff main...<branch>`

2. Analyze the code for:

   ### Correctness
   - Logic errors, edge cases, off-by-one
   - Null/None handling
   - Error propagation
   - Race conditions

   ### Security
   - Injection (SQL, command, XSS)
   - Path traversal
   - Secret leakage
   - Unsafe deserialization

   ### Performance
   - O(n²) loops, unnecessary allocations
   - Missing indexes
   - Unbounded growth

   ### Style
   - Naming conventions
   - Dead code
   - Missing error handling
   - Inconsistent patterns

   ### Tests
   - Are the changes tested?
   - Are edge cases covered?
   - Do tests assert the right things?

3. Categorize findings:
   - **[BLOCKER]** Must fix before merge
   - **[WARNING]** Should fix, but not blocking
   - **[NIT]** Minor style/preference

4. Output a structured review with `file:line` references

## Guardrails

- Be specific: reference file paths and line numbers
- Suggest fixes, don't just point out problems
- Praise good patterns when seen
- Don't suggest changes outside the diff scope
- Don't block on nits

## Verification

- [ ] All BLOCKER issues documented
- [ ] Fixes suggested for each BLOCKER
- [ ] Overall verdict given (APPROVE / REQUEST_CHANGES / NEEDS_DISCUSSION)
