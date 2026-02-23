---
title: Tokio LocalSet Testing
tags: [acp, tokio, spawn-local, testing, async, localset]
created_at: "2026-02-21T20:19:51.754496+00:00"
---

Code calling `tokio::task::spawn_local` must run within a `LocalSet` context. Without it, `spawn_local` panics at runtime.

## Test Pattern

```rust
#[tokio::test(flavor = "current_thread")]
async fn my_test() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        let handle = tokio::task::spawn_local(async { 42 });
        assert_eq!(handle.await.unwrap(), 42);
    }).await;
}
```

`flavor = "current_thread"` is required — `spawn_local` tasks are `!Send`, so multi-threaded runtime won't compile.

## Why This Matters

All ACP connection code runs inside `LocalSet` (see [[ACP Connection Lifecycle]]). Integration tests and unit tests touching ACP code need this pattern. Mock agent binaries also use LocalSet — see [[Mock ACP Agent Binary]].

See also: [[ACP Connection Lifecycle]], [[Mock ACP Agent Binary]]
