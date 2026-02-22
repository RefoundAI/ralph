//! `RalphClient`: implementation of the ACP `Client` trait.
//!
//! `RalphClient` is the tool-provider side of the ACP connection. It handles
//! file system and terminal requests from the agent, accumulates streamed text
//! for sigil extraction, and tracks files modified during the session.
//!
//! Design choice: `Rc<RefCell<>>` (not `Arc<Mutex<>>`) — all ACP futures are
//! `!Send` and everything runs on a single thread via `tokio::task::LocalSet`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use agent_client_protocol::{
    Client, ContentBlock, CreateTerminalRequest, CreateTerminalResponse,
    KillTerminalCommandRequest, KillTerminalCommandResponse, PermissionOptionKind,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, TerminalExitStatus, TerminalId,
    TerminalOutputRequest, TerminalOutputResponse, ToolKind, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};

use crate::acp::streaming::{self, RenderState};
use crate::acp::tools::{self, SessionUpdateMsg, TerminalSession};

/// Ralph's implementation of the ACP [`Client`] trait.
///
/// Handles tool requests from the agent:
/// - File reads and writes
/// - Terminal session management
/// - Permission requests (auto-approve or read-only deny)
/// - Session update notifications (streaming display + text accumulation)
/// Tracks a pending tool call waiting for its `raw_input` via `ToolCallUpdate`.
struct PendingToolCall {
    name: String,
    metadata_rendered: bool,
}

pub struct RalphClient {
    /// Project root directory; paths are resolved relative to this.
    project_root: PathBuf,
    /// Active terminal sessions, keyed by terminal ID string.
    terminals: Rc<RefCell<HashMap<String, TerminalSession>>>,
    /// Accumulated agent message text for post-session sigil extraction.
    text_accumulator: Rc<RefCell<String>>,
    /// File paths modified via `write_text_file`, normalized to be
    /// project-relative.
    files_modified: Rc<RefCell<Vec<String>>>,
    /// If `true`, file write requests are rejected.
    read_only: bool,
    /// If set, only these absolute paths may be written. All other writes
    /// are rejected with an error. Used for document-authoring sessions
    /// (spec, plan) to prevent the agent from writing source code.
    allowed_write_paths: Option<Vec<PathBuf>>,
    /// Model name for terminal display (e.g. "sonnet", "opus").
    model_name: String,
    /// Whether the next text chunk is the first after a tool call or session start.
    first_text_chunk: Rc<RefCell<bool>>,
    /// Buffer for accumulating partial lines of agent text before rendering.
    line_buffer: Rc<RefCell<String>>,
    /// Whether we are currently inside a fenced code block (``` toggled).
    in_code_block: Rc<RefCell<bool>>,
    /// Pending tool calls awaiting `raw_input` via `ToolCallUpdate`.
    pending_tool_calls: Rc<RefCell<HashMap<String, PendingToolCall>>>,
    /// Tracks whether we are inside a multi-line sigil tag.
    in_sigil: Rc<RefCell<Option<String>>>,
}

impl RalphClient {
    /// Create a new `RalphClient`.
    ///
    /// # Arguments
    ///
    /// * `project_root` — Absolute path to the project root directory.
    /// * `read_only` — If `true`, write requests are rejected (verification mode).
    /// * `model_name` — Model name for display in terminal output (e.g. "sonnet").
    pub fn new(project_root: PathBuf, read_only: bool, model_name: String) -> Self {
        Self {
            project_root,
            terminals: Rc::new(RefCell::new(HashMap::new())),
            text_accumulator: Rc::new(RefCell::new(String::new())),
            files_modified: Rc::new(RefCell::new(Vec::new())),
            read_only,
            allowed_write_paths: None,
            model_name,
            first_text_chunk: Rc::new(RefCell::new(true)),
            line_buffer: Rc::new(RefCell::new(String::new())),
            in_code_block: Rc::new(RefCell::new(false)),
            pending_tool_calls: Rc::new(RefCell::new(HashMap::new())),
            in_sigil: Rc::new(RefCell::new(None)),
        }
    }

    /// Build a `RenderState` for passing to `render_session_update()`.
    fn render_state(&self) -> RenderState {
        RenderState {
            model_name: self.model_name.clone(),
            is_first_chunk: Rc::clone(&self.first_text_chunk),
            line_buffer: Rc::clone(&self.line_buffer),
            in_code_block: Rc::clone(&self.in_code_block),
            in_sigil: Rc::clone(&self.in_sigil),
        }
    }

