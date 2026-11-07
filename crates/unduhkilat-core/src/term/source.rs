//! Where a terminal's bytes come from: a local shell (ConPTY) or an SSH channel. Both hand the
//! session the same two things: a stream of events and a small control handle.

use crate::error::Result;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEvent {
    /// Output from the program.
    Data(Vec<u8>),
    /// The program or connection ended, with its exit status when known. Always the last event.
    Closed(Option<i32>),
}

pub trait SourceControl: Send + Sync {
    /// Send input to the program.
    fn write(&self, bytes: &[u8]) -> Result<()>;
    /// Tell the program the window size changed (clamped to at least 1x1).
    fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    /// Stop the program / close the channel. Safe to call more than once.
    fn close(&self);
}

pub struct SourceIo {
    pub rx: UnboundedReceiver<SourceEvent>,
    pub control: Arc<dyn SourceControl>,
}
