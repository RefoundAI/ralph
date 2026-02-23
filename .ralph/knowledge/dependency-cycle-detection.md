---
title: Dependency Cycle Detection
tags: [dag, dependencies, graph, bfs]
created_at: "2026-02-18T00:00:00Z"
---

BFS-based cycle detection in `src/dag/dependencies.rs` prevents circular task dependencies.

## Edge Semantics

`add_dependency(blocker_id, blocked_id)`: blocker must complete before blocked can start. Stored as `(blocker_id, blocked_id)` with composite PRIMARY KEY.

## Algorithm

`would_create_cycle(blocker_id, blocked_id)` runs BFS from `blocked_id` following forward edges. If BFS reaches `blocker_id`, the edge would create a cycle — reject. O(V+E) complexity.

Self-dependencies prevented by SQL CHECK constraint (`blocker_id != blocked_id`) and explicit check before BFS.

See also: [[Auto-Transitions]], [[Task Columns Mapping]]