    /// Restrict file writes to only the specified paths.
    ///
    /// When set, `write_text_file` will reject any path not in the allowed list.
    /// Used for document-authoring sessions (spec, plan) to prevent the agent
    /// from writing source code.
    pub fn with_allowed_write_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.allowed_write_paths = Some(paths);
        self
    }

    /// Take and return all accumulated agent text, leaving the accumulator empty.
    pub fn take_accumulated_text(&self) -> String {
        let mut acc = self.text_accumulator.borrow_mut();
        std::mem::take(&mut *acc)
    }

    /// Take and return the list of files modified, leaving it empty.
    pub fn take_files_modified(&self) -> Vec<String> {
        let mut files = self.files_modified.borrow_mut();
        std::mem::take(&mut *files)
    }

    /// Kill all active terminal sessions and remove them from the map.
    ///
    /// Called during cleanup to prevent orphaned subprocesses after an
    /// iteration completes (or is interrupted). Each session's child process
    /// is killed and its reader tasks are aborted.
    pub async fn cleanup_all_terminals(&self) {
        // Drain the map first so we don't hold the borrow across await points.
        let sessions: Vec<tools::TerminalSession> = {
            let mut terminals = self.terminals.borrow_mut();
            terminals.drain().map(|(_, v)| v).collect()
        };
        for session in sessions {
            tools::release_terminal(session).await;
        }
    }

    /// Normalize `path` to be project-relative.
    ///
    /// If `path` is under `project_root`, strips the prefix and returns the
    /// relative path as a string. Otherwise returns the absolute path string.
    fn normalize_path(&self, path: &Path) -> String {
        match path.strip_prefix(&self.project_root) {
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) => path.to_string_lossy().into_owned(),
        }
    }

    /// Extract text from a `ContentBlock` if it is a text variant.
    fn content_block_text(block: &ContentBlock) -> Option<&str> {
        match block {
            ContentBlock::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    /// Determine whether a tool's `ToolKind` represents a write (mutating) operation.
    fn is_write_kind(kind: &ToolKind) -> bool {
        matches!(kind, ToolKind::Edit | ToolKind::Delete | ToolKind::Move)
    }
}

#[async_trait::async_trait(?Send)]
impl Client for RalphClient {
    // ------------------------------------------------------------------ //
    // Permission requests                                                  //
    // ------------------------------------------------------------------ //

    async fn request_permission(
        &self,
        req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        use agent_client_protocol::Error;

        // In read-only mode, deny write-typed tool calls.
        if self.read_only {
            if let Some(kind) = req.tool_call.fields.kind.as_ref() {
                if Self::is_write_kind(kind) {
                    // Find a reject option to return, or fall back to Cancelled.
                    let reject_option = req.options.iter().find(|opt| {
                        matches!(
                            opt.kind,
                            PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
                        )
                    });

                    let outcome = if let Some(opt) = reject_option {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                            opt.option_id.clone(),
                        ))
                    } else {
                        // No explicit reject option — use Cancelled to deny.
                        RequestPermissionOutcome::Cancelled
                    };

                    return Ok(RequestPermissionResponse::new(outcome));
                }
            }
        }

        // Normal mode (or read-only mode for non-write operations): auto-approve.
        // Pick the first AllowOnce option, then any allow option, or fail if none.
        let allow_option = req
            .options
            .iter()
            .find(|opt| matches!(opt.kind, PermissionOptionKind::AllowOnce))
            .or_else(|| {
                req.options.iter().find(|opt| {
                    matches!(
                        opt.kind,
                        PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                    )
                })
            });

        match allow_option {
            Some(opt) => {
                let outcome = RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    opt.option_id.clone(),
                ));
                Ok(RequestPermissionResponse::new(outcome))
            }
            None => {
                // No allow options available — this shouldn't normally happen.
                Err(Error::invalid_params().data(serde_json::json!(
                    "no allow option available in permission request"
                )))
            }
        }
    }

    // ------------------------------------------------------------------ //
    // Session notifications                                                 //
    // ------------------------------------------------------------------ //

    async fn session_notification(
        &self,
        notification: SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        let state = self.render_state();

        match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let Some(text) = Self::content_block_text(&chunk.content) {
                    // Accumulate for sigil extraction.
                    self.text_accumulator.borrow_mut().push_str(text);
                    // Render to terminal.
                    streaming::render_session_update(
                        &SessionUpdateMsg::AgentText(text.to_owned()),
                        &state,
                    );
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let Some(text) = Self::content_block_text(&chunk.content) {
                    streaming::render_session_update(
                        &SessionUpdateMsg::AgentThought(text.to_owned()),
                        &state,
                    );
                }
            }
            SessionUpdate::ToolCall(tool_call) => {
                let name = tool_call.title.clone();
                let has_raw_input = tool_call.raw_input.is_some();
                let input = tool_call
                    .raw_input
                    .as_ref()
                    .map(|v: &serde_json::Value| v.to_string())
                    .unwrap_or_default();
                let locations: Vec<String> = tool_call
                    .locations
                    .iter()
                    .map(|loc| loc.path.to_string_lossy().into_owned())
                    .collect();
                streaming::render_session_update(
                    &SessionUpdateMsg::ToolCall {
                        name: name.clone(),
                        input,
                        locations,
                    },
                    &state,
                );

                // Register for potential ToolCallUpdate with raw_input.
                let tool_call_id = tool_call.tool_call_id.0.as_ref().to_owned();
                self.pending_tool_calls.borrow_mut().insert(
                    tool_call_id,
                    PendingToolCall {
                        name,
                        metadata_rendered: has_raw_input,
                    },
                );
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let tool_call_id = update.tool_call_id.0.as_ref().to_owned();

                // If raw_input arrived and we haven't rendered metadata yet, render detail lines.
                if let Some(ref raw_input) = update.fields.raw_input {
                    let mut pending = self.pending_tool_calls.borrow_mut();
                    if let Some(entry) = pending.get_mut(&tool_call_id) {
                        if !entry.metadata_rendered {
                            entry.metadata_rendered = true;
                            let name = entry.name.clone();
                            drop(pending);
                            let detail_lines =
                                streaming::format_tool_detail_lines(&name, raw_input);
                            if !detail_lines.is_empty() {
                                streaming::render_session_update(
                                    &SessionUpdateMsg::ToolCallDetail { name, detail_lines },
                                    &state,
                                );
                            }
                        }
                    }
                }
            }
            SessionUpdate::Plan(_) => {
                // Plan updates are internal agent state; visible work shows via text + tool calls.
            }
            // CurrentModeUpdate, ConfigOptionUpdate, etc. are
            // silently accepted — no rendering needed for these.
            _ => {}
        }

        Ok(())
    }

    // ------------------------------------------------------------------ //
    // File system                                                           //
    // ------------------------------------------------------------------ //

    async fn read_text_file(
        &self,
        req: ReadTextFileRequest,
    ) -> agent_client_protocol::Result<ReadTextFileResponse> {
        use agent_client_protocol::Error;

        // Read the full file contents.
        let content = match std::fs::read_to_string(&req.path) {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::invalid_params().data(serde_json::json!(format!(
                    "file not found: {}",
                    req.path.display()
                ))));
            }
            Err(e) => {
                return Err(Error::internal_error().data(serde_json::json!(format!(
                    "failed to read file {}: {e}",
                    req.path.display()
                ))));
            }
        };

        // Apply optional line offset and limit.
        let result = if req.line.is_none() && req.limit.is_none() {
            content
        } else {
            let offset = req.line.map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
            let limit = req.limit.map(|l| l as usize).unwrap_or(usize::MAX);

            content
                .lines()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ReadTextFileResponse::new(result))
    }

    async fn write_text_file(
        &self,
        req: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        use agent_client_protocol::Error;

        if self.read_only {
            return Err(Error::invalid_params().data(serde_json::json!(
                "write_text_file is not allowed in read-only mode"
            )));
        }

        // If write paths are restricted, check the requested path against the allow-list.
        if let Some(ref allowed) = self.allowed_write_paths {
            let canonical = req.path.canonicalize().unwrap_or_else(|_| req.path.clone());
            let is_allowed = allowed.iter().any(|p| {
                let allowed_canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
                canonical == allowed_canonical
            });
            if !is_allowed {
                return Err(Error::invalid_params().data(serde_json::json!(format!(
                    "write not allowed: this session can only write to {}",
                    allowed
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))));
            }
        }

        // Create parent directories as needed.
        if let Some(parent) = req.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(Error::internal_error().data(serde_json::json!(format!(
                    "failed to create parent directories for {}: {e}",
                    req.path.display()
                ))));
            }
        }

        // Write the file.
        if let Err(e) = std::fs::write(&req.path, &req.content) {
            return Err(Error::internal_error().data(serde_json::json!(format!(
                "failed to write file {}: {e}",
                req.path.display()
            ))));
        }

        // Track the path (normalized to project-relative).
        let normalized = self.normalize_path(&req.path);
        self.files_modified.borrow_mut().push(normalized);

        Ok(WriteTextFileResponse::new())
    }

    // ------------------------------------------------------------------ //
    // Terminal management                                                   //
    // ------------------------------------------------------------------ //

    async fn create_terminal(
        &self,
        req: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        // Build the command string: command + args, shell-joined.
        let command_str = if req.args.is_empty() {
            req.command.clone()
        } else {
            format!("{} {}", req.command, req.args.join(" "))
        };

        let (terminal_id, session) = tools::create_terminal(&command_str);

        // Store the session in the map.
        self.terminals
            .borrow_mut()
            .insert(terminal_id.clone(), session);

        Ok(CreateTerminalResponse::new(TerminalId::new(terminal_id)))
    }

    async fn terminal_output(
        &self,
        req: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<TerminalOutputResponse> {
        use agent_client_protocol::Error;

        let terminal_id = req.terminal_id.0.as_ref().to_owned();
        let terminals = self.terminals.borrow();
        let session = terminals.get(&terminal_id).ok_or_else(|| {
            Error::invalid_params().data(serde_json::json!(format!(
                "terminal not found: {terminal_id}"
            )))
        })?;

        let output = tools::read_terminal_output(session);
        // `truncated` is false because we drain the full buffer.
        Ok(TerminalOutputResponse::new(output, false))
    }

    async fn wait_for_terminal_exit(
        &self,
        req: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        use agent_client_protocol::Error;

        let terminal_id = req.terminal_id.0.as_ref().to_owned();
        let mut terminals = self.terminals.borrow_mut();
        let session = terminals.get_mut(&terminal_id).ok_or_else(|| {
            Error::invalid_params().data(serde_json::json!(format!(
                "terminal not found: {terminal_id}"
            )))
        })?;

        let exit_code = tools::wait_for_exit(session).await;

        // Build exit status; exit_code is i32 from tools.rs (-1 = signal killed).
        let exit_status = if exit_code >= 0 {
            TerminalExitStatus::new().exit_code(exit_code as u32)
        } else {
            TerminalExitStatus::new()
        };

        Ok(WaitForTerminalExitResponse::new(exit_status))
    }

    async fn kill_terminal_command(
        &self,
        req: KillTerminalCommandRequest,
    ) -> agent_client_protocol::Result<KillTerminalCommandResponse> {
        use agent_client_protocol::Error;

        let terminal_id = req.terminal_id.0.as_ref().to_owned();
        let mut terminals = self.terminals.borrow_mut();
        let session = terminals.get_mut(&terminal_id).ok_or_else(|| {
            Error::invalid_params().data(serde_json::json!(format!(
                "terminal not found: {terminal_id}"
            )))
        })?;

        tools::kill_terminal(session).await;

        Ok(KillTerminalCommandResponse::new())
    }

    async fn release_terminal(
        &self,
        req: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        use agent_client_protocol::Error;

        let terminal_id = req.terminal_id.0.as_ref().to_owned();
        let session = self
            .terminals
            .borrow_mut()
            .remove(&terminal_id)
            .ok_or_else(|| {
                Error::invalid_params().data(serde_json::json!(format!(
                    "terminal not found: {terminal_id}"
                )))
            })?;

        tools::release_terminal(session).await;

        Ok(ReleaseTerminalResponse::new())
    }
}

