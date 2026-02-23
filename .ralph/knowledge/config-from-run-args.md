---
title: Config From Run Args
tags: [config, parameters, run-args, agent, testing]
created_at: "2026-02-18T00:00:00Z"
---

`Config::from_run_args()` in `src/config.rs` constructs runtime config from CLI args and project config.

## Signature (9 parameters)

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

## Agent Resolution Chain

`agent` param > `RALPH_AGENT` env var > `ralph_config.agent.command` > `"claude"`. Validated with `shlex::split()` — `None` means malformed input (unclosed quotes).

## Auto-Generated Fields

- `agent_id`: `agent-{8 hex}` from `DefaultHasher` over timestamp + PID
- `run_id`: `run-{8 hex}` from SHA-256 of timestamp + counter

## Adding a New Parameter

Update ALL call sites including test helpers in:
- `src/config.rs` (test_project(), test calls)
- `src/acp/prompt.rs` (test helpers)
- `src/strategy.rs` (test helpers)

Test helpers use `..Default::default()` on `RalphConfig`, so new config sections need `#[serde(default)]`.

See also: [[Model Strategy Selection]], [[ACP Connection Lifecycle]], [[Configuration Layers]]
