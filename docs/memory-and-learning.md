# Memory and Learning

Ralph's learning system allows the agent to accumulate knowledge across
iterations and runs. It has two mechanisms:

1. **Skills** -- Reusable procedure documents that future iterations can
   reference.
2. **CLAUDE.md updates** -- Project-level context that persists across all
   Claude sessions.

Both are controlled by the `learn` configuration flag. When learning is enabled
(the default), the system prompt instructs Claude to create skills and update
CLAUDE.md when it discovers reusable patterns or encounters gotchas.

## Configuration

### .ralph.toml

The `[execution]` section controls learning alongside verification and retries:

```toml
[execution]
learn = true       # default: true
verify = true      # default: true
max_retries = 3    # default: 3
```

The `learn` field defaults to `true` via `#[serde(default = "default_true")]` in
`src/project.rs`. Missing or empty TOML files get learning enabled automatically.

### CLI Override

```bash
ralph run my-feature --no-learn    # Disable learning for this run
```

The `--no-learn` flag sets `no_learn = true` in `Config::from_run_args()`. The
final value is computed as:

```rust
let learn = !no_learn && execution.learn;
```

Both the CLI flag and the TOML setting must agree for learning to be active. The
CLI flag always wins -- `--no-learn` disables learning even if the TOML has
`learn = true`.

### When to Disable Learning

- **CI/CD pipelines.** Prevents the agent from writing skill files or modifying
  CLAUDE.md in automated environments.
- **Deterministic runs.** Avoids side effects when you want repeatable behavior.
- **Sensitive codebases.** Prevents the agent from documenting internal
  procedures in skill files.

## Skills

### What Are Skills?

Skills are markdown documents with YAML frontmatter that describe reusable
procedures. They live at `.ralph/skills/<skill-name>/SKILL.md`. Each skill is a
self-contained procedure that Claude can follow when performing a recurring type
of task.

Examples of skills:

- How to commit code in this project's preferred style.
- How to run and interpret a specific test suite.
- How to deploy to a staging environment.
- How to handle a particular API integration pattern.

### File Format

```markdown
---
name: deploy-check
description: Verify deployment prerequisites before pushing to staging
---

# Deploy Check

1. Run the full test suite with `cargo test`
2. Check that all environment variables are set...
3. Verify the database migration is up to date...
```

The YAML frontmatter block is delimited by `---` lines at the top of the file.
Two fields are expected:

| Field         | Purpose                                                     |
| ------------- | ----------------------------------------------------------- |
| `name`        | Identifier for the skill (matches the directory name).      |
| `description` | One-line summary shown in the skills list in the system prompt. |

The body after the closing `---` contains the full procedure. It can include any
markdown content -- step-by-step instructions, code blocks, links, etc.

### Directory Layout

```
.ralph/skills/
  deploy-check/
    SKILL.md
  api-testing/
    SKILL.md
  git-commit/
    SKILL.md
```

Each skill gets its own directory under `.ralph/skills/`. The directory name
should match the `name` field in the frontmatter. The `ralph init` command
creates the `.ralph/skills/` directory as part of the project scaffold.

### How Skills Are Created

Skills are created organically by Claude during task execution. Ralph does not
have a dedicated "create skill" CLI command. When learning is enabled, the system
prompt includes instructions telling Claude to create skills when it discovers
reusable patterns:

```
## Learning

When you discover reusable patterns or encounter gotchas:

1. **Agent Skills**: Create `.ralph/skills/<skill-name>/SKILL.md` with:
   ---
   name: <skill-name>
   description: <when to use this skill>
   ---
   <step-by-step instructions>

2. **CLAUDE.md**: Add project-specific knowledge that helps future agents.
```

This section is appended by `build_system_prompt()` in `src/claude/client.rs`
(lines 398--410) only when `ctx.learn` is `true`.

Claude decides when something is worth codifying. Typical triggers:

- A multi-step procedure that required trial and error.
- A pattern that will recur in future tasks.
- A workaround for a tooling limitation.
- A project-specific convention not documented elsewhere.

### Skill Discovery

At the start of each iteration, `discover_skills()` in `src/run_loop.rs` scans
for existing skills. The algorithm:

1. Read the directory entries in `.ralph/skills/`.
2. For each subdirectory, check if `SKILL.md` exists.
3. Read the file and parse the YAML frontmatter.
4. Extract the `description` field.
5. Collect all `(name, description)` pairs into a `Vec`.

The frontmatter parser (`parse_skill_description()`) is intentionally simple. It
looks for the opening `---`, finds the closing `---`, and scans the lines
between them for a `description:` prefix. If the frontmatter is missing or
malformed, it returns `"No description"`. Files that cannot be read are silently
skipped.

If the `.ralph/skills/` directory does not exist, `discover_skills()` returns an
empty list without error. This means learning works gracefully even in projects
that were initialized before the skills feature was added.

### How Skills Appear in the System Prompt

The discovered skills are included in the "Available Skills" section of the
system prompt:

```
## Available Skills

The following skills are available in `.ralph/skills/`.
Read the full SKILL.md for details when relevant.

- **deploy-check**: Verify deployment prerequisites before pushing to staging
- **api-testing**: Run integration tests against the staging API
- **git-commit**: Commit changes following the project's commit conventions
```

Only the name and description are included inline. Claude must read the full
`SKILL.md` file if it needs the detailed procedure. This keeps the system prompt
compact while still making skills discoverable.

### Skill Lifecycle

```
Iteration N: Claude encounters a novel procedure
  -> Solves the problem through trial and error
  -> Creates .ralph/skills/deploy-check/SKILL.md
  -> Continues with the assigned task

Iteration N+1: discover_skills() finds the new skill
  -> Skills summary includes "deploy-check: Verify deployment prerequisites..."
  -> System prompt includes the summary
  -> If the current task involves deployment, Claude reads the full SKILL.md
  -> Claude follows the established procedure instead of re-discovering it

All future iterations: Skill remains in the summary
  -> Consistent procedure across all tasks and runs
  -> Knowledge persists beyond any single agent session
```

Skills accumulate over time. A project that has run through many Ralph iterations
builds up a library of procedures tailored to its specific setup. New team
members (or new Ralph runs) benefit from all previously captured knowledge.

## CLAUDE.md Updates

### What Is CLAUDE.md?

CLAUDE.md is the standard project context file for Claude Code. It is read at
the start of every Claude session -- not just Ralph-managed ones. It contains
project-specific instructions: build commands, architectural notes, coding
conventions, and common pitfalls.

### How Ralph Updates It

When learning is enabled, the system prompt instructs Claude to add
project-specific knowledge to CLAUDE.md. Unlike skills (which are
Ralph-specific), CLAUDE.md updates benefit all future Claude interactions with
the project, including interactive sessions outside of Ralph.

Typical CLAUDE.md updates made by the agent:

