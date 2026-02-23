//! Integration tests for the ACP lifecycle using mock agent binaries.
//!
//! These tests exercise the full ACP client-server lifecycle:
//! - Agent spawning, initialization handshake, session creation
//! - Prompt / response cycle with sigil extraction
//! - Tool provider: file reads/writes, terminal management
//! - Read-only mode (verification agent)
//! - RALPH_MODEL env var passing
//!
//! **Requires the `test-mock-agents` feature to build the mock binaries.**
//! Run with:
//!   cargo test --features test-mock-agents -- acp_integration
//!
//! The mock agents are in tests/mock_agent.rs (basic) and
//! tests/mock_agent_tools.rs (tool-requesting variant).

use std::path::PathBuf;

use ralph::acp::connection::{run_autonomous, SessionRestrictions};
use ralph::acp::sigils::extract_sigils;
use tempfile::TempDir;

// ============================================================================
// Helpers
// ============================================================================

/// Navigate from the test binary to the Cargo `target/debug` (or `target/release`) directory.
///
/// When Cargo runs an integration test, `current_exe()` points to:
///   `target/debug/deps/<test-binary-name>`
///
/// Walking up two levels gives us `target/debug/`, which is where Cargo
/// places `[[example]]` outputs under `examples/`.
fn target_dir() -> PathBuf {
    let exe = std::env::current_exe().expect("could not read current_exe path");
    // exe → target/debug/deps/<test-binary>
    // .parent() → target/debug/deps
    // .parent() → target/debug
    exe.parent()
        .and_then(|deps| deps.parent())
        .map(|d| d.to_path_buf())
        .expect("could not navigate to target directory from current_exe")
}

/// Path to the compiled `mock-agent` example binary.
fn mock_agent_path() -> PathBuf {
    target_dir().join("examples").join("mock-agent")
}

/// Path to the compiled `mock-agent-tools` example binary.
fn mock_agent_tools_path() -> PathBuf {
    target_dir().join("examples").join("mock-agent-tools")
}

/// Quote a string for safe use in a shell-style command parsed by `shlex::split`.
///
/// Uses `shlex::try_quote` which returns an error only for nul-byte strings.
/// Test strings should never contain nul bytes, so `.unwrap()` is safe here.
fn sh_quote(s: &str) -> String {
    shlex::try_quote(s)
        .expect("no nul bytes in test string")
        .to_string()
}

/// Build an agent command string that sets `MOCK_RESPONSE` on the subprocess.
///
/// Uses `/usr/bin/env VAR=value cmd` to avoid modifying the test process's
/// environment (which would be a data race when tests run in parallel).
fn mock_agent_cmd(response: &str) -> String {
    let path = mock_agent_path();
    let quoted_path = sh_quote(path.to_str().expect("path must be UTF-8"));
    let quoted_response = sh_quote(response);
    format!("env MOCK_RESPONSE={} {}", quoted_response, quoted_path)
}

/// Build an agent command string for `mock-agent-tools` with optional tool env vars.
fn mock_agent_tools_cmd(
    response: &str,
    read_path: Option<&str>,
    write_path: Option<&str>,
    write_content: Option<&str>,
) -> String {
    let path = mock_agent_tools_path();
    let quoted_path = sh_quote(path.to_str().expect("path must be UTF-8"));
    let quoted_response = sh_quote(response);

    let mut parts = vec![
        "env".to_string(),
        format!("MOCK_RESPONSE={}", quoted_response),
    ];

    if let Some(rp) = read_path {
        parts.push(format!("MOCK_READ_PATH={}", sh_quote(rp)));
    }
    if let Some(wp) = write_path {
        parts.push(format!("MOCK_WRITE_PATH={}", sh_quote(wp)));
    }
    if let Some(wc) = write_content {
        parts.push(format!("MOCK_WRITE_CONTENT={}", sh_quote(wc)));
    }

    parts.push(quoted_path);
    parts.join(" ")
}

// ============================================================================
// Basic mock agent tests (pure text responses, no tools)
// ============================================================================

/// Full iteration: mock returns `<task-done>t-123</task-done>`.
/// Verifies the sigils are extracted and `task_done` is populated.
#[tokio::test(flavor = "current_thread")]
async fn test_iteration_with_mock_agent() {
    let tmp = TempDir::new().unwrap();
    let response = "<task-done>t-123</task-done>";
    let cmd = mock_agent_cmd(response);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed");

    // Full text should contain the agent's response verbatim.
    assert_eq!(
        result.full_text, response,
        "full_text mismatch: {:?}",
        result.full_text
    );

    // Sigil extraction should find task_done.
    let sigils = extract_sigils(&result.full_text);
    assert_eq!(
        sigils.task_done,
        Some("t-123".to_string()),
        "expected task_done = Some(\"t-123\")"
    );
    assert!(sigils.task_failed.is_none(), "task_failed should be None");
    assert!(!sigils.is_complete, "is_complete should be false");
    assert!(!sigils.is_failure, "is_failure should be false");
}

