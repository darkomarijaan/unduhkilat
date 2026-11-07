//! UnduhKilat core: persistence, download engine, queue, IPC contracts.
//! Windows 10/11 x64 only (DEC-005); see docs/ for the authoritative specs.

#[cfg(windows)]
pub mod admin;
pub mod bridge;
pub mod category;
pub mod clipboard;
pub mod config;
pub mod db;
pub mod dpapi;
pub mod engine;
pub mod error;
pub mod ipc;
pub mod logging;
pub mod models;
pub mod native;
pub mod paths;
pub mod protocol;
pub mod repo;
pub mod update;

pub use error::{AppError, ErrorCode, Result};
