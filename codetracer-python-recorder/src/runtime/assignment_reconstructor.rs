//! Reconstructs Python `Assignment` and `BindVariable` events from per-line
//! bytecode + frame snapshots.
//!
//! M15: `sys.monitoring` does not expose STORE_* opcodes directly. We
//! reconstruct the equivalent of an "Assignment event" by:
//!
//! 1. Pre-parsing each code object's instruction stream via `dis.get_instructions`
//!    into a per-line table of `(STORE_target, RValueShape, column_range)` tuples
//!    and caching the result keyed by `code.id()`.
//! 2. Each time `on_line` fires, replaying the previously recorded line's
//!    STOREs against the (cached) instruction table to classify the RHS
//!    shape from the bytecode immediately preceding each STORE_NAME /
//!    STORE_FAST.
//! 3. Emitting `BindVariable` the first time a name is observed in a frame,
//!    followed by an `Assignment` event with the chosen `RValue` variant.
//!
//! The classifier produces one of:
//!
//! * `RValue::Literal` — when the STORE was preceded only by `LOAD_CONST`
//!   (and a trivial `RETURN_CONST`-style pattern).
//! * `RValue::Simple(VariableId)` — when the STORE was preceded by exactly one
//!   `LOAD_FAST` / `LOAD_NAME` / `LOAD_GLOBAL`.
//! * `RValue::FieldAccess { receiver, field }` — `LOAD_NAME` + `LOAD_ATTR(field)`.
//! * `RValue::IndexAccess { receiver, index }` — `LOAD_NAME` + `LOAD_CONST(int)`
//!   + `BINARY_SUBSCR`, or `UNPACK_SEQUENCE` followed by sequential STOREs
//!   (in which case each STORE gets `IndexAccess { receiver: unpacked_var,
//!   index: i }`).
//! * `RValue::FunctionReturn { call_key }` — STORE was preceded by a `CALL` /
//!   `CALL_FUNCTION` /  `CALL_KW` opcode.
//! * `RValue::Compound(vars)` — anything else where 1+ local loads contributed
//!   to the RHS expression (e.g. `total = a + b`).
//!
//! ## Column extraction
//!
//! Per the milestone deliverable, Python 3.11+ exposes per-instruction
//! `(col_offset, end_col_offset)` via `code.co_positions()`. We surface the
//! first STORE's column (1-based, matching `Line`'s convention) so
//! `StepRecord.column` is populated when possible.
//!
//! ## Pipeline placement
//!
//! The reconstructor consumes the bytecode metadata once per code object and
//! pulls per-line records out of an `Arc`-shared cache. The hot path is
//! deliberately allocation-light: the per-line lookup is an O(1)
//! `HashMap<u32, Vec<LineAssignment>>` index.

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PyTuple};
use std::collections::HashMap;
use std::sync::Arc;

use crate::code_object::CodeObjectWrapper;

/// Identifies how the RHS of an assignment was constructed.
#[derive(Debug, Clone, PartialEq)]
pub enum RValueShape {
    /// RHS was a constant literal.
    Literal,
    /// RHS loaded exactly one local/name/global.
    Simple { source: String },
    /// RHS was a field access `receiver.field`.
    FieldAccess { receiver: String, field: String },
    /// RHS was an integer-indexed subscript `receiver[index]`.
    IndexAccess { receiver: String, index: i64 },
    /// RHS was a CALL whose result was captured.
    FunctionReturn,
    /// RHS combined multiple local loads (arithmetic, format, etc.).
    Compound { sources: Vec<String> },
    /// Unable to classify the RHS (e.g. the STORE landed without a known
    /// load sequence — control flow merge, deleted instruction).
    Unknown,
}

/// A single store observed on a given source line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineAssignment {
    /// Name the STORE bound (e.g. `a`, `total`).
    pub target: String,
    /// Source-language shape of the RHS.
    pub rvalue: RValueShape,
    /// 1-based column where the target identifier begins, if recoverable
    /// from `co_positions`.
    pub column: Option<u32>,
}

