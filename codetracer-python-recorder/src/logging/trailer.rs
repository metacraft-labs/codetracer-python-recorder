use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::OnceCell;
use recorder_errors::RecorderError;

use super::logger;

static JSON_ERRORS_ENABLED: AtomicBool = AtomicBool::new(false);
static ERROR_TRAILER_WRITER: OnceCell<Mutex<Box<dyn Write + Send>>> = OnceCell::new();

pub(crate) fn set_json_errors_enabled(enabled: bool) {
    JSON_ERRORS_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn emit_error_trailer(err: &RecorderError) {
    if !JSON_ERRORS_ENABLED.load(Ordering::SeqCst) {
        return;
    }

    let Some((run_id, trace_id)) = logger::snapshot_run_and_trace() else {
        return;
    };

    let mut context = serde_json::Map::new();
    for (key, value) in &err.context {
        context.insert((*key).to_string(), serde_json::Value::String(value.clone()));
    }

    let payload = serde_json::json!({
        "run_id": run_id,
        "trace_id": trace_id,
        "error_code": err.code.as_str(),
        "error_kind": format!("{:?}", err.kind),
        "message": err.message(),
        "context": context,
    });

    if let Ok(mut bytes) = serde_json::to_vec(&payload) {
        bytes.push(b'\n');
        if let Some(writer) = ERROR_TRAILER_WRITER.get() {
            let mut guard = writer.lock().expect("error trailer writer lock");
            let _ = guard.write_all(&bytes);
            let _ = guard.flush();
        } else {
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(&bytes);
            let _ = stderr.flush();
        }
    }
}

#[cfg(test)]
pub fn set_error_trailer_writer_for_tests(writer: Box<dyn Write + Send>) {
    let _ = ERROR_TRAILER_WRITER.set(Mutex::new(writer));
}
