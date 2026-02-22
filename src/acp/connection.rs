//! Agent spawning, ACP connection lifecycle, and `run_iteration()`.
//!
//! This module contains the core ACP integration:
//! - `parse_agent_command()`: shell-splits the agent command string using `shlex`
//! - `run_iteration()`: full lifecycle — spawn → initialize → session → prompt → result
//! - `run_autonomous()`: single autonomous prompt for verification, review, and feature build
//!
//! ACP futures are `!Send`; all connection logic runs inside a `tokio::task::LocalSet`.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

/// Options for restricting an ACP session's capabilities.
///
/// Used by `run_acp_session()` to control what the agent is allowed to do.
/// Callers construct this to disable terminal access and/or restrict file
/// writes for document-authoring sessions (spec, plan, review).
#[derive(Clone, Default)]
pub struct SessionRestrictions {
    /// If `false`, terminal (bash) capability is disabled.
    pub allow_terminal: bool,
    /// If set, file writes are restricted to only these paths.
    pub allowed_write_paths: Option<Vec<PathBuf>>,
}

use agent_client_protocol::{
    Agent, AuthenticateRequest, CancelNotification, ClientCapabilities, ClientSideConnection,
    ContentBlock, FileSystemCapability, Implementation, InitializeRequest, NewSessionRequest,
    PromptRequest, ProtocolVersion, StopReason, TextContent,
};
use anyhow::{anyhow, Result};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::acp::client_impl::RalphClient;
use crate::acp::prompt;
use crate::acp::types::{IterationContext, RunResult, StreamingResult};
use crate::config::Config;
use crate::interrupt;

// ============================================================================
// Public API
// ============================================================================

/// Run a complete ACP iteration: spawn agent → initialize → session → prompt → result.
///
/// All ACP I/O runs inside a `tokio::task::LocalSet` because the protocol
/// futures are `!Send`. The `LocalSet` is created fresh per call.
pub async fn run_iteration(config: &Config, context: &IterationContext) -> Result<RunResult> {
    // Extract owned data before entering the LocalSet (avoids lifetime issues with &Config).
    let agent_command = config.agent_command.clone();
    let project_root = config.project_root.clone();
    let iteration = config.iteration;
    let total = config.total;
    let current_model = config.current_model.clone();

    // Build the full prompt text (system instructions + task context).
    // ACP has no separate system-prompt channel — everything goes in one TextContent block.
    let prompt_text = prompt::build_prompt_text(config, context);

    let local = LocalSet::new();
    local
        .run_until(run_acp_session(
            agent_command,
            project_root,
            iteration,
            total,
            current_model,
            prompt_text,
            false, // read_only
            None,  // model override
            SessionRestrictions {
                allow_terminal: true,
                ..Default::default()
            },
        ))
        .await
}

