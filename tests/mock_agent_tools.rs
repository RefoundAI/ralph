//! Mock ACP agent binary for integration testing — tool-requesting variant.
//!
//! This is the server side of the ACP protocol — it implements the `Agent` trait
//! and communicates with Ralph (the client/tool-provider) over stdin/stdout.
//!
//! In addition to plain text responses, this agent exercises Ralph's tool provider
//! during the `prompt` handler by issuing the following tool calls:
//!
//! 1. `fs/read_text_file` — reads the file at the path in `MOCK_READ_PATH` env var.
//! 2. `fs/write_text_file` — writes `MOCK_WRITE_CONTENT` to `MOCK_WRITE_PATH`.
//! 3. `terminal/create_terminal` — spawns `echo hello` on the client.
//!
//! After all tool calls complete, emits an `AgentMessageChunk` with the text from
//! `MOCK_RESPONSE` (default: "Mock response with tools") and returns `EndTurn`.
//!
//! Build: `cargo build --features test-mock-agents`

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use agent_client_protocol::{
    Agent, AgentSideConnection, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    Client, ContentBlock, ContentChunk, CreateTerminalRequest, Implementation, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    ReadTextFileRequest, SessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};
use async_trait::async_trait;
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use agent_client_protocol::Error as AcpError;
type Result<T> = std::result::Result<T, AcpError>;

// ============================================================================
// Mock agent implementation
// ============================================================================

struct MockAgentTools {
    /// Shared slot for the AgentSideConnection.
    /// See mock_agent.rs for the design rationale.
    conn: Rc<RefCell<Option<AgentSideConnection>>>,
}

#[async_trait(?Send)]
impl Agent for MockAgentTools {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        Ok(InitializeResponse::new(args.protocol_version)
            .agent_info(Implementation::new("mock-agent-tools", "0.1.0")))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        Ok(AuthenticateResponse::default())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        Ok(NewSessionResponse::new(SessionId::new(
            "mock-session-tools-1",
        )))
    }

    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        let session_id = args.session_id.clone();

        // All tool calls share the same borrow.  We hold it across multiple
        // awaits — safe in single-threaded LocalSet because we never mutably
        // borrow the slot after main() populates it.
        let borrow = self.conn.borrow();
        let conn = borrow
            .as_ref()
            .expect("AgentSideConnection not yet initialised");

        // ------------------------------------------------------------------ //
        // 1. Read a file (fs/read_text_file)                                  //
        // ------------------------------------------------------------------ //
        if let Ok(read_path) = std::env::var("MOCK_READ_PATH") {
            // Ignore errors — the test controls whether the file exists.
            let _ = conn
                .read_text_file(ReadTextFileRequest::new(
                    session_id.clone(),
                    PathBuf::from(read_path),
                ))
                .await;
        }

        // ------------------------------------------------------------------ //
        // 2. Write a file (fs/write_text_file)                                //
        // ------------------------------------------------------------------ //
        if let (Ok(write_path), Ok(write_content)) = (
            std::env::var("MOCK_WRITE_PATH"),
            std::env::var("MOCK_WRITE_CONTENT"),
        ) {
            let _ = conn
                .write_text_file(WriteTextFileRequest::new(
                    session_id.clone(),
                    PathBuf::from(write_path),
                    write_content,
                ))
                .await;
        }

        // ------------------------------------------------------------------ //
        // 3. Spawn a terminal (terminal/create_terminal) and wait for exit   //
        // ------------------------------------------------------------------ //
        // Pass the full shell command as the `command` field; Ralph's handler
        // wraps it with `sh -c`.
        let terminal_result = conn
            .create_terminal(CreateTerminalRequest::new(session_id.clone(), "echo hello"))
            .await;

        // Wait for the terminal to exit — this exercises Ralph's wait_for_terminal_exit
        // tool handler (used by test_terminal_wait_for_exit integration test).
        if let Ok(terminal_resp) = terminal_result {
            let _ = conn
                .wait_for_terminal_exit(WaitForTerminalExitRequest::new(
                    session_id.clone(),
                    terminal_resp.terminal_id,
                ))
                .await;
        }

        // ------------------------------------------------------------------ //
        // 4. Send the final text chunk back to Ralph                          //
        // ------------------------------------------------------------------ //
        let response_text = std::env::var("MOCK_RESPONSE")
            .unwrap_or_else(|_| "Mock response with tools".to_string());

        conn.session_notification(SessionNotification::new(
            session_id,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(response_text),
            ))),
        ))
        .await?;

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
            let conn_slot: Rc<RefCell<Option<AgentSideConnection>>> = Rc::new(RefCell::new(None));

            let agent = MockAgentTools {
                conn: conn_slot.clone(),
            };

            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();

            let (conn, io_task) =
                AgentSideConnection::new(agent, stdout.compat_write(), stdin.compat(), |fut| {
                    tokio::task::spawn_local(fut);
                });

            *conn_slot.borrow_mut() = Some(conn);

            let _ = io_task.await;
        })
        .await;
}
