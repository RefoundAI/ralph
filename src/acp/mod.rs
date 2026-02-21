//! ACP (Agent Client Protocol) integration module.
//!
//! Replaces `src/claude/` with an agent-agnostic ACP client that communicates
//! with any ACP-compliant agent binary over stdin/stdout (JSON-RPC 2.0).

pub mod client_impl;
pub mod connection;
pub mod interactive;
pub mod prompt;
pub mod sigils;
pub mod streaming;
pub mod tools;
pub mod types;
