---
title: "macOS sandbox restrictions and allow rules"
tags: [sandbox, macos, security, filesystem, sandbox-exec]
created_at: "2026-02-18T00:00:00Z"
---

On macOS, Ralph wraps Claude in `sandbox-exec` to restrict filesystem writes. Implementation is in `src/sandbox/`.

**Allowed writes** (default):
- Project directory (and git worktree root)
- Temp dirs (`$TMPDIR`, `/tmp`, `/private/tmp`)
- Claude state (`~/.claude`, `~/.config/claude`)
- Cache/state dirs (`~/.cache`, `~/.local/state`)

**Blocked**:
- All other filesystem writes
- `com.apple.systemevents` (prevents UI automation)

**Allow rules** (`--allow=<rule>`):
- Rules are defined in `sandbox/rules.rs`
- Example: `--allow=aws` grants write access to `~/.aws`
- Rules map to additional filesystem paths added to the sandbox profile

**Profile generation** (`sandbox/profile.rs`):
- Dynamically generates a `.sb` profile file
- Written to a temp file, passed to `sandbox-exec -f`

Disable with `--no-sandbox`. The sandbox is macOS-only — on other platforms it's a no-op.
