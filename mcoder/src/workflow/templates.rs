#![allow(dead_code)]

//! Artifact output templates - ported from specworkflow.
//!
//! Templates for output documents (proposal, design, tasks, spec, review, roadmap, config).
//! Each constant is a string with {{placeholder}} variables for rendering.

pub const PROPOSAL_TEMPLATE: &str = r#"# Proposal: {{name}}

<!--
  This is the human-AI agreement document. It captures WHY and WHAT, not HOW.
  The planner agent reads this to produce design.md, tasks.md, and delta specs.

  Quality bar:
  - Intent explains the problem with full context, not just the solution
  - Scope boundaries are explicit and justified
  - Each PR-N has Rationale documenting WHY decisions were made
  - Each PR-N documents Research, Alternatives, and Risks where applicable
  - Deliverables are observable (you can verify each one)
  - Each deliverable traces to a spec domain
-->


## Level

<!--
   risk-based level. Auto-assessed by propose, overridable by --level.
  trivial:  single file, docs/config/scaffolding, no behavior change
  light:    2-5 files, low-risk behavior change, good test coverage
  standard: cross-module, new behavior, medium risk (default)
  critical: auth/payment/data-consistency/core-path
-->

**Level**: {{level}}
**Auto-assessed**: {{auto-assessed-or-override}}
## Intent

<!--
  What problem does this change solve? Why now?
  Write as much as needed - this is the permanent record of the motivation.
  Don't describe the solution here - that goes in Approach.
-->

{{intent}}

## Scope

### In Scope

<!--
  What specific capabilities will this change add or modify?
  Be concrete: "Add theme toggle in header" not "Improve UI".
  List each item as a bullet.
-->

- {{item-1}}
- {{item-2}}

### Out of Scope

<!--
  What is explicitly NOT included? This prevents scope creep.
  Include things that might seem related but are deferred.
-->

- {{excluded-1}}
- {{excluded-2}}

## Research Landscape

<!--
  Optional. Cross-cutting research that informed the overall approach.
  Use when a single investigation affected multiple PR-Ns.
  Per-PR-specific findings belong in that PR-N's Research table.
-->

> This change was informed by investigation of:
> - {{source-1}}: {{finding}}
> - {{source-2}}: {{finding}}
> - {{source-3}}: {{finding}}

## Approach

<!--
  High-level method description. Write as much as needed to capture the strategy.
  Per-deliverable breakdown goes in each PR-N's Rationale section below.
  Don't include technical details (class names, library choices) - those go in design.md.
-->

{{approach}}

## Deliverables

<!--
  Each deliverable is an observable, verifiable capability.
  Split by user-visible behavior, not by implementation layer.

  Rules:
  - Each PR-N has a SHALL statement describing observable behavior
  - Each PR-N has a Verify method (command, test, or manual step)
  - Source traces to a spec domain (existing or new)
  - Keep PR count ≤ 5. If more, consider splitting this change.

  Per-PR-N sections:
  - Rationale: WHY this deliverable exists - discussion context, user feedback,
    problem analysis. This is the permanent record of the decision.
  - Research: What was investigated (libraries, docs, source code, external
    projects) and what was learned that influenced this deliverable.
  - Alternatives: What else was considered and why it was rejected.
  - Risks: Known risks specific to this deliverable and mitigation.
-->

### PR-1: {{deliverable-title}}

- **Domain**: specs/{{domain}}/spec.md
- **Behavior**: The system SHALL {{observable-behavior}}

**Rationale**:
<!--
  Why does this deliverable exist? What problem does it solve?
  What was discussed and decided? Capture the "why", not just the "what".
-->

{{rationale}}

**Research**:
<!--
  What external projects, libraries, documentation, or codebase analysis
  informed this deliverable? Include URLs, key findings, and impact on design.
  Skip if no research was needed for this deliverable.
-->

| Source | Finding | Impact |
|--------|---------|--------|
| {{url-or-path}} | {{finding}} | {{impact}} |

**Alternatives Considered**:
<!--
  What else was considered for this deliverable and why rejected.
  Skip if alternatives were not discussed.
-->

| Alternative | Reason Rejected |
|-------------|----------------|
| {{alt}} | {{reason}} |

**Risks & Mitigations**:
<!--
  Known risks specific to this deliverable.
  Skip if no risks were identified.