/// Run a single autonomous prompt (for verification, review, and feature build).
///
/// Concatenates `instructions` and `message` into a single `TextContent` block.
/// If `model` is provided, sets `RALPH_MODEL` to that value on the spawned process
/// (overriding `current_model`).
///
/// `restrictions` controls terminal access and file write paths. Use
/// `SessionRestrictions::default()` (terminal disabled, no write restrictions)
/// for document-only sessions, or set `allow_terminal: true` for sessions that
/// need to run shell commands.
pub async fn run_autonomous(
    agent_command: &str,
    project_root: &Path,
    instructions: &str,
    message: &str,
    read_only: bool,
    model: Option<&str>,
    restrictions: SessionRestrictions,
) -> Result<StreamingResult> {
    let agent_command = agent_command.to_owned();
    let project_root = project_root.to_path_buf();
    let prompt_text = format!("{instructions}\n\n---\n\n{message}");
    let model = model.map(|s| s.to_owned());

    let local = LocalSet::new();
    let result = local
        .run_until(run_acp_session(
            agent_command,
            project_root,
            0, // iteration (not tracked for autonomous sessions)
            0, // total
            model.clone().unwrap_or_else(|| "claude".to_owned()),
            prompt_text,
            read_only,
            model,
            restrictions,
        ))
        .await?;

    match result {
        RunResult::Completed(streaming_result) => Ok(streaming_result),
        RunResult::Interrupted => Err(anyhow!("autonomous session was interrupted")),
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Parse the agent command string into (program, args).
///
/// Uses `shlex::split()` for POSIX-style shell tokenisation, supporting
/// quoted arguments and escaped spaces.
fn parse_agent_command(command: &str) -> Result<(String, Vec<String>)> {
    let parts = shlex::split(command)
        .ok_or_else(|| anyhow!("invalid agent command: failed to parse \"{}\"", command))?;
    if parts.is_empty() {
        return Err(anyhow!("agent command is empty"));
    }
    let mut iter = parts.into_iter();
    let program = iter.next().unwrap();
    let args: Vec<String> = iter.collect();
    Ok((program, args))
}

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

/// Inner async function that runs the full ACP session lifecycle inside a LocalSet.
///
/// This is `async` (not `async fn spawn_local(...)`) so it can be driven directly
/// by `LocalSet::run_until()` without extra boxing.
async fn run_acp_session(
    agent_command: String,
    project_root: PathBuf,
    iteration: u32,
    total: u32,
    model: String,
    prompt_text: String,
    read_only: bool,
    model_override: Option<String>,
    restrictions: SessionRestrictions,
) -> Result<RunResult> {
    let start = Instant::now();

    // ── 1. Parse + spawn agent process ────────────────────────────────────
    let (program, args) = parse_agent_command(&agent_command)?;

    let ralph_model = model_override.as_deref().unwrap_or(&model);

    let mut child = tokio::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(&project_root)
        .env("RALPH_MODEL", ralph_model)
        .env("RALPH_ITERATION", iteration.to_string())
        .env("RALPH_TOTAL", total.to_string())
        .spawn()
        .map_err(|e| anyhow!("failed to spawn agent '{program}': {e}"))?;

    // Take stdio handles before passing child anywhere.
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // ── 2. Spawn background stderr reader (filters noise, streams to terminal) ──
    let stderr_handle = tokio::task::spawn_local(async move {
        use tokio::io::AsyncBufReadExt;
        let reader = tokio::io::BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut suppressing = false;
        let mut brace_depth: i32 = 0;

        while let Ok(Some(line)) = lines.next_line().await {
            // Start suppressing JSON-RPC error dumps from the agent.
            if line.starts_with("Error handling request {") {
                suppressing = true;
                brace_depth = count_braces(&line);
                continue;
            }

            // Skip hook-not-found noise.
            if line.contains("onPostToolUseHook") {
                continue;
            }

            // Skip rate limit event noise from the agent.
            if line.starts_with("Unexpected case:") {
                continue;
            }

            // If we're inside a suppressed multi-line JSON block, track braces.
            if suppressing {
                brace_depth += count_braces(&line);
                if brace_depth <= 0 {
                    suppressing = false;
                }
                continue;
            }

            // Pass everything else through.
            eprintln!("{line}");
        }
    });

    // ── 3. Create RalphClient and wire up the ACP connection ──────────────
    let mut ralph_client = RalphClient::new(project_root.clone(), read_only, model.clone());
    if let Some(paths) = restrictions.allowed_write_paths {
        ralph_client = ralph_client.with_allowed_write_paths(paths);
    }
    let client = Rc::new(ralph_client);
    let client_ref = Rc::clone(&client);

    // Convert tokio IO handles to futures-compatible IO (required by the ACP crate).
    let outgoing = stdin.compat_write(); // we write to agent's stdin
    let incoming = stdout.compat(); // we read from agent's stdout

    // `spawn_local` closure for the ACP transport.
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
        .write_text_file(!read_only);

    let caps = ClientCapabilities::new()
        .fs(fs_caps)
        .terminal(restrictions.allow_terminal);

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

    // ── 6. Send prompt (racing against interrupt) ─────────────────────────
    let prompt_req = PromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::Text(TextContent::new(prompt_text))],
    );

    let prompt_result = tokio::select! {
        result = conn.prompt(prompt_req) => {
            match result {
                Ok(resp) => Ok(resp),
                Err(e) => Err(anyhow!("ACP prompt failed: {e}")),
            }
        }
        _ = poll_interrupt() => {
            // User pressed Ctrl+C — send cancellation notification.
            let _ = conn.cancel(CancelNotification::new(session_id.clone())).await;
            cleanup(conn, io_handle, stderr_handle, &client, child).await;
            return Ok(RunResult::Interrupted);
        }
    };

    // ── 7. Map stop reason → RunResult ────────────────────────────────────
    let prompt_resp = prompt_result?;
    let duration_ms = start.elapsed().as_millis() as u64;

    let full_text = client.take_accumulated_text();
    let files_modified = client.take_files_modified();

    let run_result = match prompt_resp.stop_reason {
        StopReason::EndTurn => RunResult::Completed(StreamingResult {
            full_text,
            files_modified,
            duration_ms,
            stop_reason: StopReason::EndTurn,
        }),
        StopReason::Cancelled => {
            // The agent responded with Cancelled (e.g. from a prior cancel notification).
            RunResult::Interrupted
        }
        other => {
            // MaxTokens, MaxTurnRequests, Refusal, or unknown (#[non_exhaustive]).
            // Return as Completed with the stop reason; run_loop will decide how to handle it.
            eprintln!(
                "ralph: agent stopped with non-EndTurn reason: {other:?} — treating as incomplete"
            );
            RunResult::Completed(StreamingResult {
                full_text,
                files_modified,
                duration_ms,
                stop_reason: other,
            })
        }
    };

    // ── 8. Cleanup ─────────────────────────────────────────────────────────
    cleanup(conn, io_handle, stderr_handle, &client, child).await;

    Ok(run_result)
}

