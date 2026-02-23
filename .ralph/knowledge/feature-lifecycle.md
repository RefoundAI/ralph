---
title: Feature Lifecycle
tags: [feature, workflow, spec, plan, build, cli]
created_at: "2026-02-18T00:00:00Z"
---

Feature workflow from creation to execution via `ralph feature create <name>`.

## Unified Create Command

Single command runs three sequential phases (replaces separate spec/plan/build commands):

1. **Spec**: Interactive ACP interview → writes `.ralph/features/<name>/spec.md` → iterative review (max 5 rounds)
2. **Plan**: Interactive ACP interview (spec as context) → writes `.ralph/features/<name>/plan.md` → iterative review
3. **Build**: Autonomous ACP session reads spec+plan → emits task DAG via CLI commands

Each phase **skips if its output file already exists** on disk — natural resume on interruption. The `FeatureAction` enum only has `Create` and `List` variants. `--model` and `--agent` flags apply to all phases.

## Status Flow

`draft` → `planned` → `ready` → `running` → `done` | `failed`

## Context in Runs

`resolve_feature_context()` in [[Run Loop Lifecycle]] loads spec and plan content. Included in system prompt via [[System Prompt Construction]]. `get_scoped_ready_tasks()` filters by `feature_id`.

## Artifact Flow

Spec and plan are not consumed and discarded — they flow through the entire pipeline:
- **Plan** reads the **spec** to know what to design
- **Build** reads both to decompose work into tasks
- Every **execution iteration** reads both, giving Claude the full picture even when the immediate task is narrow

This means a Claude session implementing a narrow task still has access to the full spec's acceptance criteria and the plan's architectural decisions.

## Standalone Tasks

For one-off work: `ralph task add <title>` + `ralph run <task-id>` bypasses the feature lifecycle entirely. See [[One-Shot vs Feature Workflow]] for comparison and decision guide.

See also: [[Run Loop Lifecycle]], [[System Prompt Construction]], [[Verification Agent]], [[Execution Modes]], [[One-Shot vs Feature Workflow]]
