# Code Object Wrapper (Quick Guide)

## Why this exists
- Python hands our tracer a raw `CodeType` object.
- Touching it directly is slow and messy because every field lookup goes through Python.
- We want one light wrapper that turns those lookups into cheap Rust calls.

## What we are building
- `CodeObjectWrapper` owns the `PyCode`, remembers its `id`, and lazily caches the handful of fields we actually use (file name, qualname, first line, arg count, flags, and line table).
- The cache lives in `OnceCell` slots so each field is fetched once per code object.
- Accessors hand back borrowed data (`Bound<'py, PyCode>`, `&str`, numbers) without cloning Python objects.

## How it is used
- Keep a global `CodeObjectRegistry` keyed by `id(code)`.
- `get_or_insert` returns an `Arc<CodeObjectWrapper>` so every callback reuses the same wrapper.
- The `Tracer` trait should accept `&CodeObjectWrapper` instead of `&Bound<'_, PyAny>`.
- Typical flow inside a callback:
  1. Ask the registry for the wrapper.
  2. Call `wrapper.filename(py)?` (or similar) and work with the cached values.

## Performance promises
- No repeat attribute fetches: the first call fills the cache, later calls are pure Rust reads.
- Wrappers clone cheaply across threads because they carry `Py<PyCode>` inside an `Arc`.
- Line lookups build the mapping once and then use a binary search.

## Edge notes
- The wrapper is read‑only; mutation stays out of scope.
- We only expose the fields we need today. Add more `OnceCell` slots later if new features demand them.
- Registry growth is unbounded right now—add eviction or weak references if long‑running tools need it.

## When we are done
- All tracer callbacks receive a `&CodeObjectWrapper`.
- Code that previously poked at `PyAny` now calls the typed helpers.
- Benchmarks show fewer Python attribute hits on hot paths.

## References
- [Python `CodeType` objects](https://docs.python.org/3/reference/datamodel.html#code-objects)
- [Python monitoring API](https://docs.python.org/3/library/sys.monitoring.html)
