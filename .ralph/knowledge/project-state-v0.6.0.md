---
title: "Project state checkpoint v0.6.0"
tags: [project, state, checkpoint, architecture, v0.6.0, acp, modules]
created_at: "2026-02-22T00:00:00Z"
---

## Architecture (v0.6.0)

Ralph communicates with AI agents via ACP (Agent Client Protocol) — JSON-RPC 2.0 over stdin/stdout. The legacy `src/claude/` module has been fully removed. All agent communication flows through `src/acp/`.

### Module Map (line counts)
- `src/acp/` (3,291) — ACP integration: connection lifecycle, RalphClient trait impl, prompt construction, sigil parsing, terminal tools, streaming display, interactive/streaming sessions
- `src/dag/` (2,874) — SQLite DAG: CRUD, transitions, schema v3, dependencies with BFS cycle detection, ID generation
- `src/main.rs` (1,264) — Entry point, CLI dispatch, context assembly
- `src/knowledge.rs` (1,036) — Tag-based markdown knowledge files with deduplication
- `src/run_loop.rs` (930) — Core iteration loop, context building, outcome handling
- `src/strategy.rs` (779) — Model selection (Fixed, CostOptimized, Escalate, PlanThenExecute)
- `src/journal.rs` (746) — SQLite + FTS5 iteration journal
- `src/cli.rs` (504) — Clap CLI definitions
- `src/config.rs` (487) — Config struct, agent command resolution, ID generation
- `src/project.rs` (455) — Project init, .ralph.toml discovery
- `src/review.rs` (360) — Code review agent via run_autonomous()
- `src/feature.rs` (281) — Feature CRUD
- `src/output/` (274) — ANSI formatting, log paths
- `src/verification.rs` (157) — Read-only verification agent
- `src/interrupt.rs` (150) — SIGINT handling

### ACP Entry Points
- `run_iteration()` — main loop iteration (spawns agent, sends prompt, processes response)
- `run_autonomous()` — verification/review (supports read_only mode)
- `run_interactive()` — spec/plan commands (user input loop)
- `run_streaming()` — feature build (single autonomous prompt)

### Key Dependencies
`agent-client-protocol = "0.9"`, `tokio = "1"`, `rusqlite = "0.32"`, `clap = "4"`, `serde = "1"`

### Test Infrastructure
Integration tests use mock ACP agent binaries under `test-mock-agents` feature flag. Run with: `cargo test --features test-mock-agents -- acp_integration`.
