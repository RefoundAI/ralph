//! Task ID generation.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Global counter for collision resolution.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a new task ID.
///
/// Format: `t-` + 6 hex chars from SHA-256 of `(timestamp_nanos || counter)`.
///
/// On collision (UNIQUE violation when inserting), the caller should retry
/// by calling this function again. This function increments an internal counter
/// on each call to reduce collision probability.
pub fn generate_task_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time is before UNIX epoch")
        .as_nanos();

    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);

    // Compute SHA-256 of timestamp || counter
    let mut hasher = Sha256::new();
    hasher.update(timestamp.to_le_bytes());
    hasher.update(counter.to_le_bytes());
    let hash = hasher.finalize();

    // Take first 3 bytes (6 hex chars)
    format!("t-{:02x}{:02x}{:02x}", hash[0], hash[1], hash[2])
}

/// Generate a task ID and insert it into the database.
///
/// Retries up to `max_retries` times on UNIQUE constraint violation.
pub fn generate_and_insert_task_id<F>(mut insert_fn: F, max_retries: usize) -> Result<String>
where
    F: FnMut(&str) -> Result<(), rusqlite::Error>,
{
    for attempt in 0..max_retries {
        let id = generate_task_id();
        match insert_fn(&id) {
            Ok(()) => return Ok(id),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                if attempt == max_retries - 1 {
                    anyhow::bail!(
                        "Failed to generate unique task ID after {} attempts",
                        max_retries
                    );
                }
                // Retry with incremented counter
                continue;
            }
            Err(e) => return Err(e).context("Failed to insert task"),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_id_format() {
        let id = generate_task_id();
        // Should match regex ^t-[0-9a-f]{6}$
        assert!(id.starts_with("t-"));
        assert_eq!(id.len(), 8); // "t-" + 6 hex chars = 8 total
        let hex_part = &id[2..];
        assert_eq!(hex_part.len(), 6);
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_no_duplicates_in_1000_generates() {
        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let id = generate_task_id();
            assert!(ids.insert(id), "Duplicate ID generated");
        }
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn test_collision_retry() {
        let mut call_count = 0;
        let mut seen_ids = HashSet::new();

        let insert_fn = |id: &str| {
            call_count += 1;
            if call_count < 3 {
                // Simulate collision on first two attempts
                seen_ids.insert(id.to_string());
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ErrorCode::ConstraintViolation,
                        extended_code: 0,
                    },
                    None,
                ))
            } else {
                // Succeed on third attempt
                Ok(())
            }
        };

        let result = generate_and_insert_task_id(insert_fn, 10);
        assert!(result.is_ok());
        assert_eq!(call_count, 3);
    }

    #[test]
    fn test_max_retries_exceeded() {
        let insert_fn = |_id: &str| {
            // Always fail with UNIQUE violation
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::ConstraintViolation,
                    extended_code: 0,
                },
                None,
            ))
        };

        let result = generate_and_insert_task_id(insert_fn, 3);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to generate unique task ID after 3 attempts"));
    }
}
