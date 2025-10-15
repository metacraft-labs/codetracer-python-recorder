use crate::runtime::io_capture::events::ProxyEvent;
use crate::runtime::io_capture::mute::is_io_capture_muted;
use crate::runtime::line_snapshots::LineSnapshotStore;
use pyo3::types::PyAnyMethods;
use pyo3::Python;
use runtime_tracing::Line;
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

fn populate_from_stack(py: Python<'_>, event: &mut ProxyEvent) {
    if event.line.is_some() && (event.path_id.is_some() || event.path.is_some()) {
        return;
    }

    let frame_result = (|| {
        let sys = py.import("sys")?;
        sys.getattr("_getframe")
    })();

    let getframe = match frame_result {
        Ok(obj) => obj,
        Err(_) => return,
    };

    for depth in [2_i32, 1, 0] {
        let frame_obj = match getframe.call1((depth,)) {
            Ok(frame) => frame,
            Err(_) => continue,
        };

        let frame = frame_obj;

        if event.line.is_none() {
            if let Ok(lineno) = frame
                .getattr("f_lineno")
                .and_then(|obj| obj.extract::<i32>())
            {
                event.line = Some(Line(lineno as i64));
            }
        }

        if event.path.is_none() {
            if let Ok(code) = frame.getattr("f_code") {
                if let Ok(filename) = code
                    .getattr("co_filename")
                    .and_then(|obj| obj.extract::<String>())
                {
                    event.path = Some(filename);
                }
            }
        }

        if event.frame_id.is_none() {
            let raw = frame.as_ptr() as usize as u64;
            event.frame_id = Some(crate::runtime::line_snapshots::FrameId::from_raw(raw));
        }

        if event.line.is_some() && (event.path_id.is_some() || event.path.is_some()) {
            break;
        }
    }
}
