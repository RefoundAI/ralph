//! Raw JSON logging for debugging.

use chrono::Utc;
use std::env;
use std::path::PathBuf;

/// Set up the log directory and return the log file path.
pub fn setup_log_file() -> String {
    let log_dir = log_directory();
    std::fs::create_dir_all(&log_dir).ok();

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let log_path = log_dir.join(format!("{}.log", timestamp));

    log_path.to_string_lossy().to_string()
}

/// Get the log directory path.
pub fn log_directory() -> PathBuf {
    let base_dir = env::var("TMPDIR")
        .or_else(|_| env::var("XDG_RUNTIME_DIR"))
        .unwrap_or_else(|_| "/tmp".to_string());

    let project_name = env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    PathBuf::from(base_dir)
        .join("ralph")
        .join("logs")
        .join(project_name)
}