/// Per-code-object cached bytecode table.
///
/// Maps `source line number -> list of stores on that line`. Built once on
/// first observation of a code object.
#[derive(Debug, Clone, Default)]
pub struct LineAssignmentTable {
    by_line: HashMap<u32, Vec<LineAssignment>>,
}

impl LineAssignmentTable {
    /// Stores recorded for `line`, or an empty slice if none.
    pub fn for_line(&self, line: u32) -> &[LineAssignment] {
        self.by_line.get(&line).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// First column among the stores on `line` (lowest column wins, mirrors
    /// the leftmost target identifier on the line).
    pub fn first_column_for_line(&self, line: u32) -> Option<u32> {
        self.by_line
            .get(&line)
            .and_then(|stores| stores.iter().filter_map(|s| s.column).min())
    }
}

/// Cache of per-code-object instruction tables.
#[derive(Default)]
pub struct AssignmentReconstructor {
    cache: HashMap<usize, Arc<LineAssignmentTable>>,
}

impl AssignmentReconstructor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up or build the line-assignment table for `code`.
    ///
    /// Returns an `Arc` so callers can hold the table while the cache is
    /// mutated for other code objects.
    pub fn table_for(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<Arc<LineAssignmentTable>> {
        let id = code.id();
        if let Some(existing) = self.cache.get(&id) {
            return Ok(Arc::clone(existing));
        }
        let table = Arc::new(build_table(py, code)?);
        self.cache.insert(id, Arc::clone(&table));
        Ok(table)
    }

    /// Forget the cached table for `code` (e.g. when its code object is
    /// being torn down). Safe to call when no entry exists.
    pub fn forget(&mut self, code_id: usize) {
        self.cache.remove(&code_id);
    }

    /// Drop every cached table.
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

/// Disassemble `code` via `dis.get_instructions` and group STORE_* opcodes
/// by source line, deriving the `RValueShape` of each from the immediately
/// preceding LOAD/CALL window.
fn build_table(py: Python<'_>, code: &CodeObjectWrapper) -> PyResult<LineAssignmentTable> {
    let dis_module = py.import("dis")?;
    let instructions = dis_module.call_method1("get_instructions", (code.as_bound(py),))?;
    let instr_list: Bound<'_, PyList> = PyList::empty(py);
    for item in instructions.try_iter()? {
        instr_list.append(item?)?;
    }

    let mut decoded: Vec<DecodedInstruction> = Vec::with_capacity(instr_list.len());
    for instr in instr_list.iter() {
        let opname: String = instr.getattr("opname")?.extract()?;
        let argval = instr.getattr("argval")?;
        // `Instruction.positions` is a `Positions(lineno, end_lineno,
        // col_offset, end_col_offset)` namedtuple, stable since Python 3.11
        // and populated for every instruction (not just line starts). We
        // intentionally do NOT use `Instruction.line_number` — that property
        // was added in Python 3.13 and is missing on 3.12, where it would
        // resolve to None for every instruction and collapse the per-line
        // grouping the classifier relies on.
        let positions = instr.getattr("positions").ok();
        let (current_line, col_offset, end_col_offset) = if let Some(pos) = positions {
            let l: Option<u32> = pos.getattr("lineno").ok().and_then(|v| v.extract().ok());
            let c: Option<u32> = pos
                .getattr("col_offset")
                .ok()
                .and_then(|v| v.extract().ok());
            let e: Option<u32> = pos
                .getattr("end_col_offset")
                .ok()
                .and_then(|v| v.extract().ok());
            (l, c, e)
        } else {
            (None, None, None)
        };

        let argval_kind = classify_argval(py, &argval);

        decoded.push(DecodedInstruction {
            opname,
            argval_kind,
            line: current_line,
            col_offset,
            _end_col_offset: end_col_offset,
        });
    }

    let mut by_line: HashMap<u32, Vec<LineAssignment>> = HashMap::new();
    let mut i = 0;
    while i < decoded.len() {
        let op = &decoded[i];
        if !is_store_op(&op.opname) {
            i += 1;
            continue;
        }
        let target = match &op.argval_kind {
            ArgValKind::Name(s) => s.clone(),
            _ => {
                i += 1;
                continue;
            }
        };
        // Walk backwards from the STORE through the same line's instruction
        // window to assemble an RValue classification.
        let line = op.line.unwrap_or(0);
        // Look ahead to detect the special UNPACK_SEQUENCE pattern: the STOREs
        // emitted after an UNPACK_SEQUENCE share an RHS, so we generate one
        // IndexAccess per STORE keyed by sequential index.
        let (rvalue, advance) =
            classify_rvalue(&decoded, i, line).unwrap_or((RValueShape::Unknown, 1));

        let column = op.col_offset.map(|c| c + 1);
        by_line.entry(line).or_default().push(LineAssignment {
            target,
            rvalue,
            column,
        });

        // For the UNPACK_SEQUENCE pattern, multiple consecutive STOREs share
        // the same source (`receiver` of the unpacked LOAD). The classifier
        // returns advance > 1 only when it consumed *additional* STOREs in
        // this call; otherwise advance is 1 and we proceed to the next op.
        if advance > 1 {
            // Pull the trailing STOREs the classifier already handled.
            // We need to emit a LineAssignment for *each* of them. The
            // classifier returned the first one already (above), and `advance`
            // is the total count of consumed STOREs.
            for offset in 1..advance {
                let store = &decoded[i + offset];
                if !is_store_op(&store.opname) {
                    break;
                }
                let inner_target = match &store.argval_kind {
                    ArgValKind::Name(s) => s.clone(),
                    _ => continue,
                };
                // Safe: we only enter this branch when the surrounding loop
                // has already pushed at least one entry onto by_line[&line]
                // for this UNPACK_SEQUENCE expansion. If the entry is
                // missing for any reason, skip this STORE rather than panic.
                let Some(prior) = by_line.get(&line).and_then(|v| v.last()) else {
                    continue;
                };
                let receiver = match &prior.rvalue {
                    RValueShape::IndexAccess { receiver, .. } => receiver.clone(),
                    _ => continue,
                };
                let column = store.col_offset.map(|c| c + 1);
                by_line.entry(line).or_default().push(LineAssignment {
                    target: inner_target,
                    rvalue: RValueShape::IndexAccess {
                        receiver,
                        index: offset as i64,
                    },
                    column,
                });
            }
            i += advance;
        } else {
            i += 1;
        }
    }

    Ok(LineAssignmentTable { by_line })
}

/// Internal disassembled-instruction record.
#[derive(Debug, Clone)]
struct DecodedInstruction {
    opname: String,
    argval_kind: ArgValKind,
    line: Option<u32>,
    col_offset: Option<u32>,
    _end_col_offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
enum ArgValKind {
    /// Identifier name (LOAD_NAME / STORE_NAME / LOAD_FAST / STORE_FAST /
    /// LOAD_GLOBAL / LOAD_ATTR argval).
    Name(String),
    /// Integer literal (`LOAD_CONST n`).
    Int(i64),
    /// Non-integer constant or "anything else" we don't care about.
    Other,
    /// Argval was None / not extractable.
    None,
}

fn classify_argval(py: Python<'_>, argval: &Bound<'_, PyAny>) -> ArgValKind {
    let _ = py;
    if argval.is_none() {
        return ArgValKind::None;
    }
    if let Ok(s) = argval.extract::<String>() {
        return ArgValKind::Name(s);
    }
    if let Ok(i) = argval.extract::<i64>() {
        return ArgValKind::Int(i);
    }
    // Some opcodes (LOAD_GLOBAL with the NULL flag in 3.11+) wrap the name
    // in a tuple `(push_null: bool, name: str)`. Pull the string out so
    // we can still recognise the underlying identifier.
    if let Ok(tup) = argval.downcast::<PyTuple>() {
        for item in tup.iter() {
            if let Ok(s) = item.extract::<String>() {
                return ArgValKind::Name(s);
            }
        }
    }
    ArgValKind::Other
}

fn is_store_op(opname: &str) -> bool {
    matches!(
        opname,
        "STORE_NAME" | "STORE_FAST" | "STORE_GLOBAL" | "STORE_DEREF" | "STORE_FAST_STORE_FAST"
    )
}

fn is_load_op(opname: &str) -> bool {
    matches!(
        opname,
        "LOAD_NAME" | "LOAD_FAST" | "LOAD_GLOBAL" | "LOAD_DEREF" | "LOAD_CLASSDEREF"
    )
}

fn is_const_op(opname: &str) -> bool {
    matches!(opname, "LOAD_CONST" | "RETURN_CONST")
}

fn is_call_op(opname: &str) -> bool {
    matches!(
        opname,
        "CALL"
            | "CALL_KW"
            | "CALL_FUNCTION"
            | "CALL_FUNCTION_EX"
            | "CALL_FUNCTION_KW"
            | "CALL_METHOD"
    )
}

/// Walk the bytecode window for the STORE at `store_idx` backwards within the
/// same line to classify the RHS shape. Returns `(shape, advance)` where
/// `advance` indicates how many STORE_* instructions were consumed by this
/// classification (1 = just the current STORE; >1 only for UNPACK_SEQUENCE
/// destructuring which binds N targets at once).
fn classify_rvalue(
    decoded: &[DecodedInstruction],
    store_idx: usize,
    line: u32,
) -> Option<(RValueShape, usize)> {
    // Pass 1: detect UNPACK_SEQUENCE destructuring shape:
    //   LOAD_* <receiver>
    //   UNPACK_SEQUENCE <count>
    //   STORE_* <target_0>
    //   STORE_* <target_1>
    //   ...
    //
    // The "current" STORE is `store_idx`. Walk back to find the LOAD that
    // pushed the iterable. If we find UNPACK_SEQUENCE in that window, this
    // STORE is index 0 of the destructure and the classifier will return the
    // total count of trailing STOREs to consume.
    if let Some(unpack_info) = detect_unpack_sequence(decoded, store_idx, line) {
        return Some(unpack_info);
    }

    // Pass 2: walk back through the same line, looking for the producer of
    // the value the STORE pops. We classify the window by looking at the
    // ordered sequence of (LOAD_CONST | LOAD_*) interleaved with operators
    // (LOAD_ATTR, BINARY_SUBSCR, CALL, BINARY_OP, ...).
    let window_start = window_start_for(decoded, store_idx, line);
    let window = &decoded[window_start..store_idx];

    Some((classify_window(window), 1))
}

/// Decide where the bytecode window for the RHS begins: the largest index < `store_idx`
/// such that `decoded[index].line == line` and either `index == 0` or
/// `decoded[index - 1].line != line` (the previous instruction is on a
/// different line — i.e. the RHS starts here).
fn window_start_for(decoded: &[DecodedInstruction], store_idx: usize, line: u32) -> usize {
    let mut idx = store_idx;
    while idx > 0 && decoded[idx - 1].line == Some(line) {
        idx -= 1;
    }
    idx
}

fn detect_unpack_sequence(
    decoded: &[DecodedInstruction],
    store_idx: usize,
    line: u32,
) -> Option<(RValueShape, usize)> {
    // The unpack pattern in 3.11+:
    //   LOAD_* <receiver>
    //   UNPACK_SEQUENCE <n>
    //   STORE_* target_0    <-- store_idx if this is the first STORE
    //   STORE_* target_1
    //   ...
    //
    // Find UNPACK_SEQUENCE right before `store_idx` within `line`, then walk
    // back one more step to find the receiver LOAD.
    if store_idx < 2 {
        return None;
    }
    let unpack = &decoded[store_idx - 1];
    if unpack.opname != "UNPACK_SEQUENCE" || unpack.line != Some(line) {
        return None;
    }
    // Walk back to find the LOAD that produced the iterable.
    let mut receiver_idx = store_idx - 2;
    while receiver_idx > 0 && decoded[receiver_idx].line == Some(line) {
        if is_load_op(&decoded[receiver_idx].opname) {
            break;
        }
        receiver_idx -= 1;
    }
    let receiver_name = match &decoded[receiver_idx].argval_kind {
        ArgValKind::Name(s) => s.clone(),
        _ => return None,
    };
    // Count how many consecutive STOREs follow, sharing the same line.
    let mut count = 0usize;
    let mut probe = store_idx;
    while probe < decoded.len()
        && decoded[probe].line == Some(line)
        && is_store_op(&decoded[probe].opname)
    {
        count += 1;
        probe += 1;
    }
    if count == 0 {
        return None;
    }
    // Emit the first IndexAccess; the caller will emit the rest based on the
    // returned `advance`.
    Some((
        RValueShape::IndexAccess {
            receiver: receiver_name,
            index: 0,
        },
        count,
    ))
}

fn classify_window(window: &[DecodedInstruction]) -> RValueShape {
    // Filter out instructions that don't contribute to the data stack analysis
    // (RESUME, NOP, CACHE, COPY_FREE_VARS, etc).
    let mut filtered: Vec<&DecodedInstruction> =
        window.iter().filter(|i| !is_noise(&i.opname)).collect();

    // The last meaningful instruction *before* the STORE often tells us the
    // RHS shape directly.
    while let Some(last) = filtered.last() {
        if is_pop_top_or_kwnames(&last.opname) {
            filtered.pop();
        } else {
            break;
        }
    }

    if filtered.is_empty() {
        return RValueShape::Unknown;
    }

    // Pattern: single LOAD_CONST → Literal.
    if filtered.len() == 1 && is_const_op(&filtered[0].opname) {
        return RValueShape::Literal;
    }
    // Pattern: single LOAD → Simple.
    if filtered.len() == 1 && is_load_op(&filtered[0].opname) {
        if let ArgValKind::Name(s) = &filtered[0].argval_kind {
            return RValueShape::Simple { source: s.clone() };
        }
    }

    // Pattern: LOAD_NAME receiver; LOAD_ATTR field → FieldAccess.
    if filtered.len() >= 2 {
        // Safe: the filtered.len() >= 2 guard ensures last() is Some, but
        // an explicit `let Some(...)` keeps us off the unwrap path per the
        // project's no-unwrap-in-production policy (CLAUDE.md).
        let Some(last) = filtered.last() else {
            return RValueShape::Unknown;
        };
        if last.opname == "LOAD_ATTR" {
            if let ArgValKind::Name(field) = &last.argval_kind {
                // The instruction immediately before should be the LOAD of
                // the receiver (in the simple case `obj.field`).
                if let Some(loader) = filtered.get(filtered.len() - 2) {
                    if is_load_op(&loader.opname) {
                        if let ArgValKind::Name(receiver) = &loader.argval_kind {
                            return RValueShape::FieldAccess {
                                receiver: receiver.clone(),
                                field: field.clone(),
                            };
                        }
                    }
                }
            }
        }
        // Pattern: LOAD receiver; LOAD_CONST(int); BINARY_SUBSCR → IndexAccess.
        if last.opname == "BINARY_SUBSCR" && filtered.len() >= 3 {
            let const_op = filtered[filtered.len() - 2];
            let recv_op = filtered[filtered.len() - 3];
            if const_op.opname == "LOAD_CONST" && is_load_op(&recv_op.opname) {
                if let (ArgValKind::Name(receiver), ArgValKind::Int(index)) =
                    (&recv_op.argval_kind, &const_op.argval_kind)
                {
                    return RValueShape::IndexAccess {
                        receiver: receiver.clone(),
                        index: *index,
                    };
                }
            }
        }
        // Pattern: CALL anywhere in tail → FunctionReturn (the result on the
        // stack at STORE-time is the return value).
        if filtered.iter().rev().take(3).any(|i| is_call_op(&i.opname)) {
            return RValueShape::FunctionReturn;
        }
    }

    // Fallback Compound: collect every distinct LOAD source name.
    let mut sources: Vec<String> = Vec::new();
    for ins in &filtered {
        if is_load_op(&ins.opname) {
            if let ArgValKind::Name(s) = &ins.argval_kind {
                if !sources.contains(s) {
                    sources.push(s.clone());
                }
            }
        }
    }
    if sources.is_empty() {
        RValueShape::Unknown
    } else if sources.len() == 1 {
        RValueShape::Simple {
            source: sources.remove(0),
        }
    } else {
        RValueShape::Compound { sources }
    }
}

fn is_noise(opname: &str) -> bool {
    matches!(
        opname,
        "RESUME" | "NOP" | "CACHE" | "COPY_FREE_VARS" | "PUSH_NULL" | "PRECALL" | "KW_NAMES"
    )
}

fn is_pop_top_or_kwnames(opname: &str) -> bool {
    matches!(opname, "POP_TOP")
}

#[cfg(test)]
mod tests {
    //! These unit tests exercise the classifier with synthetic
    //! `DecodedInstruction` sequences so they do not depend on having a
    //! working CPython bytecode pipeline. The integration tests in
    //! `runtime_tracer.rs` cover the full `dis.get_instructions` -> classifier
    //! round-trip against real code objects.
    use super::*;

    fn n(opname: &str, arg: ArgValKind, line: u32) -> DecodedInstruction {
        DecodedInstruction {
            opname: opname.to_string(),
            argval_kind: arg,
            line: Some(line),
            col_offset: None,
            _end_col_offset: None,
        }
    }

    #[test]
    fn classifies_literal_assignment() {
        // a = 10
        let window = vec![n("LOAD_CONST", ArgValKind::Int(10), 1)];
        assert_eq!(classify_window(&window), RValueShape::Literal);
    }

    #[test]
    fn classifies_simple_copy() {
        // b = a
        let window = vec![n("LOAD_NAME", ArgValKind::Name("a".into()), 1)];
        assert_eq!(
            classify_window(&window),
            RValueShape::Simple { source: "a".into() }
        );
    }

    #[test]
    fn classifies_compound_arithmetic() {
        // total = a + b
        let window = vec![
            n("LOAD_NAME", ArgValKind::Name("a".into()), 1),
            n("LOAD_NAME", ArgValKind::Name("b".into()), 1),
            n("BINARY_OP", ArgValKind::Int(0), 1),
        ];
        assert_eq!(
            classify_window(&window),
            RValueShape::Compound {
                sources: vec!["a".into(), "b".into()]
            }
        );
    }

    #[test]
    fn classifies_field_access() {
        // d = obj.field
        let window = vec![
            n("LOAD_NAME", ArgValKind::Name("obj".into()), 1),
            n("LOAD_ATTR", ArgValKind::Name("field".into()), 1),
        ];
        assert_eq!(
            classify_window(&window),
            RValueShape::FieldAccess {
                receiver: "obj".into(),
                field: "field".into()
            }
        );
    }

    #[test]
    fn classifies_index_access() {
        // e = arr[0]
        let window = vec![
            n("LOAD_NAME", ArgValKind::Name("arr".into()), 1),
            n("LOAD_CONST", ArgValKind::Int(0), 1),
            n("BINARY_SUBSCR", ArgValKind::None, 1),
        ];
        assert_eq!(
            classify_window(&window),
            RValueShape::IndexAccess {
                receiver: "arr".into(),
                index: 0
            }
        );
    }

    #[test]
    fn classifies_function_return() {
        // result = foo()
        let window = vec![
            n("PUSH_NULL", ArgValKind::None, 1),
            n("LOAD_NAME", ArgValKind::Name("foo".into()), 1),
            n("CALL", ArgValKind::Int(0), 1),
        ];
        assert_eq!(classify_window(&window), RValueShape::FunctionReturn);
    }

    #[test]
    fn unpack_sequence_destructure() {
        // a, b = pair
        //   LOAD_NAME pair
        //   UNPACK_SEQUENCE 2
        //   STORE_NAME a
        //   STORE_NAME b
        let decoded = vec![
            n("LOAD_NAME", ArgValKind::Name("pair".into()), 1),
            n("UNPACK_SEQUENCE", ArgValKind::Int(2), 1),
            n("STORE_NAME", ArgValKind::Name("a".into()), 1),
            n("STORE_NAME", ArgValKind::Name("b".into()), 1),
        ];
        // store_idx = 2 (first STORE_NAME).
        let result = detect_unpack_sequence(&decoded, 2, 1).expect("unpack detected");
        assert_eq!(
            result.0,
            RValueShape::IndexAccess {
                receiver: "pair".into(),
                index: 0,
            }
        );
        assert_eq!(result.1, 2); // two STOREs consumed
    }
}
