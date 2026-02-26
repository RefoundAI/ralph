//! Interactive and streaming ACP sessions.
//!
//! - [`run_interactive`]: ACP-mediated interactive session (user ↔ agent loop).
//! - [`run_streaming`]: Single autonomous prompt, stream output.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use agent_client_protocol::{
    Agent, AuthenticateRequest, CancelNotification, ClientCapabilities, ClientSideConnection,
    ContentBlock, FileSystemCapability, Implementation, InitializeRequest, NewSessionRequest,
    PromptRequest, ProtocolVersion, TextContent,
};
use anyhow::{anyhow, Result};
use colored::Colorize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::task::LocalSet;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::acp::client_impl::RalphClient;
use crate::acp::connection;
use crate::acp::connection::auth_hint;
use crate::interrupt;
use crate::output::formatter;
use crate::ui;

/// Run an interactive ACP session (user types, agent responds, repeat).
///
/// Returns the accumulated agent text output for sigil extraction by the caller.
/// When the agent emits a `<phase-complete>` or `<tasks-created>` sigil, the
/// session auto-exits without waiting for further user input.
pub async fn run_interactive(
    agent_command: &str,
    instructions: &str,
    initial_message: &str,
    project_root: &Path,
    model: Option<&str>,
    allow_terminal: bool,
    allowed_write_paths: Option<Vec<PathBuf>>,
) -> Result<String> {
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
/// Returns the accumulated agent text output for sigil extraction by the caller.
pub async fn run_streaming(
    agent_command: &str,
    instructions: &str,
    message: &str,
    project_root: &Path,
    model: Option<&str>,
) -> Result<String> {
    let result = connection::run_autonomous(
        agent_command,
        project_root,
        instructions,
        message,
        false,
        model,
        connection::SessionRestrictions {
            allow_terminal: true,
            ..Default::default()
        },
    )
    .await?;
    Ok(result.full_text)
}

enum MultilineResult {
    Input(String),
    Exit,
    Interrupted,
}

async fn read_multiline_input(stdin: &mut BufReader<tokio::io::Stdin>) -> MultilineResult {
    if ui::is_active() {
        let hint = "Type your message. Empty line submits. Empty buffer exits. Ctrl+C interrupts.";
        return match ui::prompt_multiline("Interactive Prompt", hint) {
            Some(ui::UiPromptResult::Input(text)) => MultilineResult::Input(text),
            Some(ui::UiPromptResult::Exit) => MultilineResult::Exit,
            Some(ui::UiPromptResult::Interrupted) => MultilineResult::Interrupted,
            None => MultilineResult::Exit,
        };
    }

    let mut lines: Vec<String> = Vec::new();
    loop {
        if lines.is_empty() {
            print!("\n{} ", ">".bright_cyan());
        } else {
            print!("{} ", "|".bright_black());
        }
        let _ = std::io::stdout().flush();

        if interrupt::is_interrupted() {
            return MultilineResult::Interrupted;
        }

        let mut line_buf = String::new();
        let n = stdin.read_line(&mut line_buf).await.unwrap_or(0);
        if n == 0 {
            return if lines.is_empty() {
                MultilineResult::Exit
            } else {
                MultilineResult::Input(lines.join("\n"))
            };
        }

        let line = line_buf
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_owned();

        if line.is_empty() {
            return if lines.is_empty() {
                MultilineResult::Exit
            } else {
                MultilineResult::Input(lines.join("\n"))
            };
        }

        lines.push(line);
    }
}

async fn poll_interrupt() {
    loop {
        if interrupt::is_interrupted() {
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

async fn interactive_cleanup(
    _conn: ClientSideConnection,
    io_handle: tokio::task::JoinHandle<()>,
    stderr_handle: tokio::task::JoinHandle<()>,
    client: &Rc<RalphClient>,
    mut child: tokio::process::Child,
) {
    client.cleanup_all_terminals().await;
    io_handle.abort();
    stderr_handle.abort();
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn run_interactive_inner(
    agent_command: String,
    project_root: PathBuf,
    instructions: String,
    initial_message: String,
    model: Option<String>,
    allow_terminal: bool,
    allowed_write_paths: Option<Vec<PathBuf>>,
) -> Result<String> {
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
    let program = parts_iter.next().unwrap_or_default();
    let args: Vec<String> = parts_iter.collect();

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

    let stderr_handle = tokio::task::spawn_local(async move {
        let mut stderr = agent_stderr;
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });

    let mut ralph_client = RalphClient::new(project_root.clone(), false, ralph_model.to_string());
    if let Some(paths) = allowed_write_paths {
        ralph_client = ralph_client.with_allowed_write_paths(paths);
    }
    let client = Rc::new(ralph_client);
    let client_ref = Rc::clone(&client);

    let outgoing = agent_stdin.compat_write();
    let incoming = agent_stdout.compat();
    let (conn, io_future) = ClientSideConnection::new(client_ref, outgoing, incoming, |fut| {
        tokio::task::spawn_local(fut);
    });
    let io_handle = tokio::task::spawn_local(async move {
        let _ = io_future.await;
    });

    let fs_caps = FileSystemCapability::new()
        .read_text_file(true)
        .write_text_file(true);
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
        .map_err(|e| match auth_hint(&e) {
            Some(hint) => anyhow!("{hint}"),
            None => anyhow!("ACP initialize failed: {e}"),
        })?;

    for method in &init_resp.auth_methods {
        let _ = conn
            .authenticate(AuthenticateRequest::new(method.id.clone()))
            .await;
    }

    let session_resp = conn
        .new_session(NewSessionRequest::new(project_root.clone()))
        .await
        .map_err(|e| match auth_hint(&e) {
            Some(hint) => anyhow!("{hint}"),
            None => anyhow!("ACP new_session failed: {e}"),
        })?;
    let session_id = session_resp.session_id;

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
                    match auth_hint(&e) {
                        Some(hint) => formatter::print_warning(&format!("ralph: {hint}")),
                        None => formatter::print_warning(&format!("ralph: ACP prompt failed: {e}")),
                    }
                    true
                }
            }
        }
        _ = poll_interrupt() => {
            let _ = conn.cancel(CancelNotification::new(session_id.clone())).await;
            true
        }
    };

    if interrupted {
        let text = client.take_accumulated_text();
        interactive_cleanup(conn, io_handle, stderr_handle, &client, child).await;
        return Ok(text);
    }
    if !ui::is_active() {
        println!();
    }

    // Check for sigils after the first agent response
    {
        let sigils =
            crate::acp::sigils::extract_interactive_sigils(&client.peek_accumulated_text());
        if sigils.phase_complete.is_some() || sigils.tasks_created {
            let text = client.take_accumulated_text();
            interactive_cleanup(conn, io_handle, stderr_handle, &client, child).await;
            return Ok(text);
        }
    }

    let mut user_stdin = BufReader::new(tokio::io::stdin());
    let mut first_prompt = true;
    loop {
        if first_prompt {
            formatter::print_info(
                "  (multi-line: blank line sends, two blank lines exits, Ctrl+D sends/exits)",
            );
            first_prompt = false;
        }

        let user_input = match read_multiline_input(&mut user_stdin).await {
            MultilineResult::Input(text) => text,
            MultilineResult::Exit => break,
            MultilineResult::Interrupted => {
                let _ = conn
                    .cancel(CancelNotification::new(session_id.clone()))
                    .await;
                break;
            }
        };

        if interrupt::is_interrupted() {
            let _ = conn
                .cancel(CancelNotification::new(session_id.clone()))
                .await;
            break;
        }

        let prompt_req = PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(user_input))],
        );

        let should_exit = tokio::select! {
            result = conn.prompt(prompt_req) => {
                match result {
                    Ok(_) => false,
                    Err(e) => {
                        match auth_hint(&e) {
                            Some(hint) => formatter::print_warning(&format!("ralph: {hint}")),
                            None => formatter::print_warning(&format!("ralph: ACP prompt failed: {e}")),
                        }
                        true
                    }
                }
            }
            _ = poll_interrupt() => {
                let _ = conn.cancel(CancelNotification::new(session_id.clone())).await;
                true
            }
        };
        if should_exit {
            break;
        }
        if !ui::is_active() {
            println!();
        }

        // Check for sigils after each agent response
        {
            let sigils =
                crate::acp::sigils::extract_interactive_sigils(&client.peek_accumulated_text());
            if sigils.phase_complete.is_some() || sigils.tasks_created {
                break;
            }
        }
    }

    let text = client.take_accumulated_text();
    interactive_cleanup(conn, io_handle, stderr_handle, &client, child).await;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poll_interrupt_is_async() {
        let _: fn() -> _ = poll_interrupt;
    }

    #[test]
    fn test_first_prompt_concatenation() {
        let instructions = "You are a feature planner.";
        let initial_message = "Please help me plan this feature.";
        let prompt_text = format!("{instructions}\n\n---\n\n{initial_message}");
        assert!(prompt_text.contains("You are a feature planner."));
        assert!(prompt_text.contains("---"));
        assert!(prompt_text.contains("Please help me plan this feature."));
        let sep_pos = prompt_text.find("---").unwrap();
        let instr_pos = prompt_text.find("You are a feature planner.").unwrap();
        assert!(instr_pos < sep_pos, "instructions should precede separator");
    }

    #[test]
    fn test_run_streaming_is_async() {
        fn assert_is_future<F: std::future::Future<Output = Result<String>>>(_f: F) {}
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
        fn assert_is_future<F: std::future::Future<Output = Result<String>>>(_f: F) {}
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

    #[test]
    fn test_multiline_result_variants_exist() {
        let _input = MultilineResult::Input("hello\nworld".to_string());
        let _exit = MultilineResult::Exit;
        let _interrupted = MultilineResult::Interrupted;
    }
}
