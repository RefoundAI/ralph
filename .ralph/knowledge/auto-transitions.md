---
title: "Feature Auto-Transitions"
tags: [dag, tasks, status, state-machine, transitions, features, auto-transitions]
created_at: "2026-02-26T08:01:51.902657+00:00"
---

When all tasks for a feature resolve, the feature status is automatically updated:

- All tasks done → feature status = "done" (via `auto_complete_feature()`)
- All tasks resolved but some failed → feature status = "failed" (via `auto_update_feature_on_fail()`)

These are called from both `set_task_status()` and the recursive `auto_complete_parent()`/`auto_fail_parent()` paths to handle cases where parent task completion bypasses `set_task_status()`.

The `feature list` display also derives effective status from task counts as a fallback for stale DB data.

See [[Auto-Transitions]] for the task-level parent auto-transition pattern this builds on.
