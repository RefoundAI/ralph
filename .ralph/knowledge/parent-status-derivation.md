---
title: Parent Status Derivation
tags: [dag, tasks, status, parent, transitions, compute]
created_at: "2026-02-23T06:37:00Z"
---

Parent tasks have dual nature: a stored `status` column in the DB, but their effective status is **derived from children** when queried.

## Read Path: `get_task_status()`

In `src/dag/tasks.rs`:

- **Leaf node** (no children): returns stored `status` value directly
- **Has children**: delegates to `compute_parent_status()`

## `compute_parent_status()`

Recursively evaluates children, applies rules in order:

1. Any child's derived status is `"failed"` → parent is `"failed"`
2. All children's derived statuses are `"done"` → parent is `"done"`
3. Any child's derived status is `"in_progress"` → parent is `"in_progress"`
4. Otherwise → parent is `"pending"`

Recursion means a three-level hierarchy (grandparent → parent → children) is correctly evaluated: if all leaves are `done`, both parent and grandparent derive `done`.

## Write Path: Auto-Transitions

The auto-transition functions in [[Auto-Transitions]] handle the **write side**:

- `auto_complete_parent()`: when all children done, directly updates parent to `done` via SQL (bypasses `set_task_status()` to avoid infinite recursion), then recursively checks grandparent
- `auto_fail_parent()`: when any child fails, directly updates parent to `failed` via SQL, cascades up

## Key Distinction

| Function | Purpose | Mechanism |
|---|---|---|
| `compute_parent_status()` | **Read**: query-time status for display (`ralph task show`) | Recursive query, no DB writes |
| `auto_complete_parent()` | **Write**: update DB when children complete | Direct SQL UPDATE, recursive |
| `auto_fail_parent()` | **Write**: update DB when child fails | Direct SQL UPDATE, recursive |

The read and write paths must agree. If `compute_parent_status()` reports `done` but `auto_complete_parent()` didn't fire, the stored status is stale. In practice this doesn't happen because auto-transitions fire on every `set_task_status()` call.

## Ready Task Query

Parent tasks are **never directly executed** — only leaf nodes (no children) appear in the ready queue. The ready query uses `NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent_id = t.id)` to exclude parents. Parents auto-complete/fail through cascading transitions when their children resolve.

See also: [[Auto-Transitions]], [[Task Columns Mapping]], [[Run Loop Lifecycle]]
