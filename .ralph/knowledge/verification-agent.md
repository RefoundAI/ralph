---
title: "Verification agent for task completion"
tags: [verification, verify, agent, acp, testing, quality]
created_at: "2026-02-18T00:00:00Z"
---

After each task completion, Ralph spawns a read-only verification agent (`verification.rs`) that checks the work before accepting it.

The verification agent:
1. Runs via `run_autonomous()` with `read_only=true` — the `RalphClient` rejects `fs/write_text_file` but permits terminal operations (so it can run `cargo test`)
2. Reads relevant source files
3. Runs applicable tests
4. Checks acceptance criteria from the task description
5. Emits `<verify-pass/>` or `<verify-fail>reason</verify-fail>`

On verification failure:
- The task is retried (up to `--max-retries`, default 3)
- The failure reason is included as `RetryInfo` in the next iteration's system prompt
- Retry count is tracked in `tasks.retry_count` column

If the user interrupts verification with Ctrl+C, the `RunResult::Interrupted` variant is returned and verification treats it as a failure (`passed: false`, reason: "Verification interrupted by user"). This causes the task to be retried on the next iteration.

The verification agent spawns its own ACP agent process (same `--agent` command).

Disable with `--no-verify` flag. Configure max retries with `--max-retries=N` or `[execution] max_retries` in `.ralph.toml`.
