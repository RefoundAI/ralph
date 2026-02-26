---
title: Task/Feature ID Entropy Expansion
tags: [dag, ids, collisions, testing, reliability]
created_at: "2026-02-24T08:03:34Z"
---

Task/feature ID suffix length was expanded from 6 hex chars to 8 hex chars in `src/dag/ids.rs`.

## Why

The `test_no_duplicates_in_1000_generates` test was intermittently failing with 6 hex chars (`16^6` space) due to collision probability at 1000 draws.

## Change

- `generate_task_id()`: `t-xxxxxxxx`
- `generate_feature_id()`: `f-xxxxxxxx`
- tests updated to assert 8-hex suffix format.

## Compatibility

IDs are treated as opaque strings across the system. Existing shorter IDs remain valid; new IDs are longer and lower-collision.

See also: [[Task CRUD Operations]], [[Run Loop Lifecycle]], [[Error Handling and Resilience]]
