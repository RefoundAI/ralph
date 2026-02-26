---
title: Interrupt Handling
tags: [interrupt, signal, ctrl-c, run-loop, sigint]
created_at: "2026-02-21T00:00:00Z"
---

Graceful Ctrl+C support in `src/interrupt.rs`, integrated with [[Run Loop Lifecycle]] and [[ACP Connection Lifecycle]].

## Signal Registration

`register_signal_handler()` uses `signal-hook` to set `AtomicBool` on first SIGINT. Second Ctrl+C calls `exit(130)` for hard exit. Registered once at `run_loop::run()` start.

## Detection

ACP connection uses `tokio::select!` to race agent session against `poll_interrupt()`. Returns `RunResult::Interrupted`. Agent process killed and cleaned up.

## Interrupt Flow

1. Print interrupted banner (iteration, task ID, title)
2. Prompt for multi-line user feedback (empty line finishes)
3. If feedback: append as `**User Guidance (iteration N):**` to task description + task log
4. Release claim via `release_claim()` (resets to pending)
5. Write journal entry with outcome `"interrupted"` + feedback as notes
6. Clear interrupt flag
7. Ask "Continue? [Y/n]" — Y continues, n returns `Outcome::Interrupted`

When UI is active, steps 2 and 7 use TUI modals (multiline + confirm) instead of blocking stdin prompts.

## Subsystem Behavior

- **[[Verification Agent]]**: Interrupt → `passed: false` (task retried)
- **Review** (`src/review.rs`): Interrupt → `passed: true` (avoids infinite loops)
- **Main exit**: `Outcome::Interrupted` prints message, exits with success code

See also: [[Run Loop Lifecycle]], [[ACP Connection Lifecycle]], [[Verification Agent]], [[UI Interactive Modals and Explorer Views]]