- Build or test commands that differ from the defaults.
- Architectural patterns that are not obvious from the code.
- Gotchas encountered during task execution (e.g., "the database must be
  migrated before running integration tests").
- Module-specific conventions (e.g., "all public functions in the DAG module
  accept a `&Db` as the first argument").

### Skills vs. CLAUDE.md

| Aspect         | Skills                                 | CLAUDE.md                           |
| -------------- | -------------------------------------- | ----------------------------------- |
| Scope          | Specific procedures                    | General project knowledge           |
| Audience       | Ralph iterations only                  | All Claude sessions                 |
| Location       | `.ralph/skills/<name>/SKILL.md`        | Project root `CLAUDE.md`            |
| Discovery      | Scanned each iteration by Ralph        | Read by Claude Code automatically   |
| Content type   | Step-by-step instructions              | Build commands, patterns, pitfalls  |
| Granularity    | One file per procedure                 | Single file, multiple sections      |

Use skills for procedures that require detailed steps. Use CLAUDE.md for
knowledge that should be available in all contexts, not just Ralph runs.

## System Prompt Integration

The learning system integrates with the system prompt at two points, both in
`build_system_prompt()` in `src/claude/client.rs`.

### Skills Summary (lines 389--395)

Appended when `ctx.skills_summary` is non-empty:

```rust
if !ctx.skills_summary.is_empty() {
    prompt.push_str("\n## Available Skills\n\n");
    prompt.push_str(
        "The following skills are available in `.ralph/skills/`. \
         Read the full SKILL.md for details when relevant.\n\n"
    );
    for (name, description) in &ctx.skills_summary {
        prompt.push_str(&format!("- **{}**: {}\n", name, description));
    }
}
```

This section appears even when learning is disabled (`--no-learn`). Existing
skills are always discoverable -- the `--no-learn` flag only prevents the
creation of new skills, not the reading of existing ones.

### Learning Instructions (lines 398--410)

Appended only when `ctx.learn` is `true`:

```rust
if ctx.learn {
    prompt.push_str("\n## Learning\n\n");
    prompt.push_str("When you discover reusable patterns or encounter gotchas:\n\n");
    prompt.push_str("1. **Agent Skills**: Create `.ralph/skills/<skill-name>/SKILL.md` ...");
    prompt.push_str("2. **CLAUDE.md**: Add project-specific knowledge ...");
}
```

When `--no-learn` is active, this section is omitted entirely. Claude receives
no instruction to create skills or update CLAUDE.md, effectively making the run
read-only with respect to learning artifacts.

## Relationship to Other Systems

### Verification

Learning and verification are complementary but independent. Both are enabled by
default and can be controlled separately:

```bash
ralph run my-feature                     # Both enabled (default)
ralph run my-feature --no-verify         # Verification off, learning on
ralph run my-feature --no-learn          # Verification on, learning off
ralph run my-feature --no-verify --no-learn  # Both disabled
```

Verification checks that task output is correct using a read-only agent.
Learning codifies procedures for future use by writing files. They operate at
different points in the iteration lifecycle and do not interact with each other.

### Retry System

When a task fails verification and is retried, the retry info is included in the
system prompt alongside the skills summary. Claude can use existing skills to
avoid repeating the same mistakes. If the failure led to a new insight, Claude
can also create a skill documenting the correct approach -- which then helps on
subsequent retries or future tasks.

### Feature Specs and Plans

Skills complement specs and plans. A spec defines _what_ to build, a plan
defines _how_ to build it, and skills define reusable _procedures_ that apply
across multiple features. For example, a "run-integration-tests" skill might be
used during the implementation of many different features.

## Data Flow

The following diagram traces how learning data flows through the system:

```
                    .ralph.toml
                    [execution]
                    learn = true
                         |
                         v
                 Config { learn: true }
                         |
                         v
             build_iteration_context()
               sets ctx.learn = true
                    |           |
                    v           v
          discover_skills()   ctx.learn
          reads SKILL.md      passed to
          files from disk     build_system_prompt()
                    |           |
                    v           v
          skills_summary    "## Learning" section
          "## Available     appended to system
          Skills" section   prompt
                    |           |
                    +-----+-----+
                          |
                          v
                   System Prompt
                   sent to Claude
                          |
                          v
              Claude executes the task
                    |           |
                    v           v
              May create      May update
              new SKILL.md    CLAUDE.md
              files           with lessons
                    |
                    v
              Next iteration picks up
              new skills via discover_skills()
```

## Key Source Files

| File                   | Role                                            |
| ---------------------- | ----------------------------------------------- |
| `src/run_loop.rs`      | `discover_skills()`, `parse_skill_description()` |
| `src/claude/client.rs` | System prompt: skills summary + learning section |
| `src/config.rs`        | `learn` field on `Config`                        |
| `src/project.rs`       | `ExecutionConfig.learn` with serde defaults      |
