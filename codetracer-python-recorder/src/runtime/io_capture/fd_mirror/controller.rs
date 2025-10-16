use super::ledger::MirrorLedgers;
use crate::runtime::io_capture::sink::IoChunkConsumer;
use std::sync::Arc;

#[cfg(not(unix))]
use super::stub as unix;
#[cfg(unix)]
use super::unix;

#[cfg(not(unix))]
pub use super::stub::FdMirrorError;
#[cfg(unix)]
pub use super::unix::FdMirrorError;

pub struct FdMirrorController {
    inner: Option<unix::FdMirrorController>,
}

impl FdMirrorController {
    pub fn new(
        ledgers: MirrorLedgers,
        consumer: Arc<dyn IoChunkConsumer>,
    ) -> Result<Self, FdMirrorError> {
        let inner = if let Some(set) = ledgers.inner() {
            Some(unix::FdMirrorController::new(set, consumer)?)
        } else {
            None
        };
        Ok(Self { inner })
    }

    pub fn shutdown(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner.shutdown();
        }
        self.inner = None;
    }
}

impl Drop for FdMirrorController {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            inner.shutdown();
        }
    }
}
