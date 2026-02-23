---
title: One-Shot vs Feature Workflow
tags: [tasks, features, workflow, run-target, design, standalone]
created_at: "2026-02-23T06:37:00Z"
---

Ralph has two modes of operation: one-shot tasks for quick work and the feature workflow for complex changes.

## Key Difference: Context During Execution

Feature tasks receive the **full spec and plan** in every Claude session. One-shot tasks only get their title, description, and graph context. This is the most important practical distinction.

## One-Shot Tasks

Create: `ralph task add "Fix bug" -d "Details..."` (scriptable, prints task ID to stdout)
Run: `ralph run t-abc123`

Can build hierarchies without features:
```bash
ROOT=$(ralph task add "Refactor auth")
T1=$(ralph task add "Extract tokens" --parent "$ROOT")
T2=$(ralph task add "Add refresh" --parent "$ROOT")
ralph task deps add "$T1" "$T2"
ralph run "$ROOT"
```

**Context received:** task assignment, parent context, completed prerequisites, retry info, journal, knowledge. No spec or plan.

**Best for:** quick fixes, well-scoped changes, scripted/automated creation, exploratory work.

## Feature Workflow

```bash
ralph feature create my-feature  # Unified: spec → plan → build
ralph run my-feature
```

See [[Feature Lifecycle]] for the unified create command phases.

**Context received:** everything one-shot gets, plus full `spec.md` and `plan.md` content.

**Best for:** multi-file changes, work needing planning, complex features, documentation trail.

## RunTarget Resolution

`ralph run <target>`: if target starts with `t-` → `RunTarget::Task`, otherwise → `RunTarget::Feature`. Feature targets use `get_ready_tasks_for_feature(feature_id)`. Task targets filter to the single matching task. See [[Run Loop Lifecycle]].

## Database Differences

| Field | Standalone | Feature Task |
|---|---|---|
| `task_type` | `"standalone"` | `"feature"` |
| `feature_id` | `NULL` | Set to feature ID |

## Decision Guide

| Situation | Use |
|---|---|
| 1-2 tasks, clear scope | One-shot |
| 3+ tasks, needs planning | Feature |
| Quick fix, one sentence | One-shot |
| "Need to think about approach" | Feature |
| Scripted/automated creation | One-shot |
| Want documentation trail | Feature |
| Exploratory, unclear scope | One-shot |
| Multi-component with ordering | Feature |

**Rule of thumb:** Start with one-shot. If you're writing a long description to capture all requirements, switch to the feature workflow.

## Hybrid Approach

Both modes work in the same project. You can add tasks to an existing feature manually:
```bash
ralph task add "Extra step" --feature f-abc123 --parent t-root123
```

See also: [[Feature Lifecycle]], [[Run Loop Lifecycle]], [[Auto-Transitions]]
