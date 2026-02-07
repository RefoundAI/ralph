# CLI Restructure: Subcommands

Replace the flat CLI (`ralph [PROMPT_FILE] --flags`) with subcommands: `init`, `prompt`, `run`, `specs`, `plan`.

## Directory Structure

Ralph projects have a `.ralph/` directory at project root:

```
.ralph/
  prompts/          # prompt files (YYYY-MM-DD-*.md)
  progress.db       # SQLite DAG (replaces progress.txt)
.ralph.toml         # project config
```

`.ralph.toml` schema (TOML):

```toml
prompts-dir = ".ralph/prompts"    # default
specs-dirs = [".ralph/specs"]     # list of directories, default [".ralph/specs"]
```

## Subcommands

### R1: Top-level command restructure

Replace `Args` struct in `cli.rs` with clap `Subcommand` derive enum. The top-level `ralph` command takes no positional args and no flags other than `--version`/`--help`. Running bare `ralph` prints help.

Use `#[derive(Subcommand)]` with variants: `Init`, `Prompt`, `Run`, `Specs`, `Plan`.

**Verify:** `cargo build` clean. `ralph --help` shows five subcommands. `ralph` with no args prints help (not an error). `ralph --version` works.

### R2: `ralph init`

No flags. Interactive setup:
1. Create `.ralph/` directory structure (`prompts/`)
2. Create `.ralph.toml` with defaults if it doesn't exist
3. Create `.ralph/progress.db` (empty SQLite -- schema is out of scope for this spec, just create the file)
4. Print what was created

