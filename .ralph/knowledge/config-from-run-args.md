---
title: "Config::from_run_args parameter contract"
tags: [config, testing, parameters, run-args, agent, acp]
created_at: "2026-02-18T00:00:00Z"
---

`Config::from_run_args()` in `config.rs` takes 9 parameters (post-ACP migration):

```rust
pub fn from_run_args(
    once: bool,
    limit: Option<u32>,
    model_strategy: Option<String>,
    model: Option<String>,
    project: ProjectConfig,
    run_target: Option<RunTarget>,
    max_retries_override: Option<u32>,
    no_verify: bool,
    agent: Option<String>,
) -> Result<Self>
```

Removed from pre-ACP: `prompt_file`, `no_sandbox`, `allow` (Vec).
Added: `agent` (Option<String>).

The `agent` param resolves via: `agent` > `RALPH_AGENT` env var > `ralph_config.agent.command` > "claude". Validated with `shlex::split()`.

Adding a new parameter requires updating ALL call sites, including test helpers in:
- `config.rs` (test_project(), test helper calls)
- `acp/prompt.rs` (test helper functions)
- `strategy.rs` (test helper functions)

Test helpers use `..Default::default()` on `RalphConfig`, so new config sections must have `#[serde(default)]`.

The Config struct auto-generates `agent_id` (format `agent-{8 hex}`) and `run_id` (format `run-{8 hex}`) per invocation. The Config struct has `agent_command: String` (not Option).
