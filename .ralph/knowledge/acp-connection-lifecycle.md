---
title: ACP Connection Lifecycle
tags: [acp, connection, tokio, localset, async, lifecycle]
created_at: "2026-02-21T20:43:41.669+00:00"
---

ACP connection pattern in `src/acp/connection.rs`. All ACP futures are `!Send`, so everything runs inside `tokio::task::LocalSet`.

## Owned Data Before LocalSet

Extract all needed values as owned data *before* entering the LocalSet. Don't pass `&Config` into the async block:

```rust
let agent_command = config.agent_command.clone();
let prompt_text = prompt::build_prompt_text(config, context);
let local = LocalSet::new();
local.run_until(run_acp_session(agent_command, prompt_text, ...)).await
```

## IO Wrapping

Tokio process stdio → futures IO requires compat wrappers:
```rust
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
let outgoing = stdin.compat_write();
let incoming = stdout.compat();
```

## Shared Client Access

`Rc<RalphClient>` for shared access — `impl<T: Client> Client for Rc<T>` exists in the ACP crate.

## Stop Reason Mapping

- `EndTurn` → normal completion
- `Cancelled` → `RunResult::Interrupted`
- `MaxTokens`/`MaxTurnRequests`/`Refusal` → return as `Completed` with stop_reason; run loop handles per FR-6.6

## Interrupt Detection

`tokio::select!` races agent session against `poll_interrupt()` task. On interrupt, agent process is killed and cleaned up. See [[Interrupt Handling]].

## Permission Model

`RalphClient.request_permission()` auto-approves in normal mode. In `read_only=true` mode (used by [[Verification Agent]]): rejects `fs/write_text_file`, permits terminal operations. See [[ACP Permission Model]].

See also: [[Tokio LocalSet Testing]], [[ACP Trait Imports]], [[ACP Schema Types Import Path]], [[ACP Permission Model]], [[Interrupt Handling]]
