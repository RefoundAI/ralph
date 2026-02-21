//! Shared types for ACP integration.
//!
//! Contains types used across the ACP modules:
//! - Context types migrated from `claude/client.rs` (kept there as well for now)
//! - New ACP-specific result types

use agent_client_protocol::StopReason;

// ---- Types copied from src/claude/client.rs ----
// Originals remain in claude/client.rs and will be removed in Phase 6.

/// Information about a task assigned to the agent for the current iteration.
pub struct TaskInfo {
    pub task_id: String,
    pub title: String,
    pub description: String,
    pub parent: Option<ParentContext>,
    pub completed_blockers: Vec<BlockerContext>,
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

/// Information about a retry attempt.
pub struct RetryInfo {
    pub attempt: i32,
    pub max_retries: i32,
    pub previous_failure_reason: String,
}

/// Full iteration context passed to the system prompt.
pub struct IterationContext {
    pub task: TaskInfo,
    pub spec_content: Option<String>,
    pub plan_content: Option<String>,
    pub retry_info: Option<RetryInfo>,
    /// Unique run ID for this invocation (format: run-{8 hex chars}).
    /// Used by the journal subsystem for grouping entries; read indirectly.
    #[allow(dead_code)]
    pub run_id: String,
    /// Pre-rendered markdown from journal::render_journal_context().
    pub journal_context: String,
    /// Pre-rendered markdown from knowledge::render_knowledge_context().
    pub knowledge_context: String,
}

// ---- New ACP-specific types ----

/// A knowledge entry parsed from a `<knowledge>` sigil in agent output.
///
/// Defined here (rather than sigils.rs) because it is referenced by `SigilResult`,
/// which lives in types.rs. When sigils.rs is populated it will import from here.
#[derive(Debug, Clone)]
pub struct KnowledgeSigil {
    pub title: String,
    pub tags: Vec<String>,
    pub body: String,
}

/// Result of running a single ACP iteration.
pub enum RunResult {
    /// The agent finished and produced a streaming result.
    Completed(StreamingResult),
    /// The user interrupted the iteration with Ctrl+C.
    Interrupted,
}

/// Data collected from a streaming ACP session.
pub struct StreamingResult {
    /// Accumulated full agent text output (for sigil extraction).
    pub full_text: String,
    /// File paths written during the session (from write_text_file calls).
    pub files_modified: Vec<String>,
    /// Iteration duration in milliseconds (wall clock, tracked by Ralph).
    pub duration_ms: u64,
    /// Why the agent stopped (EndTurn, MaxTokens, Refusal, etc.).
    pub stop_reason: StopReason,
}

/// All sigils extracted from a session's text output.
pub struct SigilResult {
    /// Task ID from `<task-done>...</task-done>` sigil, if present.
    pub task_done: Option<String>,
    /// Task ID from `<task-failed>...</task-failed>` sigil, if present.
    pub task_failed: Option<String>,
    /// Model hint from `<next-model>...</next-model>` sigil, if present.
    pub next_model_hint: Option<String>,
    /// Notes from `<journal>...</journal>` sigil, if present.
    pub journal_notes: Option<String>,
    /// Knowledge entries from `<knowledge>` sigils, if any.
    pub knowledge_entries: Vec<KnowledgeSigil>,
    /// True if `<promise>COMPLETE</promise>` was found.
    #[allow(dead_code)]
    pub is_complete: bool,
    /// True if `<promise>FAILURE</promise>` was found.
    pub is_failure: bool,
}