-->

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| {{risk}} | {{high/med/low}} | {{mitigation}} |

- **Verify**: {{verification-method}}
- **Files**: {{expected-file-paths}}

### PR-2: {{deliverable-title}}

- **Domain**: specs/{{domain}}/spec.md
- **Behavior**: The system SHALL {{observable-behavior}}

**Rationale**:
<!--
  Why does this deliverable exist? What problem does it solve?
  What was discussed and decided? Capture the "why", not just the "what".
-->

{{rationale}}

**Research**:
<!--
  What external projects, libraries, documentation, or codebase analysis
  informed this deliverable? Include URLs, key findings, and impact on design.
  Skip if no research was needed for this deliverable.
-->

| Source | Finding | Impact |
|--------|---------|--------|
| {{url-or-path}} | {{finding}} | {{impact}} |

**Alternatives Considered**:
<!--
  What else was considered for this deliverable and why rejected.
  Skip if alternatives were not discussed.
-->

| Alternative | Reason Rejected |
|-------------|----------------|
| {{alt}} | {{reason}} |

**Risks & Mitigations**:
<!--
  Known risks specific to this deliverable.
  Skip if no risks were identified.
-->

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| {{risk}} | {{high/med/low}} | {{mitigation}} |

- **Verify**: {{verification-method}}
- **Files**: {{expected-file-paths}}


## Dependencies

<!--
  Optional. If this change depends on another change being archived first,
  list it here. `workflow(action=continue)` will block apply until the dependency is archived.
  Format: - <change-name>
-->

- {{dependency-change}}

## Roadmap Reference

<!--
  Optional. If this change belongs to a milestone/phase in roadmap.md,
  reference it here. This helps track progress and prevent direction drift.
-->

- **Milestone**: {{milestone-name}}
- **Phase**: {{phase-name}}
"#;

pub const DESIGN_TEMPLATE: &str = r#"# Design: {{name}}

<!--
  Structured technical design. Produced by the planner agent.
  This is the blueprint executors follow - its quality determines implementation quality.

  Quality bar:
  - Every DS-N is a module boundary with single responsibility
  - Every D-N decision has real alternatives considered
  - Architecture diagram shows data flow, not just boxes
  - File manifest is complete (no "etc." or "and other files")
  - Every interface includes error responses
  - Every DS-N traces to a PR-N in proposal.md
-->

## Design Items

<!--
  Component decomposition. Each DS-N is a module boundary.
  One module = a cohesive set of functions/classes with a single responsibility.

  Rules:
  - Every PR-N in proposal.md must be referenced by at least one DS-N
  - Each DS-N has: refs (PR-N), Source (PR-N), Responsibility
  - A single PR may need multiple DS if it spans layers
  - Multiple PRs may share a DS if they modify the same module
-->

### DS-1: {{component-name}}

- **Refs**: PR-{{id}}
- **Source**: PR-{{id}} (proposal.md)
- **Responsibility**: {{what this component is responsible for - one sentence}}
- **Key Interfaces**: {{public functions/classes this component exposes}}

#### Detailed Design

<!--
  REQUIRED. Must be filled to a level where the executor can implement without guessing.
  Choose content appropriate to the component type:

  - Data/state components: internal state transitions, data structures, read/write paths, error scenarios
  - UI components: Props, events, states (loading/empty/error/success), layout constraints
  - API/CLI: parameter validation rules, response format, error codes, auth requirements
  - Tools/libraries: algorithm description, configuration options, input/output contracts

  Do NOT just repeat the Key Interfaces section.
  This is the executor's final reference during implementation.
-->

{{detailed-design}}

### DS-2: {{component-name}}

- **Refs**: PR-{{id}}, PR-{{id}}
- **Source**: PR-{{id}} (proposal.md)
- **Responsibility**: {{responsibility}}
- **Key Interfaces**: {{interfaces}}

#### Detailed Design

<!--
  REQUIRED. Must be filled to a level where the executor can implement without guessing.
  Choose content appropriate to the component type:

  - Data/state components: internal state transitions, data structures, read/write paths, error scenarios
  - UI components: Props, events, states (loading/empty/error/success), layout constraints
  - API/CLI: parameter validation rules, response format, error codes, auth requirements
  - Tools/libraries: algorithm description, configuration options, input/output contracts

  Do NOT just repeat the Key Interfaces section.
  This is the executor's final reference during implementation.
