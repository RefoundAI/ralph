---
title: "Mock ACP agent binary pattern with AgentSideConnection"
tags: [acp, mock-agent, testing, agentsideconnection, cargo]
feature: "acp"
created_at: "2026-02-21T21:45:58.592062+00:00"
---

When building an ACP agent binary (server side), use `AgentSideConnection` from `agent_client_protocol`:

```rust
let (conn, io_task) = AgentSideConnection::new(
    agent_impl,
    stdout.compat_write(),  // tokio_util::compat
    stdin.compat(),
    |fut| { tokio::task::spawn_local(fut); },
);
*conn_slot.borrow_mut() = Some(conn);
let _ = io_task.await;  // drives the JSON-RPC loop
```

**Sharing the connection with the Agent impl**:
The `Agent::prompt()` method has `&self` but needs to call `conn.session_notification()`. Pattern:

```rust
struct MyAgent {
    conn: Rc<RefCell<Option<AgentSideConnection>>>,
}
// In main(): create agent → new connection → populate slot
// In prompt(): self.conn.borrow() to get conn, call methods
```

Holding `RefCell::borrow()` across `.await` is safe in `?Send` single-threaded async (LocalSet). The borrow is never mutably taken after initialization.

**Result type**: Use `type Result<T> = std::result::Result<T, agent_client_protocol::Error>` to match the Agent trait signature.

**Cargo layout**: Use `[[bin]]` entries with `required-features = ["test-mock-agents"]` and `autotests = false` in `[package]` to prevent auto-discovery of `tests/*.rs` files as integration tests when they contain `fn main()`.

**Protocol version**: Agent impl should echo `args.protocol_version` back in `InitializeResponse::new()`.
