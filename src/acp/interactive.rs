//! Interactive and streaming ACP sessions.
//!
//! - [`run_interactive`]: ACP-mediated interactive session (user ↔ agent loop).
//! - [`run_streaming`]: Single autonomous prompt, stream output. Used by `feature build`.
//!
//! Both functions create a fresh [`tokio::task::LocalSet`] for the ACP connection
//! lifecycle (ACP futures are `!Send`). The shared connection setup is factored into
//! [`run_interactive_inner`], which mirrors [`super::connection::run_acp_session`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use agent_client_protocol::{
    Agent, AuthenticateRequest, CancelNotification, ClientCapabilities, ClientSideConnection,
    ContentBlock, FileSystemCapability, Implementation, InitializeRequest, NewSessionRequest,
    PromptRequest, ProtocolVersion, TextContent,
};
use anyhow::{anyhow, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::acp::client_impl::RalphClient;
use crate::acp::connection;
use crate::interrupt;

// ============================================================================
// Public API
// ============================================================================

/// Run an interactive ACP session (user types, agent responds, repeat).
///
/// The `instructions` are prepended to `initial_message` in the first prompt.
/// Subsequent user inputs are sent as standalone prompts.
/// The session ends when the user sends an empty line or EOF (Ctrl+D).
///
/// If `model` is provided, sets `RALPH_MODEL` env var on the spawned agent process.
/// If `allow_terminal` is false, terminal (bash) capability is disabled — use this
/// for document-authoring sessions (spec, plan) where the agent should only read/write files.
/// If `allowed_write_paths` is set, file writes are restricted to those paths only.
/// ACP has no equivalent to `plan_mode` from the old Claude CLI integration — permission
/// management is now handled by Ralph's tool provider (auto-approve in normal mode).
pub async fn run_interactive(
    agent_command: &str,
    instructions: &str,
    initial_message: &str,
    project_root: &Path,
    model: Option<&str>,
    allow_terminal: bool,
    allowed_write_paths: Option<Vec<PathBuf>>,
) -> Result<()> {
    // Extract owned values before entering the LocalSet to avoid lifetime issues.
    let agent_command = agent_command.to_owned();
    let project_root = project_root.to_path_buf();
    let instructions = instructions.to_owned();
    let initial_message = initial_message.to_owned();
    let model = model.map(|s| s.to_owned());

    let local = LocalSet::new();
    local
        .run_until(run_interactive_inner(
            agent_command,
            project_root,
            instructions,
            initial_message,
            model,
            allow_terminal,
            allowed_write_paths,
        ))
        .await
}

/// Run a non-interactive streaming session (single prompt, agent runs autonomously).
///
/// Concatenates `instructions` and `message` into a single prompt.
/// Used by `feature build` to let the agent autonomously create a task DAG via
/// `ralph task add` and `ralph task deps add` CLI commands.
///
/// If `model` is provided, sets `RALPH_MODEL` env var on the spawned agent process.
pub async fn run_streaming(
    agent_command: &str,
    instructions: &str,
    message: &str,
    project_root: &Path,
    model: Option<&str>,
) -> Result<()> {
    // Delegate to run_autonomous — same lifecycle, different return type.
    connection::run_autonomous(
        agent_command,
        project_root,
        instructions,
        message,
        false,
        model,
    )
    .await
    .map(|_| ())
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Poll the interrupt flag every 100 ms.
///
/// Returns as soon as `interrupt::is_interrupted()` becomes true.
async fn poll_interrupt() {
    loop {
        if interrupt::is_interrupted() {
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

/// Kill all resources for an interactive session.
///
/// Kills terminal subprocesses, aborts background I/O tasks, and kills the
/// agent process. Mirrors the `cleanup()` helper in `connection.rs`.
async fn interactive_cleanup(
    _conn: ClientSideConnection,
    io_handle: tokio::task::JoinHandle<()>,
    stderr_handle: tokio::task::JoinHandle<()>,
    client: &Rc<RalphClient>,
    mut child: tokio::process::Child,
) {
    // Kill all terminal subprocesses tracked by the client.
    client.cleanup_all_terminals().await;

    // Abort the JSON-RPC transport task.
    io_handle.abort();
    // Abort the stderr reader task.
    stderr_handle.abort();

    // Kill the agent process. Ignore errors (process may have already exited).
    let _ = child.kill().await;
    // Wait briefly to avoid zombie processes.
    let _ = child.wait().await;
}

/// Inner async function that runs the full interactive ACP session inside a LocalSet.
///
/// This follows the same connection lifecycle as `run_acp_session` in `connection.rs`:
/// spawn → initialize → new_session → (prompt → user_input)* → cleanup.
async fn run_interactive_inner(
    agent_command: String,
    project_root: PathBuf,
    instructions: String,
    initial_message: String,
    model: Option<String>,
    allow_terminal: bool,
    allowed_write_paths: Option<Vec<PathBuf>>,
) -> Result<()> {
    // ── 1. Parse + spawn agent process ────────────────────────────────────
    let parts = shlex::split(&agent_command).ok_or_else(|| {
        anyhow!(
            "invalid agent command: failed to parse \"{}\"",
            agent_command
        )
    })?;
    if parts.is_empty() {
        return Err(anyhow!("agent command is empty"));
    }
    let mut parts_iter = parts.into_iter();
    let program = parts_iter.next().unwrap();
    let args: Vec<String> = parts_iter.collect();

    // RALPH_MODEL: use explicit override if provided, otherwise default to "claude".
    let ralph_model = model.as_deref().unwrap_or("claude");

    let mut child = tokio::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(&project_root)
        .env("RALPH_MODEL", ralph_model)
        .env("RALPH_ITERATION", "0")
        .env("RALPH_TOTAL", "0")
        .spawn()
        .map_err(|e| anyhow!("failed to spawn agent '{program}': {e}"))?;

    let agent_stdin = child.stdin.take().expect("stdin piped");
    let agent_stdout = child.stdout.take().expect("stdout piped");
    let agent_stderr = child.stderr.take().expect("stderr piped");

    // ── 2. Spawn background stderr reader (discard — interactive sessions
    //       don't log to a file) ───────────────────────────────────────────
    let stderr_handle = tokio::task::spawn_local(async move {
        let mut stderr = agent_stderr;
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {} // discard
            }
        }
    });

    // ── 3. Create RalphClient and wire up the ACP connection ──────────────
    let mut ralph_client = RalphClient::new(project_root.clone(), false);
    if let Some(paths) = allowed_write_paths {
        ralph_client = ralph_client.with_allowed_write_paths(paths);
    }
    let client = Rc::new(ralph_client);
    let client_ref = Rc::clone(&client);

    // Convert tokio IO handles → futures-compatible IO (required by the ACP crate).
    let outgoing = agent_stdin.compat_write(); // Ralph → agent stdin
    let incoming = agent_stdout.compat(); // agent stdout → Ralph

    let (conn, io_future) = ClientSideConnection::new(client_ref, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });

    // Drive the JSON-RPC transport in the background.
    let io_handle = tokio::task::spawn_local(async move {
        let _ = io_future.await;
    });

    // ── 4. ACP handshake ──────────────────────────────────────────────────
    let fs_caps = FileSystemCapability::new()
        .read_text_file(true)
        .write_text_file(true); // interactive sessions allow writes

    let caps = ClientCapabilities::new()
        .fs(fs_caps)
        .terminal(allow_terminal);
    let client_info = Implementation::new("ralph", env!("CARGO_PKG_VERSION"));

    let init_req = InitializeRequest::new(ProtocolVersion::LATEST)
        .client_capabilities(caps)
        .client_info(client_info);

    let init_resp = conn
        .initialize(init_req)
        .await
        .map_err(|e| anyhow!("ACP initialize failed: {e}"))?;

    // Attempt authentication if the agent advertises auth methods.
    // Failures are non-fatal: some agents (e.g. claude-agent-acp) advertise methods
    // but don't implement the authenticate RPC, expecting out-of-band auth instead.
    for method in &init_resp.auth_methods {
        let _ = conn
            .authenticate(AuthenticateRequest::new(method.id.clone()))
            .await;
    }

    // ── 5. Create session ─────────────────────────────────────────────────
    let session_resp = conn
        .new_session(NewSessionRequest::new(project_root.clone()))
        .await
        .map_err(|e| anyhow!("ACP new_session failed: {e}"))?;

    let session_id = session_resp.session_id;

    // ── 6. First prompt: instructions + initial_message concatenated ───────
    // ACP has no separate system-prompt channel, so instructions and the
    // initial message are joined into one TextContent block.
    let first_prompt_text = format!("{instructions}\n\n---\n\n{initial_message}");
    let first_req = PromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::Text(TextContent::new(first_prompt_text))],
    );

    let interrupted = tokio::select! {
        result = conn.prompt(first_req) => {
            match result {
                Ok(_) => false,
                Err(e) => {
                    eprintln!("ralph: ACP prompt failed: {e}");
                    true
                }
            }
        }
        _ = poll_interrupt() => {
            // User pressed Ctrl+C during the initial response.
            let _ = conn.cancel(CancelNotification::new(session_id.clone())).await;
            true
        }
    };

    if interrupted {
        interactive_cleanup(conn, io_handle, stderr_handle, &client, child).await;
        return Ok(());
    }

    // Ensure cursor is on a fresh line after the agent's streamed response.
    println!();

    // ── 7. Interactive loop: read user input → send prompt → render ────────
    let mut user_stdin = BufReader::new(tokio::io::stdin());
    loop {
        // Print the user prompt indicator.
        print!("\n> ");
        let _ = std::io::stdout().flush();

        // Read one line from the user's terminal asynchronously.
        let mut line_buf = String::new();
        let n = user_stdin.read_line(&mut line_buf).await.unwrap_or(0);
        if n == 0 {
            // EOF (Ctrl+D) — exit gracefully.
            break;
        }

        // Trim the trailing newline (and any CR on Windows).
        let user_input = line_buf
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_owned();

        if user_input.is_empty() {
            // Empty line — exit gracefully.
            break;
        }

        // Check the interrupt flag before sending the next prompt.
        // (Handles Ctrl+C pressed while the blocking stdin read was completing.)
        if interrupt::is_interrupted() {
            let _ = conn
                .cancel(CancelNotification::new(session_id.clone()))
                .await;
            break;
        }

        // Send the user's input as the next prompt in the same session.
        let prompt_req = PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(user_input))],
        );

        let should_exit = tokio::select! {
            result = conn.prompt(prompt_req) => {
                match result {
                    Ok(_) => false,
                    Err(e) => {
                        eprintln!("ralph: ACP prompt failed: {e}");
                        true
                    }
                }
            }
            _ = poll_interrupt() => {
                // User pressed Ctrl+C during the agent's response.
                let _ = conn.cancel(CancelNotification::new(session_id.clone())).await;
                true
            }
        };

        if should_exit {
            break;
        }

        // Ensure cursor is on a fresh line after the streamed response.
        println!();
    }

    // ── 8. Cleanup ─────────────────────────────────────────────────────────
    interactive_cleanup(conn, io_handle, stderr_handle, &client, child).await;
    Ok(())
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_interrupt_is_async() {
        // Just verify the function exists and has the right signature.
        // We don't actually run it (would block forever without an interrupt).
        let _: fn() -> _ = poll_interrupt;
    }

    #[test]
    fn test_first_prompt_concatenation() {
        // Verify the format of the concatenated first prompt.
        let instructions = "You are a feature planner.";
        let initial_message = "Please help me plan this feature.";
        let prompt_text = format!("{instructions}\n\n---\n\n{initial_message}");

        assert!(prompt_text.contains("You are a feature planner."));
        assert!(prompt_text.contains("---"));
        assert!(prompt_text.contains("Please help me plan this feature."));
        // Instructions come before the separator.
        let sep_pos = prompt_text.find("---").unwrap();
        let instr_pos = prompt_text.find("You are a feature planner.").unwrap();
        assert!(instr_pos < sep_pos, "instructions should precede separator");
    }

    #[test]
    fn test_run_streaming_is_async() {
        // Verify run_streaming can be called with the right argument types by
        // creating the future (not awaiting it — we have no runtime here).
        fn assert_is_future<F: std::future::Future<Output = Result<()>>>(_f: F) {}
        let root = std::path::Path::new("/tmp");
        assert_is_future(run_streaming(
            "claude",
            "instructions",
            "message",
            root,
            None,
        ));
    }

    #[test]
    fn test_run_interactive_is_async() {
        // Verify run_interactive can be called with the right argument types.
        fn assert_is_future<F: std::future::Future<Output = Result<()>>>(_f: F) {}
        let root = std::path::Path::new("/tmp");
        assert_is_future(run_interactive(
            "claude",
            "instructions",
            "initial",
            root,
            None,
            true,
            None,
        ));
    }
}
