---
title: "ACP connection lifecycle pattern with LocalSet and owned data"
tags: [acp, connection, tokio, localset, async, lifetime]
feature: "acp"
created_at: "2026-02-21T20:43:41.669+00:00"
---

When implementing an ACP connection in a LocalSet, avoid passing `&Config` or other borrowed references into the async block. Extract all needed data as owned values first:

```rust
pub async fn run_iteration(config: &Config, context: &IterationContext) -> Result<RunResult> {
    // Extract owned data BEFORE the LocalSet
    let agent_command = config.agent_command.clone();
    let project_root = config.project_root.clone();
    let prompt_text = prompt::build_prompt_text(config, context); // call here!

    let local = LocalSet::new();
    local.run_until(run_acp_session(agent_command, project_root, prompt_text, ...)).await
}
```

Key IO wrapping: tokio::process stdio → futures IO requires compat:
```rust
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
let outgoing = stdin.compat_write();  // we write to agent's stdin
let incoming = stdout.compat();       // we read from agent's stdout
let (conn, io_future) = ClientSideConnection::new(client_ref, outgoing, incoming, |fut| {
    tokio::task::spawn_local(fut);
});
```

Use `Rc<RalphClient>` to keep access to client state after passing it to the connection. Since `impl<T: Client> Client for Rc<T>` exists in the ACP crate, `Rc<RalphClient>` implements both `Client` and `MessageHandler<ClientSide>`.

Stop reason mapping:
- EndTurn → normal completion
- Cancelled → Interrupted
- MaxTokens/MaxTurnRequests/Refusal/unknown → return as Completed with the stop_reason; let run_loop handle per FR-6.6
