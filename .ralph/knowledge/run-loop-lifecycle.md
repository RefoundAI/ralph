---
title: Run Loop Lifecycle
tags: [run-loop, iteration, lifecycle, execution]
created_at: "2026-02-18T00:00:00Z"
---

Core iteration loop in `src/run_loop.rs`. Registers SIGINT handler at startup via [[Interrupt Handling]].

## Iteration Sequence

1. **Get ready tasks**: `get_scoped_ready_tasks()` filters by feature or task ID
2. **Check state**: Empty DAG → `NoPlan`, all resolved → `Complete`, no ready tasks → `Blocked`
3. **Claim task**: Atomically claim one ready task with agent ID
4. **Build context**: `build_iteration_context()` — parent, blockers, spec/plan ([[Feature Lifecycle]]), retry info, journal (smart-select, [[Journal System]]), knowledge (tag-match + link-expand, [[Knowledge System]])
5. **Select model**: Strategy picks model — see [[Model Strategy Selection]]
6. **Run ACP agent**: `run_iteration()` spawns agent via [[ACP Connection Lifecycle]]
7. **Check interrupt**: If `Interrupted`, enter interrupt flow ([[Interrupt Handling]])
8. **Parse output**: `extract_sigils()` — see [[Sigil Parsing]]
9. **Handle FAILURE**: `<promise>FAILURE</promise>` exits immediately, no DAG update
10. **Handle task sigils**: `<task-done>` / `<task-failed>`, run [[Verification Agent]] if enabled
11. **Post-iteration**: Write journal entry (always), write knowledge entries (if sigils present)
12. **Check completion**: All resolved → exit 0, limit reached → exit 0, blocked → exit 2

## Helper Functions

**`advance_iteration_with_model_selection(config, db, progress_db, hint)`**: Increments iteration, selects the next model (via [[Model Strategy Selection]]), logs the override to SQLite. Called at end of each iteration regardless of outcome.

**`recover_stuck_target_claim(config, db)`**: When targeting a single task (`RunTarget::Task`) and no ready tasks exist, checks if the target task is `in_progress` claimed by the *same* agent — if so, releases the stale claim and retries. Prevents self-deadlock from prior crash.

## No-Sigil Behavior

If Claude emits no task sigil, the claim is released and the task reverts to `pending`.

## Stop Reason Handling (FR-6.6)

Non-EndTurn stop reasons (`MaxTokens`, `Refusal`, etc.) release the claim and continue to next iteration — don't treat as task failure.

## Outcome Enum

`Complete`, `Failure`, `LimitReached`, `Blocked`, `NoPlan`, `Interrupted`

`Blocked` typically means dependency deadlock (remaining tasks depend on failed blockers) or all remaining tasks are claimed by another agent. `NoPlan` means no `feature build` or `task add` has populated the DAG yet.

See also: [[Sigil Parsing]], [[Model Strategy Selection]], [[ACP Connection Lifecycle]], [[Interrupt Handling]], [[Verification Agent]], [[Journal System]], [[Knowledge System]], [[Feature Lifecycle]], [[Auto-Transitions]], [[Error Handling and Resilience]], [[Execution Modes]]