// ========================================================================= //
// Unit tests                                                                 //
// ========================================================================= //

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::task::LocalSet;

    use agent_client_protocol::{
        PermissionOption, PermissionOptionId, SessionId, ToolCallId, ToolCallUpdate,
        ToolCallUpdateFields,
    };

    /// Helper macro: run an async block inside a LocalSet (required for spawn_local).
    macro_rules! with_local_set {
        ($body:expr) => {{
            let local = LocalSet::new();
            local.run_until($body).await;
        }};
    }

    fn make_client(tmp: &TempDir, read_only: bool) -> RalphClient {
        RalphClient::new(
            tmp.path().to_path_buf(),
            read_only,
            "test-model".to_string(),
        )
    }

    fn make_allow_option(id: &str) -> PermissionOption {
        PermissionOption::new(
            PermissionOptionId::new(id),
            "Allow",
            PermissionOptionKind::AllowOnce,
        )
    }

    fn make_reject_option(id: &str) -> PermissionOption {
        PermissionOption::new(
            PermissionOptionId::new(id),
            "Reject",
            PermissionOptionKind::RejectOnce,
        )
    }

    fn make_permission_request(
        options: Vec<PermissionOption>,
        tool_kind: Option<ToolKind>,
    ) -> RequestPermissionRequest {
        let mut fields = ToolCallUpdateFields::new();
        if let Some(kind) = tool_kind {
            fields = fields.kind(kind);
        }
        let tool_call = ToolCallUpdate::new(ToolCallId::new("tc-1"), fields);
        RequestPermissionRequest::new(SessionId::new("session-1"), tool_call, options)
    }

    // ------------------------------------------------------------------ //
    // read_text_file tests                                                  //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_read_text_file_basic() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("hello.txt");
        std::fs::write(&file_path, "Hello, world!\nLine 2\n").unwrap();

        let client = make_client(&tmp, false);
        let req = ReadTextFileRequest::new(SessionId::new("s"), file_path);
        let resp = client.read_text_file(req).await.unwrap();

        assert!(resp.content.contains("Hello, world!"));
        assert!(resp.content.contains("Line 2"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_read_text_file_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);
        let req = ReadTextFileRequest::new(SessionId::new("s"), tmp.path().join("nope.txt"));
        let result = client.read_text_file(req).await;

        assert!(result.is_err(), "expected error for nonexistent file");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_read_text_file_with_line_limit() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("multi.txt");
        std::fs::write(&file_path, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let client = make_client(&tmp, false);
        let req = ReadTextFileRequest::new(SessionId::new("s"), file_path)
            .line(2u32) // 1-based: start at line 2
            .limit(2u32); // read 2 lines
        let resp = client.read_text_file(req).await.unwrap();

        let content = resp.content;
        assert!(content.contains("line2"), "expected line2 in {content:?}");
        assert!(content.contains("line3"), "expected line3 in {content:?}");
        assert!(
            !content.contains("line1"),
            "should not contain line1 in {content:?}"
        );
        assert!(
            !content.contains("line4"),
            "should not contain line4 in {content:?}"
        );
    }

    // ------------------------------------------------------------------ //
    // write_text_file tests                                                 //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_write_text_file_basic() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);
        let file_path = tmp.path().join("output.txt");

        let req = WriteTextFileRequest::new(SessionId::new("s"), &file_path, "written content");
        client.write_text_file(req).await.unwrap();

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(read_back, "written content");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_write_text_file_creates_parents() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);
        let nested_path = tmp.path().join("a").join("b").join("c").join("file.txt");

        let req = WriteTextFileRequest::new(SessionId::new("s"), &nested_path, "nested");
        client.write_text_file(req).await.unwrap();

        assert!(nested_path.exists(), "nested file should exist");
        let content = std::fs::read_to_string(&nested_path).unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_write_text_file_read_only_rejected() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, true); // read_only = true
        let file_path = tmp.path().join("should-not-be-written.txt");

        let req = WriteTextFileRequest::new(SessionId::new("s"), &file_path, "secret");
        let result = client.write_text_file(req).await;

        assert!(
            result.is_err(),
            "write should be rejected in read-only mode"
        );
        assert!(!file_path.exists(), "file should not have been created");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_write_text_file_tracks_modified() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);
        let file_path = tmp.path().join("tracked.txt");

        let req = WriteTextFileRequest::new(SessionId::new("s"), &file_path, "content");
        client.write_text_file(req).await.unwrap();

        let modified = client.take_files_modified();
        assert_eq!(modified.len(), 1, "should track one modified file");
        // Should be project-relative (just the filename without tmp prefix)
        assert_eq!(modified[0], "tracked.txt");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_write_text_file_tracks_nested_relative() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);
        let file_path = tmp.path().join("sub").join("nested.txt");

        let req = WriteTextFileRequest::new(SessionId::new("s"), &file_path, "content");
        client.write_text_file(req).await.unwrap();

        let modified = client.take_files_modified();
        assert_eq!(modified.len(), 1);
        // Should be relative path within project root
        let rel = &modified[0];
        assert!(
            rel.contains("nested.txt"),
            "path should contain filename: {rel}"
        );
        // Should NOT contain the tmp dir absolute path prefix
        assert!(
            !rel.starts_with('/'),
            "path should be project-relative, got: {rel}"
        );
    }

    // ------------------------------------------------------------------ //
    // request_permission tests                                              //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_request_permission_auto_approve() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false); // normal mode

        let allow_opt = make_allow_option("allow-1");
        let req = make_permission_request(vec![allow_opt], None);
        let resp = client.request_permission(req).await.unwrap();

        match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.0.as_ref(), "allow-1");
            }
            other => panic!("expected Selected outcome, got: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_request_permission_auto_approve_prefers_allow_once() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);

        let reject_opt = make_reject_option("reject-1");
        let allow_opt = make_allow_option("allow-1");
        let req = make_permission_request(vec![reject_opt, allow_opt], None);
        let resp = client.request_permission(req).await.unwrap();

        match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.0.as_ref(), "allow-1");
            }
            other => panic!("expected Selected(allow-1), got: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_request_permission_read_only_denies_writes() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, true); // read_only = true

        let allow_opt = make_allow_option("allow-1");
        let reject_opt = make_reject_option("reject-1");
        let req = make_permission_request(
            vec![allow_opt, reject_opt],
            Some(ToolKind::Edit), // write-type operation
        );
        let resp = client.request_permission(req).await.unwrap();

        match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                // Should have selected the reject option, not the allow option.
                assert_eq!(sel.option_id.0.as_ref(), "reject-1");
            }
            RequestPermissionOutcome::Cancelled => {
                // Also acceptable — cancelled is effectively a deny.
            }
            #[allow(unreachable_patterns)]
            other => panic!("expected reject outcome, got: {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_request_permission_read_only_approves_reads() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, true); // read_only = true

        let allow_opt = make_allow_option("allow-1");
        let req = make_permission_request(
            vec![allow_opt],
            Some(ToolKind::Read), // read operation — should be allowed even in read-only mode
        );
        let resp = client.request_permission(req).await.unwrap();

        match resp.outcome {
            RequestPermissionOutcome::Selected(sel) => {
                assert_eq!(sel.option_id.0.as_ref(), "allow-1");
            }
            other => panic!("expected Selected(allow-1), got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------ //
    // Terminal tests                                                        //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_create_terminal_and_output() {
        let tmp = TempDir::new().unwrap();
        with_local_set!(async {
            let client = make_client(&tmp, false);

            let req = CreateTerminalRequest::new(SessionId::new("s"), "echo hello");
            let resp = client.create_terminal(req).await.unwrap();
            let terminal_id = resp.terminal_id;

            // Give the reader task time to collect output.
            tokio::time::sleep(Duration::from_millis(200)).await;

            let output_req = TerminalOutputRequest::new(SessionId::new("s"), terminal_id.clone());
            let output_resp = client.terminal_output(output_req).await.unwrap();
            assert!(
                output_resp.output.contains("hello"),
                "expected 'hello' in output: {:?}",
                output_resp.output
            );

            // Release the terminal.
            let release_req = ReleaseTerminalRequest::new(SessionId::new("s"), terminal_id);
            client.release_terminal(release_req).await.unwrap();

            // Map should be empty after release.
            assert!(client.terminals.borrow().is_empty());
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_kill_terminal() {
        let tmp = TempDir::new().unwrap();
        with_local_set!(async {
            let client = make_client(&tmp, false);

            let req = CreateTerminalRequest::new(SessionId::new("s"), "sleep 60");
            let resp = client.create_terminal(req).await.unwrap();
            let terminal_id = resp.terminal_id;

            let kill_req =
                KillTerminalCommandRequest::new(SessionId::new("s"), terminal_id.clone());
            client.kill_terminal_command(kill_req).await.unwrap();

            // After kill, we should be able to wait for exit promptly.
            let wait_req =
                WaitForTerminalExitRequest::new(SessionId::new("s"), terminal_id.clone());
            let _exit_resp = client.wait_for_terminal_exit(wait_req).await.unwrap();

            // Clean up.
            let release_req = ReleaseTerminalRequest::new(SessionId::new("s"), terminal_id);
            client.release_terminal(release_req).await.unwrap();
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_release_terminal_removes_from_map() {
        let tmp = TempDir::new().unwrap();
        with_local_set!(async {
            let client = make_client(&tmp, false);

            let req = CreateTerminalRequest::new(SessionId::new("s"), "echo cleanup");
            let resp = client.create_terminal(req).await.unwrap();
            let terminal_id = resp.terminal_id.clone();

            // Terminal should be in the map.
            assert!(client
                .terminals
                .borrow()
                .contains_key(terminal_id.0.as_ref()));

            let release_req = ReleaseTerminalRequest::new(SessionId::new("s"), terminal_id);
            client.release_terminal(release_req).await.unwrap();

            // Terminal should be removed.
            assert!(client.terminals.borrow().is_empty());
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_terminal_not_found_returns_error() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);

        let req = TerminalOutputRequest::new(SessionId::new("s"), TerminalId::new("nonexistent"));
        let result = client.terminal_output(req).await;
        assert!(result.is_err(), "should error for nonexistent terminal");
    }

    // ------------------------------------------------------------------ //
    // Accessor tests                                                        //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_take_accumulated_text() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);

        // Inject text directly via the notification path.
        client.text_accumulator.borrow_mut().push_str("hello world");

        let text = client.take_accumulated_text();
        assert_eq!(text, "hello world");

        // After taking, accumulator should be empty.
        let empty = client.take_accumulated_text();
        assert!(empty.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_take_files_modified() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false);

        // Write two files.
        let file1 = tmp.path().join("file1.txt");
        let file2 = tmp.path().join("file2.txt");

        client
            .write_text_file(WriteTextFileRequest::new(SessionId::new("s"), &file1, "a"))
            .await
            .unwrap();
        client
            .write_text_file(WriteTextFileRequest::new(SessionId::new("s"), &file2, "b"))
            .await
            .unwrap();

        let files = client.take_files_modified();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.contains("file1.txt")));
        assert!(files.iter().any(|f| f.contains("file2.txt")));

        // After taking, should be empty.
        assert!(client.take_files_modified().is_empty());
    }

    // ------------------------------------------------------------------ //
    // allowed_write_paths tests                                             //
    // ------------------------------------------------------------------ //

    #[tokio::test(flavor = "current_thread")]
    async fn test_allowed_write_paths_permits_listed_file() {
        let tmp = TempDir::new().unwrap();
        let allowed_path = tmp.path().join("plan.md");
        let client = RalphClient::new(tmp.path().to_path_buf(), false, "test-model".to_string())
            .with_allowed_write_paths(vec![allowed_path.clone()]);

        let req = WriteTextFileRequest::new(SessionId::new("s"), &allowed_path, "# Plan");
        Client::write_text_file(&client, req).await.unwrap();

        let content = std::fs::read_to_string(&allowed_path).unwrap();
        assert_eq!(content, "# Plan");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_allowed_write_paths_rejects_unlisted_file() {
        let tmp = TempDir::new().unwrap();
        let allowed_path = tmp.path().join("plan.md");
        let forbidden_path = tmp.path().join("src").join("main.rs");
        let client = RalphClient::new(tmp.path().to_path_buf(), false, "test-model".to_string())
            .with_allowed_write_paths(vec![allowed_path]);

        let req = WriteTextFileRequest::new(SessionId::new("s"), &forbidden_path, "fn main() {}");
        let result: agent_client_protocol::Result<WriteTextFileResponse> =
            Client::write_text_file(&client, req).await;

        assert!(result.is_err(), "write to unlisted path should be rejected");
        assert!(
            !forbidden_path.exists(),
            "file should not have been created"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_no_write_path_restriction_allows_any() {
        let tmp = TempDir::new().unwrap();
        let client = make_client(&tmp, false); // no allowed_write_paths set

        let file_path = tmp.path().join("anything.rs");
        let req = WriteTextFileRequest::new(SessionId::new("s"), &file_path, "content");
        client.write_text_file(req).await.unwrap();

        assert!(file_path.exists(), "file should have been written");
    }
}
