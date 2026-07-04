//! Shared domain types for chaos.
//!
//! This crate is the single source of truth for everything that crosses the
//! wire between `chaos-server` and its clients (web, desktop, future mobile).
//! It must stay lightweight and compile on both native and wasm targets:
//! no I/O, no async runtime, no framework dependencies.

pub mod api;
pub mod dashboard;
pub mod links;
pub mod service;

pub use api::*;
pub use dashboard::*;
pub use links::*;
pub use service::*;
