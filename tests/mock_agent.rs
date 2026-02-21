//! Mock ACP agent binary for integration testing.
//!
//! This is the server side of the ACP protocol — it implements the `Agent` trait
//! and communicates with Ralph (the client/tool-provider) over stdin/stdout.
//!
//! Behaviour:
//! - `initialize`: echoes back the protocol version, announces itself as "mock-agent"
//! - `session/new`: returns a fixed session ID ("mock-session-1")
//! - `prompt`: emits an `AgentMessageChunk` with text from the `MOCK_RESPONSE`
//!   environment variable (default: "Mock response"), then returns `EndTurn`.
//! - Does NOT request any tools — pure text response.
//!
//! Build: `cargo build --features test-mock-agents`

use std::cell::RefCell;
use std::rc::Rc;

use agent_client_protocol::{
    Agent, AgentSideConnection, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    Client, ContentBlock, ContentChunk, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use async_trait::async_trait;
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Error type alias matching agent_client_protocol's Result.
use agent_client_protocol::Error as AcpError;
type Result<T> = std::result::Result<T, AcpError>;

// ============================================================================
// Mock agent implementation
// ============================================================================

struct MockAgent {
    /// Shared slot for the AgentSideConnection.
    ///
    /// The connection is populated in `main()` after `AgentSideConnection::new()`
    /// returns it. The agent uses it inside `prompt()` to send session notifications
    /// back to Ralph. Rc<RefCell<>> is safe here because all ACP futures are !Send
    /// and everything runs on a single-threaded LocalSet.
    conn: Rc<RefCell<Option<AgentSideConnection>>>,
}

#[async_trait(?Send)]
impl Agent for MockAgent {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version)
            .agent_info(Implementation::new("mock-agent", "0.1.0")))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::default())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new(SessionId::new("mock-session-1")))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        let raw_response =
            std::env::var("MOCK_RESPONSE").unwrap_or_else(|_| "Mock response".to_string());

        // Special sentinel: when MOCK_RESPONSE == "ECHO_RALPH_MODEL", the agent echoes
        // the value of the RALPH_MODEL env var back to the client.  This is used by the
        // test_iteration_model_env_passed integration test to verify that Ralph correctly
        // sets RALPH_MODEL on the spawned agent process.
        let response_text = if raw_response == "ECHO_RALPH_MODEL" {
            std::env::var("RALPH_MODEL").unwrap_or_else(|_| "RALPH_MODEL_NOT_SET".to_string())
        } else {
            raw_response
        };

        // Send an AgentMessageChunk notification back to Ralph.
        // The borrow is held across the .await but this is safe in ?Send / single-threaded context:
        // - We never mutably borrow conn_slot after main() populates it.
        // - All ACP futures run on the same LocalSet thread; no concurrent borrow conflicts.
        let borrow = self.conn.borrow();
        if let Some(conn) = borrow.as_ref() {
            conn.session_notification(SessionNotification::new(
                args.session_id,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(response_text),
                ))),
            ))
            .await?;
        }
        drop(borrow);

        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        Ok(())
    }
}

// ============================================================================
// Entry point
// ============================================================================

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Shared slot: populated after AgentSideConnection::new() so the
            // Agent impl can call Client methods on it during prompt().
            let conn_slot: Rc<RefCell<Option<AgentSideConnection>>> = Rc::new(RefCell::new(None));

            let agent = MockAgent {
                conn: conn_slot.clone(),
            };

            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();

            // Create the server-side ACP connection.
            // - `agent` handles incoming requests (initialize, prompt, …)
            // - `outgoing` = stdout (we write JSON-RPC responses / notifications here)
            // - `incoming` = stdin  (Ralph writes JSON-RPC requests here)
            // - The spawn closure runs I/O subtasks as local tasks on this LocalSet.
            let (conn, io_task) =
                AgentSideConnection::new(agent, stdout.compat_write(), stdin.compat(), |fut| {
                    tokio::task::spawn_local(fut);
                });

            // Populate the shared slot so prompt() can use the connection.
            *conn_slot.borrow_mut() = Some(conn);

            // Drive the JSON-RPC transport loop.
            // This future completes when the client closes the connection.
            let _ = io_task.await;
        })
        .await;
}
