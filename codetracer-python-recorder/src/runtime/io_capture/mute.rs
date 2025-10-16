use std::cell::Cell;

thread_local! {
    static IO_CAPTURE_MUTE_DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// RAII guard that suppresses IO capture while it is alive.
pub struct ScopedMuteIoCapture;

impl ScopedMuteIoCapture {
    pub fn new() -> Self {
        IO_CAPTURE_MUTE_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        ScopedMuteIoCapture
    }
}

impl Default for ScopedMuteIoCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScopedMuteIoCapture {
    fn drop(&mut self) {
        IO_CAPTURE_MUTE_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current.saturating_sub(1));
        });
    }
}

/// Returns true when IO capture should be bypassed for the current thread.
pub fn is_io_capture_muted() -> bool {
    IO_CAPTURE_MUTE_DEPTH.with(|depth| depth.get() > 0)
}
