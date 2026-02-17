---
title: "Run loop iteration lifecycle"
tags: [run-loop, iteration, lifecycle, loop, execution]
created_at: "2026-02-18T00:00:00Z"
---

Each iteration in `run_loop.rs` follows this sequence:

1. **Get ready tasks**: `get_scoped_ready_tasks()` filters by feature or task ID
2. **Print DAG summary**: Shows task counts and current state
3. **Claim task**: Atomically claim one ready task with agent ID
4. **Build context**: `build_iteration_context()` assembles parent info, blockers, spec/plan, retry info, journal entries (smart-select: recent + FTS), knowledge entries (tag-matched)
5. **Select model**: Strategy picks opus/sonnet/haiku based on history
6. **Spawn Claude**: `stream_claude()` with system prompt and task context
7. **Parse output**: Extract sigils from Claude's text output
8. **Handle FAILURE**: If `<promise>FAILURE</promise>`, exit immediately (no DAG update)
9. **Handle task sigils**: Process `<task-done>` or `<task-failed>`, run verification if enabled
10. **Post-iteration**: Write journal entry (always), write knowledge entries (if any `<knowledge>` sigils)
11. **Check completion**: All resolved → exit 0, limit reached → exit 0, blocked → exit 2

If Claude emits no sigil, the task claim is released and the task reverts to pending for the next iteration.

Journal entries are written after task state updates so they record the final outcome (done/failed/retried/blocked). Knowledge entries are written per-sigil, one `.md` file each.
