//! Ralph library — re-exports internal modules for integration testing.
//!
//! Integration tests in `tests/` cannot access items from a binary crate.
//! This `lib.rs` creates a library target alongside the binary so that
//! `tests/acp_integration.rs` can import `ralph::acp::connection::run_autonomous`, etc.
//!
//! **All application logic lives in the module files (src/acp/, src/config.rs, …).**
//! This file merely makes those modules reachable to external test crates.

#![allow(dead_code)]

pub mod acp;
pub mod cli;
pub mod config;
pub mod dag;
pub mod feature;
pub mod interrupt;
pub mod journal;
pub mod knowledge;
pub mod output;
pub mod project;
pub mod review;
pub mod run_loop;
pub mod strategy;
pub mod ui;
pub mod verification;
