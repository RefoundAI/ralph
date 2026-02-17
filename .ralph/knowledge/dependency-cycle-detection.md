---
title: "Dependency cycle detection via BFS"
tags: [dependencies, cycle-detection, bfs, dag]
created_at: "2026-02-18T00:00:00Z"
---

Dependencies in `dag/dependencies.rs` use BFS to detect cycles before insertion.

Edge semantics: `blocker_id` must complete before `blocked_id` can start. The `dependencies` table has a composite PRIMARY KEY (blocker_id, blocked_id) and CHECK (blocker_id != blocked_id).

Cycle detection algorithm (`would_create_cycle()`):
1. Start BFS from `blocked_id` (the task that would be waiting)
2. Traverse forward: for each node, find all tasks it blocks (SELECT blocked_id WHERE blocker_id = current)
3. If `blocker_id` is reached: adding this edge would create a cycle → reject
4. Track visited set to avoid infinite loops in existing DAG

Self-dependencies (blocker_id == blocked_id) are checked explicitly before BFS and also enforced by the SQL CHECK constraint.

The BFS is O(V+E) — fine for typical DAGs but could be slow on very large graphs. Foreign key constraints ensure both task IDs exist before insertion.
