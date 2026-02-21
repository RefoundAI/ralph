---
title: "Graceful Ctrl+C interrupt handling"
tags: [interrupt, signal, ctrl-c, run-loop, feedback, sigint]
created_at: "2026-02-21T00:00:00Z"
---

Ralph supports graceful Ctrl+C interrupts during the run loop via `src/interrupt.rs`.

**Signal registration**: `register_signal_handler()` uses `signal-hook` to set an `AtomicBool` on SIGINT. A second Ctrl+C calls `std::process::exit(130)` for hard exit. Called once at `run_loop::run()` start.

**Detection**: `stream_output()` in `claude/client.rs` checks `is_interrupted()` before each streamed line. Returns `StreamResult::Interrupted` which propagates to `RunResult::Interrupted`. Both direct and sandboxed execution paths handle this — killing the child process and cleaning up.

**Run loop interrupt flow** (in `run_loop.rs`):
1. Print interrupted banner with iteration, task ID, and title
2. Prompt user for multi-line feedback (empty line finishes, Enter skips)
3. If feedback given: append as `**User Guidance (iteration N):**` section to task description + add task log entry
4. Release task claim via `release_claim()` (resets to pending)
5. Write journal entry with outcome `"interrupted"` and user feedback as notes
6. Clear interrupt flag for clean next iteration
7. Ask "Continue? [Y/n]" — Y continues the loop, n returns `Outcome::Interrupted`

**Subsystem behavior on interrupt**:
- **Verification** (`verification.rs`): Returns `passed: false` — task will be retried
- **Review** (`review.rs`): Returns `passed: true` — avoids infinite review loops
- **Main exit** (`main.rs`): `Outcome::Interrupted` prints message and exits with success code

**Key functions**: `register_signal_handler()`, `is_interrupted()`, `clear_interrupt()`, `prompt_for_feedback(task)`, `append_feedback_to_description(desc, feedback, iteration)`, `should_continue()`.
