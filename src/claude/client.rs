//! Claude CLI process spawning and streaming.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::thread;

use crate::config::Config;
use crate::output::formatter::{self, ToolCallInfo};
use crate::sandbox;

use super::events::{Event, ResultEvent};
use super::parser;

/// Information about a task assigned to Claude for the current iteration.
pub struct TaskInfo {
    pub task_id: String,
    pub title: String,
    pub description: String,
    pub parent: Option<ParentContext>,
    pub completed_blockers: Vec<BlockerContext>,
    pub specs_dirs: Vec<String>,
}

/// Context about a task's parent task.
pub struct ParentContext {
    pub title: String,
    pub description: String,
}

/// Context about a completed blocker (prerequisite) task.
pub struct BlockerContext {
    pub task_id: String,
    pub title: String,
    pub summary: String,
}

/// Build a task context block for the assigned task.
///
/// Returns a formatted markdown block with task details, parent context,
/// completed prerequisites, and specs directory references.
pub fn build_task_context(task: &TaskInfo) -> String {
    let mut output = String::new();

    output.push_str("## Assigned Task\n\n");
    output.push_str(&format!("**ID:** {}\n", task.task_id));
    output.push_str(&format!("**Title:** {}\n", task.title));
    output.push_str("\n### Description\n");
    output.push_str(&task.description);
    output.push_str("\n");

    // Add parent context if present
    if let Some(ref parent) = task.parent {
        output.push_str("\n### Parent Context\n");
        output.push_str(&format!("**Parent:** {}\n", parent.title));
        output.push_str(&parent.description);
        output.push_str("\n");
    }

    // Add completed blockers if present
    if !task.completed_blockers.is_empty() {
        output.push_str("\n### Completed Prerequisites\n");
        for blocker in &task.completed_blockers {
            output.push_str(&format!(
                "- [{}] {}: {}\n",
                blocker.task_id, blocker.title, blocker.summary
            ));
        }
    }

    // Add specs directory references
    if !task.specs_dirs.is_empty() {
        output.push_str("\n### Reference Specs\n");
        output.push_str(&format!("Read all files in: {}\n", task.specs_dirs.join(", ")));
    }

    output
}

/// Build the CLI args vec for invoking the `claude` command.
fn build_claude_args(config: &Config) -> Vec<String> {
    let system_prompt = build_system_prompt(config);

    let mut args = vec![
        "--print".to_string(),
        "--verbose".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--no-session-persistence".to_string(),
        "--model".to_string(),
        config.current_model.clone(),
        "--system-prompt".to_string(),
        system_prompt,
        format!("@{}", config.prompt_file),
    ];

    // Add tool args based on sandbox mode
    if config.use_sandbox {
        args.push("--dangerously-skip-permissions".to_string());
    } else {
        let tools = config.allowed_tools.join(" ");
        args.push("--allowed-tools".to_string());
        args.push(tools);
    }

    args
}

/// Run Claude with the given config and stream output.
/// Returns the final result event, if any.
pub fn run(config: &Config, log_file: Option<&str>) -> Result<Option<ResultEvent>> {
    let args = build_claude_args(config);

    if config.use_sandbox {
        run_sandboxed(&args, log_file, config)
    } else {
        run_direct(&args, log_file)
    }
}

fn run_direct(args: &[String], log_file: Option<&str>) -> Result<Option<ResultEvent>> {
    let mut child = Command::new("claude")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude process")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let stderr_thread = drain_stderr(stderr);

    let result = stream_output(stdout, log_file)?;

    let status = child.wait().context("Failed to wait for claude process")?;
    let stderr_output = stderr_thread.join().unwrap_or_default();

    if !status.success() {
        if stderr_output.is_empty() {
            anyhow::bail!("claude exited with status: {}", status);
        } else {
            anyhow::bail!("claude exited with status: {}\nstderr: {}", status, stderr_output);
        }
    } else if !stderr_output.is_empty() {
        eprintln!("{}", stderr_output);
    }

    Ok(result)
}

