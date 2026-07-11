//! Shared domain types for chaos.
//!
//! This crate is the single source of truth for everything that crosses the
//! wire between `chaos-server` and its clients (web, desktop, future mobile).
//! It must stay lightweight and compile on both native and wasm targets:
//! no I/O, no async runtime, no framework dependencies.

pub mod api;
pub mod auth;
pub mod calendar;
pub mod dashboard;
pub mod home;
pub mod links;
pub mod search;
pub mod service;

pub use api::*;
pub use auth::*;
pub use calendar::*;
pub use dashboard::*;
pub use home::*;
pub use links::*;
pub use search::*;
pub use service::*;
