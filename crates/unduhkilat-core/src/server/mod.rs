//! Saved servers: SSH, SFTP, FTP and FTPS (design: docs/superpowers/specs/2026-11-07-server-client-design.md).

pub mod model;
pub mod store;
pub mod secrets;
pub mod hostkeys;
pub mod ssh;
pub mod service;
pub mod shell;
pub mod fs;
