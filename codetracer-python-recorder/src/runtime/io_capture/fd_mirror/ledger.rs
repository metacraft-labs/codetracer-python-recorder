use crate::runtime::io_capture::events::IoStream;
use std::sync::Arc;

#[cfg(not(unix))]
use super::stub::MirrorLedgerSet;
#[cfg(unix)]
use super::unix::MirrorLedgerSet;

#[derive(Clone, Default)]
pub struct MirrorLedgers(Option<Arc<MirrorLedgerSet>>);

impl MirrorLedgers {
    pub fn new_enabled() -> Self {
        #[cfg(unix)]
        {
            Self(Some(Arc::new(MirrorLedgerSet::new())))
        }
        #[cfg(not(unix))]
        {
            Self(None)
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn begin_proxy_write(&self, stream: IoStream, payload: &[u8]) -> Option<LedgerTicket> {
        self.0
            .as_ref()
            .and_then(|inner| inner.begin_proxy_write(stream, payload))
    }

    pub(crate) fn inner(&self) -> Option<Arc<MirrorLedgerSet>> {
        self.0.clone()
    }
}

#[cfg(not(unix))]
pub use super::stub::LedgerTicket;
#[cfg(unix)]
pub use super::unix::LedgerTicket;
