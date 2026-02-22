---
title: "ACP schema types import path"
tags: [acp, agent-client-protocol, imports, schema]
feature: "acp"
created_at: "2026-02-21T20:28:48.669432+00:00"
---

The `agent-client-protocol` crate re-exports ALL `agent_client_protocol_schema` types at the top level via `pub use agent_client_protocol_schema::*`. This means you can (and must) import schema types directly:

```rust
// CORRECT
use agent_client_protocol::{
    ContentBlock, SessionUpdate, PermissionOptionKind,
    RequestPermissionOutcome, SelectedPermissionOutcome, ToolKind,
};

// WRONG — will fail to compile
use agent_client_protocol::agent_client_protocol_schema::{
    ContentBlock, ...
};
```

Same applies in tests: `use agent_client_protocol::{PermissionOption, SessionId, ...}`.

The `TerminalId`, `SessionId`, `ToolCallId`, `PermissionOptionId` are all newtype wrappers over `Arc<str>`. Access the inner string via `.0.as_ref()`.