-->

{{detailed-design}}

## Architecture Decisions

<!--
  Record decisions that have real alternatives. Skip trivial choices.
  Each D-N must answer: What did you decide? Why? What else did you consider?

  Good: "Context over Redux - simple binary state, no complex transitions"
  Bad: "Use TypeScript - project uses TypeScript" (no alternative considered)
-->

### D-1: {{decision-title}}

- **Status**: ACCEPTED
- **Decision**: {{what was decided}}
- **Reason**: {{why this choice - include the constraint or tradeoff that drove it}}
- **Alternatives**: {{what else was considered and why rejected}}

### D-2: {{decision-title}}

- **Status**: ACCEPTED
- **Decision**: {{what was decided}}
- **Reason**: {{why}}
- **Alternatives**: {{rejected alternatives}}

## Technical Approach

### Architecture Diagram

<!--
  ASCII art showing component relationships for THIS CHANGE only.
  Annotate every node:
  - [NEW] - being created by this change
  - [MODIFIED] - existing, being changed
  - [EXISTING] - existing, not changed (for context)

  Show data flow with arrows. Don't draw the entire system.
-->

```text
{{architecture-diagram}}
```

### Core Data Structures

<!--
  Key types/interfaces introduced or modified.
  Use TypeScript interface format. Brief description per type.
  Only include types that are part of the component contract,
  not every internal type.
-->

```typescript
{{data-structures}}
```

### Data Flow

<!--
  Step-by-step flow from trigger to effect.
  Number each step. Include file paths for key operations.
-->

1. {{step-1}}
2. {{step-2}}
3. {{step-3}}

### Interface Design

<!--
  For each external-facing interface (API endpoint, CLI command, public function):
  - Full request/response schema
  - Error responses (not just happy path)
  - Source: trace to delta spec requirement

  If this change has no external interfaces, write "No external interfaces."
-->

#### {{endpoint-name}} `{{HTTP_METHOD}} {{path}}`

- **Headers**: {{required-headers}}
- **Request body**:
  ```json
  {{request-example}}
  ```
- **Response 200**:
  ```json
  {{response-example}}
  ```
- **Response 400**: {{error-description}}
- **Response 401**: {{error-description}}
- **Source**: specs/{{domain}}/spec.md#{{requirement-id}}

## External Dependencies

<!--
  External APIs, services, or libraries used by this change.
  Include full URL, auth method, and what it's used for.
  If none, write "No external dependencies."
-->

| Service | Base URL | Auth | Used For | Source |
|---------|----------|------|----------|--------|
| {{name}} | `{{url}}` | {{auth-method}} | {{purpose}} | DS-{{id}} |


## Impact Analysis

<!--
   7.2.1: Who depends on the changed code?
-->

### Direct Impacts

- {{modified-file}}: {{what-changes}}

### Indirect Impacts (callers/dependents)

- {{caller-file}}: imports {{modified-module}}, may need update

### Test Impacts

- {{test-file}}: tests {{modified-behavior}}, may break

## File Manifest

<!--
  EVERY file that will be created or modified.
  No "etc." or "and other files". If you forgot a file, the executor won't know about it.

  Action: Create | Modify | Delete
-->

| File Path | Description | Action | Source |
|-----------|-------------|--------|--------|
| `{{path}}` | {{description}} | Create | DS-{{id}} |
| `{{path}}` | {{description}} | Modify | DS-{{id}} |

## TDD Strategy

<!--
  How TDD applies to this change.
  - behavior tasks: RED (failing test) -> GREEN (minimal impl) -> REFACTOR
  - Other types: direct implementation
  Note any testing challenges or special setup needed.
-->

- **behavior tasks**: RED -> GREEN -> REFACTOR (3 commits per task)
- **config/scaffolding/docs**: direct implementation (1 commit per task)
- **refactor**: verify tests pass -> refactor -> verify again

{{testing-notes}}

## Risks

<!--
  Specific, actionable risks for THIS change.
  Not generic "might be slow" - say "localStorage write on every toggle may cause performance issues if toggled rapidly".

  Include mitigation for each risk.
  If no significant risks, write "No significant risks identified."
-->

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| {{risk-1}} | {{impact}} | {{likelihood}} | {{mitigation}} |
| {{risk-2}} | {{impact}} | {{likelihood}} | {{mitigation}} |
"#;

