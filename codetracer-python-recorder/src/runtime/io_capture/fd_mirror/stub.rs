use crate::runtime::io_capture::events::IoStream;
use crate::runtime::io_capture::sink::IoChunkConsumer;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct FdMirrorError {
    message: String,
}

impl FdMirrorError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FdMirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FdMirrorError {}

#[derive(Default)]
pub struct MirrorLedgerSet;

pub struct LedgerTicket;

pub struct FdMirrorController;

impl MirrorLedgerSet {
    pub fn new() -> Self {
        Self
    }

    pub fn begin_proxy_write(&self, _: IoStream, _: &[u8]) -> Option<LedgerTicket> {
        None
    }
}

impl LedgerTicket {
    pub fn commit(self) {}
}

impl FdMirrorController {
    pub fn new(
        _: Arc<MirrorLedgerSet>,
        _: Arc<dyn IoChunkConsumer>,
    ) -> Result<Self, FdMirrorError> {
        Ok(Self)
    }

    pub fn shutdown(&mut self) {}
}
