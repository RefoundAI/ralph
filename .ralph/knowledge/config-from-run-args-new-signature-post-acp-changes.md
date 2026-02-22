---
title: "Config::from_run_args new signature (post-ACP changes)"
tags: [config, agent, acp, from_run_args, testing]
feature: "acp"
created_at: "2026-02-21T20:05:41.029350+00:00"
---

After task t-f6a3b5, `Config::from_run_args()` has a new 9-parameter signature (removed prompt_file/no_sandbox/allow, added agent):

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

The `agent` param resolves via: `agent` > `RALPH_AGENT` env var > `ralph_config.agent.command` > "claude". Validated with `shlex::split()` — None return means malformed input (e.g., unclosed quotes), returns error "invalid agent command: failed to parse \"...\"".

Test helpers across config.rs, acp/prompt.rs, strategy.rs all updated to match. The `--no-sandbox` and `--allow` CLI flags have been removed.

The Config struct no longer has: `prompt_file`, `use_sandbox`, `allowed_tools`, `allow_rules`. It now has `agent_command: String`.
