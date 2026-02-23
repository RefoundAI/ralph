---
title: ACP Schema Types Import Path
tags: [acp, agent-client-protocol, imports, schema]
created_at: "2026-02-21T20:28:48.669432+00:00"
---

The `agent-client-protocol` crate re-exports all schema types at top level via `pub use agent_client_protocol_schema::*`.

## Correct Import

```rust
use agent_client_protocol::{
    ContentBlock, SessionUpdate, PermissionOptionKind,
    RequestPermissionOutcome, SelectedPermissionOutcome, ToolKind,
};
```

Do NOT try `use agent_client_protocol::agent_client_protocol_schema::...` — won't compile.

## Newtype Wrappers

`TerminalId`, `SessionId`, `ToolCallId`, `PermissionOptionId` are newtypes over `Arc<str>`. Access inner string via `.0.as_ref()`.

See also: [[ACP Trait Imports]], [[ACP Connection Lifecycle]]