fn run_sandboxed(args: &[String], log_file: Option<&str>, config: &Config) -> Result<Option<ResultEvent>> {
    let sandbox_profile = sandbox::profile::generate(config);
    let profile_path = write_temp_profile(&sandbox_profile)?;

    let project_dir = std::env::current_dir()
        .context("Failed to get current directory")?
        .to_string_lossy()
        .to_string();
    let home = std::env::var("HOME").unwrap_or_default();
    let root_git_dir = detect_git_dir();

    let mut sandbox_args = vec![
        "-f".to_string(),
        profile_path.clone(),
        "-D".to_string(),
        format!("PROJECT_DIR={}", project_dir),
        "-D".to_string(),
        format!("HOME={}", home),
        "-D".to_string(),
        format!("ROOT_GIT_DIR={}", root_git_dir),
        "claude".to_string(),
    ];
    sandbox_args.extend(args.iter().cloned());

    let mut child = Command::new("sandbox-exec")
        .args(&sandbox_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn sandbox-exec process")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;
    let stderr_thread = drain_stderr(stderr);

    let result = stream_output(stdout, log_file);

    // Clean up temp profile
    let _ = std::fs::remove_file(&profile_path);

    let status = child.wait().context("Failed to wait for sandbox-exec process")?;
    let stderr_output = stderr_thread.join().unwrap_or_default();

    if !status.success() {
        if stderr_output.is_empty() {
            anyhow::bail!("sandbox-exec exited with status: {}", status);
        } else {
            anyhow::bail!("sandbox-exec exited with status: {}\nstderr: {}", status, stderr_output);
        }
    } else if !stderr_output.is_empty() {
        eprintln!("{}", stderr_output);
    }

    result
}

fn stream_output<R: std::io::Read>(
    reader: R,
    log_file: Option<&str>,
) -> Result<Option<ResultEvent>> {
    let mut log_handle = log_file
        .map(|path| File::create(path))
        .transpose()
        .context("Failed to create log file")?;

    let buf_reader = BufReader::new(reader);
    let mut tool_calls: HashMap<String, ToolCallInfo> = HashMap::new();
    let mut last_result: Option<ResultEvent> = None;

    for line in buf_reader.lines() {
        let line = line.context("Failed to read line from stdout")?;

        // Log raw output
        if let Some(ref mut f) = log_handle {
            writeln!(f, "{}", line).ok();
        }

        // Parse and format
        match parser::parse_line(&line) {
            Ok(Some(event)) => {
                match &event {
                    Event::Result(result) => {
                        last_result = Some(ResultEvent {
                            result: result.result.clone(),
                            duration_ms: result.duration_ms,
                            total_cost_usd: result.total_cost_usd,
                            next_model_hint: result.next_model_hint.clone(),
                            task_done: result.task_done.clone(),
                            task_failed: result.task_failed.clone(),
                        });
                    }
                    _ => {}
                }
                formatter::format_event(&event, &mut tool_calls);
            }
            Ok(None) => {}
            Err(_) => {
                // Ignore parse errors for non-JSON lines
            }
        }
    }

    Ok(last_result)
}

/// Drain stderr on a background thread to prevent pipe buffer deadlocks.
fn drain_stderr(mut stderr: std::process::ChildStderr) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    })
}

fn build_system_prompt(config: &Config) -> String {
    let progress_db = config.project_root.join(".ralph/progress.db");
    format!(
        r#"You are operating in a Ralph loop - an autonomous, iterative coding workflow.

## Your Task

1. Read {} to understand what to do
2. Read {} to see what has already been completed
3. Find the SINGLE highest-priority incomplete task
4. Implement that ONE task only - do not work on multiple tasks
5. Run tests and type checks to verify your work
6. Append a terse summary of what you did to {}
7. Commit your changes with a descriptive message; load the committing:git skill first.

## Critical Rules

- ONE TASK PER LOOP. This is essential. Do not implement multiple features.
- Do not assume code exists - search the codebase before implementing
- Do not implement placeholders or stubs - implement fully working code
- If tests fail, fix them before completing
- Update {} with what was actually done, not what was planned
- Update AGENTS.md if you encounter any problems and learn how to solve them, e.g.:
    - Bash tool calls that took multiple attempts to write correctly
- The progress file is always gitignored

## Completion

When ALL tasks and specs are complete, output exactly:
<promise>COMPLETE</promise>

Only output this sigil when there is genuinely no more work to do.

## Critical Failure

If you encounter a situation where you cannot continue and further iterations would
be futile, output exactly:
<promise>FAILURE</promise>

Use this when:
- The prompt contains contradictory or impossible requirements
- You are stuck in a loop making no progress after multiple attempts

Document the reason for failure in {} before outputting the sigil.

## Model Hint

You can influence which model Ralph selects for the NEXT iteration by emitting a
model hint sigil anywhere in your output:

- `<next-model>opus</next-model>` — request the most capable (and expensive) model
- `<next-model>sonnet</next-model>` — request the balanced model
- `<next-model>haiku</next-model>` — request the fastest and cheapest model

Rules:
- The hint applies to the NEXT iteration only; it is not persistent
- Valid values are exactly: `opus`, `sonnet`, `haiku`
- If omitted, Ralph's configured model strategy decides automatically
- Use this when you can tell the next task is trivial (hint haiku) or complex (hint opus)"#,
        config.prompt_file,
        progress_db.display(),
        progress_db.display(),
        progress_db.display(),
        progress_db.display(),
    )
}

