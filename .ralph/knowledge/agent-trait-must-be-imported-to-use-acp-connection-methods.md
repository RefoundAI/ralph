---
title: "Agent trait must be imported to use ACP connection methods"
tags: [acp, agent-client-protocol, imports, trait, clientsideconnection]
feature: "acp"
created_at: "2026-02-21T20:55:48.336+00:00"
---

When using `ClientSideConnection` from the `agent-client-protocol` crate, you MUST import the `Agent` trait to call its methods (`initialize`, `new_session`, `prompt`, `cancel`):

```rust
use agent_client_protocol::{Agent, ClientSideConnection, ...};
```

Without this import, you get E0599 errors like:
```
no method named `initialize` found for struct `ClientSideConnection`
help: trait `Agent` which provides `initialize` is implemented but not in scope
```

The same applies in any module that calls these methods — each module using `ClientSideConnection` method calls needs `use agent_client_protocol::Agent;` in scope. This is standard Rust trait method scoping behavior.

See: `src/acp/connection.rs` (imports `Agent`) and `src/acp/interactive.rs` (also imports `Agent`).
