---
title: ACP Permission Model
tags: [acp, permissions, security, read-only]
created_at: "2026-02-18T00:00:00Z"
---

Permission handling via `RalphClient.request_permission()` in `src/acp/client_impl.rs`, implementing the ACP `Client` trait.

## Normal Mode

Auto-approves all permission requests. The agent binary (e.g., `claude`) is responsible for its own sandboxing.

## Read-Only Mode

Used by [[Verification Agent]] via `run_autonomous(read_only=true)`:
- **Rejects**: `fs/write_text_file` operations
- **Permits**: Terminal operations (agent needs to run `cargo test`, etc.)

## Write-Restricted Mode

Used during document authoring (spec/plan phases of [[Feature Lifecycle]]):
- `allowed_write_paths: Option<Vec<PathBuf>>` restricts file writes to specific paths
- Permits writes only to the target spec or plan file

## Post-ACP Notes

Ralph no longer manages its own macOS `sandbox-exec` wrapper (removed during ACP migration). The `--no-sandbox` and `--allow` CLI flags were removed.

See also: [[ACP Connection Lifecycle]], [[Verification Agent]], [[Feature Lifecycle]]
