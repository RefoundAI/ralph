---
title: Integration Test Binary Paths
tags: [integration-tests, cargo, binary-path, testing]
created_at: "2026-02-21T22:12:26.175046+00:00"
---

`CARGO_BIN_EXE_<name>` is NOT set for `[[bin]]` targets with `required-features`. Use `current_exe()` instead:

```rust
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("current_exe");
    // test binary: target/debug/deps/<name>
    // step up two levels: target/debug/
    exe.parent().and_then(|d| d.parent())
        .map(|d| d.to_path_buf()).expect("target dir")
}

fn mock_agent_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_mock_agent") {
        return PathBuf::from(p);
    }
    target_dir().join("mock-agent")
}
```

See also: [[Mock ACP Agent Binary]]
