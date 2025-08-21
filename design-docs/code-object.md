# Code Object Wrapper Design

## Overview

The Python Monitoring API delivers a generic `CodeType` object to every tracing callback.  The current `Tracer` trait surfaces this object as `&Bound<'_, PyAny>`, forcing every implementation to perform attribute lookups and type conversions manually.  This document proposes a `CodeObjectWrapper` type that exposes a stable, typed interface to the underlying code object while minimizing per-event overhead.

## Goals
- Provide a strongly typed API for common `CodeType` attributes needed by tracers and recorders.
- Ensure lookups are cheap by caching values and avoiding repeated Python attribute access.
- Maintain a stable identity for each code object to correlate events across callbacks.
- Avoid relying on the unstable `PyCodeObject` layout from the C API.

## Non-Goals
- Full re‑implementation of every `CodeType` attribute. Only the fields required for tracing and time‑travel debugging are exposed.
- Direct mutation of `CodeType` objects. The wrapper offers read‑only access.

## Proposed API

```rs
pub struct CodeObjectWrapper {
    /// Owned reference to the Python `CodeType` object.
    /// Stored as `Py<PyCode>` so it can be held outside the GIL.
    obj: Py<PyCode>,
    /// Stable identity equivalent to `id(code)`.
    id: usize,
    /// Lazily populated cache for expensive lookups.
    cache: CodeObjectCache,
}

pub struct CodeObjectCache {
    filename: OnceCell<String>,
    qualname: OnceCell<String>,
    firstlineno: OnceCell<u32>,
    argcount: OnceCell<u16>,
    flags: OnceCell<u32>,
    /// Mapping of instruction offsets to line numbers.
    lines: OnceCell<Vec<LineEntry>>,
}

pub struct LineEntry {
    pub offset: u32,
    pub line: u32,
}

impl CodeObjectWrapper {
    /// Construct from a `CodeType` object. Computes `id` eagerly.
    pub fn new(py: Python<'_>, obj: &Bound<'_, PyCode>) -> Self;

    /// Borrow the owned `Py<PyCode>` as a `Bound<'py, PyCode>`.
    /// This follows PyO3's recommendation to prefer `Bound<'_, T>` over `Py<T>`
    /// for object manipulation.
    pub fn as_bound<'py>(&'py self, py: Python<'py>) -> Bound<'py, PyCode>;

    /// Accessors fetch from the cache or perform a one‑time lookup under the GIL.
    pub fn filename<'py>(&'py self, py: Python<'py>) -> PyResult<&'py str>;
    pub fn qualname<'py>(&'py self, py: Python<'py>) -> PyResult<&'py str>;
    pub fn first_line(&self, py: Python<'_>) -> PyResult<u32>;
    pub fn arg_count(&self, py: Python<'_>) -> PyResult<u16>;
    pub fn flags(&self, py: Python<'_>) -> PyResult<u32>;

    /// Return the source line for a given instruction offset using a binary search on `lines`.
    pub fn line_for_offset(&self, py: Python<'_>, offset: u32) -> PyResult<Option<u32>>;

    /// Expose the stable identity for cross‑event correlation.
    pub fn id(&self) -> usize;
}
```

### Trait Integration

The `Tracer` trait will be adjusted so every callback receives `&CodeObjectWrapper` instead of a generic `&Bound<'_, PyAny>`:

```rs
fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32);
fn on_py_start(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32);
// ...and similarly for the remaining callbacks.
```

## Usage Examples

### Constructing the wrapper inside a tracer

```rs
fn on_line(&mut self, py: Python<'_>, code: &Bound<'_, PyCode>, lineno: u32) {
    let wrapper = CodeObjectWrapper::new(py, code);
    let filename = wrapper.filename(py).unwrap_or("<unknown>");
    eprintln!("{}:{}", filename, lineno);
}
```

### Reusing a cached wrapper

```rs
let wrapper = CodeObjectWrapper::new(py, code);
cache.insert(wrapper.id(), wrapper.clone());

if let Some(saved) = cache.get(&wrapper.id()) {
    let qualname = saved.qualname(py)?;
    println!("qualified name: {}", qualname);
}
```

## Performance Considerations
- `Py<PyCode>` allows cloning the wrapper without holding the GIL, enabling cheap event propagation.
- Methods bind the owned reference to `Bound<'py, PyCode>` on demand, following PyO3's `Bound`‑first guidance and avoiding accidental `Py` clones.
- Fields are loaded lazily and stored inside `OnceCell` containers to avoid repeated attribute lookups.
- `line_for_offset` memoizes the full line table the first time it is requested; subsequent calls perform an in‑memory binary search.
- Storing strings and small integers directly in the cache eliminates conversion cost on hot paths.

## Open Questions
- Additional attributes such as `co_consts` or `co_varnames` may be required for richer debugging features; these can be added later as new `OnceCell` fields.
- Thread‑safety requirements may necessitate wrapping the cache in `UnsafeCell` or providing internal mutability strategies compatible with `Send`/`Sync`.

## References
- [Python `CodeType` objects](https://docs.python.org/3/reference/datamodel.html#code-objects)
- [Python monitoring API](https://docs.python.org/3/library/sys.monitoring.html)
