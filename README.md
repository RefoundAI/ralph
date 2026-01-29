# Ralph

Autonomous agent loop harness for [Claude Code][claude-code].

Ralph iteratively invokes Claude Code until all tasks are complete, enabling
hands-off execution of multi-step coding workflows. On macOS, it sandboxes
Claude to restrict filesystem writes to the project directory.

> [!WARNING]
> Ralph can (and possibly WILL) destroy anything you have access to, according
> to the whims of the LLM. Use `--once` to test before unleashing unattended
> loops.

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
./target/release/ralph --help
```

## Usage

```bash
ralph                    # Loop until complete (reads "prompt" file)
ralph --once             # Single iteration for testing
ralph --limit=5          # Max 5 iterations
ralph task.md            # Use custom prompt file
```

### How It Works

1. Ralph reads your prompt file and progress file
2. Invokes Claude Code with a system prompt instructing it to complete ONE task
3. Claude appends a summary to the progress file and commits changes
4. Ralph checks for completion sigils in Claude's output:
   - `<promise>COMPLETE</promise>` — all tasks done, exit 0
   - `<promise>FAILURE</promise>` — critical failure, exit 1
5. If neither sigil found and limit not reached, loop continues

### Project Files

- **prompt** (or custom file) — describes tasks for Claude to complete
- **progress.txt** — tracks completed work across iterations
- **specs/** — if empty, triggers interactive spec generation mode

## Sandbox Mode (macOS)

By default, Ralph wraps Claude in `sandbox-exec` to restrict filesystem writes:

- Allowed: project directory, `/tmp`, `~/.config/claude`, `~/.cache`, git
  worktree root
- Blocked: everything else, plus `com.apple.systemevents` (prevents UI
  automation)

```bash
ralph --no-sandbox       # Disable sandboxing
ralph --allow=aws        # Grant write access to ~/.aws
```

## CLI Reference

```
ralph [OPTIONS] [PROMPT_FILE]

Arguments:
  [PROMPT_FILE]           Path to prompt file [default: prompt]

Options:
  -o, --once              Run exactly once
      --limit <N>         Maximum iterations (0 = unlimited)
      --no-sandbox        Disable macOS sandbox
      --progress-file     Progress file path [default: progress.txt]
      --specs-dir         Specs directory path [default: specs]
      --allowed-tools     Tool whitelist (with --no-sandbox)
  -a, --allow <RULE>      Enable sandbox rule (e.g., aws)
  -h, --help              Print help
  -V, --version           Print version
```

### Environment Variables

| Variable              | Description                    |
| :-------------------- | :----------------------------- |
| `RALPH_FILE`          | Default prompt file            |
| `RALPH_PROGRESS_FILE` | Default progress file          |
| `RALPH_SPECS_DIR`     | Default specs directory        |
| `RALPH_LIMIT`         | Default iteration limit        |
| `RALPH_ITERATION`     | Current iteration (for resume) |
| `RALPH_TOTAL`         | Total iterations (for display) |

## Development

Requires Rust toolchain. With Nix:

```bash
nix develop
cargo build
cargo test
```

## License

MIT

[claude-code]: https://claude.ai/code
