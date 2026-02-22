---
title: "Locating required-features binaries in integration tests"
tags: [integration-tests, acp, cargo, binary-path, testing]
feature: "acp"
created_at: "2026-02-21T22:12:26.175046+00:00"
---

When a Cargo `[[bin]]` target uses `required-features`, the `CARGO_BIN_EXE_<name>` environment variable is NOT set at test runtime. Use `current_exe()` to locate sibling binaries instead:

```rust
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("could not read current_exe");
    // Integration test binary lives at: target/debug/deps/<test-name>
    // Step up two levels to reach:       target/debug/
    exe.parent()
        .and_then(|deps| deps.parent())
        .map(|d| d.to_path_buf())
        .expect("could not navigate to target directory")
}

fn mock_agent_path() -> PathBuf {
    // Try CARGO_BIN_EXE_* first (works for non-required-features bins).
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_mock_agent") {
        return PathBuf::from(p);
    }
    target_dir().join("mock-agent")
}
```

Also: use `autotests = false` in `[package]` of `Cargo.toml` to prevent Cargo from auto-discovering `tests/*.rs` files as integration tests when they contain `fn main()` (i.e., the mock agent binaries).
