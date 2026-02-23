---
title: Mock ACP Agent Binary
tags: [acp, mock-agent, testing, agentsideconnection, cargo]
created_at: "2026-02-21T21:45:58.592062+00:00"
---

Mock agent binaries for integration testing use `AgentSideConnection` from the ACP crate.

## Agent-Side Pattern

```rust
let (conn, io_task) = AgentSideConnection::new(
    agent_impl,
    stdout.compat_write(),
    stdin.compat(),
    |fut| { tokio::task::spawn_local(fut); },
);
*conn_slot.borrow_mut() = Some(conn);
let _ = io_task.await;  // drives JSON-RPC loop
```

## Sharing Connection with Agent Impl

`Agent::prompt()` has `&self` but needs `conn.session_notification()`. Pattern:

```rust
struct MyAgent {
    conn: Rc<RefCell<Option<AgentSideConnection>>>,
}
```

`RefCell::borrow()` across `.await` is safe in `!Send` single-threaded async (LocalSet). See [[Tokio LocalSet Testing]].

## Cargo Layout

- `[[bin]]` entries with `required-features = ["test-mock-agents"]`
- `autotests = false` in `[package]` prevents auto-discovery of `tests/*.rs` files containing `fn main()`
- Echo `args.protocol_version` back in `InitializeResponse::new()`
- Binary paths found via [[Integration Test Binary Paths]]

See also: [[Tokio LocalSet Testing]], [[ACP Connection Lifecycle]], [[Integration Test Binary Paths]]
