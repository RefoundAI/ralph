---
title: Error Handling and Resilience
tags: [run-loop, error-handling, stop-reason, resilience, agent]
created_at: "2026-02-23T06:37:00Z"
---

The run loop is designed to be resilient to individual iteration failures.

## Agent Process Failures

**Agent crashes or returns no result:** Task is released via `release_claim()`, returning it to `pending`. Picked up on next iteration. No data loss.

**Verification agent crashes:** Treated as verification failure. If retries remain, task retried. Otherwise failed. See [[Verification Agent]].

## Stop Reason Mapping

Non-`EndTurn` stop reasons are handled without treating them as task failures:

| Stop Reason | Action |
|---|---|
| `EndTurn` | Normal — extract sigils, update DAG |
| `Cancelled` | `RunResult::Interrupted` (handled by interrupt flow) |
| `MaxTokens` / `MaxTurnRequests` | Release claim, journal `"blocked"`, log warning |
| `Refusal` | Fail the task, journal `"failed"` |
| Unknown variants | Release claim, journal `"blocked"`, log warning |

See [[Run Loop Lifecycle]] step 7 and [[ACP Connection Lifecycle]] for stop reason mapping.

## Mismatched Sigil IDs

If Claude emits `<task-done>` or `<task-failed>` with an ID that **doesn't match** the assigned task: warning printed to stderr, **no state transition occurs**. Task remains `in_progress` — requires manual intervention via `ralph task reset <ID>`.

## No Sigil Emitted

If Claude produces no completion sigil at all: `release_claim()` transitions task back to `pending`, clears `claimed_by`. Task becomes eligible for pickup on next iteration. This is the normal recovery path for agent timeouts or confused outputs.

## Database Errors

Propagated as `anyhow::Error` and cause the loop to exit. The current task may remain `in_progress` with stale `claimed_by`.

**Recovery:** `ralph task reset <ID>` manually returns tasks to `pending`.

## Abnormal Exit

If the loop exits abnormally (panic, kill signal, DB error), tasks may be left in `in_progress` with stale agent claims. The `claimed_by` field has no TTL — stale claims require manual cleanup.

## ACP Cost Tracking

ACP does not report API cost. Journal entries record `cost_usd = 0.0`. Duration tracked by Ralph's own `Instant::now()` timer. Cost line omitted from journal rendering when `cost_usd == 0.0`.

See also: [[Run Loop Lifecycle]], [[ACP Connection Lifecycle]], [[Interrupt Handling]], [[Sigil Parsing]]