/// Mock returns `<task-failed>t-123</task-failed>`.
/// Verifies `task_failed` is extracted and `task_done` is absent.
#[tokio::test(flavor = "current_thread")]
async fn test_iteration_task_failed_sigil() {
    let tmp = TempDir::new().unwrap();
    let response = "<task-failed>t-123</task-failed>";
    let cmd = mock_agent_cmd(response);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed");

    let sigils = extract_sigils(&result.full_text);
    assert!(
        sigils.task_done.is_none(),
        "task_done should be None, got: {:?}",
        sigils.task_done
    );
    assert_eq!(
        sigils.task_failed,
        Some("t-123".to_string()),
        "expected task_failed = Some(\"t-123\")"
    );
}

/// Mock returns plain text with no sigils.
/// Verifies all sigil fields are absent/empty.
#[tokio::test(flavor = "current_thread")]
async fn test_iteration_no_sigil() {
    let tmp = TempDir::new().unwrap();
    let response = "Just some plain text with no special sigils at all.";
    let cmd = mock_agent_cmd(response);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed");

    assert!(
        result.full_text.contains("plain text"),
        "full_text should contain response: {:?}",
        result.full_text
    );

    let sigils = extract_sigils(&result.full_text);
    assert!(sigils.task_done.is_none(), "task_done should be None");
    assert!(sigils.task_failed.is_none(), "task_failed should be None");
    assert!(
        sigils.next_model_hint.is_none(),
        "next_model_hint should be None"
    );
    assert!(
        sigils.journal_notes.is_none(),
        "journal_notes should be None"
    );
    assert!(
        sigils.knowledge_entries.is_empty(),
        "knowledge_entries should be empty"
    );
    assert!(!sigils.is_complete, "is_complete should be false");
    assert!(!sigils.is_failure, "is_failure should be false");
}

/// Mock returns both `<journal>` and `<knowledge>` sigils.
/// Verifies both are correctly extracted from the streamed text.
#[tokio::test(flavor = "current_thread")]
async fn test_iteration_journal_and_knowledge() {
    let tmp = TempDir::new().unwrap();
    let response = concat!(
        "<journal>notes here</journal>",
        "<knowledge tags=\"test\" title=\"Test\">body</knowledge>"
    );
    let cmd = mock_agent_cmd(response);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed");

    let sigils = extract_sigils(&result.full_text);

    assert_eq!(
        sigils.journal_notes,
        Some("notes here".to_string()),
        "expected journal_notes = Some(\"notes here\")"
    );

    assert_eq!(
        sigils.knowledge_entries.len(),
        1,
        "expected exactly 1 knowledge entry"
    );
    let entry = &sigils.knowledge_entries[0];
    assert_eq!(entry.title, "Test", "knowledge title mismatch");
    assert_eq!(entry.body, "body", "knowledge body mismatch");
    assert!(
        entry.tags.contains(&"test".to_string()),
        "knowledge tags should contain 'test': {:?}",
        entry.tags
    );
}

/// Verify that `RALPH_MODEL` env var is set on the spawned agent process.
///
/// The mock agent echoes `RALPH_MODEL` back in its response when
/// `MOCK_RESPONSE` equals the sentinel string `"ECHO_RALPH_MODEL"`.
#[tokio::test(flavor = "current_thread")]
async fn test_iteration_model_env_passed() {
    let tmp = TempDir::new().unwrap();
    let expected_model = "test-model-for-env-check";

    // "ECHO_RALPH_MODEL" is a special sentinel: the mock agent reads RALPH_MODEL
    // from its own environment and emits that value as its response text.
    let cmd = mock_agent_cmd("ECHO_RALPH_MODEL");

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        Some(expected_model),
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed");

    assert!(
        result.full_text.contains(expected_model),
        "Expected RALPH_MODEL={expected_model} to appear in agent response, got: {:?}",
        result.full_text
    );
}

// ============================================================================
// Tool-requesting mock agent tests
// ============================================================================

