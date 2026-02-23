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

## Local Testing

```bash
dist plan    # Preview build plan
dist build   # Build for current platform
dist generate # Regenerate CI workflow
```
