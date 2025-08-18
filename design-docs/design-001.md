# Python sys.monitoring Tracer Design

## Overview

This document outlines the design for integrating Python's `sys.monitoring` API with the `runtime_tracing` format. The goal is to produce CodeTracer-compatible traces for Python programs without modifying the interpreter.

The tracer collects `sys.monitoring` events, converts them to `runtime_tracing` events, and streams them to `trace.json`/`trace.bin` along with metadata and source snapshots.

## Architecture

### Tracer Abstraction
Rust code exposes a `Tracer` trait representing callbacks for Python
`sys.monitoring` events. Implementations advertise their desired events via an
`EventMask` bit flag returned from `interest`. A `Dispatcher` wraps a trait
object and forwards events only when the mask contains the corresponding flag,
allowing tracers to implement just the methods they care about.

### Tool Initialization
- Acquire a tool identifier via `sys.monitoring.use_tool_id`; store it for the lifetime of the tracer.
  ```rs
  pub const MONITORING_TOOL_NAME: &str = "codetracer";
  pub struct ToolId { pub id: u8 }
  pub fn acquire_tool_id() -> PyResult<ToolId>;
  ```
- Register one callback per event using `sys.monitoring.register_callback`.
  ```rs
  #[repr(transparent)]
  pub struct EventId(pub u64); // Exact value loaded from sys.monitoring.events.*

  pub struct MonitoringEvents {
      pub BRANCH: EventId,
      pub CALL: EventId,
      pub C_RAISE: EventId,
      pub C_RETURN: EventId,
      pub EXCEPTION_HANDLED: EventId,
      pub INSTRUCTION: EventId,
      pub JUMP: EventId,
      pub LINE: EventId,
      pub PY_RESUME: EventId,
      pub PY_RETURN: EventId,
      pub PY_START: EventId,
      pub PY_THROW: EventId,
      pub PY_UNWIND: EventId,
      pub PY_YIELD: EventId,
      pub RAISE: EventId,
      pub RERAISE: EventId,
      pub STOP_ITERATION: EventId,
  }

  pub fn load_monitoring_events(py: Python<'_>) -> PyResult<MonitoringEvents>;

  // Python-level callback registered via sys.monitoring.register_callback
  pub type CallbackFn = PyObject;
  pub fn register_callback(tool: &ToolId, event: &EventId, cb: &CallbackFn) -> PyResult<()>;
  ```
- Enable all desired events by bitmask with `sys.monitoring.set_events`.
  ```rs
  #[derive(Clone, Copy)]
  pub struct EventSet(pub u64);

  pub fn events_union(ids: &[EventId]) -> EventSet;
  pub fn set_events(tool: &ToolId, set: EventSet) -> PyResult<()>;
  ```

### Writer Management
- Open a `runtime_tracing` writer (`trace.json` or `trace.bin`) during `start_tracing`.
  ```rs
  pub enum OutputFormat { Json, Binary }
  pub struct TraceWriter { pub format: OutputFormat }
  pub fn start_tracing(path: &Path, format: OutputFormat) -> io::Result<TraceWriter>;
  ```
- Expose methods to append metadata and file copies using existing `runtime_tracing` helpers.
  ```rs
  pub fn append_metadata(writer: &mut TraceWriter, meta: &TraceMetadata);
  pub fn copy_source_file(writer: &mut TraceWriter, path: &Path) -> io::Result<()>;
  ```
- Flush and close the writer when tracing stops.
  ```rs
  pub fn stop_tracing(writer: TraceWriter) -> io::Result<()>;
  ```

### Frame and Thread Tracking
- Maintain a per-thread stack of activation identifiers to correlate `CALL`, `PY_START`, yields, and returns. Since `sys.monitoring` callbacks provide `CodeType` and offsets (not frames), we rely on the nesting order of events to track activations.
  ```rs
  pub type ActivationId = u64;
  pub struct ThreadState { pub stack: Vec<ActivationId> }
  pub fn current_thread_state() -> &'static mut ThreadState;
  ```
