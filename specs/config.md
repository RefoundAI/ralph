# Project Configuration (`.ralph.toml`)

Ralph discovers a `.ralph.toml` file to resolve project-level settings. All paths in config are relative to the file's parent directory.

## Schema

```toml
[specs]
dirs = [".ralph/specs"]  # spec directories, default: [".ralph/specs"]

[prompts]
dir = ".ralph/prompts"   # prompt directory, default: ".ralph/prompts"
```

Empty file is valid -- all fields have defaults. Unknown keys are silently ignored (forward compat).

## Config Discovery

### R1: Walk-up discovery

- Starting from CWD, walk parent directories until `.ralph.toml` is found or filesystem root is reached.
- When found, set `project_root` to the directory containing `.ralph.toml`. All relative paths in the config resolve against `project_root`.
- When not found, commands that need config (`run`, `plan`, `specs`) error with: `No .ralph.toml found. Run 'ralph init' to create one.`
- `ralph init` does not require an existing `.ralph.toml`.

New module: `src/project.rs`.
- `pub fn discover() -> Result<ProjectConfig>` -- walk-up search, parse, return.
- `pub struct ProjectConfig { pub root: PathBuf, pub config: RalphConfig }`.

**Verify:** Unit tests:
- `.ralph.toml` in CWD is found.
- `.ralph.toml` two directories up is found.
- No `.ralph.toml` anywhere returns error containing `ralph init`.
- Relative paths resolve against the config file's directory, not CWD.

### R2: TOML parsing

Add `toml` crate to `Cargo.toml` dependencies.

Deserialize into typed structs using `serde::Deserialize`:

```rust
#[derive(Deserialize, Default)]
pub struct RalphConfig {
    #[serde(default)]
    pub specs: SpecsConfig,
    #[serde(default)]
    pub prompts: PromptsConfig,
}

#[derive(Deserialize)]
pub struct SpecsConfig {
    #[serde(default = "default_specs_dirs")]
    pub dirs: Vec<String>,
}

#[derive(Deserialize)]
pub struct PromptsConfig {
    #[serde(default = "default_prompts_dir")]
    pub dir: String,
}
```

Use `#[serde(deny_unknown_fields)]` -- NO. Unknown keys ignored for forward compat.

**Verify:** Unit tests:
- Empty string parses to defaults.
- `[specs]\ndirs = ["custom"]` parses; `prompts.dir` still default.
- Invalid TOML (e.g., missing quote) returns `Err`.
- Unknown key `[foo]\nbar = 1` parses without error.

### R3: Defaults

- `specs.dirs` -> `vec![".ralph/specs"]`
- `prompts.dir` -> `".ralph/prompts"`

**Verify:** Unit test: `RalphConfig::default()` returns these values exactly.

## `ralph init`

### R4: Init command

Add `init` subcommand to CLI. When invoked:

1. If `.ralph.toml` exists in CWD, print `".ralph.toml already exists, skipping."` and do NOT overwrite.
2. Otherwise, write `.ralph.toml` with commented defaults:
   ```toml
   [specs]
   # dirs = [".ralph/specs"]

   [prompts]
   # dir = ".ralph/prompts"
   ```
3. Create directories: `.ralph/`, `.ralph/prompts/`, `.ralph/specs/`. Use `create_dir_all` -- no error if they exist.
4. Create `.ralph/progress.db` as an empty file (SQLite schema creation is out of scope for this spec -- just touch the file).
5. If `.gitignore` exists, check if it contains `.ralph/progress.db`. If not, append `\n.ralph/progress.db\n`. If `.gitignore` does not exist, create it with `.ralph/progress.db\n`.

Idempotent: running twice produces no errors, does not overwrite `.ralph.toml`, creates only missing dirs/files.

**Verify:** Integration test in a `tempdir`:
- First run creates `.ralph.toml`, `.ralph/prompts/`, `.ralph/specs/`, `.ralph/progress.db`, and `.gitignore` entry.
- Second run prints skip message, all files unchanged.
- Pre-existing `.gitignore` with other content gets `.ralph/progress.db` appended without duplicates.

## Migration

### R5: Remove replaced CLI flags

- Remove `--specs-dir` / `RALPH_SPECS_DIR` from `Args` in `src/cli.rs`.
- Remove `--progress-file` / `RALPH_PROGRESS_FILE` from `Args` in `src/cli.rs`.
- Remove `specs_dir: Option<String>` and `progress_file: Option<String>` from `Args`.
- Remove `specs_dir: String` and `progress_file: String` from `Config`.
- `Config::from_args` no longer sets these fields. Instead, `Config` gains a `project_root: PathBuf` field (from discovery).
- Callers in `run_loop.rs` that reference `config.specs_dir` or `config.progress_file` now derive paths from `config.project_root` + `RalphConfig` values:
  - `config.progress_file` -> `config.project_root.join(".ralph/progress.db")`
  - `config.specs_dir` -> iterate `ralph_config.specs.dirs`, resolve each against `project_root`
  - `config.prompt_file` -> `config.project_root.join(ralph_config.prompts.dir).join(...)` (prompt file selection logic is out of scope for this spec)
- Update test helpers in `src/config.rs` and `src/cli.rs` that reference removed fields.

**Verify:** `cargo build` clean. `cargo test` passes. No references to `--specs-dir` or `--progress-file` remain in source (grep confirms).

### R6: `.ralph/` directory structure

After `ralph init`:
```
.ralph/
  prompts/
  specs/
  progress.db
```

All three created by R4. `progress.db` is always at `.ralph/progress.db` (not configurable). `prompts/` and `specs/` locations are configurable but default here.

**Verify:** After `ralph init` in a temp dir, all three paths exist. `progress.db` is a regular file. `prompts/` and `specs/` are directories.

## Wiring

### R7: Integrate discovery into main

In `src/main.rs`:
- Before `Config::from_args`, call `project::discover()`.
- If `discover()` returns error and the command is not `init`, propagate the error.
- Pass `ProjectConfig` into `Config::from_args` (signature change: `from_args(args, project: ProjectConfig)`).
- `Config` stores `project_root: PathBuf` and `ralph_config: RalphConfig`.

In `src/run_loop.rs`:
- Replace `touch_file(&config.prompt_file)` with path derived from project config.
- Replace `has_specs(&config.specs_dir)` with iteration over `ralph_config.specs.dirs` resolved against `project_root`.
- Replace `touch_file(&config.progress_file)` -- progress is now `.ralph/progress.db`, always exists after init.

**Verify:** `cargo build` clean. `cargo test` passes. `ralph --once` in a directory with `.ralph.toml` runs without error.

## Tasks

- [ ] [R1] Add `src/project.rs` with `discover()` and `ProjectConfig`
- [ ] [R2] Add `toml` dep; define `RalphConfig`, `SpecsConfig`, `PromptsConfig` with serde
- [ ] [R3] Implement `Default` for config structs with specified values
- [ ] [R4] Add `ralph init` subcommand; create `.ralph.toml`, dirs, `.gitignore` entry
- [ ] [R5] Remove `--specs-dir`, `--progress-file` from CLI; update `Config` and callers
- [ ] [R6] Verify `.ralph/` structure after init
- [ ] [R7] Wire `discover()` into `main.rs` and `run_loop.rs`

Checkpoint: after R1-R3, run `cargo build && cargo test`. After R4, run init integration test. After R5-R7, full `cargo build && cargo test`.
