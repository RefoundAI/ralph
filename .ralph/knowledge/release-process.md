---
title: Release Process
tags: [release, cargo-dist, ci, git, tags]
created_at: "2026-02-18T00:00:00Z"
---

Ralph uses cargo-dist v0.30.3 for releases. Config in `dist-workspace.toml` (not Cargo.toml).

## Steps

1. Bump version in `Cargo.toml`
2. Commit the version bump
3. Create **annotated** tag: `git tag -a vX.Y.Z -m "vX.Y.Z"`
4. Push tag: `git push origin vX.Y.Z`
5. CI builds tarballs, installer, checksums, source archive

## Gotcha

Bare `git tag vX.Y.Z` (lightweight) fails — repo requires tag messages. Always use `-a`.

## Targets

`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`

## Pre-Release Quality Gates

Run these before tagging:

```bash
cargo fmt --check
cargo test -q
cargo test -q --features test-mock-agents --test acp_integration
cargo run -q -- --help   # verify --no-ui is documented and visible
```

Reuse CI smoke scripts for behavioral sanity:

- `tests/smoke/tty_smoke.sh` — TTY scenarios
- `tests/smoke/non_tty_smoke.sh` — non-TTY fallback

## Gitignore Hardening

`.gitignore` must cover SQLite sidecar artifacts:

- `.ralph/progress.db` (already covered)
- `.ralph/*.db-wal` and `.ralph/*.db-shm` (WAL/SHM sidecars)

Verify no transient smoke artifacts are tracked before release commits.

## Release Notes Template

Include these sections for any release with user-visible changes:

1. Default TUI in TTY and fallback behavior
2. Interactive modals and explorer views coverage
3. `--no-ui` and `RALPH_UI` controls
4. Compatibility notes (scripting and non-TTY behavior)
5. ID format changes (e.g. `t-`/`f-` 8 hex suffix) with backward compatibility statement

Report artifact: `docs/release-readiness-ui.md` (quality gate commands and results).

## Local Testing

```bash
dist plan    # Preview build plan
dist build   # Build for current platform
dist generate # Regenerate CI workflow
```

See also: [[CI Smoke Testing]]