- Associate activations with `CodeType` objects and instruction/line offsets as needed for cross-referencing, without depending on `PyFrameObject`.
  ```rs
  pub struct Activation {
      pub id: ActivationId,
      // Hold a GIL-independent handle to the CodeType object.
      // Access required attributes via PyO3 attribute lookup (getattr) under the GIL.
      pub code: PyObject,
  }
  ```
- Record thread start/end events when a thread first emits a monitoring event and when it finishes.
  ```rs
  pub fn on_thread_start(thread_id: u64);
  pub fn on_thread_stop(thread_id: u64);
  ```

### Code Object Access Strategy (no reliance on PyCodeObject internals)
- Rationale: PyO3 exposes `ffi::PyCodeObject` as an opaque type. Instead of touching its unstable layout, treat code objects as generic Python objects and access only stable Python-level attributes via PyO3's `getattr` on `&PyAny`.
  ```rs
  use pyo3::{prelude::*, types::PyAny};

  #[derive(Clone)]
  pub struct CodeInfo {
      pub filename: String,
      pub qualname: String,
      pub firstlineno: u32,
      pub flags: u32,
  }

  /// Stable identity for a code object during its lifetime.
  /// Uses the object's address while GIL-held; equivalent to Python's id(code).
  pub fn code_id(py: Python<'_>, code: &PyAny) -> usize {
      code.as_ptr() as usize
  }

  /// Extract just the attributes we need, via Python attribute access.
  pub fn extract_code_info(py: Python<'_>, code: &PyAny) -> PyResult<CodeInfo> {
      let filename: String = code.getattr("co_filename")?.extract()?;
      // Prefer co_qualname if present, else fallback to co_name
      let qualname: String = match code.getattr("co_qualname") {
          Ok(q) => q.extract()?,
          Err(_) => code.getattr("co_name")?.extract()?,
      };
      let firstlineno: u32 = code.getattr("co_firstlineno")?.extract()?;
      let flags: u32 = code.getattr("co_flags")?.extract()?;
      Ok(CodeInfo { filename, qualname, firstlineno, flags })
  }

  /// Cache minimal info to avoid repeated getattr and to assign stable IDs.
  pub struct CodeRegistry {
      pub map: std::collections::HashMap<usize, CodeInfo>,
  }

  impl CodeRegistry {
      pub fn new() -> Self { Self { map: Default::default() } }
      pub fn intern(&mut self, py: Python<'_>, code: &PyAny) -> PyResult<usize> {
          let id = code_id(py, code);
          if !self.map.contains_key(&id) {
              let info = extract_code_info(py, code)?;
              self.map.insert(id, info);
          }
          Ok(id)
      }
  }
  ```
- Event handler inputs use `PyObject` for the `code` parameter. Borrow to `&PyAny` with `let code = code.bind(py);` when needed, then consult `CodeRegistry`.
- For line numbers: rely on the `LINE` event’s provided `line_number`. If instruction offsets need mapping, call `code.getattr("co_lines")()?.call0()?` and iterate lazily; avoid caching unless necessary.

## Event Handling

Each bullet below represents a low-level operation translating a single `sys.monitoring` event into the `runtime_tracing` stream.

### Control Flow
- **PY_START** – Create a `Function` event for the code object and push a new activation ID onto the thread's stack.
  ```rs
  pub fn on_py_start(code: PyObject, instruction_offset: i32);
  ```
- **PY_RESUME** – Emit an `Event` log noting resumption and update the current activation's state.
  ```rs
  pub fn on_py_resume(code: PyObject, instruction_offset: i32);
  ```
- **PY_RETURN** – Pop the activation ID, write a `Return` event with the value (if retrievable), and link to the caller.
  ```rs
  pub struct ReturnRecord { pub activation: ActivationId, pub value: Option<ValueRecord> }
  pub fn on_py_return(code: PyObject, instruction_offset: i32, retval: *mut PyObject);
  ```
- **PY_YIELD** – Record a `Return` event flagged as a yield and keep the activation on the stack for later resumes.
  ```rs
  pub fn on_py_yield(code: PyObject, instruction_offset: i32, retval: *mut PyObject);
  ```
