---
title: Auto-Transitions
tags: [dag, tasks, status, state-machine, transitions]
created_at: "2026-02-18T00:00:00Z"
---

Task status state machine with cascading auto-transitions in `src/dag/transitions.rs`.

## Valid Transitions

`pending` → `in_progress` → `done` | `failed`. Also: `in_progress` → `pending` (release claim), `failed` → `pending` (retry).

## Cascading Effects

**On task done:**
- `auto_unblock_tasks()`: dependents whose blockers are all done become ready
- `auto_complete_parent()`: if all children done, parent transitions to done (recursive up tree)

**On task failed:**
- `auto_fail_parent()`: immediately fails parent (recursive up tree). One failed child = failed parent.

## Force Transitions

`force_complete_task`, `force_fail_task` step through valid intermediate states (e.g., failed → pending → in_progress → done). `force_reset_task` uses direct SQL for done → pending.

## Gotcha

`auto_complete_parent` and `auto_fail_parent` use direct SQL (not `set_task_status`) to avoid infinite recursion while still cascading up the parent tree.

## Ready Query

A task is ready when: `pending`, leaf node (no children), parent not `failed`, all blockers `done`. Ordered by `priority ASC` then `created_at ASC`. See [[Run Loop Lifecycle]] for how ready tasks are claimed.

See also: [[Task Columns Mapping]], [[Run Loop Lifecycle]], [[Dependency Cycle Detection]], [[Parent Status Derivation]]