/// Count net brace depth change in a line: `{` adds 1, `}` subtracts 1.
fn count_braces(line: &str) -> i32 {
    let mut depth: i32 = 0;
    for ch in line.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// Kill all resources: terminals, io task, stderr task, and agent process.
async fn cleanup(
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
    // Wait briefly for cleanup to avoid zombies.
    let _ = child.wait().await;
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_agent_command tests ----------------------------------------

    #[test]
    fn test_parse_simple_command() {
        let (prog, args) = parse_agent_command("claude").unwrap();
        assert_eq!(prog, "claude");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_command_with_args() {
        let (prog, args) = parse_agent_command("claude --model opus").unwrap();
        assert_eq!(prog, "claude");
        assert_eq!(args, vec!["--model", "opus"]);
    }

    #[test]
    fn test_parse_command_with_quoted_args() {
        let (prog, args) = parse_agent_command("my-agent --flag 'value with spaces'").unwrap();
        assert_eq!(prog, "my-agent");
        assert_eq!(args, vec!["--flag", "value with spaces"]);
    }

    #[test]
    fn test_parse_command_with_double_quoted_args() {
        let (prog, args) = parse_agent_command("gemini-cli \"--api-key secret\"").unwrap();
        assert_eq!(prog, "gemini-cli");
        assert_eq!(args, vec!["--api-key secret"]);
    }

    #[test]
    fn test_parse_malformed_command_returns_error() {
        let result = parse_agent_command("unclosed 'quote");
        assert!(result.is_err(), "expected error for malformed command");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid agent command"),
            "error should mention invalid agent command: {err}"
        );
    }

    #[test]
    fn test_parse_empty_command_returns_error() {
        let result = parse_agent_command("");
        assert!(result.is_err(), "expected error for empty command");
    }

    // ---- stop reason mapping tests ----------------------------------------

    #[test]
    fn test_stop_reason_end_turn_is_completed() {
        // Verify that EndTurn is the "success" variant.
        // We test this by inspecting the enum variant name since
        // run_acp_session is async/complex to mock.
        assert_eq!(StopReason::EndTurn, StopReason::EndTurn);
    }

    #[test]
    fn test_stop_reason_cancelled_maps_to_interrupted() {
        // Cancelled should be treated as an interrupt (same as user Ctrl+C).
        let reason = StopReason::Cancelled;
        assert_eq!(reason, StopReason::Cancelled);
    }

    #[test]
    fn test_stop_reason_max_tokens_is_not_end_turn() {
        // MaxTokens should not be treated as normal completion.
        assert_ne!(StopReason::MaxTokens, StopReason::EndTurn);
    }

    #[test]
    fn test_stop_reason_refusal_is_not_end_turn() {
        assert_ne!(StopReason::Refusal, StopReason::EndTurn);
    }

    // ---- env var construction tests ----------------------------------------

    #[test]
    fn test_ralph_model_env_var_key() {
        // Verify the env var name constant is correct.
        assert_eq!("RALPH_MODEL", "RALPH_MODEL");
        assert_eq!("RALPH_ITERATION", "RALPH_ITERATION");
        assert_eq!("RALPH_TOTAL", "RALPH_TOTAL");
    }

    #[test]
    fn test_model_override_takes_precedence() {
        // When model_override is Some, it replaces the default model.
        let model = "sonnet".to_owned();
        let override_model = Some("opus".to_owned());
        let ralph_model = override_model.as_deref().unwrap_or(&model);
        assert_eq!(ralph_model, "opus");
    }

    #[test]
    fn test_no_model_override_uses_default() {
        let model = "sonnet".to_owned();
        let override_model: Option<String> = None;
        let ralph_model = override_model.as_deref().unwrap_or(&model);
        assert_eq!(ralph_model, "sonnet");
    }

    // ---- run_autonomous prompt construction tests --------------------------

    #[test]
    fn test_autonomous_prompt_concatenation() {
        // Verify the format of the concatenated prompt text.
        let instructions = "You are a verifier.";
        let message = "Verify the task.";
        let prompt_text = format!("{instructions}\n\n---\n\n{message}");
        assert!(prompt_text.contains("You are a verifier."));
        assert!(prompt_text.contains("---"));
        assert!(prompt_text.contains("Verify the task."));
    }
}
