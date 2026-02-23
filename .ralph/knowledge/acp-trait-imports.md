---
title: ACP Trait Imports
tags: [acp, agent-client-protocol, imports, trait]
created_at: "2026-02-21T20:55:48.336+00:00"
---

The `Agent` trait must be in scope to call `ClientSideConnection` methods (`initialize`, `new_session`, `prompt`, `cancel`):

```rust
use agent_client_protocol::{Agent, ClientSideConnection, ...};
```

Without this import: E0599 "no method named `initialize` found for struct `ClientSideConnection`".

Every module calling these methods needs the import. See `src/acp/connection.rs` and `src/acp/interactive.rs` for examples.

See also: [[ACP Schema Types Import Path]], [[ACP Connection Lifecycle]]