fn write_temp_profile(content: &str) -> Result<String> {
    let tmp_dir = std::env::temp_dir();
    let random: u32 = rand_simple();
    let path = tmp_dir.join(format!("ralph-sandbox-{}.sb", random));
    std::fs::write(&path, content).context("Failed to write sandbox profile")?;
    Ok(path.to_string_lossy().to_string())
}

/// Simple random number generator without external deps.
fn rand_simple() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Mix nanoseconds for pseudo-randomness
    (duration.as_nanos() as u32).wrapping_mul(1103515245).wrapping_add(12345)
}

fn detect_git_dir() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // Expand to absolute path
            std::path::Path::new(&dir)
                .canonicalize()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/dev/null".to_string())
        }
        _ => "/dev/null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectConfig, RalphConfig, SpecsConfig, PromptsConfig};
    use std::path::PathBuf;

    fn test_config() -> Config {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig {
                specs: SpecsConfig { dirs: vec![".ralph/specs".to_string()] },
                prompts: PromptsConfig { dir: ".ralph/prompts".to_string() },
            },
        };
        Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("cost-optimized".to_string()),
            None,
            project,
        )
        .unwrap()
    }

    #[test]
    fn system_prompt_contains_next_model_tag() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains("<next-model>"),
            "system prompt should document the <next-model> sigil"
        );
    }

    #[test]
    fn system_prompt_contains_all_three_model_names() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(prompt.contains("opus"), "system prompt should mention opus");
        assert!(
            prompt.contains("sonnet"),
            "system prompt should mention sonnet"
        );
        assert!(
            prompt.contains("haiku"),
            "system prompt should mention haiku"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_opus_example() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains("<next-model>opus</next-model>"),
            "system prompt should show opus example"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_sonnet_example() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains("<next-model>sonnet</next-model>"),
            "system prompt should show sonnet example"
        );
    }

    #[test]
    fn system_prompt_contains_next_model_haiku_example() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains("<next-model>haiku</next-model>"),
            "system prompt should show haiku example"
        );
    }

    #[test]
    fn system_prompt_contains_completion_sigils() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains("<promise>COMPLETE</promise>"),
            "system prompt should document COMPLETE sigil"
        );
        assert!(
            prompt.contains("<promise>FAILURE</promise>"),
            "system prompt should document FAILURE sigil"
        );
    }

    #[test]
    fn system_prompt_contains_prompt_file() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains(&config.prompt_file),
            "system prompt should reference the prompt file"
        );
    }

    #[test]
    fn system_prompt_contains_progress_db() {
        let config = test_config();
        let prompt = build_system_prompt(&config);
        assert!(
            prompt.contains(".ralph/progress.db"),
            "system prompt should reference the progress database"
        );
    }

    #[test]
    fn claude_args_contain_model_flag() {
        let config = test_config();
        let args = build_claude_args(&config);
        assert!(
            args.contains(&"--model".to_string()),
            "args should contain --model flag"
        );
    }

    #[test]
    fn claude_args_model_flag_followed_by_model_name() {
        let config = test_config();
        let args = build_claude_args(&config);
        let model_idx = args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            args[model_idx + 1], config.current_model,
            "args --model should be followed by the current model name"
        );
    }

    #[test]
    fn claude_args_model_reflects_fixed_strategy() {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig {
                specs: SpecsConfig { dirs: vec![".ralph/specs".to_string()] },
                prompts: PromptsConfig { dir: ".ralph/prompts".to_string() },
            },
        };
        let config = Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("fixed".to_string()),
            Some("opus".to_string()),
            project,
        )
        .unwrap();
        let cli_args = build_claude_args(&config);
        let model_idx = cli_args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            cli_args[model_idx + 1], "opus",
            "fixed strategy with --model=opus should pass opus to claude CLI"
        );
    }

    #[test]
    fn claude_args_model_reflects_escalate_strategy() {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig {
                specs: SpecsConfig { dirs: vec![".ralph/specs".to_string()] },
                prompts: PromptsConfig { dir: ".ralph/prompts".to_string() },
            },
        };
        let config = Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("escalate".to_string()),
            None,
            project,
        )
        .unwrap();
        let cli_args = build_claude_args(&config);
        let model_idx = cli_args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            cli_args[model_idx + 1], "haiku",
            "escalate strategy should initially pass haiku to claude CLI"
        );
    }

    #[test]
    fn claude_args_model_reflects_plan_then_execute_strategy() {
        let project = ProjectConfig {
            root: PathBuf::from("/test"),
            config: RalphConfig {
                specs: SpecsConfig { dirs: vec![".ralph/specs".to_string()] },
                prompts: PromptsConfig { dir: ".ralph/prompts".to_string() },
            },
        };
        let config = Config::from_run_args(
            None,
            false,
            false,
            None,
            vec![],
            Some("plan-then-execute".to_string()),
            None,
            project,
        )
        .unwrap();
        let cli_args = build_claude_args(&config);
        let model_idx = cli_args.iter().position(|a| a == "--model").unwrap();
        assert_eq!(
            cli_args[model_idx + 1], "opus",
            "plan-then-execute strategy should initially pass opus to claude CLI"
        );
    }

    // --- build_task_context tests ---

    #[test]
    fn task_context_with_all_fields() {
        let task = TaskInfo {
            task_id: "t-abc123".to_string(),
            title: "Implement feature X".to_string(),
            description: "Add the new feature X to the codebase.".to_string(),
            parent: Some(ParentContext {
                title: "Epic Y".to_string(),
                description: "The larger epic Y that encompasses this task.".to_string(),
            }),
            completed_blockers: vec![
                BlockerContext {
                    task_id: "t-prereq1".to_string(),
                    title: "Setup foundation".to_string(),
                    summary: "Created the base structure".to_string(),
                },
                BlockerContext {
                    task_id: "t-prereq2".to_string(),
                    title: "Add dependencies".to_string(),
                    summary: "Installed required packages".to_string(),
                },
            ],
            specs_dirs: vec!["specs/api".to_string(), "specs/infra".to_string()],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-abc123"));
        assert!(output.contains("**Title:** Implement feature X"));
        assert!(output.contains("### Description"));
        assert!(output.contains("Add the new feature X to the codebase."));
        assert!(output.contains("### Parent Context"));
        assert!(output.contains("**Parent:** Epic Y"));
        assert!(output.contains("The larger epic Y that encompasses this task."));
        assert!(output.contains("### Completed Prerequisites"));
        assert!(output.contains("- [t-prereq1] Setup foundation: Created the base structure"));
        assert!(output.contains("- [t-prereq2] Add dependencies: Installed required packages"));
        assert!(output.contains("### Reference Specs"));
        assert!(output.contains("Read all files in: specs/api, specs/infra"));
    }

    #[test]
    fn task_context_no_parent_omits_parent_section() {
        let task = TaskInfo {
            task_id: "t-xyz789".to_string(),
            title: "Standalone task".to_string(),
            description: "A task with no parent.".to_string(),
            parent: None,
            completed_blockers: vec![],
            specs_dirs: vec![".ralph/specs".to_string()],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-xyz789"));
        assert!(output.contains("**Title:** Standalone task"));
        assert!(!output.contains("### Parent Context"), "Should not contain parent section");
        assert!(!output.contains("**Parent:**"), "Should not contain parent field");
    }

    #[test]
    fn task_context_no_blockers_omits_prerequisites_section() {
        let task = TaskInfo {
            task_id: "t-def456".to_string(),
            title: "Initial task".to_string(),
            description: "A task with no prerequisites.".to_string(),
            parent: Some(ParentContext {
                title: "Parent task".to_string(),
                description: "Parent description.".to_string(),
            }),
            completed_blockers: vec![],
            specs_dirs: vec![".ralph/specs".to_string()],
        };

        let output = build_task_context(&task);

        assert!(output.contains("## Assigned Task"));
        assert!(output.contains("**ID:** t-def456"));
        assert!(output.contains("### Parent Context"));
        assert!(!output.contains("### Completed Prerequisites"), "Should not contain prerequisites section");
    }

    #[test]
    fn task_context_with_two_blockers() {
        let task = TaskInfo {
            task_id: "t-multi".to_string(),
            title: "Task with multiple blockers".to_string(),
            description: "Depends on two tasks.".to_string(),
            parent: None,
            completed_blockers: vec![
                BlockerContext {
                    task_id: "t-blocker1".to_string(),
                    title: "First blocker".to_string(),
                    summary: "Completed first".to_string(),
                },
                BlockerContext {
                    task_id: "t-blocker2".to_string(),
                    title: "Second blocker".to_string(),
                    summary: "Completed second".to_string(),
                },
            ],
            specs_dirs: vec![],
        };

        let output = build_task_context(&task);

        assert!(output.contains("### Completed Prerequisites"));
        assert!(output.contains("- [t-blocker1] First blocker: Completed first"));
        assert!(output.contains("- [t-blocker2] Second blocker: Completed second"));
    }

    #[test]
    fn task_context_verbatim_fields() {
        let task = TaskInfo {
            task_id: "t-verbatim-123".to_string(),
            title: "Special chars: <>&\"'".to_string(),
            description: "Description with\nnewlines and\ttabs.".to_string(),
            parent: None,
            completed_blockers: vec![],
            specs_dirs: vec![],
        };

        let output = build_task_context(&task);

        assert!(output.contains("**ID:** t-verbatim-123"));
        assert!(output.contains("**Title:** Special chars: <>&\"'"));
        assert!(output.contains("Description with\nnewlines and\ttabs."));
    }
}