- **STOP_ITERATION** – Emit an `Event` indicating iteration exhaustion for the current activation.
  ```rs
  pub fn on_stop_iteration(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```
- **PY_UNWIND** – Mark the beginning of stack unwinding and note the target handler in an `Event`.
  ```rs
  pub fn on_py_unwind(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```
- **PY_THROW** – Emit an `Event` describing the thrown value and the target generator/coroutine.
  ```rs
  pub fn on_py_throw(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```
- **RERAISE** – Log a re-raise event referencing the original exception.
  ```rs
  pub fn on_reraise(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```

### Call and Line Tracking
- **CALL** – Record a `Call` event, capturing the `callable` and the first argument if available (`arg0` as provided by `sys.monitoring`), and associate a new activation.
  ```rs
  pub fn on_call(code: PyObject, instruction_offset: i32, callable: *mut PyObject, arg0: Option<*mut PyObject>) -> ActivationId;
  ```
- **LINE** – Write a `Step` event with current path and line number; ensure the path is registered.
  ```rs
  pub fn on_line(code: PyObject, line_number: u32);
  ```
- **INSTRUCTION** – Optionally emit a fine-grained `Event` keyed by `instruction_offset`. Opcode names can be derived from the `CodeType` if desired.
  ```rs
  pub fn on_instruction(code: PyObject, instruction_offset: i32);
  ```
- **JUMP** – Append an `Event` describing the jump target offset for control-flow visualization.
  ```rs
  pub fn on_jump(code: PyObject, instruction_offset: i32, destination_offset: i32);
  ```
- **BRANCH** – Record an `Event` with `destination_offset`; whether the branch was taken can be inferred by comparing to the fallthrough offset.
  ```rs
  pub fn on_branch(code: PyObject, instruction_offset: i32, destination_offset: i32);
  ```
  _Note_: Current runtime_tracing doesn't support branching events, but instead relies on AST tree-sitter analysis. So for the initial version we will ignore them and can add support after modifications to the tracing format.

### Exception Lifecycle
- **RAISE** – Emit an `Event` containing exception type and message when raised.
  ```rs
  pub fn on_raise(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```
- **EXCEPTION_HANDLED** – Log an `Event` marking when an exception is caught.
  ```rs
  pub fn on_exception_handled(code: PyObject, instruction_offset: i32, exception: *mut PyObject);
  ```

### C API Boundary
- **C_RETURN** – On returning from a C function, emit a `Return` event tagged as foreign. Note: `sys.monitoring` does not provide the result object for `C_RETURN`.
  ```rs
  pub fn on_c_return(code: PyObject, instruction_offset: i32, callable: *mut PyObject, arg0: Option<*mut PyObject>);
  ```
- **C_RAISE** – When a C function raises, record an `Event` that a C-level callable raised. Note: `sys.monitoring` does not pass the exception object for `C_RAISE`.
  ```rs
  pub fn on_c_raise(code: PyObject, instruction_offset: i32, callable: *mut PyObject, arg0: Option<*mut PyObject>);
  ```

### No Events
- **NO_EVENTS** – Special constant; used only to disable monitoring. No runtime event is produced.
  ```rs
  pub const NO_EVENTS: EventSet = EventSet(0);
  ```

## Metadata and File Capture
- Collect the working directory, program name, and arguments and store them in `trace_metadata.json`.
  ```rs
  pub struct TraceMetadata { pub cwd: PathBuf, pub program: String, pub args: Vec<String> }
  pub fn write_metadata(writer: &mut TraceWriter, meta: &TraceMetadata);
  ```
- Track every file path referenced; copy each into the trace directory under `files/`.
  ```rs
  pub fn track_file(writer: &mut TraceWriter, path: &Path) -> io::Result<()>;
  ```
- Record `VariableName`, `Type`, and `Value` entries when variables are inspected or logged.
  ```rs
  pub struct VariableRecord { pub name: String, pub ty: TypeId, pub value: ValueRecord }
  pub fn record_variable(writer: &mut TraceWriter, rec: VariableRecord);
  ```

