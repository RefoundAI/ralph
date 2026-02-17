---
title: "Feature lifecycle and workflow"
tags: [feature, spec, plan, build, lifecycle, workflow]
created_at: "2026-02-18T00:00:00Z"
---

Features progress through a defined lifecycle: `draft` → `planned` → `ready` → `running` → `done`/`failed`.

Workflow stages:
1. **`ralph feature spec <name>`**: Creates feature in `draft` state. Opens interactive Claude session to write `.ralph/features/<name>/spec.md`.
2. **`ralph feature plan <name>`**: Creates implementation plan from spec. Writes `.ralph/features/<name>/plan.md`. Feature becomes `planned`.
3. **`ralph feature build <name>`**: Decomposes plan into task DAG via interactive Claude session. Claude uses `ralph task add` and `ralph task deps add` CLI commands (not JSON). Feature becomes `ready`.
4. **`ralph run <name>`**: Picks ready tasks one at a time, invokes Claude to complete them. Feature becomes `running`, then `done`/`failed`.

Feature context (spec + plan content) is loaded by `resolve_feature_context()` in `run_loop.rs` and included in each iteration's system prompt.

Feature management functions in `feature.rs`: `create_feature`, `get_feature` (by name), `get_feature_by_id`, `list_features`, `update_feature_status/spec_path/plan_path`, `ensure_feature_dirs`, `read_spec`, `read_plan`.

For quick one-off work, standalone tasks bypass the feature lifecycle: `ralph task add` + `ralph run <task-id>`.
