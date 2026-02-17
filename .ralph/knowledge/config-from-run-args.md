---
title: "Config::from_run_args parameter contract"
tags: [config, testing, parameters, run-args]
created_at: "2026-02-18T00:00:00Z"
---

`Config::from_run_args()` in `config.rs` takes 11 parameters (annotated `#[allow(clippy::too_many_arguments)]`):

```rust
pub fn from_run_args(
    prompt_file: Option<String>,
    once: bool,
    no_sandbox: bool,
    limit: Option<u32>,
    allow: Vec<String>,
    model_strategy: Option<String>,
    model: Option<String>,
    project: ProjectConfig,
    run_target: Option<RunTarget>,
    max_retries_override: Option<u32>,
    no_verify: bool,
) -> Result<Self>
```

Adding a new parameter requires updating ALL call sites, including test helpers in:
- `config.rs` (test_project(), test helper calls)
- `client.rs` (test helper functions)
- `strategy.rs` (test helper functions)

Test helpers use `..Default::default()` on `RalphConfig`, so new config sections must have `#[serde(default)]`.

The Config struct also auto-generates `agent_id` (format `agent-{8 hex}`) and `run_id` (format `run-{8 hex}`) per invocation. These are not parameters — they're computed internally.