## Value Translation and Recording
- Maintain a type registry that maps Python `type` objects to `runtime_tracing` `Type` entries and assigns new `type_id` values on first encounter.
  ```rs
  pub type TypeId = u32;
  pub type ValueId = u64;
  pub enum ValueRecord { Int(i64), Float(f64), Bool(bool), None, Str(String), Raw(Vec<u8>), Sequence(Vec<ValueRecord>), Tuple(Vec<ValueRecord>), Struct(Vec<(String, ValueRecord)>), Reference(ValueId) }
  pub struct TypeRegistry { next: TypeId, map: HashMap<*mut PyTypeObject, TypeId> }
  pub fn intern_type(reg: &mut TypeRegistry, ty: *mut PyTypeObject) -> TypeId;
  ```
- Convert primitives (`int`, `float`, `bool`, `None`, `str`) directly to their corresponding `ValueRecord` variants.
  ```rs
  pub fn encode_primitive(obj: *mut PyObject) -> Option<ValueRecord>;
  ```
- Encode `bytes` and `bytearray` as `Raw` records containing base64 text to preserve binary data.
  ```rs
  pub fn encode_bytes(obj: *mut PyObject) -> ValueRecord;
  ```
- Represent lists and sets as `Sequence` records and tuples as `Tuple` records, converting each element recursively.
  ```rs
  pub fn encode_sequence(iter: &PySequence) -> ValueRecord;
  pub fn encode_tuple(tuple: &PyTupleObject) -> ValueRecord;
  ```
- Serialize dictionaries as a `Sequence` of two-element `Tuple` records for key/value pairs to avoid fixed field layouts.
  ```rs
  pub fn encode_dict(dict: &PyDictObject) -> ValueRecord;
  ```
- For objects with accessible attributes, emit a `Struct` record with sorted field names; fall back to `Raw` with `repr(obj)` when inspection is unsafe.
  ```rs
  pub fn encode_object(obj: *mut PyObject) -> ValueRecord;
  ```
- Track object identities to detect cycles and reuse `Reference` records with `id(obj)` for repeated structures.
  ```rs
  pub struct SeenSet { map: HashMap<usize, ValueId> }
  pub fn record_reference(seen: &mut SeenSet, obj: *mut PyObject) -> Option<ValueRecord>;
  ```

## Shutdown
- On `stop_tracing`, call `sys.monitoring.set_events` with `NO_EVENTS` for the tool ID.
  ```rs
  pub fn disable_events(tool: &ToolId);
  ```
- Unregister callbacks and free the tool ID with `sys.monitoring.free_tool_id`.
  ```rs
  pub fn unregister_callbacks(tool: ToolId);
  pub fn free_tool_id(tool: ToolId);
  ```
- Close the writer and ensure all buffered events are flushed to disk.
  ```rs
  pub fn finalize(writer: TraceWriter) -> io::Result<()>;
  ```

## Current Limitations
- **No structured support for threads or async tasks** – the trace format lacks explicit identifiers for concurrent execution.
  Distinguishing events emitted by different Python threads or `asyncio` tasks requires ad hoc `Event` entries, complicating
  analysis and preventing downstream tools from reasoning about scheduling.
- **Generic `Event` log** – several `sys.monitoring` notifications like resume, unwind, and branch outcomes have no dedicated
  `runtime_tracing` variant. They must be encoded as free‑form `Event` logs, which reduces machine readability and hinders
  automation.
- **Heavy value snapshots** – arguments and returns expect full `ValueRecord` structures. Serializing arbitrary Python objects is
  expensive and often degrades to lossy string dumps, limiting the visibility of rich runtime state.
- **Append‑only path and function tables** – `runtime_tracing` assumes files and functions are discovered once and never change.
  Dynamically generated code (`eval`, REPL snippets) forces extra bookkeeping and cannot update earlier entries, making
  dynamic features awkward to trace.
- **No built‑in compression or streaming** – traces are written as monolithic JSON or binary files. Long sessions quickly grow in
  size and cannot be streamed to remote consumers without additional tooling.

## Future Extensions
- Add filtering to enable subsets of events for performance-sensitive scenarios.
- Support streaming traces over a socket for live debugging.
