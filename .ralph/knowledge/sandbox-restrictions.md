---
title: "Sandbox and permission handling (post-ACP)"
tags: [sandbox, permissions, acp, security]
created_at: "2026-02-18T00:00:00Z"
---

After the ACP migration, Ralph no longer manages its own macOS `sandbox-exec` wrapper. The `src/sandbox/` module has been removed.

Permission handling is now done through the ACP protocol:
- The `RalphClient` in `src/acp/client_impl.rs` implements `request_permission()` from the ACP `Client` trait
- In normal mode: auto-approves all permission requests
- In read-only mode (`run_autonomous(read_only=true)`): rejects `fs/write_text_file` operations but permits terminal operations

The ACP agent binary itself (e.g. `claude`) is responsible for its own sandboxing. Ralph's `--no-sandbox` and `--allow` CLI flags were removed during the ACP migration.
