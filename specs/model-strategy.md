# Model Strategy System

Ralph swaps between Claude models (`opus`, `sonnet`, `haiku`) across loop iterations to optimize cost.

## Models

Ordered by cost/capability (high to low): `opus`, `sonnet`, `haiku`.

Valid model names are exactly these three strings. Reject anything else at CLI parse time.

## CLI Interface

### R1: CLI flags

- `--model-strategy=<strategy>` selects strategy. Values: `fixed`, `cost-optimized`, `escalate`, `plan-then-execute`. Default: `cost-optimized`.
- `--model=<model>` sets model for `fixed` strategy. When `--model` is passed alone (no `--model-strategy`), implies `fixed`.
- Env vars: `RALPH_MODEL_STRATEGY`, `RALPH_MODEL`. Same semantics as flags.

**Verify:** `cargo test -- model` passes; `cargo build` clean. Unit tests cover:
- `--model=opus` alone implies `fixed`
- `--model-strategy=fixed` without `--model` errors
- default (no flags) resolves to `cost-optimized`
- invalid model name errors
- invalid strategy name errors
- env vars work identically to flags

### R2: Config changes

Add to `Config`:
- `model_strategy: ModelStrategy` enum (`Fixed`, `CostOptimized`, `Escalate`, `PlanThenExecute`)
- `model: Option<String>` (only required for `Fixed`)
- `current_model: String` -- the model selected for the current iteration, updated each loop

`Config::from_args` validates: if strategy is `Fixed` and `model` is `None`, bail.

`Config::next_iteration` preserves strategy and model state.

**Verify:** `cargo test -- config` passes; `cargo build` clean.

## Strategies

### R3: `fixed` strategy

Always use the model from `--model`. No swapping. Pass `--model <model>` to `claude` CLI every iteration.

**Verify:** Unit test: fixed strategy always returns the configured model for any iteration number.

### R4: `cost-optimized` strategy (default)

Ralph picks cheapest model it believes can handle each iteration:
- Read progress file content, analyze for complexity signals (length of remaining work, error mentions, iteration count)
- Default to `sonnet` when uncertain or when heuristic is inconclusive
- Claude hint (R7) always overrides heuristic

Heuristic guidelines (not exhaustive -- implementation has latitude):
- Early iterations with no progress yet: `sonnet`
- Progress file mentions errors/failures/stuck: `opus`
- Progress file shows steady completion of simple tasks: `haiku`
- Uncertain: `sonnet`

**Verify:** Unit tests: heuristic returns expected model for sample progress file contents. Test that `sonnet` is default when input is empty/ambiguous.

### R5: `escalate` strategy

Start at `haiku`. On failure signals (errors in result, no progress detected, Claude hints escalation), move up: `haiku` -> `sonnet` -> `opus`. Never de-escalate automatically. Only de-escalate if Claude hints a lower model.

Track escalation level in config state across iterations.

**Verify:** Unit tests: escalation sequence works; stays at escalated level; de-escalates only on Claude hint.

### R6: `plan-then-execute` strategy

Iteration 1: `opus`. Iterations 2+: `sonnet` by default. Claude hint can override to `haiku` for simple tasks or `opus` for hard ones.

**Verify:** Unit test: iteration 1 returns `opus`, iteration 2+ returns `sonnet`, hint overrides apply.

## Claude Hint Mechanism

### R7: `<next-model>` sigil

Claude can emit in its output:
- `<next-model>opus</next-model>`
- `<next-model>sonnet</next-model>`
- `<next-model>haiku</next-model>`

Rules:
- Hint ALWAYS wins over Ralph's strategy heuristic
- Applies to the NEXT iteration only (not persistent across multiple iterations)
- Optional -- if absent, strategy decides

Parse this sigil from the result text the same way `COMPLETE`/`FAILURE` sigils are parsed. Extract model name from inside the tags.

Add to `ResultEvent` or return alongside it: `next_model_hint: Option<String>`.

**Verify:** Unit tests: parse `<next-model>opus</next-model>` from result text; ignore malformed sigils; extract correct model name. Integration: hint overrides strategy choice.

### R8: System prompt update

Add the `<next-model>` sigil documentation to the system prompt built in `client.rs::build_system_prompt`. Claude must know it can emit this sigil and what valid values are.

**Verify:** `build_system_prompt` output contains `<next-model>` documentation. Test string contains all three model names.

## Override Logging

### R9: Log override events

When Claude's hint disagrees with the strategy's choice, log:
- Iteration number
- Strategy's choice
- Claude's hint
- Reason (if Claude provided one adjacent to the sigil -- best-effort extraction, not required)

Log destination: append to progress file in a `## Model Overrides` section, or a separate `model-decisions.log` file (implementer's choice -- either is acceptable).

**Verify:** Unit test: when hint != strategy choice, override is recorded. When hint == strategy choice, no override logged.

## Claude CLI Integration

### R10: Pass `--model` to claude CLI

In `client.rs`, add `--model <model>` to the args vec passed to `claude`. Use `config.current_model` as the value.

Currently no `--model` is passed -- this is new. Insert before the `--print` arg or anywhere in the args list.

**Verify:** Unit test or inspection: args vec contains `--model` followed by the selected model name. `cargo test` passes.

### R11: Display selected model

The model name from the assistant event is already printed (`formatter.rs` line 24). Verify this reflects the actually-selected model after this feature lands. No change expected if Claude's response includes the model field -- just confirm it works.

**Verify:** Manual: run `ralph --model=haiku --once` and confirm output shows haiku model name.

## Tasks

- [ ] [R1] Add `--model-strategy` and `--model` to `Args` in `cli.rs` with env var support
- [ ] [R2] Add `ModelStrategy` enum, `model`/`model_strategy`/`current_model` fields to `Config`; validate in `from_args`
- [ ] [R3] Implement `fixed` strategy (return configured model unconditionally)
- [ ] [R4] Implement `cost-optimized` strategy with progress-file heuristic, default `sonnet`
- [ ] [R5] Implement `escalate` strategy with level tracking
- [ ] [R6] Implement `plan-then-execute` strategy
- [ ] [R7] Parse `<next-model>` sigil from result text; wire hint into model selection for next iteration
- [ ] [R8] Add `<next-model>` sigil docs to system prompt
- [ ] [R9] Log override events when hint disagrees with strategy
- [ ] [R10] Pass `--model <current_model>` to claude CLI args in `client.rs`
- [ ] [R11] Verify model display in formatter output

Checkpoint: after R1-R2, run `cargo build && cargo test` before proceeding. After R3-R6, run tests again. After R7-R10, full `cargo test`.
