---
title: "Release process and cargo-dist"
tags: [release, cargo-dist, git, tags, ci]
created_at: "2026-02-18T00:00:00Z"
---

Releases use cargo-dist. Config lives in `dist-workspace.toml` (not Cargo.toml).

Steps to cut a release:
1. Bump version in `Cargo.toml`
2. Commit the version bump
3. Create an **annotated** tag: `git tag -a vX.Y.Z -m "vX.Y.Z"` — bare `git tag vX.Y.Z` fails because the repo requires tag messages
4. Push the tag: `git push origin vX.Y.Z`
5. CI (`.github/workflows/release.yml`) builds platform tarballs, installer script, checksums, and source archive

Target platforms: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-apple-darwin, aarch64-apple-darwin.

Local commands:
- `dist plan` — Preview what will be built
- `dist build` — Build for current platform
- `dist generate` — Regenerate CI workflow after config changes
