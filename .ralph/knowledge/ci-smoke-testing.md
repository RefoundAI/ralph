---
title: CI Smoke Testing
tags: [ci, testing, smoke, tty, fallback, expect, mock-agent]
created_at: "2026-02-26T00:00:00Z"
---

Ralph has CI smoke testing in `.github/workflows/ci-smoke.yml` that catches TUI regressions (TTY flows) and fallback regressions (non-TTY flows).

## Workflow

- **Triggers:** pull requests and pushes to main
- **Runner:** `ubuntu-latest`
- **Setup:** Rust toolchain + `expect`
- **Build:** `cargo build --features test-mock-agents --examples` (builds mock agent binaries)

## Test Matrix

1. `cargo test -q` — unit tests
2. TTY smoke (`tests/smoke/tty_smoke.sh`) — PTY-based validation using `expect` scripts
3. Non-TTY smoke (`tests/smoke/non_tty_smoke.sh`) — piped output assertions

## TTY Smoke

Uses `expect` scripts for modal interactions:

- `tests/smoke/tty_smoke.sh` — orchestrates PTY scenarios
- Validates: `run`, explorer commands (`feature list`, `task list/show/tree`, `task deps list`), modal interactions (`task create`, `feature create` early-exit)

## Non-TTY Smoke

Asserts fallback behavior when output is piped (no PTY):

- No alternate-screen escape sequence (`\x1b[?1049h`) in output
- Expected summary/output lines appear
- Commands return expected exit codes

## Pitfall: Automated PTY Interrupt Testing

Automated PTY signal injection (Ctrl+C via `expect`) is unreliable for interrupt modal flows. Signal delivery races with modal rendering — results vary between exit codes `0`, `2`, `130`, and timeouts depending on timing. The interrupt flow has been validated via manual human-driven retesting; automated interrupt expect scripts (`tests/smoke/interrupt_*.expect`) exist as drafts but should not be used for CI gating until stabilized.

## Adding New Smoke Scenarios

1. Add `expect` scripts in `tests/smoke/` for TTY modal interactions
2. Add piped-output assertions in `tests/smoke/non_tty_smoke.sh` for non-TTY
3. Smoke logs are uploaded as CI artifacts on failure
4. Smoke artifacts directory (`tests/smoke/artifacts/`) is gitignored

See also: [[Mock ACP Agent Binary]], [[Ratatui UI Runtime]], [[UI Event Routing and Plain Fallback]], [[Release Process]]
