---
title: "Testing spawn_local in unit tests requires explicit LocalSet"
tags: [acp, tokio, spawn-local, testing, async]
feature: "acp"
created_at: "2026-02-21T20:19:51.754496+00:00"
---

When writing unit tests for code that calls `tokio::task::spawn_local`, you must ensure the test runs within a `LocalSet` context. `spawn_local` panics if there is no active `LocalSet`.

Pattern that works:

```rust
#[tokio::test(flavor = "current_thread")]
async fn my_test() {
    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        // spawn_local calls here work correctly
        let handle = tokio::task::spawn_local(async { 42 });
        assert_eq!(handle.await.unwrap(), 42);
    }).await;
}
```

The `flavor = "current_thread"` is required because `spawn_local` tasks are `!Send`. With the multi-threaded runtime (the default for `#[tokio::test]`), you'd get a compile error.

A convenience macro can simplify repetitive test setup:
```rust
macro_rules! with_local_set {
    ($body:expr) => {{
        let local = LocalSet::new();
        local.run_until($body).await;
    }};
}
```
