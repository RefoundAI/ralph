---
title: "Model strategy selection and escalation"
tags: [model, strategy, escalation, cost, opus, sonnet, haiku]
created_at: "2026-02-18T00:00:00Z"
---

Model selection in `strategy.rs` determines which Claude model to use each iteration:

- **Fixed**: Always uses `--model` value. No swapping.
- **CostOptimized** (default): Starts with sonnet. Escalates to opus on error signals (compile errors, test failures, panics). Drops to haiku on clean task completions.
- **Escalate**: Starts at haiku. On failure signals, escalates haiku→sonnet→opus. Monotonic — never auto-de-escalates. Only a `<next-model>` hint from Claude can step back down.
- **PlanThenExecute**: Opus for iteration 1 (planning), sonnet for all subsequent iterations.

Claude can override any strategy for the next iteration via `<next-model>opus|sonnet|haiku</next-model>`. Hints apply to the next iteration only and always override the strategy's choice.

The escalation level is tracked in `Config.escalation_level` (0=haiku, 1=sonnet, 2=opus) and persisted across iterations via `config.next_iteration()`.
