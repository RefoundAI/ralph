---
title: Model Strategy Selection
tags: [strategy, model, config, run-loop]
created_at: "2026-02-18T00:00:00Z"
---

Model selection strategies in `src/strategy.rs` determine which Claude model runs each iteration.

## Strategies

- **Fixed**: Always uses `--model` value. Implied when `--model` passed without `--model-strategy`.
- **CostOptimized** (default): Starts at `sonnet`; escalates to `opus` on error signals; drops to `haiku` on clean completions.
- **Escalate**: Starts at `haiku`, monotonically escalates on failure. Only `<next-model>` hint can de-escalate.
- **PlanThenExecute**: `opus` for iteration 1, `sonnet` thereafter.

## Claude Override

`<next-model>opus|sonnet|haiku</next-model>` always wins — overrides strategy for the next iteration only. See [[Sigil Parsing]].

## Escalation Tracking

`Config.escalation_level` (0=haiku, 1=sonnet, 2=opus) persists across iterations within a run.

## SQLite Override Tracking

`select_model_with_db()` reads from and `log_model_override()` writes to the `model_overrides` table in `progress.db` (see [[Schema Migrations]] v5). This replaced earlier flat-file tracking. Each iteration's strategy choice and hint are recorded for analysis.

## CLI Resolution

`--model` alone → Fixed. `--model-strategy=fixed` requires `--model`. Default: CostOptimized with sonnet. See [[Config From Run Args]].

See also: [[Run Loop Lifecycle]], [[Sigil Parsing]], [[Config From Run Args]], [[Schema Migrations]]
