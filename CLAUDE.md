# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (stripped, LTO)
cargo test               # Run tests
cargo run -- --help      # Show CLI usage
cargo run -- --once      # Run single iteration (for testing)
```

## Project Overview

Ralph is an autonomous agent loop harness that iteratively invokes Claude Code until tasks are complete. It reads a prompt file, tracks progress, and loops until detecting completion/failure sigils or hitting an iteration limit.

## Architecture

### Core Loop (`src/run_loop.rs`)
The main loop is simple: spawn Claude, stream output, check for sigils, repeat or exit. No async runtime - uses synchronous `std::process` with `BufReader::lines()` for streaming.

### Claude Integration (`src/claude/`)
- **client.rs**: Spawns `claude` CLI with `--output-format stream-json`, handles both direct and sandboxed execution
- **events.rs**: Typed event structs for NDJSON parsing (Assistant, ToolResult, Result)
- **parser.rs**: Deserializes raw JSON into typed events

### Sandbox (`src/sandbox/`)
macOS `sandbox-exec` integration for filesystem write restrictions:
- **profile.rs**: Generates sandbox.sb profiles dynamically
- **rules.rs**: Defines allow rules (e.g., `--allow=aws` grants `~/.aws` write access)

The sandbox denies all writes except: project directory, temp dirs, Claude state (`~/.claude`, `~/.config/claude`), `~/.cache`, `~/.local/state`, and git worktree roots. Also blocks `com.apple.systemevents` to prevent UI automation.

### Completion Detection
Claude's final output is scanned for sigils:
- `<promise>COMPLETE</promise>` - All tasks done, exit 0
- `<promise>FAILURE</promise>` - Critical failure, exit 1

### Key Files
- `prompt` (default) - Task description file read by Claude each iteration
- `progress.txt` (default) - Claude appends summaries here; tracks what's done
- `specs/` - If empty, triggers interactive spec generation mode

## CLI Flags

```
ralph [PROMPT_FILE]       # Default: "prompt"
  --once                  # Single iteration
  --limit=N               # Max iterations (0=unlimited)
  --no-sandbox            # Disable macOS sandbox
  --allow=RULE            # Enable sandbox rule (e.g., aws)
  --progress-file=PATH    # Default: "progress.txt"
  --specs-dir=PATH        # Default: "specs"
```

Environment variables: `RALPH_FILE`, `RALPH_LIMIT`, `RALPH_PROGRESS_FILE`, etc.

## Nix Development

```bash
nix develop  # Enters shell with Rust toolchain via rust-overlay
```

## Releases

Uses `cargo-dist` v0.30.3. Config lives in `dist-workspace.toml` (not Cargo.toml).

### Key Files
- `dist-workspace.toml` - cargo-dist configuration (targets, installers, CI settings)
- `.github/workflows/release.yml` - Generated CI workflow, triggers on `v*` tags

### Commands

```bash
dist plan                # Preview what will be built
dist build               # Build for current platform locally
dist generate            # Regenerate CI workflow after config changes
```

### Cutting a Release
1. Bump version in `Cargo.toml`
2. Commit, tag `vX.Y.Z`, push tag
3. CI builds tarballs (.tar.xz), installer script, checksums, and source archive

### Targets
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`