/// Mock requests `fs/read_text_file` — Ralph serves the correct file content.
///
/// Verifies that the tool provider successfully reads and returns file contents
/// from disk when the agent requests it.
#[tokio::test(flavor = "current_thread")]
async fn test_agent_reads_file() {
    let tmp = TempDir::new().unwrap();

    // Create a file for the mock agent to read.
    let file_content = "hello from the file\nline 2";
    let file_path = tmp.path().join("test_file.txt");
    std::fs::write(&file_path, file_content).unwrap();

    let cmd = mock_agent_tools_cmd("read done", Some(file_path.to_str().unwrap()), None, None);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed with file read");

    // The mock agent successfully completes after the read (file content is silently
    // returned to the agent; the agent's response text comes from MOCK_RESPONSE).
    assert!(
        result.full_text.contains("read done"),
        "Expected MOCK_RESPONSE text, got: {:?}",
        result.full_text
    );
}

/// Mock requests `fs/write_text_file` — Ralph writes the file to disk and
/// tracks it in `files_modified`.
#[tokio::test(flavor = "current_thread")]
async fn test_agent_writes_file() {
    let tmp = TempDir::new().unwrap();
    let write_path = tmp.path().join("written_file.txt");
    let write_content = "content written by mock agent";

    let cmd = mock_agent_tools_cmd(
        "write done",
        None,
        Some(write_path.to_str().unwrap()),
        Some(write_content),
    );

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed with file write");

    // Verify the file was actually written to disk.
    assert!(
        write_path.exists(),
        "Expected the mock agent to write file at {:?}",
        write_path
    );
    let actual_content = std::fs::read_to_string(&write_path).unwrap();
    assert_eq!(
        actual_content, write_content,
        "File content mismatch after agent write"
    );

    // Verify the path is recorded in files_modified.
    assert!(
        !result.files_modified.is_empty(),
        "files_modified should be non-empty after a write"
    );
    let written_name = write_path.file_name().unwrap().to_str().unwrap();
    assert!(
        result
            .files_modified
            .iter()
            .any(|p| p.contains(written_name)),
        "Expected '{}' in files_modified: {:?}",
        written_name,
        result.files_modified
    );
}

/// Mock creates a terminal running `echo hello` — Ralph spawns the process
/// and the session completes successfully.
#[tokio::test(flavor = "current_thread")]
async fn test_agent_runs_terminal() {
    let tmp = TempDir::new().unwrap();
    let cmd = mock_agent_tools_cmd("terminal done", None, None, None);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions {
            allow_terminal: true,
            ..Default::default()
        },
    )
    .await
    .expect("run_autonomous should succeed with terminal creation");

    assert!(
        result.full_text.contains("terminal done"),
        "Expected MOCK_RESPONSE text after terminal, got: {:?}",
        result.full_text
    );
}

/// Read-only `RalphClient` returns an error for `fs/write_text_file` but
/// still allows terminal operations (matching verification agent behaviour).
///
/// The mock agent receives a tool-call error for the write request but
/// continues and completes the session with the configured MOCK_RESPONSE text.
#[tokio::test(flavor = "current_thread")]
async fn test_read_only_rejects_writes() {
    let tmp = TempDir::new().unwrap();
    let write_path = tmp.path().join("should_not_be_written.txt");

    let cmd = mock_agent_tools_cmd(
        "done despite write error",
        None,
        Some(write_path.to_str().unwrap()),
        Some("should not appear"),
    );

    // read_only = true means write_text_file calls are rejected.
    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        true,
        None,
        SessionRestrictions::default(),
    )
    .await
    .expect("run_autonomous should succeed even when writes are rejected");

    // The file must NOT have been created (write was rejected by the read-only client).
    assert!(
        !write_path.exists(),
        "File should not be written in read-only mode: {:?}",
        write_path
    );

    // Despite the write rejection, the agent still sends its text response.
    assert!(
        result.full_text.contains("done despite write error"),
        "Expected MOCK_RESPONSE text after write rejection, got: {:?}",
        result.full_text
    );
}

/// Mock creates a terminal and waits for it to exit.
/// Verifies that `wait_for_terminal_exit` completes correctly.
///
/// `mock_agent_tools` calls `create_terminal("echo hello")` then
/// `wait_for_terminal_exit`, so the echo process exits cleanly (code 0)
/// before the agent sends its final response.
#[tokio::test(flavor = "current_thread")]
async fn test_terminal_wait_for_exit() {
    let tmp = TempDir::new().unwrap();
    let cmd = mock_agent_tools_cmd("exit done", None, None, None);

    let result = run_autonomous(
        &cmd,
        tmp.path(),
        "instructions",
        "task message",
        false,
        None,
        SessionRestrictions {
            allow_terminal: true,
            ..Default::default()
        },
    )
    .await
    .expect("run_autonomous should succeed after terminal wait-for-exit");

    // The session must complete successfully after the terminal exits.
    assert!(
        result.full_text.contains("exit done"),
        "Expected MOCK_RESPONSE text after terminal exit, got: {:?}",
        result.full_text
    );
}
