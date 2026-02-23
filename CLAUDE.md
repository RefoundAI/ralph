# CLAUDE.md

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (stripped, LTO)
cargo test               # Run tests
cargo run -- --help      # Show CLI usage
```

## Project Overview

Ralph is an autonomous agent loop harness that iteratively invokes Claude Code via the Agent Client Protocol (ACP). It decomposes work into a DAG of tasks stored in SQLite, picks ready tasks one at a time, and loops until all tasks are resolved or a limit is hit.

## Before Modifying Code

Ralph's design decisions, pitfalls, and constraints are documented in `.ralph/knowledge/` as a graph of interconnected entries. **Read relevant entries before changing any subsystem.**

### How to use the knowledge graph

1. **Find your entry point.** Pick the knowledge file most relevant to what you're changing (filenames are descriptive, e.g. `run-loop-lifecycle.md` for the run loop, `auto-transitions.md` for task status changes).
2. **Follow the links.** Each entry contains `[[Title]]` references to related entries. Read those too — they capture constraints and interactions you'd otherwise miss.
3. **Check the tags.** Each entry has YAML frontmatter with `tags:` — use these to find entries you might have missed by scanning filenames.

### Entry points by subsystem

| Subsystem | Start here | You'll be led to |
|-----------|-----------|-------------------|
| Run loop | `run-loop-lifecycle.md` | execution modes, sigils, journal, knowledge, interrupts, verification |
| ACP protocol | `acp-connection-lifecycle.md` | permissions, trait imports, schema types, tokio patterns |
| Task DAG | `auto-transitions.md` | columns mapping, CRUD, parent status, cycle detection, schema |
| Features | `feature-lifecycle.md` | one-shot vs features, execution modes, verification |
| Config | `configuration-layers.md` | run args contract, model strategy |
| Prompts | `system-prompt-construction.md` | sigil parsing, journal, knowledge, roam linking |
| Testing | `mock-acp-agent-binary.md` | LocalSet patterns, integration test binary paths |

### Commonly needed knowledge

- **Adding a config field**: Read `config-from-run-args.md` — has a 12-param contract you must update everywhere
- **Changing task schema**: Read `schema-migrations.md` + `task-columns-mapping.md`
- **Modifying the run loop**: Read `run-loop-lifecycle.md` + `error-handling-resilience.md`
- **ACP imports**: Read `acp-trait-imports.md` + `acp-schema-types-import-path.md`
- **Linter reverting your edits**: Read `linter-hook-reverts-files-on-compile-error.md`

## Source Layout

```
src/
  main.rs           CLI entry point, subcommand dispatch
  cli.rs            Argument definitions (clap)
  config.rs         Config struct, model strategy, run target
  run_loop.rs       Core DAG-driven agent loop
  project.rs        .ralph.toml discovery, `ralph init`
  feature.rs        Feature CRUD
  strategy.rs       Model selection (fixed, cost-optimized, escalate, plan-then-execute)
  journal.rs        Iteration history (SQLite + FTS5)
  knowledge.rs      Tag-based knowledge with [[roam]] linking
  verification.rs   Read-only verification agent
  interrupt.rs      SIGINT handling
  review.rs         Code review agent
  acp/              ACP integration (connection, client, prompt, sigils, tools, streaming)
  dag/              Task DAG (schema, CRUD, transitions, dependencies, IDs)
  output/           Terminal formatting, logging
```

## Key Files

- `.ralph.toml` — Project configuration (discovered by walking up directory tree)
- `.ralph/progress.db` — SQLite DAG database (gitignored)
- `.ralph/features/<name>/spec.md` — Feature specifications
- `.ralph/features/<name>/plan.md` — Feature implementation plans
- `.ralph/knowledge/<name>.md` — Knowledge entries (YAML frontmatter + `[[links]]`)
- `.claude/skills/<name>/SKILL.md` — Reusable agent skills

## CLI

```
ralph init                        # Initialize project
ralph feature create <name>       # Interview -> spec -> plan -> task DAG
ralph feature list                # List features and status
ralph task add <TITLE> [flags]    # Non-interactive task creation
ralph task create [--model M]     # Interactive task creation
ralph task show <ID> [--json]     # Task details
ralph task list [filters] [--json]
ralph task update <ID> [flags]
ralph task delete <ID>
ralph task done <ID>              # Mark done (triggers auto-transitions)
ralph task fail <ID> [-r reason]
ralph task reset <ID>
ralph task log <ID> [-m msg]
ralph task deps add <A> <B>       # A must complete before B
ralph task deps rm <A> <B>
ralph task deps list <ID>
ralph task tree <ID> [--json]
ralph run <target>                # Run agent loop (feature name or task ID)
  --limit=N / --model=MODEL / --model-strategy=STRAT
  --agent=CMD / --max-retries=N / --no-verify
```

Env vars: `RALPH_LIMIT`, `RALPH_MODEL`, `RALPH_MODEL_STRATEGY`, `RALPH_AGENT`.

## Releases

Uses `cargo-dist`. Config in `dist-workspace.toml`. Always use annotated tags: `git tag -a vX.Y.Z -m "vX.Y.Z"`.

```bash
dist plan      # Preview build
dist build     # Build locally
dist generate  # Regenerate CI after config changes
```

## Nix

```bash
nix develop    # Enters dev shell with Rust toolchain
```
