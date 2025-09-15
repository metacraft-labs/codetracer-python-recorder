use once_cell::sync::OnceCell;
use pyo3::prelude::*;
use pyo3::types::PyCode;
use dashmap::DashMap;
use std::sync::Arc;

/// A wrapper around Python `code` objects providing cached access to
/// common attributes and line information.
pub struct CodeObjectWrapper {
    obj: Py<PyCode>,
    id: usize,
    cache: CodeObjectCache,
}

#[derive(Default)]
struct CodeObjectCache {
    filename: OnceCell<String>,
    qualname: OnceCell<String>,
    firstlineno: OnceCell<u32>,
    argcount: OnceCell<u16>,
    flags: OnceCell<u32>,
    lines: OnceCell<Vec<LineEntry>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEntry {
    pub offset: u32,
    pub line: u32,
}

impl CodeObjectWrapper {
    /// Construct from a `CodeType` object. Computes `id` eagerly.
    pub fn new(_py: Python<'_>, obj: &Bound<'_, PyCode>) -> Self {
        let id = obj.as_ptr() as usize;
        Self {
            obj: obj.clone().unbind(),
            id,
            cache: CodeObjectCache::default(),
        }
    }

    /// Borrow the owned `Py<PyCode>` as a `Bound<'py, PyCode>`.
    pub fn as_bound<'py>(&'py self, py: Python<'py>) -> &Bound<'py, PyCode> {
        self.obj.bind(py)
    }

    /// Return the stable identity of the code object (equivalent to `id(code)`).
    pub fn id(&self) -> usize {
        self.id
    }

    pub fn filename<'py>(&'py self, py: Python<'py>) -> PyResult<&'py str> {
        let value = self.cache.filename.get_or_try_init(|| -> PyResult<String> {
            let s: String = self
                .as_bound(py)
                .getattr("co_filename")?
                .extract()?;
            Ok(s)
        })?;
        Ok(value.as_str())
    }

    pub fn qualname<'py>(&'py self, py: Python<'py>) -> PyResult<&'py str> {
        let value = self.cache.qualname.get_or_try_init(|| -> PyResult<String> {
            let s: String = self
                .as_bound(py)
                .getattr("co_qualname")?
                .extract()?;
            Ok(s)
        })?;
        Ok(value.as_str())
    }

    pub fn first_line(&self, py: Python<'_>) -> PyResult<u32> {
        let value = *self.cache.firstlineno.get_or_try_init(|| -> PyResult<u32> {
            let v: u32 = self
                .as_bound(py)
                .getattr("co_firstlineno")?
                .extract()?;
            Ok(v)
        })?;
        Ok(value)
    }

    pub fn arg_count(&self, py: Python<'_>) -> PyResult<u16> {
        let value = *self.cache.argcount.get_or_try_init(|| -> PyResult<u16> {
            let v: u16 = self
                .as_bound(py)
                .getattr("co_argcount")?
                .extract()?;
            Ok(v)
        })?;
        Ok(value)
    }

    pub fn flags(&self, py: Python<'_>) -> PyResult<u32> {
        let value = *self.cache.flags.get_or_try_init(|| -> PyResult<u32> {
            let v: u32 = self
                .as_bound(py)
                .getattr("co_flags")?
                .extract()?;
            Ok(v)
        })?;
        Ok(value)
    }

    fn lines<'py>(&'py self, py: Python<'py>) -> PyResult<&'py [LineEntry]> {
        let vec = self.cache.lines.get_or_try_init(|| -> PyResult<Vec<LineEntry>> {
            let mut entries = Vec::new();
            let iter = self.as_bound(py).call_method0("co_lines")?;
            let iter = iter.try_iter()?;
            for item in iter {
                let (start, _end, line): (u32, u32, Option<u32>) = item?.extract()?;
                if let Some(line) = line {
                    entries.push(LineEntry { offset: start, line });
                }
            }
            Ok(entries)
        })?;
        Ok(vec.as_slice())
    }

    /// Return the source line for a given instruction offset using a binary search.
    pub fn line_for_offset(&self, py: Python<'_>, offset: u32) -> PyResult<Option<u32>> {
        let lines = self.lines(py)?;
        match lines.binary_search_by_key(&offset, |e| e.offset) {
            Ok(idx) => Ok(Some(lines[idx].line)),
            Err(0) => Ok(None),
            Err(idx) => Ok(Some(lines[idx - 1].line)),
        }
    }
}

/// Global registry caching `CodeObjectWrapper` instances by code object id.
#[derive(Default)]
pub struct CodeObjectRegistry {
    map: DashMap<usize, Arc<CodeObjectWrapper>>,
}

impl CodeObjectRegistry {
    /// Retrieve the wrapper for `code`, inserting a new one if needed.
    pub fn get_or_insert(
        &self,
        py: Python<'_>,
        code: &Bound<'_, PyCode>,
    ) -> Arc<CodeObjectWrapper> {
        let id = code.as_ptr() as usize;
        self.map
            .entry(id)
            .or_insert_with(|| Arc::new(CodeObjectWrapper::new(py, code)))
            .clone() //AI? Why do we need to clone here?
    }

    /// Remove the wrapper for a given code id, if present.
    pub fn remove(&self, id: usize) {
        self.map.remove(&id);
    }

    /// Clear all cached wrappers.
    pub fn clear(&self) {
        self.map.clear();
    }
}

