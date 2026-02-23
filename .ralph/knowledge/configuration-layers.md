---
title: Configuration Layers
tags: [config, toml, cli, environment, agent, ralph-toml]
created_at: "2026-02-23T06:37:00Z"
---

Configuration resolves from three sources, with later sources overriding earlier ones.

## Layer 1: `.ralph.toml` (Project Config)

Discovered by walking up directory tree from CWD. Parsed into `RalphConfig` in `src/project.rs`:

```toml
[execution]
max_retries = 3
verify = true

[agent]
command = "claude"
```

All sections use `#[serde(default)]` — partial configs work. Unknown keys silently ignored for forward compatibility.

## Layer 2: CLI Flags

Flags on `ralph run` override `.ralph.toml`:

| Flag | Overrides |
|---|---|
| `--model MODEL` | Sets fixed strategy with given model |
| `--model-strategy STRAT` | Model selection strategy |
| `--max-retries N` | `execution.max_retries` |
| `--no-verify` | `execution.verify` (sets false) |
| `--agent CMD` | `agent.command` |
| `--limit N` | Iteration limit (0 = unlimited) |

**`--model` alone implies `--model-strategy=fixed`**. `--model-strategy=fixed` requires `--model` to be set. Validated in `cli::resolve_model_strategy()`.

## Layer 3: Environment Variables

| Variable | Equivalent Flag |
|---|---|
| `RALPH_LIMIT` | `--limit` |
| `RALPH_MODEL` | `--model` |
| `RALPH_MODEL_STRATEGY` | `--model-strategy` |
| `RALPH_AGENT` | `--agent` |
| `RALPH_ITERATION` | (internal) Starting iteration number |
| `RALPH_TOTAL` | (internal) Total planned iterations |

`RALPH_MODEL`, `RALPH_ITERATION`, and `RALPH_TOTAL` are also **passed through** to the spawned agent subprocess as env vars.

## Agent Command Resolution

Specific chain in [[Config From Run Args]]:
`--agent` flag > `RALPH_AGENT` env > `[agent].command` in `.ralph.toml` > `"claude"`

Validated with `shlex::split()` — `None` return means malformed input (e.g., unclosed quotes).

## Exit Codes

| Code | Outcome | Meaning |
|---|---|---|
| 0 | Complete | All tasks resolved |
| 0 | LimitReached | Iteration limit hit |
| 1 | Failure | Critical failure (`<promise>FAILURE</promise>`) |
| 2 | Blocked | No ready tasks but incomplete remain |
| 3 | NoPlan | DAG empty |

See also: [[Config From Run Args]], [[Model Strategy Selection]], [[Run Loop Lifecycle]]
