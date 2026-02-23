---
title: Verification Agent
tags: [verification, agent, acp, testing, quality]
created_at: "2026-02-18T00:00:00Z"
---

Read-only verification agent in `src/verification.rs`, spawned after each task completion.

## How It Works

1. Runs via `run_autonomous(read_only=true)` — see [[ACP Permission Model]]
2. Reads relevant source files
3. Runs tests via terminal (terminal ops permitted, file writes rejected)
4. Checks acceptance criteria from task description
5. Emits `<verify-pass/>` or `<verify-fail>reason</verify-fail>` — see [[Sigil Parsing]]

## On Failure

Task retried up to `max_retries` (default 3, configurable via `--max-retries` or `[execution] max_retries`). Failure reason included as `RetryInfo` in next iteration's [[System Prompt Construction]]. Retry count tracked in `tasks.retry_count` column.

## On Interrupt

Returns `passed: false` (reason: "Verification interrupted by user") → task retried next iteration. See [[Interrupt Handling]].

## No Sigil Found

Treated as verification failure.

## Disabling

`--no-verify` flag or `[execution] verify = false` in `.ralph.toml`.

See also: [[ACP Permission Model]], [[Sigil Parsing]], [[Run Loop Lifecycle]], [[Interrupt Handling]], [[System Prompt Construction]]
