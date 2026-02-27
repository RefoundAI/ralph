---
title: "Auto-Transitions"
tags: [dag, tasks, status, state-machine, transitions, features, auto-transitions, events]
created_at: "2026-02-26T08:01:51.902657+00:00"
---

## AutoTransition Enum

`set_task_status()` in `src/dag/transitions.rs` returns `Result<Vec<AutoTransition>>` describing all cascading state changes triggered by a status update:

```rust
pub enum AutoTransition {
    Unblocked { blocked_id, blocker_id },
    ParentCompleted { parent_id },
    ParentFailed { parent_id, child_id },
    FeatureDone { feature_name },
    FeatureFailed { feature_name },
}
```

Callers (run loop, CLI commands) wire returned events to `emit_event_info()` for the [[Event Emission System]].

## Task-Level Parent Auto-Transitions

When `set_task_status()` marks a task done/failed, it cascades upward:

- `auto_complete_parent()`: all children done → parent set to `done` via direct SQL (bypasses `set_task_status()` to avoid recursion) → recurse to grandparent
- `auto_fail_parent()`: any child failed → parent set to `failed` via direct SQL → cascade up
- Unblocked tasks: releases tasks whose dependencies are now satisfied

## Feature-Level Auto-Transitions

When all tasks for a feature resolve:

- All tasks done → feature status = "done" (via `auto_complete_feature()`)
- All tasks resolved but some failed → feature status = "failed" (via `auto_update_feature_on_fail()`)

Called from both `set_task_status()` and the recursive parent auto-transition paths.

The `feature list` display also derives effective status from task counts as a fallback for stale DB data.

See also: [[Parent Status Derivation]], [[Task Columns Mapping]], [[Run Loop Lifecycle]], [[Event Emission System]]
