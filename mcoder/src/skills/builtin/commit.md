---
name: commit
description: Generate a well-structured git commit with conventional commit format. Use when the user asks to commit, save changes, or create a checkpoint.
when_to_use: "commit", "save changes", "checkpoint", "git commit"
disable_model_invocation: true
allowed_tools:
  - bash
  - read
---

You are creating a git commit for the current changes.

## Context

$ARGUMENTS

## Recent changes

!`git status --short`

## Steps

1. Review changes:
   - Run `git diff --staged` for staged changes
   - Run `git diff` for unstaged changes
   - If nothing is staged, stage relevant files with `git add <file>` (avoid `git add -A`)

2. Analyze the changes and determine the commit type:
   - `feat`: new feature
   - `fix`: bug fix
   - `refactor`: code restructuring without behavior change
   - `docs`: documentation only
   - `test`: adding/updating tests
   - `chore`: build/tooling changes
   - `perf`: performance improvement
   - `style`: formatting only

3. Write a concise commit message:
   - Subject line: `<type>(<scope>): <description>` (max 50 chars)
   - Body: explain WHY (not what), wrap at 72 chars
   - Reference issue numbers if relevant

4. Commit with `git commit -m "<message>"`

5. Show the final commit hash and summary

## Guardrails

- NEVER commit secrets (.env, credentials, API keys)
- NEVER use `--no-verify` or `--amend` unless explicitly asked
- Stage specific files, not `git add -A`
- Don't commit generated files (target/, dist/, node_modules/)
- If changes span multiple concerns, suggest splitting into multiple commits

## Verification

- [ ] No secrets in staged files
- [ ] Commit message follows conventional format
- [ ] Commit succeeded (hash returned)
