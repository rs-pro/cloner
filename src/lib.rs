//! Reusable building blocks for Cloner.
//!
//! The binary in `main.rs` is a thin CLI over these modules. Keeping them in a
//! library makes the database/container/backup primitives usable by the
//! internal backup tool without pulling in the JavaScript DSL or the CLI.

pub mod admin;
pub mod backup;
pub mod compose;
pub mod config;
pub mod db;
pub mod dsl;
pub mod engine;
pub mod k8s;
pub mod restore;
pub mod runtime;
pub mod shell;
pub mod ssh;
pub mod sync;
pub mod util;
