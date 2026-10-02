use crate::runtime::io_capture::events::ProxyEvent;
use crate::runtime::io_capture::mute::is_io_capture_muted;
use crate::runtime::line_snapshots::LineSnapshotStore;
use crate::runtime::tracer::filtering::is_real_filename;
use codetracer_trace_types::Line;
use pyo3::types::PyAnyMethods;
use pyo3::Python;
use std::sync::Arc;

pub struct EventEnricher {
    snapshots: Arc<LineSnapshotStore>,
}

impl EventEnricher {
    pub fn new(snapshots: Arc<LineSnapshotStore>) -> Self {
        Self { snapshots }
    }

    pub fn enrich(&self, py: Python<'_>, mut event: ProxyEvent) -> Option<ProxyEvent> {
        if is_io_capture_muted() {
            return None;
        }

        if event.frame_id.is_none() || event.path_id.is_none() || event.line.is_none() {
            if let Some(snapshot) = self.snapshots.snapshot_for_thread(event.thread_id) {
                if event.frame_id.is_none() {
                    event.frame_id = Some(snapshot.frame_id());
                }
                if event.path_id.is_none() {
                    event.path_id = Some(snapshot.path_id());
                }
                if event.line.is_none() {
                    event.line = Some(snapshot.line());
                }
            }
        }

        if event.line.is_none() || (event.path_id.is_none() && event.path.is_none()) {
            populate_from_stack(py, &mut event);
        }

        Some(event)
    }
}

/// Attribute an event that no traced step on its thread has located to the
/// innermost frame whose code has a real source file. Frames of synthetic
/// code (`<string>`, `<frozen ...>`, `<stdin>`: the names the trace filter
/// already treats as synthetic and steps skip) are passed over, so output
/// written by `exec`'d code is attributed to the source frame that ran it,
/// and to no path at all when no such frame exists. A synthetic name is not
/// a source file, and naming one would give it a `paths.dat` record.
fn populate_from_stack(py: Python<'_>, event: &mut ProxyEvent) {
    if event.line.is_some() && (event.path_id.is_some() || event.path.is_some()) {
        return;
    }

    let Ok(mut frame) = py
        .import("sys")
        .and_then(|sys| sys.getattr("_getframe"))
        .and_then(|getframe| getframe.call1((0_i32,)))
    else {
        return;
    };

    for _ in 0..MAX_FRAMES_SEARCHED {
        if frame.is_none() {
            return;
        }
        let filename = frame
            .getattr("f_code")
            .and_then(|code| code.getattr("co_filename"))
            .and_then(|name| name.extract::<String>());
        if let Ok(filename) = filename {
            if is_real_filename(&filename) {
                if event.line.is_none() {
                    if let Ok(lineno) = frame
                        .getattr("f_lineno")
                        .and_then(|obj| obj.extract::<i32>())
                    {
                        event.line = Some(Line(lineno as i64));
                    }
                }
                if event.path.is_none() {
                    event.path = Some(filename);
                }
                if event.frame_id.is_none() {
                    let raw = frame.as_ptr() as usize as u64;
                    event.frame_id = Some(crate::runtime::line_snapshots::FrameId::from_raw(raw));
                }
                return;
            }
        }
        frame = match frame.getattr("f_back") {
            Ok(back) => back,
            Err(_) => return,
        };
    }
}

/// How far out the stack is searched for a frame with a real source file.
const MAX_FRAMES_SEARCHED: usize = 256;
