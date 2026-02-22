//! Tool handlers: terminal session management.
//!
//! Provides `TerminalSession` for managing subprocess lifecycles and buffered
//! I/O. Uses `Rc<RefCell<>>` (not `Arc<Mutex<>>`) because all ACP futures run
//! on a single thread via `tokio::task::LocalSet`. Reader tasks are spawned
//! with `spawn_local` for the same reason.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncReadExt;

/// Maximum size in bytes for each I/O buffer (1 MB).
const MAX_BUF_SIZE: usize = 1024 * 1024;

/// Monotonic counter for generating unique terminal IDs.
static TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A managed terminal subprocess with buffered stdout and stderr.
///
/// Reader tasks continuously drain the child's stdout/stderr into
/// `Rc<RefCell<Vec<u8>>>` buffers capped at 1 MB each (oldest bytes are
/// discarded when the cap is reached).
pub struct TerminalSession {
    pub(crate) child: tokio::process::Child,
    pub(crate) stdout_buf: Rc<RefCell<Vec<u8>>>,
    pub(crate) stderr_buf: Rc<RefCell<Vec<u8>>>,
    pub(crate) stdout_reader: tokio::task::JoinHandle<()>,
    pub(crate) stderr_reader: tokio::task::JoinHandle<()>,
}

/// Spawn a shell command and return a terminal ID plus a managed session.
///
/// The command is executed as `sh -c <command>` so shell features (pipes,
/// redirects, etc.) work correctly. Two `spawn_local` tasks continuously
/// drain stdout and stderr into 1 MB ring buffers.
///
/// Must be called from within a `tokio::task::LocalSet` context because
/// it calls `spawn_local` internally.
pub fn create_terminal(command: &str) -> (String, TerminalSession) {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn terminal command");

    let stdout = child.stdout.take().expect("child stdout not piped");
    let stderr = child.stderr.take().expect("child stderr not piped");

    let stdout_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let stderr_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

    let stdout_buf_clone = Rc::clone(&stdout_buf);
    let stderr_buf_clone = Rc::clone(&stderr_buf);

    let stdout_reader = tokio::task::spawn_local(async move {
        let mut stdout = stdout;
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let mut b = stdout_buf_clone.borrow_mut();
                    b.extend_from_slice(&buf[..n]);
                    if b.len() > MAX_BUF_SIZE {
                        let excess = b.len() - MAX_BUF_SIZE;
                        b.drain(..excess);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let stderr_reader = tokio::task::spawn_local(async move {
        let mut stderr = stderr;
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let mut b = stderr_buf_clone.borrow_mut();
                    b.extend_from_slice(&buf[..n]);
                    if b.len() > MAX_BUF_SIZE {
                        let excess = b.len() - MAX_BUF_SIZE;
                        b.drain(..excess);
                    }
                }
                Err(_) => break,
            }
        }
    });

    let id = TERMINAL_COUNTER.fetch_add(1, Ordering::SeqCst);
    let terminal_id = format!("terminal-{id}");

    let session = TerminalSession {
        child,
        stdout_buf,
        stderr_buf,
        stdout_reader,
        stderr_reader,
    };

    (terminal_id, session)
}

/// Drain all buffered output (stdout then stderr) from the session and return
/// it as a UTF-8 string. Buffers are cleared after reading.
pub fn read_terminal_output(session: &TerminalSession) -> String {
    let stdout_bytes: Vec<u8> = session.stdout_buf.borrow_mut().drain(..).collect();
    let stderr_bytes: Vec<u8> = session.stderr_buf.borrow_mut().drain(..).collect();

    let mut output = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if !stderr_str.is_empty() {
        output.push_str(&stderr_str);
    }
    output
}

/// Send SIGKILL to the child process.
pub async fn kill_terminal(session: &mut TerminalSession) {
    let _ = session.child.kill().await;
}

/// Kill the child process, abort reader tasks, and drop the session.
///
/// This is the preferred cleanup path — it prevents orphaned processes.
pub async fn release_terminal(mut session: TerminalSession) {
    let _ = session.child.kill().await;
    session.stdout_reader.abort();
    session.stderr_reader.abort();
    // `session` drops here, releasing all Rc buffers
}

/// Wait for the child process to exit and return its exit code.
///
/// Returns -1 if the exit status is unavailable (e.g. the process was
/// killed by a signal on Unix and provided no numeric code).
pub async fn wait_for_exit(session: &mut TerminalSession) -> i32 {
    match session.child.wait().await {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    }
}

// ---- Session update messaging ----

/// Messages that describe ACP session updates, used to decouple the
/// `Client` trait callbacks from terminal rendering.
#[allow(dead_code)]
pub enum SessionUpdateMsg {
    /// A chunk of agent-generated text content.
    AgentText(String),
    /// A chunk of agent reasoning / thought content.
    AgentThought(String),
    /// The agent invoked a tool.
    ToolCall { name: String, input: String },
    /// A tool call returned an error.
    ToolCallError { name: String, error: String },
    /// A tool call status/content update (streaming progress).
    ToolCallProgress {
        title: Option<String>,
        content: String,
    },
    /// The session has finished (prompt completed).
    Finished,
}

// ---- Unit tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::task::LocalSet;

    /// Helper: run an async block inside a LocalSet (required for spawn_local).
    macro_rules! with_local_set {
        ($body:expr) => {{
            let local = LocalSet::new();
            local.run_until($body).await;
        }};
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_terminal_create_and_output() {
        with_local_set!(async {
            let (id, session) = create_terminal("echo hello");

            // Terminal ID must be non-empty and contain the counter.
            assert!(!id.is_empty(), "terminal ID should not be empty");
            assert!(
                id.starts_with("terminal-"),
                "terminal ID should start with 'terminal-'"
            );

            // Give the reader task time to collect the output.
            tokio::time::sleep(Duration::from_millis(200)).await;

            let output = read_terminal_output(&session);
            assert!(
                output.contains("hello"),
                "expected 'hello' in output, got: {output:?}"
            );

            // Buffers should now be drained (cleared after read).
            let second_read = read_terminal_output(&session);
            assert!(
                second_read.is_empty(),
                "buffer should be empty after drain, got: {second_read:?}"
            );

            // Clean up.
            release_terminal(session).await;
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_terminal_kill() {
        with_local_set!(async {
            let (_, mut session) = create_terminal("sleep 60");

            // Kill the long-running process.
            kill_terminal(&mut session).await;

            // wait_for_exit should return promptly now that the child is dead.
            // The exit code will be -1 (signal) or some platform-specific value.
            let _exit_code = wait_for_exit(&mut session).await;

            // Just reaching here (without hanging indefinitely) verifies the kill worked.
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_terminal_release_cleanup() {
        with_local_set!(async {
            let (_, session) = create_terminal("echo cleanup test && sleep 10");

            // Allow a brief moment for output to be buffered.
            tokio::time::sleep(Duration::from_millis(50)).await;

            // release_terminal should kill the child and abort reader tasks.
            // If it hangs, the test framework will time out — that itself is a
            // meaningful signal that cleanup is broken.
            release_terminal(session).await;

            // Successfully reaching this point means resources were cleaned up.
        });
    }
}
