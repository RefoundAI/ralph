//! Claude CLI invocation for prompt, specs, and plan subcommands.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// Build the command for launching Claude in interactive mode.
///
/// Returns a `Command` ready to execute. Extracted for testability.
fn build_interactive_command(
    system_prompt: &str,
    initial_message: &str,
    model: Option<&str>,
) -> Command {
    let mut cmd = Command::new("claude");
    cmd.arg("--system-prompt").arg(system_prompt);

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    cmd.arg(initial_message);
    cmd
}

/// Launch Claude in interactive mode with system prompt and initial message.
///
/// The `initial_message` is passed as a positional argument to `claude`,
/// causing Claude to respond immediately when the session opens.
/// The `model` is optional -- when provided, passed as `--model <model>`.
pub fn run_interactive(
    system_prompt: &str,
    initial_message: &str,
    model: Option<&str>,
) -> Result<()> {
    let status = build_interactive_command(system_prompt, initial_message, model)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to spawn claude process. Is `claude` installed and in PATH?")?;

    if !status.success() {
        anyhow::bail!("claude exited with status: {}", status);
    }

    Ok(())
}

/// Launch Claude in non-interactive streaming mode.
///
/// Spawns `claude --print --verbose --output-format stream-json` with a system
/// prompt and an initial message. Claude runs autonomously (no user input),
/// executing tool calls as needed. Output is streamed and formatted in real time.
///
/// Used by `feature build` to let Claude autonomously create a task DAG via
/// `ralph task add` and `ralph task deps add` CLI commands.
pub fn run_streaming(
    system_prompt: &str,
    initial_message: &str,
    model: Option<&str>,
) -> Result<()> {
    let mut cmd = Command::new("claude");
    cmd.arg("--print")
        .arg("--verbose")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--dangerously-skip-permissions")
        .arg("--system-prompt")
        .arg(system_prompt);

    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }

    cmd.arg(initial_message);

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .context("Failed to spawn claude process. Is `claude` installed and in PATH?")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let stderr_thread = super::client::drain_stderr(stderr);

    super::client::stream_output(stdout, None, false)?;

    let status = child.wait().context("Failed to wait for claude process")?;
    let stderr_output = stderr_thread.join().unwrap_or_default();

    if !status.success() {
        if stderr_output.is_empty() {
            anyhow::bail!("claude exited with status: {}", status);
        } else {
            anyhow::bail!(
                "claude exited with status: {}\nstderr: {}",
                status,
                stderr_output
            );
        }
    } else if !stderr_output.is_empty() {
        eprintln!("{}", stderr_output);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_interactive_command_basic() {
        let cmd = build_interactive_command("test prompt", "hello", None);
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();

        assert_eq!(args, ["--system-prompt", "test prompt", "hello"]);
    }

    #[test]
    fn test_build_interactive_command_with_model() {
        let cmd = build_interactive_command("test prompt", "hello", Some("opus"));
        let args: Vec<&str> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();

        assert_eq!(
            args,
            ["--system-prompt", "test prompt", "--model", "opus", "hello"]
        );
    }
}
