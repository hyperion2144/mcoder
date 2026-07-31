#![allow(dead_code)]

const ORCHESTRATOR_RULE: &str =
    "**You are the orchestrator - dispatch sub-agents; do not do their work yourself.**\n\n";

const CONTEXT_REMINDER: &str = "### Context injection\n\nContext is auto-injected by mcoder at session_start and after compaction. Do NOT call `workflow(action=context)` yourself - the system already provides the same material.\n\n";

pub fn init_prompt() -> String {
    r##"## Input

- User has already run `/workflow init`. All project settings are configured.
- `.mcoder/workflow/config.yaml` exists and is fully configured.

## Steps

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow init` only outputs these steps - it does not auto-execute.

### Step 1: Check project type

Read `.mcoder/workflow/config.yaml` - check the `brownfield` field.

**Greenfield (`brownfield: false`):**
Continue to Step 2.

**Brownfield (`brownfield: true`):**
Skip to Step 3.

### Step 2: Write coding conventions (greenfield)

Read `.mcoder/workflow/conventions/coding.md`. If it is empty or only has a template header, fill it in based on the tech stack from `.mcoder/workflow/config.yaml`:

- Read `.mcoder/workflow/config.yaml` for tech stack context
- Read the project root for config files (tsconfig.json, eslint.config.js, etc.)
- Write conventions covering:
  - **Naming**: file naming (kebab-case), function naming (camelCase), type naming (PascalCase), constants (UPPER_SNAKE_CASE)
  - **Code style**: indentation, quotes, semicolons, max line length (from project config)
  - **Imports**: ordering, path aliases, barrel exports
  - **Error handling**: try/catch pattern, error types, logging
  - **Testing**: test framework, file naming (*.test.ts), test structure (describe/it)
  - **Types**: strictness level, type vs interface preference

No need to ask the user - derive conventions from the project's existing config files and tech stack.

### Step 3: Brownfield scan (brownfield only)

Dispatch a **codebase-scanner** sub-agent to analyze the existing codebase and extract behavioral contracts into `.mcoder/workflow/specs/`.

1. Prepare scanner context:
   - Project root directory path
   - `.mcoder/workflow/config.yaml` path
   - Instruction: "Read the codebase-scanner agent prompt, then scan the source code and write spec files to `.mcoder/workflow/specs/<domain>/spec.md`"

2. Dispatch via `subagent(op=spawn, role=codebase-scanner)`:
   - Fresh context: yes
   - Isolated: no (scanner is read-only on source code, writes only to `.mcoder/workflow/specs/`)

3. Wait for scanner to complete.

4. Verify output:
   - Check that `.mcoder/workflow/specs/` has at least 1 domain directory with spec.md
   - Each spec.md has ## Purpose and ## Requirements sections
   - Each requirement uses SHALL/MUST and has at least 1 scenario

### Step 4: Verify coding conventions

Check `.mcoder/workflow/conventions/coding.md`:
- Has real content (not just template header)
- Covers naming, code style, imports, error handling, testing
- It is written to `.mcoder/workflow/conventions/coding.md`

### Step 3b: Verify codebase map

Check that `.mcoder/workflow/.codebase-map.json` exists (run `graph_index(path=".")` if missing). The planner queries the map on-demand via `graph_search(action=symbol, pattern="")` and `graph_search(action=symbol, pattern="<name>")` during plan.

### Step 5: Suggest next step

Suggest running `/workflow continue`:

```
Project initialized. Run `/workflow continue` to check project status and discover next steps.
```

## Guardrails

- NEVER re-ask configuration questions - the init CLI already handled profile, platform, etc.
- NEVER run `/workflow init` or `/workflow update` - user did this already
- Brownfield: dispatch codebase-scanner sub-agent. Do NOT scan code yourself.
- Greenfield: write coding conventions into `.mcoder/workflow/conventions/coding.md`, verify specs exist
- ALWAYS suggest `/workflow continue`
"##.to_string()
}

pub fn propose_prompt(change_name: &str) -> String {
    let body = format!(
        r##"## Input

- **`{name}`** (required): change name (kebab-case)
- **`--phase <milestone>/<phase>`** (optional): reference a roadmap phase

## Steps

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow propose` only outputs these steps - it does not auto-execute.

### Step 0: Risk assessment and level assignment

Assess the change's risk level based on scope and failure cost:

- **Trivial**: single file, docs/config/scaffolding only, no behavior change -> inline execution, no sub-agents
- **Light**: 2-5 files, low-risk behavior change, good test coverage -> single agent, TDD optional
- **Standard**: cross-module, new behavior, medium risk (DEFAULT) -> planner + wave executor + triple review
- **Critical**: involves auth/payment/data-consistency/core-path -> full flow + security audit + human approval gate

Auto-assess based on the user's described scope. If --level <X> provided, use that instead.
Write the level to proposal.md's ## Level section.

**If trivial or light**: may skip Step 1 (grill) per existing lightweight logic. Go directly to Step 2 with a minimal proposal derived from the user's one-line description. Fill template directly, no interview.
**If standard or critical**: continue to Step 1 (relentless interview).
**If critical**: flag for security audit in design.md.

### Step 1: Grill the user on requirements (Skip if Step 0 classified as trivial or light) (RELENTLESS - do NOT skip)

Before writing anything, you must reach FULL shared understanding with the user.
This is NOT a checklist. It is a relentless interview that walks every branch of the decision tree,
resolving dependencies between decisions one by one.

Process:
1. Start with what the user described. Map the decision tree in your mind:
   every choice, dependency, edge case, scope boundary, and unknown.
2. Pick the first unresolved branch. Ask ONE focused question about it.
   **Provide your recommended answer** so the user can just confirm or correct.
3. If the question can be answered by exploring the codebase, explore it yourself - do NOT ask the user.
4. After the user answers, check if their answer opened new branches. If so, ask about those next.
5. Repeat until every branch is resolved and you have shared understanding.

What to grill on (walk every branch):
- **Problem**: What problem does this change solve? Why now?
- **Scope**: What is in scope? What is explicitly excluded? Where does this change stop?
- **Deliverables**: What observable behaviors? What inputs/outputs? What error conditions?
- **Approach**: What technical approach? What alternatives were considered? Why this one?
- **Research**: What needs investigation during discussion (libraries, existing code, external projects)? Track these for Step 1b.
- **Edge cases**: What happens when input is invalid? Empty? Concurrent? Large scale?
- **Dependencies**: Does this depend on existing code? Other changes? External services?
- **Constraints**: Performance targets? Library choices? Backwards compatibility?
- **Roadmap context**: If --phase provided, how does this align with the phase goal?

**Hard rules:**
- Ask ONE question at a time. Wait for the answer. Do not batch.
- Always provide a recommended answer when one exists.
- Do NOT proceed to Step 2 until you can describe every deliverable without guessing.
- Do NOT use [ASSUMPTION] tags. If you are about to assume, STOP and ask instead.
- If the user says "use your best judgment" on a specific point, you may proceed without asking.

### Step 1b: Technical research

For non-trivial changes (standard or critical), research the technical landscape
before writing the proposal. This ensures external references and codebase
information are captured in the proposal, not lost after the discussion.

> **Skip this step for trivial/light changes** - go directly to Step 2.

1. **Codebase patterns** - Read relevant source files referenced in discussion.
   What conventions, APIs, or constraints exist?
2. **External projects/references** - If the discussion mentioned specific
   libraries, projects, or documentation URLs, read them. Do NOT rely on
   training data for technical details.
3. **Call-site analysis** - For modifications, use LSP references or grep
   to find current callers of code to be changed.
4. **Web research** - Use web_search for anything unresolved.

**Document as you go.** Keep notes per deliverable - each finding will go into
its PR-N's Research table. If a finding affects multiple PR-Ns, note it for
## Research Landscape instead.

**If you cannot find the information needed**, return to the user with specific
questions: "To confirm the PR-1 approach, I need to check X - found A and B but
not C. Can you point me to C?"

### Step 2: Create change directory

```bash
mkdir -p .mcoder/workflow/changes/{name}
```

If `--phase` is provided, note the milestone/phase for the proposal's Roadmap Reference section.

### Step 3: Write the detailed proposal

Get the proposal template and fill it based on the discussion and research:

1. Run `workflow(action=template, type=proposal)` to get the template
2. Fill EVERY section, following these rules:

   **Intent** - Write as much as needed. This is the permanent record of the
   motivation. Include the problem context, why now, and what triggered the change.

   **Scope (In/Out)** - Be precise. "Support GitHub OAuth login" not "improve auth".
   List concrete capabilities in In Scope, explicit exclusions in Out of Scope.

   **Research Landscape** - Use when a single investigation (e.g. reading a library's
   docs) affected multiple PR-Ns. Per-PR-specific findings go in that PR-N's table.
   Skip this section if no cross-cutting research was done.

   **Approach** - High-level strategy. Per-deliverable breakdown goes in each PR-N's
   Rationale section. Write as much as needed.

   **Deliverables (PR-N)** - This is the core. Fill ALL sub-fields for each PR-N:

   - **Behavior**: The SHALL statement - observable capability.
   - **Rationale**: WHY this deliverable exists. Capture what was discussed:
     user pain points, tradeoff conclusions, decision context. This is the
     permanent record - someone reading this 3 months later should understand
     why this choice was made.
   - **Research**: Per-deliverable research findings from Step 1b. What was
     checked, what was found, how it affected the design. Skip if no research
     was needed for this deliverable.
   - **Alternatives Considered**: What else was discussed and why rejected.
     Skip if no alternatives were discussed for this deliverable.
   - **Risks & Mitigations**: Known risks identified during discussion.
     Skip if no risks were identified.
   - **Verify**: How to verify this deliverable works.
   - **Files**: Expected file paths (new or modified).

   **Dependencies** / **Roadmap Reference** - Fill if applicable.

3. Write to `.mcoder/workflow/changes/{name}/proposal.md`

### Step 4: Verify proposal quality

Before finishing, check:
- [ ] Intent clearly states the problem with full context
- [ ] Scope has both In Scope and Out of Scope sections
- [ ] Each PR-N has ALL sub-fields filled (Behavior, Rationale, Verify)
- [ ] Each PR-N has Research filled IF research was done during discussion/Step 1b
- [ ] Each PR-N has Alternatives Considered filled IF alternatives were discussed
- [ ] Each PR-N has Risks & Mitigations filled IF risks were identified
- [ ] Template placeholders replaced - no unreplaced template variables remain
- [ ] PR count <= 5 (if more, suggest splitting)
- [ ] The proposal captures the user's actual requirements, not AI guesswork
- [ ] (Optional) Research Landscape filled IF cross-cutting research was done

### Step 5: Commit and suggest next step

```bash
# Update roadmap: If proposal has `## Roadmap Reference`, read `.mcoder/workflow/roadmap.md`, find corresponding phase, add `- [ ] {name}` to its Changes list if not already present.
bash(commands=["git add -A", "git commit -m 'docs(proposal): {name}'"])
```

Output:
```
Created .mcoder/workflow/changes/{name}/proposal.md
  Proposal is ready for planning.

  Next: /workflow plan {name}
  (or: /workflow continue)
```

## Guardrails

- **ALWAYS discuss with the user before writing.** Do not guess the requirements.
- **ALWAYS research what was discussed.** Step 1b is mandatory for standard/critical changes.
- **DO write the proposal in detail.** The proposal is the permanent record of the
  discussion - lost details cannot be recovered later. Every PR-N's Rationale matters.
- Do NOT create design.md, tasks.md, or specs/ - that's the planner's job
- Do NOT run /workflow plan automatically - let the user review the proposal first
- If the user wants to skip proposal review and go straight to planning, they can run /workflow plan {name} directly
- Architecture decisions and technical design come from the planner, not from propose
"##,
        name = change_name
    );
    format!("{}{}{}", ORCHESTRATOR_RULE, CONTEXT_REMINDER, body)
}

pub fn plan_prompt(change_name: &str, fix_mode: bool) -> String {
    let fix_note = if fix_mode {
        "**FIX MODE ACTIVE** - The `--fix` flag is set. Follow all fix-mode instructions in the steps below. The planner must read review.md and focus on D-prefixed issues.\n\n"
    } else {
        ""
    };
    let body = format!(
        r##"## Input

- **`{name}`** (optional): change name. If empty, use most recently proposed change.
- **`--fix`** (optional): fix mode - planner reads review.md D-issues and redesigns.

## Prerequisites

- `proposal.md` exists in change directory and is not a template

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow plan` only outputs these steps - it does not auto-execute. Codebase queries and impact analysis are done by the planner sub-agent.

### Step 1: Resolve change name and paths

If `{name}` is empty:
- List `.mcoder/workflow/changes/` for active changes (not in `archive/`)
- If multiple exist, ask the user which one
- If none exist, suggest `/workflow propose <name>`

Change directory: `.mcoder/workflow/changes/{name}/`

### Step 2: Classify change (lightweight vs full)

Read `proposal.md` deliverables:
- **Lightweight**: All deliverables are config/docs/refactor/scaffolding (no new behavior)
- **Full**: Any deliverable introduces new behavior

### Step 3: Dispatch planner (Full mode)

**If FULL: dispatch planner sub-agent. Do NOT write design/tasks/specs yourself.**

1. Prepare planner context:
   - Change name and directory path
   - List files to read: proposal.md, .mcoder/workflow/specs/<domain>/spec.md (per affected domain), .mcoder/workflow/conventions/coding.md, .mcoder/workflow/config.yaml
   - Instruction: "Read planner agent prompt, produce design.md, tasks.md, and specs/<domain>/spec.md (delta specs under the change directory, NOT .mcoder/workflow/specs/)"
   - In --fix mode: also include review.md, focus on D-prefixed issues

2. Dispatch via `subagent(op=spawn, role=planner)`:
   - Fresh context: yes
   - Isolated: no (planner is read-only on source code, writes only to change directory)

   The planner sub-agent will:
   - Query the codebase map (`graph_search(action=symbol, pattern="")`, `graph_search(action=symbol, pattern="<name>")`, `graph_relations(direction=callers, symbol="<module>")`) for module structure and dependencies
   - Perform impact analysis and write `## Impact Analysis` section in design.md
   - Produce design.md, tasks.md, and delta specs

3. Wait for planner to complete.

**If LIGHTWEIGHT:**
1. Fill design.md template directly
2. Fill tasks.md with 1 wave
3. No delta specs needed (no behavioral changes)

### Step 4: Review planner output for design quality

Before committing, review the planner's design across FIVE content-quality dimensions. These are NOT format checks (Step 5 handles format) - they assess whether the design is correct, complete, and implementable. A flawed design cascades into implementation failure.

For each dimension, ask the specific questions. If ANY fails, re-dispatch the planner with structured feedback (dimension + specific problem + expected state).

#### Dimension 1: Implementability (can the executor build it without guessing?)

For each DS-N, read its Detailed Design section and ask:
- Are interface signatures complete (parameters, return types, types)?
- For data/state components: are internal state transitions, data structures, and read/write paths described?
- For UI components: are Props, events, and all states (loading/empty/error/success) listed?
- For API/CLI: are parameter validation rules, response format, and error codes specified?
- Are error paths and side effects described (not just the happy path)?
- Does Detailed Design add implementation detail beyond Key Interfaces, or does it just repeat them?

FAIL example: DS-N Detailed Design says only "Implement ThemeContext class" - no state fields, no toggle logic, no persistence strategy. The executor would have to guess everything.

#### Dimension 2: Design Correctness (is the architecture internally consistent?)

- Do DS-N dependencies match the Architecture Diagram's arrows?
- Does the Data Flow section cover every DS-N involved in the flow?
- Do [NEW]/[MODIFIED]/[EXISTING] annotations in the diagram match the File Manifest's Action column?
- Do Core Data Structures match the types used in DS-N Key Interfaces?
- Are there circular dependencies or missing intermediate components?

FAIL example: Architecture Diagram shows DS-2 depends on DS-1, but DS-2's Key Interfaces reference an export that DS-1 does not provide.

#### Dimension 3: Decision Completeness (are all real technical choices recorded?)

Check whether every technical choice with genuine alternatives has a D-N decision record:
- State management approach (Context/Redux/Zustand/...)
- Error handling strategy (try-catch/Result type/either/...)
- Data persistence mechanism (localStorage/IndexedDB/cookie/...)
- Async/concurrency pattern (callbacks/promises/async-await/observables/...)
- Any external dependency introduction (recorded in External Dependencies table?)

For each D-N, verify:
- Reason states the driving constraint/tradeoff (not just "project uses X")
- Alternatives were genuinely considered (not filler like "could also use Y")

FAIL example: Design introduces localStorage persistence but has no D-N deciding "why not cookie/IndexedDB", and no D-N on "sync write vs debounced write".

#### Dimension 4: Impact Completeness (did the planner find all downstream effects?)

- Did the planner run `graph_relations(direction=callers, symbol="<module>")` for each modified module? (check the Impact Analysis section references impact queries)
- Direct Impacts: does every File Manifest "Modify" entry appear here with a change description?
- Indirect Impacts: are callers/dependents listed? If a public export's signature changes, Indirect Impacts MUST be non-empty.
- Test Impacts: are existing tests that may break identified?
- Is there a modified public export with empty Indirect Impacts? (likely a missed `graph_relations(direction=callers, ...)` query)

FAIL example: File Manifest modifies src/core/auth.ts login() signature, but Indirect Impacts is empty (planner didn't run `graph_relations(direction=callers, symbol="auth")`).

#### Dimension 5: File Manifest Consistency (does every file trace to a component?)

- Does every DS-N have at least one File Manifest entry with Source: DS-N?
- Does every File Manifest row's Source point to an existing DS-N?
- Are there orphan files (in Manifest but no DS claims them) or orphan components (DS with no files)?
- Any "etc." / "and other files" / "..." vague references? (must be exhaustive)

FAIL example: DS-3 claims responsibility for ThemePersistence, but no File Manifest row has Source: DS-3.

#### If problems found

Re-dispatch the planner with structured feedback per finding:
- Dimension: <1-5 name>
- DS-N / file: <which component or file>
- Problem: <what's wrong>
- Expected: <what the design should show>

After re-dispatch, return to Step 4 to review the updated output. Repeat until the design passes all five dimensions. Do NOT proceed with a flawed design - it will cascade into implementation failures.

### Step 5: Verify output

**Traceability:**
- Every PR-N in proposal.md referenced by at least one DS-N in design.md
- Every DS-N in design.md referenced by at least one T-N in tasks.md
- Every type:behavior task has `spec_ref` pointing to delta spec

**Completeness:**
- design.md has: Design Items, Architecture Decisions, Technical Approach, File Manifest, Impact Analysis
- tasks.md has: TDD Type Annotations, at least 1 Wave, Pre-Archive Checklist
- Delta specs exist for affected domain (specs/<domain>/spec.md)
- Delta specs use correct sections (ADDED/MODIFIED/REMOVED)
- File manifest lists every file (no "etc.")

**Structural Completeness** (format checks - content quality is covered in Step 4):
- No template placeholders remaining in any file
- type:behavior tasks have RED descriptions (GIVEN/WHEN/THEN)
- Requirements use SHALL/MUST/SHOULD correctly
- Each requirement has at least 1 scenario

If any check fails: re-dispatch planner with specific feedback on what's missing.

### Step 6: Task granularity check

After planner produces tasks.md, check granularity and warn if too large:

- tasks total > 20 -> warn: 'Change may be too large. Consider splitting into multiple changes.'
- files in File Manifest > 15 -> warn: 'File manifest exceeds 15 files. Consider splitting.'
- wave count > 5 -> warn: 'Excessive wave decomposition. Consolidate independent tasks.'
- single wave task count > 8 -> warn: 'Wave too large. Split into multiple waves.'

### Step 7: Commit and suggest next step

```bash
# Update roadmap: If the change's proposal.md has `## Roadmap Reference`, read `.mcoder/workflow/roadmap.md`, find the change in that phase's Changes list, and update it to `- [-] {name} (planned YYYY-MM-DD)`.
bash(commands=["git add -A", "git commit -m 'docs(plan): design + tasks + delta specs for {name}'"])
```
  Next: /workflow apply {name}
  (or: /workflow continue)

Output:
```
Planner completed for {name}
  - design.md: N design items, N decisions
  - tasks.md: N tasks in N wave(s)
  - specs/: N delta spec(s)

  Next: /workflow apply {name}
  (or: /workflow continue)
```

## Guardrails

- **Context is auto-injected by mcoder at session_start and after compaction.** Do NOT call `workflow(action=context)` yourself.
- **Full mode: MUST dispatch sub-agent.** Do NOT write design/tasks/specs yourself.
- Lightweight mode: write templates directly (no sub-agent needed)
- tasks.md boxes must remain UNCHECKED
- In --fix mode: planner only redesigns - does NOT modify tasks.md or specs
- **Review planner output before committing.** If design is flawed, re-dispatch planner - do not proceed with broken design.
- **Task granularity is advisory, not blocking.** Warn on tasks>20/files>15/waves>5/per-wave>8, but let the user decide whether to proceed or split.
- **Level-aware dispatch**: Trivial/Light changes may skip planner sub-agent (orchestrator fills templates directly). Standard/Critical MUST dispatch planner. Critical adds security dimension to design.
"##,
        name = change_name
    );
    format!("{}{}{}{}", ORCHESTRATOR_RULE, CONTEXT_REMINDER, fix_note, body)
}

pub fn apply_prompt(change_name: &str, fix_mode: bool) -> String {
    let fix_note = if fix_mode {
        "**FIX MODE ACTIVE** - The `--fix` flag is set. Executors must read review.md and fix R/Q/G issues, then mark each issue `[ ]` -> `[~]` (fixed, pending verification).\n\n"
    } else {
        ""
    };
    let body = format!(
        r##"## Input

- **`{name}`** (optional): change name. If empty, use the most recently planned change.
- **`--fix`** (optional): fix mode - executors read review.md issues and fix them.

## Prerequisites

- `design.md` exists and is not a template
- `tasks.md` exists, has at least 1 wave, checkboxes are unchecked (normal mode)
- Delta specs exist for each affected domain
- In --fix mode: `review.md` exists with unresolved R/Q/G issues

## Steps

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow apply` only outputs these steps - it does not auto-execute. Implementation is done by executor sub-agents.

### Step 1: Resolve change name and paths

Same as plan workflow Step 1.

### Step 2: Classify change (lightweight vs full)

Read `tasks.md` task types:
- **Lightweight**: ALL tasks are type:config|docs|refactor|scaffolding (no type:behavior)
- **Full**: any type:behavior task

### Step 3: Wave analysis (Full mode)

Read `tasks.md` and parse into execution plan:

1. **Extract waves**: Read all `## Wave N: <theme>` sections. Keep wave order.

2. **Build inter-wave dependency graph**:
   - For each task, extract `depends_on` field
   - If task in Wave B has `depends_on` referencing a task in Wave A -> Wave B depends on Wave A
   - Result: DAG where nodes = waves, edges = cross-wave depends_on

3. **File manifest overlap check**:
   - For each pair of waves in the same round, compare their tasks' `files` fields
   - If two waves modify the SAME file, they CANNOT run concurrently
   - Force overlapping waves into the same wave (serial within one sub-agent) or sequential rounds
   - This prevents merge conflicts when isolated worktrees merge back

4. **Generate execution plan**:

   - Waves with NO unmet cross-wave dependencies -> can run concurrently
   - Waves WITH cross-wave dependencies -> must wait for predecessor wave(s)

5. **For each wave, prepare executor dispatch prompt**:
   - Change name and directory path
   - Wave number and which task IDs (T-N) are in this wave
   - Summary of completed tasks from prior waves: task ID, title, files, key public interfaces
   - In --fix mode: which R/Q/G issue numbers are assigned to this wave

   **CRITICAL: Do NOT inject file contents into the dispatch prompt.**
   The executor has `read` tool access and will read these files itself:
   - tasks.md (for task details, RED tests, acceptance criteria)
   - design.md (for DS-N technical context)
   - specs/<domain>/spec.md (for delta specs)
   - .mcoder/workflow/conventions/coding.md (for coding conventions)
   - review.md (in --fix mode, for issue details)

   Providing paths saves tokens and prevents the orchestrator from biasing
   the executor with its interpretation of the content.

### Step 4: Dispatch executor waves (Full mode)

**Execute round by round:**

Each round:
1. Identify waves with no unmet dependencies -> ready to run
2. Dispatch ALL ready waves CONCURRENTLY (one `subagent(op=spawn, role=executor)` call per wave, all in one batch)
   - Each wave gets its own executor sub-agent
   - Fresh context: yes
   - Isolated: yes (executors modify source files and make git commits concurrently)
3. Wait for ALL waves in this round to complete

**After each round, verify each wave's output:**

For each completed wave:
1. **Check git log**: `git log --oneline -5` - confirm new commits exist with correct hashes
2. **Check git diff**: `git diff --stat HEAD~N` - confirm files actually changed (not no-op)
3. **Check tasks.md**: confirm tasks marked [x] with `<!-- commit: HASH -->` annotation
4. **Run wave's tests**: run the project's test command for <test-files> - confirm tests pass
5. **If any task missing commit annotation**: re-run that task manually or re-dispatch

**If any wave fails verification:**
- Re-dispatch the failed wave with specific feedback
- Do NOT proceed to next round until all waves in current round pass

**After all rounds complete:**
1. Run the project's build check and full test suite (per .mcoder/workflow/config.yaml stack)
2. If failures: identify which wave caused them, re-dispatch with fix instructions
3. If all pass: mark the `## Pre-Archive Checklist` in `tasks.md`:
   - `- [ ] type-check/build passes` -> `- [x]`
   - `- [ ] test suite passes - all suites green` -> `- [x]`
   - `- [ ] Every task in every wave is marked [x] with a commit hash` -> `- [x]` (verify this first)
   - `- [ ] All wave acceptance criteria confirmed` -> `- [x]`
   (Skip `No template placeholders` - that's verified by the planner during planning.)

### Step 5: Lightweight mode (if classified as lightweight)

If all tasks are non-behavior:
1. Implement tasks yourself, one by one
2. After each task: run relevant tests, commit with `bash(commands=["git add -A", "git commit -m '<message>'"])` or direct git
3. Mark [x] with commit hash in tasks.md
4. After all tasks: run full test suite
5. Mark the `## Pre-Archive Checklist` in tasks.md (same as Step 4 item 3)

### Step 6: Commit and suggest next step

```bash
# Update roadmap: If the change is linked to a roadmap phase, update it to `- [-] {name} (implemented YYYY-MM-DD)`.
bash(commands=["git add -A", "git commit -m 'feat: implementation complete for {name}'"])
```
  Next: /workflow review {name}
  (or: /workflow continue)

Output:
```
Implementation complete for {name}
  - N tasks implemented in N wave(s)
  - N commits created
  - All tests pass

  Next: /workflow review {name}
  (or: /workflow continue)
```

## Guardrails

- **Context is auto-injected by mcoder at session_start and after compaction.** Do NOT call `workflow(action=context)` yourself.
- **Full mode: MUST dispatch sub-agents per wave.** Do NOT implement behavior tasks yourself.
- **Concurrent waves in the same round: dispatch ALL in one `subagent(op=spawn, role=executor)` call (parallel).**
- **After each wave: verify git log, tasks.md marking, test pass.** No-op or incomplete = failure.
- **NEVER skip review.** Apply's test pass is NOT a replacement for review.
- In --fix mode: executors read review.md, fix R/Q/G issues, then mark each issue `[ ]` -> `[~]` (`~` = fixed, pending verification). Do NOT mark `[x]` - that's the re-review's job. Do NOT fix D issues (those need replan).
- Do NOT run /workflow review automatically - let the user decide.
- **Wave retry limit: max 2 re-dispatches per wave (global cap: config.budget.max_subagent_runs, default 5).** If a wave fails verification 2 times after re-dispatch with specific feedback, STOP and report as blocker. Do not re-dispatch indefinitely.
- **Budget awareness**: Track sub-agent dispatch count against config.budget.max_subagent_runs (default 5). Track wall time against config.budget.max_wall_time_min (default 60). If estimated token usage approaches config.budget.estimated_token_cap (default 500000), warn. These are advisory - stop and report if exceeded.
- **Level-aware execution**: Trivial = inline (no sub-agent). Light = single agent, TDD optional. Standard = wave + TDD. Critical = wave + TDD + security audit checkpoint.
"##,
        name = change_name
    );
    format!("{}{}{}{}", ORCHESTRATOR_RULE, CONTEXT_REMINDER, fix_note, body)
}

pub fn review_prompt(change_name: &str, fix_mode: bool) -> String {
    let fix_note = if fix_mode {
        "**FIX MODE ACTIVE** - The `--fix` flag is set. Reviewer must verify each `[~]` issue before marking `[x]`, following the three-state process (`[ ]`->`[~]`->`[x]`).\n\n"
    } else {
        ""
    };
    let body = format!(
        r##"## Input

- **`{name}`** (optional): change name. If empty, use the most recently applied change.
- **`--fix`** (optional): re-review mode - reviewer marks resolved issues in existing review.md.

## Prerequisites

- Code is implemented (tasks.md has [x] entries with commit hashes)
- Build check and test suite pass (per .mcoder/workflow/config.yaml stack)
- In --fix mode: `review.md` exists with unresolved issues, fixes have been applied


## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow review` only outputs these steps - it does not auto-execute. Review is done by reviewer sub-agent.

## Steps

### Step 1: Resolve change name and paths

Same as plan workflow Step 1.

### Step 2: Pre-review verification

Run before dispatching reviewer:
```bash
# Run the project's build check and test suite.
# Read .mcoder/workflow/config.yaml for the tech stack and test framework.
# Examples by stack:
#   TypeScript: tsc --noEmit && npx vitest run
#   Python:     mypy . && pytest
#   Go:         go build ./... && go test ./...
#   Rust:       cargo build && cargo test
#   Java:       mvn compile && mvn test
```

If build or tests fail: do NOT dispatch reviewer. Report the failures and suggest `/workflow apply --fix {name}` to fix them first.

### Step 3: Classify change (lightweight vs full)

- **Lightweight** (all non-behavior tasks, no delta specs): orchestrator does a quick review directly
  - Check: all tasks [x], tests pass, no obvious issues
  - Write a simplified review.md (may skip spec review if no delta specs)
- **Full** (any behavior task, has delta specs): dispatch reviewer sub-agent

### Step 4: Dispatch reviewer (Full mode)

**Do NOT write review.md yourself. Dispatch reviewer sub-agent.**

1. Prepare reviewer context:
   - Change name and directory path
   - List of files to read: proposal.md, design.md, tasks.md, specs/<domain>/spec.md, .mcoder/workflow/specs/<domain>/spec.md, .mcoder/workflow/conventions/coding.md
   - Instruction: "Read the reviewer agent prompt, then perform triple review and write review.md"
   - In --fix mode: "Read the reviewer agent prompt (Fix Mode section), verify each [~] issue before marking [x], follow the three-state process ([ ]->[~]->[x])"

2. Dispatch via `subagent(op=spawn, role=reviewer)`:
   - Fresh context: yes
   - Isolated: no (reviewer is read-only on source code, writes only review.md)

3. Wait for reviewer to complete.

### Step 5: Read review.md and route

After reviewer completes:

1. Read `.mcoder/workflow/changes/{name}/review.md`
2. Extract the Overall Verdict and Issues list
3. Route based on findings:

**If Overall Verdict is PASS (zero issues):**
```
Review PASSED for {name}
  All three dimensions clean.
```

**HARD RULE: Ask the user before archiving.** Present the PASS verdict and any
notable findings, then ask: "Review PASSED. Shall I archive this change?"

- If user confirms -> run `workflow(action=finalize, name={name})`
- If user wants changes -> run `workflow(action=step, name=apply, change={name}, fix=true)`
- If user wants to review findings first -> present them and wait

Do NOT auto-archive. The guardrail in the guardrails section below says the
same thing, but this is your immediate instruction.

**If D-prefixed issues exist (design flaw):**
```
Review FAILED for {name}
  D issues found (design/architecture problems):
  - D1: <list actual D-issue descriptions from review.md>

  These require redesign, not code fix.
  Next: /workflow plan --fix {name}
```

**If only R/Q/G issues (code fixable):**
```
Review NEEDS_REVISION for {name}
  Issues found (code fixable):
  - R1: <list actual R-issue descriptions from review.md>
  - Q1: <list actual Q-issue descriptions from review.md>
  - G1: <list actual G-issue descriptions from review.md>

  Next: /workflow apply --fix {name}
```


**If [FUSE] diminishing returns detected:**
Do NOT auto-route to another fix. Instead:
1. Read review.md Issues section to understand remaining open findings
2. Present remaining issues to user for human verification
3. If user confirms all resolved: write `## Human Verdict: PASS` below the Issues section in review.md, then run `workflow(action=finalize, name={name})`
4. If user finds new problems or disagrees with resolution: run `workflow(action=step, name=apply, change={name}, fix=true)` (this resets the review round counter for fuse detection)

### Step 6: Commit review.md

```bash
# Update roadmap: If the change is linked to a roadmap phase, update it to `- [x] {name} (reviewed YYYY-MM-DD)`.
bash(commands=["git add -A", "git commit -m 'docs(review): triple review for {name}'"])
```

## Guardrails

- Do NOT run /workflow archive automatically - let the user review the findings first.
- **Context is auto-injected by mcoder at session_start and after compaction.** Do NOT call `workflow(action=context)` yourself.
- **Fix loop limit: max config.budget.max_review_rounds rounds (default 3).** If the change has been through that many fix rounds (count re-reviews in review.md Review History) and issues still persist, the diminishing-returns fuse may trigger - see [FUSE] recovery path below. Do not auto-route to another fix beyond the limit - escalate for human decision.
- **Level-aware review**: Trivial = orchestrator quick check (no sub-agent). Light = optional review. Standard = triple review. Critical = triple review + security audit + human approval before archive.
- **Critical approval gate**: Critical-level changes require explicit approval. If config.approvers is non-empty, only listed approvers can PASS a Critical review. In CI mode, Critical changes always exit 1 (require interactive approval).
- **CI mode (--ci)**: When run with --ci, skip all human-confirmation steps. If review verdict is not PASS, the command exits 1 immediately. No fix loop in CI -- failures must be fixed in a separate interactive session.
- **Level-aware CI**: Critical changes require human approval even in CI mode (exit 1 with 'Critical change requires human review'). Trivial/Light changes auto-pass in CI if tests pass.
"##,
        name = change_name
    );
    format!("{}{}{}{}", ORCHESTRATOR_RULE, CONTEXT_REMINDER, fix_note, body)
}

pub fn archive_prompt(change_name: &str) -> String {
    let body = format!(
        r##"## Input

- **`{name}`** (optional): change name. If empty, use the most recently reviewed change.

## Prerequisites

- `review.md` exists and Overall Verdict is PASS
- No unresolved issues in review.md `## Issues` section

## Steps

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow archive` only outputs these steps - it does not auto-execute. Actual archive is done by `workflow(action=finalize)`.

### Step 1: Resolve change name and paths

Same as plan workflow Step 1.

### Step 2: Pre-archive check (optional but recommended)

Read `.mcoder/workflow/changes/{name}/review.md`:
- Check Overall Verdict is PASS
- Check `## Issues` section has no `- [ ]` entries (all should be [x] or empty)
- Run `git status --porcelain` - if there are uncommitted changes outside .mcoder/workflow/, warn the user. Uncommitted half-done code may get archived alongside the change.

If review is not PASS:
```
Cannot archive: review not passed
  Verdict: FAIL/NEEDS_REVISION
  Unresolved issues: N

  Fix issues first: /workflow apply --fix {name}
```

### Step 3: Run finalize command

The `workflow(action=finalize)` command handles everything: review verification, delta spec merge, change directory move, and roadmap update.

```bash
workflow(action=finalize, name={name})
```

The command will:
1. Verify review.md verdict is PASS and no unresolved issues
2. Gate context.jsonl validity for archive phase
3. Merge each delta spec (`specs/<domain>/spec.md`) into global spec (`.mcoder/workflow/specs/<domain>/spec.md`) via mergeDeltaSpec
4. Move change directory to `.mcoder/workflow/changes/archive/<date>-{name}/`
5. Update `.mcoder/workflow/roadmap.md` if proposal has `## Roadmap Reference`

If the command reports a merge conflict, resolve it in the delta spec and re-run.

### Step 4: Verify archive success

Check the following after the command completes:

1. **Archive directory exists**: `.mcoder/workflow/changes/archive/<date>-{name}/` exists and contains all artifacts (proposal.md, design.md, tasks.md, specs/, review.md)
2. **Source directory removed**: `.mcoder/workflow/changes/{name}/` no longer exists
3. **Delta specs merged**: For each domain in the change's `specs/`:
   - ADDED requirements appear in `.mcoder/workflow/specs/<domain>/spec.md`
   - REMOVED requirements are gone from `.mcoder/workflow/specs/<domain>/spec.md`
   - MODIFIED requirements are replaced in `.mcoder/workflow/specs/<domain>/spec.md`
4. **Roadmap updated** (if proposal had `## Roadmap Reference`): the change is marked `- [x]` in `.mcoder/workflow/roadmap.md`
5. **No conflict residue**: the command output shows no merge conflict errors

If any check fails, investigate - the archive command may have partially completed.

### Step 5: Commit changes

The archive command does NOT run git commit. Commit the merged specs, roadmap, and archived change:

```bash
bash(commands=["git add -A", "git commit -m 'archive: {name} - specs merged, roadmap updated'"])
```

### Step 6: Suggest next step

Output:
```
Archived {name}
  - Delta specs merged into .mcoder/workflow/specs/
  - Change moved to .mcoder/workflow/changes/archive/<date>-{name}/
  - Roadmap updated

  Next: /workflow continue (or: /workflow propose <new-change>)
```

## Guardrails

- **Review must PASS before archive.** The archive command enforces this, but pre-checking saves a failed command.
- **If merge conflict occurs**, resolve in the delta spec (change directory) and re-run `workflow(action=finalize, name={name})`. Do NOT edit global specs directly.
- **Archive preserves full context.** All artifacts move to archive together.
- **CI mode (--ci)**: Skip working-tree warnings and post-archive suggestions. Exit 0 on success, 1 on any failure.
- **Commit is the orchestrator's job** - the archive command does not commit.
"##,
        name = change_name
    );
    body
}

pub fn roadmap_prompt() -> String {
    r#"## Input

- No parameters: operate on the current project

## Orchestrator Steps

> These are the steps you (orchestrator) execute in order. `/workflow roadmap` only outputs these steps - it does not auto-execute.

### Step 1: Discuss project requirements

Before defining milestones, understand the project goals and requirements:

Use `ask` to discuss:
- **Project goal**: What is this project trying to achieve? What problem does it solve?
- **Target users**: Who will use this? What are their needs?
- **Key features**: What are the main capabilities required?
- **Constraints**: Any technical, timeline, or resource constraints?
- **Existing codebase**: If brownfield, what exists already? What needs to change?

Take notes. These inform the roadmap structure.

### Step 1b: Research (do this yourself, do NOT ask the user)

After the initial discussion, research the project context:
- If brownfield: read existing source code, understand current architecture, identify what exists vs what needs to change
- Read the project's dependency and config files to understand the tech stack
- Read `.mcoder/workflow/specs/` if specs exist (brownfield after codebase-scanner)
- Check for linting and formatting configuration to understand conventions

### Step 1c: Follow-up questions (if needed)

After research, assess whether you have enough to define milestones. If ANY of these are unclear, use `ask` to follow up:
- User described a feature but didn't specify the approach -> ask which approach they prefer
- User's stated tech stack contradicts what the codebase actually uses -> clarify the contradiction
- User's scope is too large (>5 phases in one milestone) -> suggest splitting
- User mentioned a feature but you don't know if existing code already partially implements it -> ask

Do NOT proceed to Step 2 until you can answer: "What are the concrete, unambiguous deliverables for each phase?"

### Step 2: Get context

Read `.mcoder/workflow/config.yaml` and `.mcoder/workflow/specs/` to understand the project scope, tech stack, and existing behavioral contracts.

### Step 3: Detect roadmap state

Read `.mcoder/workflow/roadmap.md`. Check if it already has defined milestones (look for `## Milestone:` headers that have real content, not template placeholders).

**First time (no milestones defined):**
Continue to Step 4.

**Adding a new milestone (roadmap already exists):**
- Append new milestone(s) BELOW existing milestones, separated by `---`
- Keep existing milestones with their status unchanged

### Step 4: Choose planning mode (first time only)

Use `ask` to determine the planning mode:

- **MVP mode** (product-facing): each phase delivers user-facing value
- **Technical-layer mode** (infrastructure/CLI): each phase produces a runnable/testable artifact

### Step 5: Define Milestones

Get the roadmap template: `workflow(action=template, type=roadmap)`. Fill with milestones and phases.

**Default: 1 milestone = the entire project.** Milestones are product releases, NOT development phases.

### Step 6: Validate

Check before finishing:
- All project requirements from Step 1 discussion are covered by some phase
- Phase dependencies form a DAG (no cycles)
- Each phase has a concrete, verifiable deliverable
- Phase count per milestone: small 1-2, medium 2-3, large 3-4
- First phase is always the thinnest possible end-to-end path
- No template placeholders remaining (`{{`)

## Output

- `.mcoder/workflow/roadmap.md` - structured roadmap with milestone and phase info

Write using `bash(commands=["cat > .mcoder/workflow/roadmap.md << 'ROADMAP_EOF'\n...\nROADMAP_EOF"])`.

Then use `ask` to confirm the roadmap with the user and suggest next step: `/workflow propose`.

## Guardrails

- **Default: 1 milestone.** No "foundation", "setup", "scaffolding" - M1 = shippable product.
- Mode (MVP/technical-layer) shapes phases within a milestone, not the milestones themselves.
- First phase = thinnest end-to-end path (always first phase, never "phase 0").
- **Adding new milestone**: append new ones below existing, don't overwrite.
- Do NOT create milestone directories - roadmap.md is the single tracking document.
"#.to_string()
}

pub fn ff_prompt() -> String {
    r##"## Input

- **`$ARGUMENTS`** (optional): change name. If empty, starts from current project state.

## What to do

Fast-forward: execute the current step, then auto-call `workflow(action=continue)` to get the next step, then execute that. Repeat until complete.

### Loop

For each iteration:

1. **Get current step**:
   ```bash
   workflow(action=continue)
   ```
   The tool outputs the next step's full workflow instructions.

2. **Execute those instructions** - dispatch sub-agents, write files, run code, etc. as the instructions describe.

3. **After the step completes**, return to step 1.

4. **Stop when**:
   - `workflow(action=continue)` shows no more actionable steps (no active changes, roadmap has no `[ ]` items)
   - OR an unrecoverable error occurs (report it and stop)

### Constraints

- Respect all gates: review must PASS before archive; design issues (D-prefixed) route to plan --fix; code issues route to apply --fix.
- You MAY ask the user clarifying questions if truly blocked (e.g. ambiguous requirement). But default to proceeding with the most reasonable interpretation.
- Each `workflow(action=continue)` invocation is independent - it re-checks artifact state.
- Report progress to the user after each iteration.

## Guardrails

- Do NOT skip the review gate.
- Do NOT auto-archive if review verdict is FAIL or NEEDS_REVISION.
- If `workflow(action=continue)` suggests a fix loop (plan --fix or apply --fix), execute that fix loop before continuing.
- If a step is unclear or the output is unexpected, stop and ask the user.

## Context Reminder
Context is auto-injected by mcoder at session_start and after compaction. Do NOT call `workflow(action=context)` yourself.
"##.to_string()
}

pub fn continue_prompt() -> String {
    r##"## Input

- **`$ARGUMENTS`** (optional): change name. If empty, auto-detects.

## Note

> `workflow(action=continue)` auto-detects progress and outputs the next step's workflow instructions. Follow the instructions it outputs.

## What to do

Call the tool:

```bash
workflow(action=continue)
```

The tool runs schema-driven detection in code and outputs:
1. Current artifact status (proposal/design/tasks/specs/review existence + task completion count)
2. Next recommended step (command + description)
3. Full workflow instructions for the next step

**Follow the tool output.** Do not manually check files or determine the next step yourself - the code does it.

## When to use

- After `/workflow init` (tool detects empty roadmap -> suggests /workflow roadmap)
- After any step completes (tool detects next step based on schema)
- When unsure what to do next (tool shows current progress)

## Guardrails

- The tool does ALL detection. You just follow its output.
- If the tool says "Next: /workflow plan <name>", run the plan workflow instructions it outputs.
- If multiple active changes exist, the tool lists them. Pick one and re-run `workflow(action=continue)`.
"##.to_string()
}

pub fn loop_prompt() -> String {
    r##"## Input

- **`$ARGUMENTS`** (optional): change name. If empty, starts from current project state.

## Note

> `/workflow loop` auto-advance through steps by calling `workflow(action=continue)` after each. Follow the instructions each `workflow(action=continue)` outputs.

## What to do

Autonomous loop: **skip ALL user interaction**. Run until the roadmap has no remaining `[ ]` items.

### Loop

For each iteration:

1. **Get current step**: 
   ```bash
   workflow(action=continue)
   ```
   The tool outputs the next step's full workflow instructions.

2. **Execute those instructions WITHOUT asking the user anything.** Make the most reasonable interpretation and proceed. If the instructions say to use `ask`, you must SKIP that ask step and use sensible defaults instead.

3. **After the step completes**, return to step 1.

4. **Stop when**:
   - The roadmap has no `[ ]` items (all milestones shipped, all phases completed, all changes archived)
   - OR an unrecoverable error occurs (report it and stop)

### NO INTERACTION - CRITICAL

- Do NOT call `ask` for anything.
- Do NOT use `ask_user_question`.
- Do NOT pause to ask the user.
- For requirement questions in roadmap/propose: use the most reasonable defaults, document your assumption in the artifact, and continue.
- For ambiguous tool output: log a note, make your best guess, and continue.
- Only stop on hard errors (test failures that can't be fixed in 1 attempt, unrecoverable build errors).

## Guardrails

- Do NOT skip the review gate.
- Do NOT auto-archive if review verdict is FAIL or NEEDS_REVISION - but DO still attempt `workflow(action=step, name=apply, fix=true)` once. If that doesn't resolve, stop and report.
- If `workflow(action=continue)` suggests a fix loop, execute it.
- Report progress concisely after each iteration.
- When done, summarize what was completed.
"##.to_string()
}