If `.ralph.toml` already exists, print message and exit 0 (no-op, don't overwrite).

**Verify:** `cargo test -- init` passes. Tests:
- Creates `.ralph/`, `.ralph/prompts/`, `.ralph.toml`, `.ralph/progress.db` in a temp dir
- Running init twice is idempotent (second run doesn't overwrite `.ralph.toml`)
- `.ralph.toml` contains valid TOML with `prompts-dir` and `specs-dirs` keys

### R3: `ralph prompt`

No flags. Behavior:
1. Read `prompts-dir` from `.ralph.toml` (fall back to `.ralph/prompts` if no config)
2. Launch `claude` in interactive mode (no `--output-format`, no `--print`) with a system prompt instructing it to co-author a prompt file with the user
3. The system prompt MUST instruct Claude to write the resulting file to the prompts directory with naming format `YYYY-MM-DD-<slug>.md`
4. Exit when Claude exits

Does NOT validate the output filename -- Claude is instructed but the user/Claude have final say.

**Verify:** `cargo build` clean. `ralph prompt --help` shows no required args. Unit test: the system prompt string contains `YYYY-MM-DD` and the prompts directory path.

### R4: `ralph run`

```
ralph run [PROMPT_FILE]
  -o, --once                    # single iteration
  --limit=<N>                   # max iterations, 0=unlimited
  --model=<MODEL>               # opus, sonnet, haiku
  --model-strategy=<STRATEGY>   # fixed, cost-optimized, escalate, plan-then-execute
  --no-sandbox                  # disable macOS sandbox
  -a, --allow=<RULE>            # sandbox allow rule (repeatable)
```

Env vars: `RALPH_FILE` (prompt file), `RALPH_LIMIT`, `RALPH_MODEL`, `RALPH_MODEL_STRATEGY`.

Removed flags (vs current root command):
- `--progress-file` -- hardcoded to `.ralph/progress.db`
- `--specs-dir` -- read from `.ralph.toml` `specs-dirs`
- `--allowed-tools` -- keep internal, remove from CLI surface

When `PROMPT_FILE` is provided:
- Must be a file path. Help text: `"Path to a prompt file (not a raw prompt string)"`.
- Execute the loop with that file.

When `PROMPT_FILE` is omitted:
- Read `prompts-dir` from `.ralph.toml` (fall back to `.ralph/prompts`)
- List `*.md` files matching `YYYY-MM-DD-*.md` pattern
- Sort by date prefix descending (most recent first)
- If no files found, print error message pointing user to `ralph prompt` and exit 1
- If files found, display interactive picker (numbered list, user types number)
- Selected file becomes the prompt file for the run

Config resolution for `specs-dirs`:
- Read `.ralph.toml` `specs-dirs` array
- Fall back to `[".ralph/specs"]` if no config file or key missing
- `Config.specs_dir: String` becomes `Config.specs_dirs: Vec<String>`

Config resolution for progress:
- `Config.progress_file: String` becomes hardcoded `.ralph/progress.db`
- Remove `progress_file` field from `Config`; use constant or method

**Verify:** `cargo test -- run` passes; `cargo build` clean. Tests:
- `ralph run --help` shows `PROMPT_FILE` as optional positional arg
- `--progress-file` flag does not exist (parse error if passed)
- `--specs-dir` flag does not exist (parse error if passed)
- `RALPH_FILE` env var still works for prompt file
- `--model`, `--model-strategy`, `--limit`, `--once`, `--no-sandbox`, `--allow` all parse correctly
- `--once` and `--limit` remain mutually exclusive
- Prompt picker lists files sorted by date descending (unit test with temp dir containing dated files)
- Non-matching filenames in prompts dir are excluded from picker

### R5: `ralph specs`

No flags. Behavior:
1. Read `specs-dirs` from `.ralph.toml` (fall back to `[".ralph/specs"]`)
2. Read existing spec files from all listed directories
3. Launch `claude` in interactive mode with system prompt for spec authoring
4. System prompt includes list of existing spec files and their paths
5. Exit when Claude exits

This replaces the current `run_interactive_specs()` in `run_loop.rs`. Remove that function and the `has_specs` check from the run loop.

**Verify:** `cargo build` clean. `ralph specs --help` shows no required args. The `run_loop.rs` no longer calls `run_interactive_specs` or checks `has_specs`.

### R6: `ralph plan`

No flags (for now -- prompt file selection uses same picker logic as `ralph run`).

Behavior:
1. Select prompt file (same picker as `ralph run` when no arg given, or accept optional positional `PROMPT_FILE`)
2. Read all specs from `specs-dirs`
3. Launch `claude` with `--print` and a system prompt instructing it to decompose the prompt + specs into a task DAG
4. Store result in `.ralph/progress.db`

The DAG schema and storage are out of scope for this spec. For now, stub the plan storage -- the command should parse correctly and launch Claude. Actual DAG implementation is a separate spec.

**Verify:** `cargo build` clean. `ralph plan --help` works. Command launches Claude (manual verification).

## Migration

### R7: Remove dead code

After R1-R6:
- Remove `--progress-file` from `Args` and `Config`
- Remove `--specs-dir` from `Args` and `Config`
- Remove `--allowed-tools` from `Args` (keep `DEFAULT_ALLOWED_TOOLS` const, use internally)
- Remove `run_interactive_specs()` and `has_specs()` from `run_loop.rs`
- Remove positional `PROMPT_FILE` from root command (it's now on `run`)
- Update `Config::from_args` to accept the `Run` subcommand args (not the top-level `Args`)

**Verify:** `cargo build` clean. No compiler warnings about unused fields. `cargo test` passes.

### R8: `.ralph.toml` parsing

Add TOML config file reading. Use `toml` crate (add to `Cargo.toml` dependencies).

Config resolution order (highest priority first):
1. CLI flags (on `ralph run`)
2. Environment variables
3. `.ralph.toml`
4. Defaults

Parse `.ralph.toml` from current working directory. If file doesn't exist, use defaults silently (no error).

Struct:

```rust
struct ProjectConfig {
    prompts_dir: Option<String>,  // default: ".ralph/prompts"
    specs_dirs: Option<Vec<String>>,  // default: [".ralph/specs"]
}
```

**Verify:** `cargo test -- toml` passes. Tests:
- Missing `.ralph.toml` returns defaults
- Valid `.ralph.toml` with `prompts-dir` overrides default
- Valid `.ralph.toml` with `specs-dirs` overrides default
- Malformed `.ralph.toml` returns error (not silent fallback to defaults)
- CLI flags override `.ralph.toml` values

## Backward Compatibility

### R9: Bare `ralph` behavior

Currently `ralph` with no args runs the loop with default `prompt` file. After restructure, bare `ralph` prints help.

This is a breaking change. Document in changelog/release notes.

`ralph run` with no args and no `RALPH_FILE` env var triggers the interactive picker. `ralph run prompt` (explicit file) preserves the old `ralph prompt` behavior (running the loop with file named "prompt").

**Verify:** `ralph` prints help. `ralph run prompt` runs the loop using file `prompt`. `RALPH_FILE=prompt ralph run` also works.

## Tasks

- [ ] [R8] Add `toml` crate dependency; implement `.ralph.toml` parsing
- [ ] [R1] Restructure `cli.rs` to subcommand enum
- [ ] [R2] Implement `ralph init`
- [ ] [R4] Implement `ralph run` with prompt picker, migrate all existing run flags
- [ ] [R7] Remove dead code (`--progress-file`, `--specs-dir`, `--allowed-tools` CLI surface, `run_interactive_specs`, `has_specs`)
- [ ] [R3] Implement `ralph prompt`
- [ ] [R5] Implement `ralph specs`
- [ ] [R6] Implement `ralph plan` (stub DAG storage)
- [ ] [R9] Verify backward compat; update help text

Checkpoint: after R8+R1, `cargo build && cargo test`. After R4+R7, `cargo build && cargo test`. After all, full `cargo test`.
