---
title: "Task status auto-transitions"
tags: [task, status, transitions, dag, parent, dependencies]
created_at: "2026-02-18T00:00:00Z"
---

Task status transitions in `dag/transitions.rs` follow a state machine with automatic cascading effects.

Valid transitions: pending→in_progress, pending→blocked, in_progress→done, in_progress→failed, in_progress→pending, blocked→pending, failed→pending.

Auto-transitions triggered by `set_task_status()`:
- **Task marked done** → `auto_unblock_tasks()` checks all blocked tasks; if ALL their blockers are done, transitions them blocked→pending. Then `auto_complete_parent()` checks if all siblings are done; if so, marks parent done (recursive up the tree).
- **Task marked failed** → `auto_fail_parent()` immediately fails the parent (recursive up the tree). One failed child = failed parent.

Force-transition functions (`force_complete_task`, `force_fail_task`) step through valid intermediate states (e.g., failed→pending→in_progress→done) rather than bypassing the state machine.

`force_reset_task()` uses direct SQL for done→pending since that's not a valid transition in the state machine.

Gotcha: auto_complete_parent and auto_fail_parent use direct SQL updates (not set_task_status) to avoid infinite recursion while still cascading up the parent tree.