pub const TASKS_TEMPLATE: &str = r#"# Tasks: {{name}}

<!--
  Structured implementation checklist. Produced by the planner agent.
  Executors receive ONE wave at a time and implement its tasks via TDD.

  Quality bar:
  - Each task is independently testable (one behavioral path)
  - type:behavior tasks have RED descriptions (GIVEN/WHEN/THEN)
  - type:behavior tasks have spec_ref pointing to delta spec
  - Wave decomposition is based on real layer dependencies
  - depends_on is minimal (only when task B can't compile/test without task A)
  - Every DS-N in design.md is referenced by at least one task
-->

## TDD Type Annotations

| type | Meaning | TDD Protocol | Commit type |
|------|---------|-------------|-------------|
| `behavior` | Business behavior - observable, testable feature | RED -> GREEN -> REFACTOR | test + feat + refactor |
| `config` | Configuration - env vars, CI/CD, lint, tsconfig | Direct implementation | chore |
| `refactor` | Improve structure without changing behavior | Verify tests -> refactor -> verify | refactor |
| `docs` | Documentation - README, API docs, comments | Direct implementation | docs |
| `scaffolding` | Skeleton code - module shells, directory structure | Direct implementation | chore |

## Wave 1: {{theme}}

<!--
  Wave decomposition:
  - Default is 1 wave. Add more ONLY when tasks have layer dependencies.
  - Example of real layer dependency:
    Wave 1: data model + repository (can test independently)
    Wave 2: service layer (depends on Wave 1 models)
    Wave 3: API endpoints (depends on Wave 2 services)
  - Do NOT create multiple waves for tasks that are merely "related".
  - Each wave must be independently verifiable (tsc + tests pass after wave completes).
-->

- [ ] T-1: [type:behavior] {{task-title}} <!-- commit: -->
  - **refs**: DS-{{id}}
  - **spec_ref**: specs/{{domain}}/spec.md#{{requirement-id}}
  - **files**: {{file-path-1}}, {{file-path-1-test}}
  - **acceptance**: {{binary-criteria - e.g., "toggle() changes theme from 'light' to 'dark'"}}
  - **RED**:
    - **GIVEN** {{precondition}}
    - **WHEN** {{action}}
    - **THEN** {{observable-result}}
    - **AND** {{additional-assertion}}

- [ ] T-2: [type:behavior] {{task-title}} <!-- commit: -->
  - **refs**: DS-{{id}}
  - **spec_ref**: specs/{{domain}}/spec.md#{{requirement-id}}
  - **files**: {{file-path}}, {{file-path-test}}
  - **acceptance**: {{binary-criteria}}
  - **RED**:
    - **GIVEN** {{precondition}}
    - **WHEN** {{action}}
    - **THEN** {{observable-result}}
  - **depends_on**: T-1

- [ ] T-3: [type:scaffolding] {{task-title}} <!-- commit: -->
  - **refs**: DS-{{id}}
  - **files**: {{file-path}}
  - **acceptance**: {{criteria - e.g., "component file exists with correct imports"}}

## Wave 2: {{theme}}

<!--
  Only present if Wave 1 tasks are depended on by Wave 2 tasks.
  Remove this section if not needed.
-->

- [ ] T-4: [type:behavior] {{task-title}} <!-- commit: -->
  - **refs**: DS-{{id}}
  - **spec_ref**: specs/{{domain}}/spec.md#{{requirement-id}}
  - **files**: {{file-path}}, {{file-path-test}}
  - **acceptance**: {{binary-criteria}}
  - **RED**:
    - **GIVEN** {{precondition}}
    - **WHEN** {{action}}
    - **THEN** {{observable-result}}
  - **depends_on**: T-3

## Pre-Archive Checklist

<!--
  Verified by the orchestrator after all waves complete.
  These are the gates before review can run.
-->

- [ ] type-check/build passes with no errors
- [ ] test suite passes (per project test command)
- [ ] Every task in every wave is marked `[x]` with a commit hash
- [ ] No `{{` template placeholders remaining in any artifact
- [ ] All wave acceptance criteria confirmed
"#;

pub const SPEC_TEMPLATE: &str = r#"# Delta Spec: {{domain}}

<!--
  Behavioral contract for this change. Produced by the planner agent.
  This is NOT implementation documentation - it describes WHAT the system does, not HOW.

  Quality bar:
  - Requirements describe observable behavior (inputs, outputs, error conditions)
  - NOT implementation details (class names, library choices, function signatures)
  - Each requirement has at least 1 scenario (happy path)
  - Requirements with error conditions have error scenarios
  - SHALL/MUST used for absolute requirements, SHOULD for recommended, MAY for optional
  - MODIFIED requirements include the full new version + "was:" annotation
  - REMOVED requirements include the reason

  On archive:
  - ADDED -> appended to .mcoder/workflow/specs/<domain>/spec.md
  - MODIFIED -> replaces existing requirement in .mcoder/workflow/specs/<domain>/spec.md
  - REMOVED -> deleted from .mcoder/workflow/specs/<domain>/spec.md
-->

> Change: {{change-name}} | Domain: {{domain}}

## ADDED Requirements

<!--
  New behavior being introduced by this change.
  These will be appended to the global spec on archive.

  Requirement naming: use a noun phrase describing the capability.
  Good: "Theme Selection", "Two-Factor Authentication", "Session Expiration"
  Bad: "ThemeFeature", "2FA", "SessionStuff"
-->

### Requirement: {{requirement-name}}

The system SHALL {{behavior-description}}.

#### Scenario: {{scenario-name}}

- **GIVEN** {{precondition}}
- **WHEN** {{action}}
- **THEN** {{observable-result}}
- **AND** {{additional-assertion}}

#### Scenario: {{edge-case-name}}

- **GIVEN** {{edge-precondition}}
- **WHEN** {{edge-action}}
- **THEN** {{edge-result}}

#### Scenario: {{error-case-name}}

- **GIVEN** {{error-precondition}}
- **WHEN** {{error-action}}
- **THEN** {{error-result}}
- **AND** {{side-effect}}

## MODIFIED Requirements

<!--
  Existing behavior being changed.
  Include the FULL new requirement (not just the diff).
  Add "was:" annotation showing what changed.

  The requirement header MUST match the existing one in .mcoder/workflow/specs/<domain>/spec.md
  so the merge can find and replace it.
-->

### Requirement: {{existing-requirement-name}}

The system SHALL {{new-behavior}}.
(was: {{old-behavior-summary}})

#### Scenario: {{updated-scenario-name}}

- **GIVEN** {{precondition}}
- **WHEN** {{action}}
- **THEN** {{new-result}}

## REMOVED Requirements

<!--
  Existing behavior being removed.
  List the requirement header (must match global spec) and reason.
  Do NOT include scenarios - they're being deleted.

  Verify before removing:
  - No other code depends on this behavior
  - The removal is intentional, not accidental
-->

### Requirement: {{removed-requirement-name}}

**Reason**: {{why this behavior is being removed}}
"#;

pub const REVIEW_TEMPLATE: &str = r#"# Review: {{name}}

<!--
  Triple review result. Produced by the reviewer agent.
  This is the gate between apply and archive.

  Three dimensions:
  1. Spec Review (Spec Gate): delta spec requirements vs implementation
  2. Quality Review (Quality Gate): code bugs, security, conventions
  3. Goal Review (Goal Gate): proposal deliverables vs implementation

  Issue prefixes:
  - R-N: Spec non-compliance -> reapply (workflow(action=step, name=apply, fix=true))
  - Q-N: Quality issue -> reapply (workflow(action=step, name=apply, fix=true))
  - G-N: Goal not achieved -> reapply (workflow(action=step, name=apply, fix=true))
  - D-N: Design/architecture flaw -> replan (workflow(action=step, name=plan, fix=true))

  Verdict rules:
  - Zero issues -> PASS
  - Any D issue -> FAIL
  - Any BLOCKER severity -> FAIL
  - Only R/Q/G (no D, no BLOCKER) -> NEEDS_REVISION
-->


## Level Assessment

<!--
 : Reviewer can escalate if actual risk exceeds proposal's level.
-->

- **Proposal Level**: {{level}}
- **Reviewer Assessment**: {{same-or-escalated}}
- **Escalation**: {{none-or-Light-to-Critical-with-reason}}

## Approval

<!--
   7.2.5: Required for Critical-level changes when config.approvers is set.
-->

- Approved by: {{approver-name}}
- Date: {{approval-date}}

## Overall Verdict: {{PASS | FAIL | NEEDS_REVISION}}

---

## Spec Review

### Constraint Checklist

| # | Requirement | Type | Status | Evidence |
|---|-------------|------|--------|----------|
| R1 | {{requirement-name}} | ADDED | {{PASS/FAIL/N/A}} | {{file:line}} |
| R2 | {{requirement-name}} | MODIFIED | {{PASS/FAIL/N/A}} | {{file:line}} |
| R3 | {{requirement-name}} | REMOVED | {{PASS/FAIL/N/A}} | {{file:line}} |

### Scenario Coverage

| Scenario | Test Location | Status |
|----------|--------------|--------|
| {{scenario-name}} | {{test-file:line}} | PASS |
| {{scenario-name}} | {{test-file:line}} | PASS |
| {{scenario-name}} | - | MISSING |

### Spec Verdict: {{PASS | FAIL | NEEDS_REVISION}}

---

## Quality Review

### Issues

<!--
  Detail view: Quality issues found during review. Every issue listed here MUST also have
  a checkbox line in the global ## Issues section below (for fix tracking).
  The global ## Issues is the checkbox-based tracking source of truth.
-->

| # | Severity | Category | Location | Description | Fix |
|---|----------|----------|----------|-------------|-----|
| Q1 | {{BLOCKER/MAJOR/MINOR}} | {{Bug/Security/Convention/AI-Smell}} | {{file:line}} | {{specific-description}} | {{actionable-fix}} |

### Convention Compliance
---

## Goal Review

### Goal Checklist

| # | Deliverable | Status | Evidence |
|---|-------------|--------|----------|
| G1 | PR-1: {{deliverable-title}} | {{ACHIEVED/PARTIAL/NOT_ACHIEVED}} | {{evidence}} |
| G2 | PR-2: {{deliverable-title}} | {{ACHIEVED/PARTIAL/NOT_ACHIEVED}} | {{evidence}} |

### Goal Verdict: {{PASS | FAIL | NEEDS_REVISION}}

---


## Review History

<!--
  Auto-maintained by reviewer. Each re-review appends a row.
  Used by continue engine for diminishing-returns fuse detection.
-->

| Round | Date | New Issues | Blockers | Verdict |
|-------|------|------------|----------|---------|
| 1 | {{date}} | {{count}} | {{blockers}} | {{verdict}} |

## Issues

<!--
  Every finding gets ONE checkbox line: - [ ] R1 - description (source)
  Prefixes: R=spec, Q=quality, G=goal, D=design

  Three states:
  - [ ]  open (not fixed yet)
  - [~]  fixed, pending verification (set by executor after code fix)
  - [x]  verified and resolved (set by reviewer after re-review)

  The verdict MUST match the Issues section: any [ ] or [~] = not PASS.
-->

- [ ] R1 - {{spec requirement not implemented}} (spec)
- [ ] Q1 - {{code quality issue description}} (quality)
- [ ] G1 - {{goal not achieved}} (goal)
- [ ] D1 - {{design/architecture flaw}} (design)
<!-- Remove placeholder lines above. Add as many - [ ] lines as there are findings. -->

## Routing

- **D issues**: {{count}} ({{list or "none"}})
- **R/Q/G issues**: {{count}} ({{list or "none"}})

**Recommendation**: `workflow(action={{action}}, name={{name}})`
<!-- Advisory only. Orchestrator MUST ask the user before archiving, regardless of this recommendation. -->
"#;

pub const ROADMAP_TEMPLATE: &str = r#"# Roadmap: {{project-name}}

<!--
  Living document. Tracks project direction and progress.
  NOT a state machine - it doesn't gate change execution.

  Purpose:
  1. Make direction explicit (prevent drift)
  2. Track progress (count of archived changes per phase)
  3. Show what's planned next

  Updated automatically by `workflow(action=finalize)` (marks changes as [x], increments counts).
  Updated manually by editing roadmap.md (add milestones, phases, planned changes).

  Format rules:
  - Status tags: [NOT_STARTED], [ACTIVE], [IN_PROGRESS], [COMPLETED], [SHIPPED]
  - Milestone: M{id} (e.g., M1, M2)
  - Phase: P{milestone}.{id} (e.g., P1.1, P1.2) - full-structure milestones only
  - Change: listed under phase with [x] (done) or [ ] (pending)
  - All three layers share Goal / What / Deliverables / Outcomes fields
  - Full-structure milestone: phases + changes decomposed
  - Placeholder milestone: Goal/What/Deliverables/Outcomes + Key Decisions only (no phases)
  - Phase status must match between heading [STATUS] and **Status** line
  - Progress Summary: full-structure = numeric; placeholder = "-/-"
-->

## Milestone: M1 - {{milestone-name}} [ACTIVE]

**Goal**: {{product-level goal - what problem this milestone solves}}
**What**: {{work scope - brief summary of phase coverage}}
**Deliverables**: {{release-level artifacts - e.g. v2.0 CLI, docs site}}
**Outcomes**: {{verifiable release criteria - e.g. user can run workflow(action=init)->workflow(action=finalize) end-to-end}}
**Status**: {{PLANNED | ACTIVE | SHIPPED}}

<!--
  Add as many phases as needed (P1.1, P1.2, P1.3...).
  Phase count is driven by layer dependencies and scope, NOT a template limit.
  First phase = thinnest end-to-end path.
-->

### Phase: P1.1 - {{phase-name}} [{{STATUS}}]

- **Goal**: {{value or runnable artifact this phase delivers}}
- **What**: {{work involved - key areas, approach, constraints}}
- **Deliverables**: {{concrete artifacts - files, commands, features, tests}}
- **Outcomes**: {{verifiable result - prefer Given/When/Then or executable command}}
- **Depends on**: {{prior phase id, e.g. P1.1; none if first phase}}
- **Spec domain**: {{domain-name}}
- **Changes**: {{completed}}/{{total}} completed
- **Status**: {{NOT_STARTED | IN_PROGRESS | COMPLETED}}

### Key Decisions

<!-- Roadmap discussion outcomes: technical conventions, design decisions, constraints. Reference format: [P1.1-KD] decision summary -->

- [P1.1-KD] {{subject}} - {{decision}} (reason: {{why}}; alt: {{alternatives}})

**Changes**:

- [x] {{change-name}} (archived {{date}})
  - **Goal**: {{what this change achieves}}
  - **What**: {{work involved - key areas, approach, constraints}}
  - **Deliverables**: {{concrete artifacts produced}}
  - **Outcomes**: {{verifiable result after landing}}
  - **Depends on**: {{prior change name; none if first}}
- [ ] {{change-name}} (proposed {{date}})
  - **Goal**: {{what this change achieves}}
  - **What**: {{work involved - key areas, approach, constraints}}
  - **Deliverables**: {{concrete artifacts produced}}
  - **Outcomes**: {{verifiable result after landing}}
  - **Depends on**: {{prior change name; none if first}}

**Next**: {{next-change-or "All changes completed"}}

---

## Milestone: M2 - {{milestone-name}} [PLANNED]

<!--
  Placeholder milestone - direction known, but NOT fully discussed yet.
  DO NOT decompose into phases or list changes here.
  Promote to full structure (with phase decomposition) when discussion is complete.
-->

**Goal**: {{what this milestone aims to achieve - TBD OK if high-level}}
**What**: {{known scope or "TBD - details deferred"}}
**Deliverables**: {{known artifacts or "TBD"}}
**Outcomes**: {{known criteria or "TBD"}}
**Status**: PLANNED

### Key Decisions

<!-- Decisions discussed for this milestone, if any. Use milestone-level id since no phases yet. -->

- [M2-KD] {{subject}} - {{decision}} (reason: {{why}}; alt: {{alternatives}})

---

## Future Considerations

<!--
  Things discussed during roadmap planning that don't belong to a specific milestone yet.
  Prevents losing ideas that came up in conversation.
  Once scope solidifies, promote items here into a new milestone.
-->

- {{topic}} - {{notes}}

---

## Progress Summary

| Milestone | Phases | Changes | Status |
|-----------|--------|---------|--------|
| M1 - {{name}} | {{completed}}/{{total}} | {{archived}}/{{total}} | {{status}} |
| M2 - {{name}} | -/- | -/- | PLANNED |
"#;

pub const CONFIG_TEMPLATE: &str = r#"# Blueprint Project Configuration (v2)
# Generated by workflow(action=init) - {{date}}

version: 2
platform:
  - omp
  - claude-code
  - agent
  - codex

# Workflow profile - controls rigor vs speed
# lite:     no review gate, TDD optional, single agent for lightweight changes
# standard: review gate (must PASS before archive), TDD for behavior, sub-agent waves
profile: standard

# Workflow version - controls process intensity
# 2.1: current (four-level grading, budget, fuse)
# 2.0: legacy (lite/standard binary)
workflow_version: '0.6.1'

# Brownfield project (existing codebase with code scanning)
brownfield: false

# Auto-commit documentation files alongside code
commitDocs: false

# Project context - injected into ALL sub-agent prompts
context: |
  Project: {{project-name}}
  Tech stack: {{tech-stack}}
  Testing: {{test-framework}}
  Artifact Language: {{artifact-language}}

# Artifact rules - injected into specific sub-agent prompts
rules:
  proposal:
    - "Each deliverable must have an observable SHALL statement and a Verify method"
    - "Keep PR count ≤ 5 per change; if more, suggest splitting"
  specs:
    - "Use Given/When/Then format for all scenarios"
    - "Each Requirement must have at least 1 scenario"
    - "Use MUST for security/data-integrity, SHOULD for UX, MAY for optional features"
  design:
    - "Each DS-N must have Source: PR-{id} annotation tracing to proposal"
    - "Architecture diagram must annotate [NEW]/[MODIFIED]/[EXISTING]"
    - "File manifest must list every file - no 'etc.' or 'and other files'"
    - "Every interface must include error responses, not just happy path"
  tasks:
    - "type:behavior tasks must have RED test description (GIVEN/WHEN/THEN)"
    - "depends_on only when task B cannot compile/test without task A"
    - "Default to 1 wave; add waves only for real layer dependencies"
    - "acceptance criteria must be binary (pass/fail), not subjective"

# Default schema - defines artifact dependency graph
schema: spec-driven

# Model configuration - maps roles to platform model tiers
models: {}

# Conventions injection
conventions:
  inject: true

# Git configuration
git:
  create_tag: true

# Critical change approvers - who can approve Critical-level changes
# Empty = anyone can approve. Set to restrict Critical review to specific users.
approvers: []

# Budget controls - per-change cost and convergence limits
# Prevents runaway sub-agent dispatch and infinite review-fix loops
budget:
  max_subagent_runs: 5        # Sub-agent dispatch count limit per change
  max_review_rounds: 3        # Max review-fix loop rounds
  max_wall_time_min: 60       # Wall clock limit (minutes)
  estimated_token_cap: 500000 # Soft token cap (warning)
  no_progress_fuse_rounds: 2   # Consecutive no-progress rounds before auto-stop
"#;

pub const GLOBAL_SPEC_TEMPLATE: &str = r#"# Global Spec: {{domain}}

> Accumulated behavioral contract for the {{domain}} domain.

## Purpose

The {{domain}} domain governs the {{domain-purpose}} aspects of the system. This spec accumulates all behavioral requirements for this domain as changes are archived.

Requirements follow RFC 2119: MUST/SHALL for absolute requirements, SHOULD for recommended, MAY for optional capabilities.

## Requirements

### Requirement: {{requirement-name-1}}

The system SHALL {{behavior-description}}.

#### Scenario: {{scenario-name}}

- **GIVEN** {{precondition}}
- **WHEN** {{action}}
- **THEN** {{observable-result}}

### Requirement: {{requirement-name-2}}

The system SHALL {{behavior-description}}.

#### Scenario: {{scenario-name}}

- **GIVEN** {{precondition}}
- **WHEN** {{action}}
- **THEN** {{observable-result}}
"#;

/// Lookup a template by its type name.
///
/// Returns one of: proposal, design, tasks, spec, review, roadmap, config, global-spec.
pub fn get_template(type_name: &str) -> Option<&'static str> {
    match type_name {
        "proposal" => Some(PROPOSAL_TEMPLATE),
        "design" => Some(DESIGN_TEMPLATE),
        "tasks" => Some(TASKS_TEMPLATE),
        "spec" => Some(SPEC_TEMPLATE),
        "review" => Some(REVIEW_TEMPLATE),
        "roadmap" => Some(ROADMAP_TEMPLATE),
        "config" => Some(CONFIG_TEMPLATE),
        "global-spec" => Some(GLOBAL_SPEC_TEMPLATE),
        _ => None,
    }
}
