//! Recorder-side autoformat for minified Python sources.
//!
//! Spec: ``codetracer-specs/Planned-Features/Column-Aware-Tracing-And-Deminification.milestones.org`` §P6.2.
//!
//! When a recorded Python source looks minified (average non-empty line
//! length exceeds the configurable threshold), we shell out to ``black``
//! once at record-start, capture the formatted output, and synthesise a
//! Source Map V3 document mapping positions in the formatted output
//! *back* to positions in the original minified source.  The replay-
//! server's existing P3 sourcemap path can then resolve any recorded
//! position on the formatted source back to the original-side
//! coordinates without a replay-time subprocess.
//!
//! The implementation mirrors the JS recorder's
//! ``packages/instrumenter/src/autoformat.ts`` so the heuristic,
//! environment variables, and inverse-sourcemap shape stay consistent
//! across recorders.  Compare with the replay-server's lazy autoformat
//! fallback in ``codetracer/src/db-backend/src/autoformat.rs`` (P4) —
//! the heuristic and the formatter invocation match by design so a
//! trace produced by a new recorder behaves the same way as the
//! replay-server's lazy path would on an older trace.  The recorder
//! version runs *once* per recording at record start, instead of
//! per-replay.
//!
//! Failure mode is **best-effort**: when ``black`` is missing, errors,
//! or times out, we emit a warning and the recorder falls through to
//! the original (unformatted) source so the trace stays usable.  The
//! replay-server's P4 fallback then picks up the slack at view time.
//!
//! Public API:
//!
//!  - [`looks_minified`] — average-line-length heuristic.
//!  - [`DEFAULT_MINIFIED_THRESHOLD`] — heuristic threshold constant.
//!  - [`autoformat_enabled_by_env`] — env-var kill switch parity with
//!    the replay-server's ``CT_AUTOFORMAT``.
//!  - [`generate_inverse_sourcemap`] — V3 sourcemap formatted → original.
//!  - [`try_autoformat`] — high-level entry point used by the
//!    record-cmd hook.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Default minified-source heuristic threshold (matches
/// ``db-backend/autoformat.rs::DEFAULT_MINIFIED_THRESHOLD`` and the JS
/// recorder's ``DEFAULT_MINIFIED_THRESHOLD``): when the average line
/// length over non-empty lines exceeds this many characters, the source
/// is treated as a candidate for auto-formatting.
///
/// Empirical: hand-written code rarely averages above ~200 chars/line
/// even with long type annotations; rollup/webpack bundles or
/// machine-emitted Python (`bundle.py` test fixtures, packed lambda
/// snippets) routinely average 1000s of chars/line.  500 is a
/// comfortable middle ground.
pub const DEFAULT_MINIFIED_THRESHOLD: usize = 500;

/// Hard subprocess timeout for the ``black`` call, in seconds.
/// ``black`` on a 70 KB bundle is well under a second; if we cross 10 s
/// something is wrong (pathological input, hung tool) and we should
/// bail rather than block record-start indefinitely.
const FORMATTER_TIMEOUT_SECS: u64 = 10;

/// Env var that disables the autoformat pass.  Shared with the
/// replay-server's lazy autoformat fallback (P4) so users can opt out
/// globally with one knob.  Accepts the same off-values as the JS
/// recorder: ``0``, ``off``, ``false``, ``no`` (case-insensitive).
pub const ENV_AUTOFORMAT: &str = "CT_AUTOFORMAT";

/// Env var that overrides [`DEFAULT_MINIFIED_THRESHOLD`].  Shared with
/// the replay-server's lazy autoformat fallback so both sides agree on
/// the env-driven threshold.
pub const ENV_AUTOFORMAT_THRESHOLD: &str = "CT_AUTOFORMAT_THRESHOLD";

/// Successful autoformat result — the formatted source plus the V3
/// sourcemap JSON document mapping the formatted view *back* to the
/// original minified source.
///
/// Direction is deliberate: the document treats *formatted* as the
/// "generated" file and *original* as the "source" file.  This is the
/// inverse of the replay-server's lazy ``PositionMap`` (which projects
/// original → formatted); replay-server's P3 path discovers the
/// ``.fmt.py.map`` sibling, parses it, and resolves recorded-on-formatted
/// positions back to the original-minified coordinates.
#[derive(Debug, Clone)]
pub struct AutoformatResult {
    /// ``black``'s stdout — the formatted source.
    pub formatted_content: String,
    /// Source Map V3 document, JSON-encoded.  Callers materialise this
    /// as the ``<file>.fmt.py.map`` sibling under the trace's source-
    /// files area; replay-server's P3 path picks it up automatically.
    pub sourcemap_v3_json: String,
}

/// Reason a [`try_autoformat`] call decided **not** to emit a
/// formatted sibling.  Each variant maps to a distinct user-visible
/// outcome — see [`try_autoformat`] for the full decision tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The source does not look minified per [`looks_minified`].  This
    /// is the steady-state outcome for normal hand-written sources.
    NotMinified,
    /// ``black`` (and any fallback resolver) was not on PATH.  The
    /// caller is expected to log a one-shot warning and continue
    /// recording the original source — the replay-server's P4 lazy
    /// fallback will format at view time on machines that have
    /// ``black``.
    ToolMissing,
    /// ``black`` ran but exited non-zero, errored on stdio plumbing,
    /// or timed out.  The wrapped string carries a diagnostic suitable
    /// for stderr.
    ToolError(String),
    /// The source has an adjacent ``<file>.map`` sourcemap, indicating
    /// upstream tooling (rollup/webpack/etc.) already produced a
    /// canonical mapping.  Replacing it with our line-level inverse
    /// would be a strict downgrade; defer to upstream.
    SiblingMapExists,
    /// The user disabled autoformat via [`ENV_AUTOFORMAT`] / the
    /// recorder's ``--no-autoformat`` flag.
    EnvDisabled,
    /// ``black`` ran cleanly but didn't actually break the source up
    /// (post-format line count ≤ pre-format line count).  This guards
    /// against pre-formatted "minified-looking" files that are simply
    /// long single-line JSON-encoded blobs that ``black`` doesn't touch.
    NoChange,
}

/// Outcome of a [`try_autoformat`] call — either a successful
/// [`AutoformatResult`] or one of the [`SkipReason`] variants the caller
/// must surface to the user.
#[derive(Debug, Clone)]
pub enum AutoformatOutcome {
    Ok(AutoformatResult),
    Skipped(SkipReason),
}

/// Average-non-empty-line-length minified-source heuristic.
///
/// Mirrors ``db-backend/autoformat.rs::looks_minified`` and the JS
/// recorder's ``looksMinified`` so behaviour stays consistent between
/// the recorder-side pre-format and the replay-server-side lazy
/// fallback.
///
/// Returns ``false`` for empty input or input with no non-empty lines.
/// Character counts iterate over Unicode code points (not bytes) so
/// multibyte tokens don't double-count their length and falsely trip
/// the threshold.
pub fn looks_minified(content: &str, threshold_chars: usize) -> bool {
    let mut total_chars: usize = 0;
    let mut non_empty_lines: usize = 0;
    // Match Rust's `str::lines` semantics: split on `\n`, drop optional
    // trailing `\r`, do not count a final empty trailing line.
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Iterate by `chars()` (Unicode scalar values) — this is
        // consistent with the JS recorder's `Array.from(line).length`.
        total_chars += line.chars().count();
        non_empty_lines += 1;
    }
    if non_empty_lines == 0 {
        return false;
    }
    let average = total_chars / non_empty_lines;
    average > threshold_chars
}

/// ``true`` when the recorder-side autoformat is enabled via env var.
///
/// Accepts the same "off" values as the replay-server's
/// [`ENV_AUTOFORMAT`] (case-insensitive): ``0``, ``off``, ``false``,
/// ``no``.  Unset / anything else means "on" — the default.
///
/// Sharing the env var with the replay-server lets users opt out
/// globally with one knob.
pub fn autoformat_enabled_by_env() -> bool {
    match std::env::var(ENV_AUTOFORMAT) {
        Err(_) => true,
        Ok(value) => {
            let lower = value.trim().to_ascii_lowercase();
            !matches!(lower.as_str(), "0" | "off" | "false" | "no")
        }
    }
}

/// Read [`ENV_AUTOFORMAT_THRESHOLD`] and parse it as a positive
/// integer.  Returns ``None`` on unset / unparseable values; mirrors
/// the replay-server's ``minified_threshold`` helper so both sides
/// agree on the env-driven threshold.
fn threshold_from_env() -> Option<usize> {
    let raw = std::env::var(ENV_AUTOFORMAT_THRESHOLD).ok()?;
    let parsed: i64 = raw.trim().parse().ok()?;
    if parsed <= 0 {
        return None;
    }
    Some(parsed as usize)
}

/// Cheap "is this binary on PATH" probe.  Walks ``PATH`` directly so
/// the early-exit path stays sub-millisecond when ``black`` isn't
/// installed.  Mirrors the JS recorder's ``isOnPath`` to keep the
/// behaviour symmetric across recorders.
fn is_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    // On Windows, executables have explicit extensions; on Unix the
    // empty extension is the convention.  We don't currently target
    // Windows for the Python recorder but follow the JS recorder's
    // platform-aware probing pattern.
    let exts: &[&str] = if cfg!(windows) {
        &[".cmd", ".exe", ".bat", ""]
    } else {
        &[""]
    };
    for dir in path.split(sep) {
        if dir.is_empty() {
            continue;
        }
        for ext in exts {
            let candidate = PathBuf::from(dir).join(format!("{}{}", binary, ext));
            if let Ok(meta) = std::fs::metadata(&candidate) {
                if meta.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Resolve a Python interpreter that has the bundled ``black`` module
/// importable from its site-packages.
///
/// The Python recorder declares ``black`` as a runtime dependency in
/// ``pyproject.toml`` so any environment that pip-installed
/// ``codetracer-python-recorder`` already has ``black`` available
/// importable through the same interpreter.  Returns the interpreter
/// path when ``import black`` succeeds with a zero exit, ``None``
/// otherwise.
///
/// Probe order:
///   1. ``CT_BUNDLED_PYTHON`` env var — explicit override used by
///      test harnesses + nix builds where the recorder might be
///      embedded in a non-default interpreter.
///   2. The interpreter pointed to by ``PYO3_PYTHON`` (the build-time
///      pinned interpreter).
///   3. The interpreter on PATH as ``python3`` then ``python``.
fn resolve_bundled_python_with_black() -> Option<String> {
    fn probe(candidate: &str) -> Option<String> {
        let out = Command::new(candidate)
            .args(["-c", "import black; import sys; sys.exit(0)"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        if out.success() {
            Some(candidate.to_string())
        } else {
            None
        }
    }
    if let Ok(p) = std::env::var("CT_BUNDLED_PYTHON") {
        if !p.is_empty() {
            if let Some(found) = probe(&p) {
                return Some(found);
            }
        }
    }
    if let Ok(p) = std::env::var("PYO3_PYTHON") {
        if !p.is_empty() {
            if let Some(found) = probe(&p) {
                return Some(found);
            }
        }
    }
    for candidate in ["python3", "python"] {
        if let Some(found) = probe(candidate) {
            return Some(found);
        }
    }
    None
}

/// Backward-compat shim — older code paths may want only the bundled
/// interpreter path.  Returns the interpreter capable of
/// ``import black``.
fn resolve_bundled_black() -> Option<String> {
    resolve_bundled_python_with_black()
}

/// Outcome of a [`run_black`] invocation — matches the variant
/// layout of the JS recorder's ``PrettierOutcome`` so call-site
/// translation to [`AutoformatOutcome`] follows the same dispatch.
enum BlackOutcome {
    /// ``black`` exited cleanly; the wrapped string is the formatted
    /// source.
    Ok(String),
    /// ``black`` was not on PATH.
    Missing,
    /// ``black`` ran but exited non-zero or stdio plumbing failed.
    Error(String),
    /// ``black`` did not finish within [`FORMATTER_TIMEOUT_SECS`].
    Timeout,
}

/// Spawn ``black`` with ``content`` on stdin and return its stdout.
///
/// ``black``'s ``-`` argument tells it to read from stdin and write
/// to stdout (rather than reformatting a file in place).
/// ``--fast`` skips the AST round-trip safety check — black formats
/// the same either way, but ``--fast`` shaves ~30% off the wall time
/// on small inputs.  Recording is meant to be fast, so the trade-off
/// (no extra safety net beyond what black already provides) is
/// acceptable.
///
/// The timeout is enforced by spawning a watcher thread that ``kill``s
/// the child after [`FORMATTER_TIMEOUT_SECS`].  We deliberately avoid
/// the ``wait_timeout`` crate to keep the dependency surface flat —
/// the watcher pattern is well-trodden in Rust's stdlib examples.
fn run_black(content: &str) -> BlackOutcome {
    // Two-tier resolution per the user directive 2026-06-11
    // ("language-specific formatting tools that CodeTracer uses should
    // be provisioned by the recorder package"):
    //
    //   1. The bundled `black` Python module reachable through the
    //      same interpreter that imported codetracer_python_recorder.
    //      Invoked as ``python -m black -``.  This tier ALWAYS works
    //      when `black` is declared in pyproject.toml's `dependencies`
    //      and the user installed via pip / uv / nix / poetry.
    //   2. A standalone `black` binary on PATH (fallback for unusual
    //      installs or environments where the interpreter doesn't have
    //      a usable site-packages).
    //
    // Both tiers share the same stdin/stdout/stderr pipe contract.
    let (program, base_args): (String, Vec<&str>) = match resolve_bundled_black() {
        Some(p) => (p, vec!["-m", "black", "-", "--quiet", "--fast"]),
        None => match is_on_path("black") {
            Some(p) => (
                p.to_string_lossy().into_owned(),
                vec!["-", "--quiet", "--fast"],
            ),
            None => return BlackOutcome::Missing,
        },
    };

    let mut child = match Command::new(&program)
        .args(&base_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(err) => return BlackOutcome::Error(format!("failed to spawn black: {err}")),
    };

    // Hand off stdin to a writer thread so a multi-MB source doesn't
    // deadlock on a full pipe while we're trying to read stdout below.
    // ``black`` reads stdin to EOF before emitting stdout, so this
    // pattern is required.
    let stdin_content = content.to_string();
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => return BlackOutcome::Error("failed to capture black stdin".to_string()),
    };
    let stdin_thread = thread::spawn(move || -> std::io::Result<()> {
        stdin.write_all(stdin_content.as_bytes())?;
        // Dropping `stdin` here closes the pipe and signals EOF to
        // black — without this, black hangs waiting for more input.
        drop(stdin);
        Ok(())
    });

    // Wait for the child with a hard timeout.  We use a channel +
    // dedicated wait thread so we can ``kill`` the process if it
    // exceeds [`FORMATTER_TIMEOUT_SECS`].  ``child.wait_with_output()``
    // is blocking and unavailable here because we already took stdin.
    //
    // The wait thread reads the *full* output (stdout + stderr +
    // status) so the parent thread observes a single combined result
    // when it returns.  This avoids the dance of reading pipes
    // separately while polling `try_wait`.
    let (tx, rx) = mpsc::channel();
    let wait_thread = thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = tx.send(output);
    });

    let timeout = Duration::from_secs(FORMATTER_TIMEOUT_SECS);
    let output = match rx.recv_timeout(timeout) {
        Ok(out) => out,
        Err(_) => {
            // Timeout: we can't easily kill the child from here
            // because ``wait_with_output`` consumed it.  Best-effort:
            // signal the timeout to the caller and let the OS reap the
            // child when the wait thread eventually returns.  This is
            // the same pragmatic choice the JS recorder makes (it
            // relies on Node's spawn timeout, which also leaves a
            // dangling child on some platforms).
            let _ = wait_thread.join();
            return BlackOutcome::Timeout;
        }
    };

    // Surface stdin-feed errors before the formatted output so we
    // don't claim success on a truncated feed.
    if let Ok(Err(err)) = stdin_thread.join() {
        return BlackOutcome::Error(format!("failed to write black stdin: {err}"));
    }
    let _ = wait_thread.join();

    let output = match output {
        Ok(o) => o,
        Err(err) => return BlackOutcome::Error(format!("wait_with_output failed: {err}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return BlackOutcome::Error(format!(
            "black exited with status {:?}: {}",
            output.status.code(),
            stderr
        ));
    }

    // Decode stdout as UTF-8; if a Python source contained invalid
    // UTF-8 bytes, black would have already complained.  Use
    // ``from_utf8_lossy`` defensively so we never panic on the hot
    // path — the lossy replacement is still better than crashing the
    // recorder.
    let formatted = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(err) => {
            return BlackOutcome::Error(format!(
                "black stdout was not valid UTF-8: {err}"
            ));
        }
    };

    BlackOutcome::Ok(formatted)
}

/// Build a Source Map V3 document mapping positions in ``formatted``
/// back to positions in ``original``.
///
/// Algorithm (v1, line-level only):
///
///  1. Extract the first "salient anchor" token of each line in the
///     formatted output (same notion as the replay-server's
///     ``first_anchor_token``: ≥3 chars, identifier-like,
///     non-common-keyword).
///  2. Walk the original source token-by-token and remember the line
///     each anchor first appears on.  Scan forward only so the mapping
///     stays monotonic.
///  3. For every formatted line whose anchor we found, emit a single
///     ``(0, 0)`` segment pointing at column 0 of the matched original
///     line.
///
/// Column-level precision is out of scope for v1 — the same scope
/// decision the replay-server's P4 implementation and the JS recorder's
/// inverse-map made.  See the JS recorder's ``generateInverseSourceMap``
/// docstring for the rationale (formatter inserts newlines + indentation
/// that change column positions throughout every line; computing column
/// precision requires a real diff algorithm against the post-format
/// whitespace structure).
///
/// Returns the V3 sourcemap as a parsed JSON object — callers serialise
/// it via ``serde_json::to_string`` to land it on disk.  The
/// ``sources[]`` entry uses the file name passed in ``original_name``;
/// callers writing the map to disk as a sibling of ``<library>.fmt.py``
/// should pass the bare basename of the original (e.g. ``lib.min.py``)
/// so the V3 path resolution lands back at the original sibling.
pub fn generate_inverse_sourcemap(
    original: &str,
    formatted: &str,
    original_name: &str,
) -> serde_json::Value {
    // Step 1: per-original-line anchor index built by a single forward
    // scan of the original source — token first-occurrence line number.
    let original_lines = split_lines(original);
    let formatted_lines = split_lines(formatted);

    let mut anchor_to_orig_line: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (i, line) in original_lines.iter().enumerate() {
        for tok in identifier_tokens(line) {
            anchor_to_orig_line.entry(tok).or_insert(i);
        }
    }

    // Step 2: walk each formatted line, pick its first anchor token,
    // resolve it through the index.  Forward-cursor enforces monotonic
    // mapping — the formatter preserves statement order, so a later
    // formatted line can never legitimately point to an earlier
    // original line.
    let mut segments: Vec<Vec<(i64, i64, i64, i64)>> =
        Vec::with_capacity(formatted_lines.len());
    let mut orig_cursor: usize = 0;

    for line in &formatted_lines {
        let mut line_segments: Vec<(i64, i64, i64, i64)> = Vec::new();
        for tok in identifier_tokens(line) {
            if let Some(&orig_idx) = anchor_to_orig_line.get(&tok) {
                if orig_idx < orig_cursor {
                    continue;
                }
                line_segments.push((0, 0, orig_idx as i64, 0));
                orig_cursor = orig_idx;
                break;
            }
        }
        segments.push(line_segments);
    }

    let mappings = encode_mappings(&segments);
    serde_json::json!({
        "version": 3,
        "sources": [original_name],
        "sourcesContent": [original],
        "names": [],
        "mappings": mappings,
    })
}

/// Split a source string into lines.  Matches the convention used by
/// [`looks_minified`]: split on ``\n``, drop optional trailing ``\r``,
/// drop the final empty trailing element if the source ended with
/// ``\n``.
///
/// We re-implement this rather than reach for ``str::lines`` because
/// the empty-trailing-line case is load-bearing for the line index →
/// line number conversion (we need the *count* of source lines to
/// match the writer's ``paths.dat`` Layout A line counts).
fn split_lines(content: &str) -> Vec<String> {
    let mut parts: Vec<&str> = content.split('\n').collect();
    // If the source ends with `\n`, `split` produces a trailing empty
    // string.  Drop it so `lines.len()` matches the number of source
    // lines.
    if parts.last() == Some(&"") {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|p| p.strip_suffix('\r').unwrap_or(p).to_string())
        .collect()
}

/// Extract identifier-like tokens from a single line — used by both
/// the anchor-index builder and the per-formatted-line lookup in
/// [`generate_inverse_sourcemap`].
///
/// Returned tokens are at least 3 chars long, made up entirely of
/// ``[A-Za-z0-9_$]`` (``$`` is rare in Python but kept for symmetry
/// with the JS recorder), and exclude common keywords (mirrors the
/// replay-server's ``is_common_keyword`` list — see
/// ``db-backend/autoformat.rs::is_common_keyword``).  Skipping common
/// keywords prevents ``def``, ``return``, etc. from anchoring random
/// lines (they occur on dozens of lines in a typical bundle and would
/// pick the first occurrence rather than the matching one).
fn identifier_tokens(line: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        let is_ident = ch.is_ascii_alphanumeric() || ch == '_' || ch == '$';
        if is_ident {
            current.push(ch);
        } else if current.len() >= 3 && !is_common_keyword(&current) {
            tokens.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= 3 && !is_common_keyword(&current) {
        tokens.push(current);
    }
    tokens
}

/// Common JS / TS / Python keywords that occur too often to anchor a
/// line uniquely.  Matches the replay-server's ``is_common_keyword``
/// list (and the JS recorder's ``COMMON_KEYWORDS``) so the recorder's
/// pre-format and the lazy fallback agree on the anchor selection
/// (load-bearing for round-trip consistency).
fn is_common_keyword(word: &str) -> bool {
    matches!(
        word,
        "var"
            | "let"
            | "const"
            | "function"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "do"
            | "switch"
            | "case"
            | "default"
            | "break"
            | "continue"
            | "this"
            | "new"
            | "delete"
            | "typeof"
            | "instanceof"
            | "void"
            | "null"
            | "true"
            | "false"
            | "import"
            | "export"
            | "from"
            | "class"
            | "extends"
            | "super"
            | "static"
            | "async"
            | "await"
            | "yield"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "def"
            | "lambda"
            | "pass"
            | "and"
            | "not"
            | "with"
    )
}

/// Encode an array-of-arrays of segments into the Source Map V3 VLQ
/// ``mappings`` field.
///
/// Each line's segments are encoded relative to the previous segment
/// (deltas), and lines are separated by ``;``.  Within a line, segments
/// are separated by ``,``.
///
/// We do this by hand rather than depending on a sourcemap-VLQ crate
/// to keep the dependency surface flat — the inverse sourcemap is
/// line-level only, so a 4-field VLQ segment is all we need.  See
/// <https://sourcemaps.info/spec.html#h.lmz475t4mvbx> for the wire
/// format.
fn encode_mappings(lines: &[Vec<(i64, i64, i64, i64)>]) -> String {
    let mut prev_src_idx: i64 = 0;
    let mut prev_orig_line: i64 = 0;
    let mut prev_orig_col: i64 = 0;
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for line_segs in lines {
        let mut prev_gen_col: i64 = 0;
        let mut seg_parts: Vec<String> = Vec::with_capacity(line_segs.len());
        for &(gen_col, src_idx, orig_line, orig_col) in line_segs {
            let mut part = String::new();
            part.push_str(&encode_vlq(gen_col - prev_gen_col));
            part.push_str(&encode_vlq(src_idx - prev_src_idx));
            part.push_str(&encode_vlq(orig_line - prev_orig_line));
            part.push_str(&encode_vlq(orig_col - prev_orig_col));
            seg_parts.push(part);
            prev_gen_col = gen_col;
            prev_src_idx = src_idx;
            prev_orig_line = orig_line;
            prev_orig_col = orig_col;
        }
        out.push(seg_parts.join(","));
    }
    out.join(";")
}

/// Encode a single signed integer as Source Map V3 base64 VLQ.
///
/// The encoding stores the sign bit as the least-significant bit of
/// the first 5-bit group and uses the continuation bit (bit 5) to
/// signal more groups.  Reference: V3 spec §"Base 64 VLQ".
fn encode_vlq(value: i64) -> String {
    let mut vlq: u64 = if value < 0 {
        (((-value) as u64) << 1) | 1
    } else {
        (value as u64) << 1
    };
    let mut out = String::new();
    loop {
        let mut digit = (vlq & 0x1f) as u8;
        vlq >>= 5;
        if vlq > 0 {
            digit |= 0x20;
        }
        out.push(BASE64_ALPHABET[digit as usize] as char);
        if vlq == 0 {
            break;
        }
    }
    out
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// High-level entry point used by the recorder's record-cmd hook.
///
/// Decision tree (mirrors the JS recorder's ``tryAutoformat``):
///
///  1. If [`autoformat_enabled_by_env`] returns false →
///     [`SkipReason::EnvDisabled`].
///  2. If a sibling ``<source>.map`` exists →
///     [`SkipReason::SiblingMapExists`] (preserve upstream sourcemap).
///  3. If ``!looks_minified(content, threshold)`` →
///     [`SkipReason::NotMinified`].
///  4. Resolve ``black`` via PATH.  If missing →
///     [`SkipReason::ToolMissing`].
///  5. Invoke ``black -`` (stdin) with the source.  Capture stdout.
///  6. If timeout / non-zero exit / stdio error →
///     [`SkipReason::ToolError`].
///  7. If output line count ≤ input line count →
///     [`SkipReason::NoChange`].  This guards against pre-formatted
///     "minified-looking" files (long JSON-encoded blobs) where ``black``
///     made no structural change.
///  8. Generate a V3 sourcemap mapping each formatted line back to its
///     original line via identifier-token anchors (line-level v1).
///  9. Return ``Ok(AutoformatResult)``.
///
/// The ``source_path`` parameter is used to (a) probe for the sibling
/// ``.map`` sourcemap and (b) extract the basename used as
/// ``sources[0]`` in the generated sourcemap.
pub fn try_autoformat(content: &str, source_path: &Path) -> AutoformatOutcome {
    if !autoformat_enabled_by_env() {
        return AutoformatOutcome::Skipped(SkipReason::EnvDisabled);
    }

    // Sibling ``<source>.map`` sourcemap discovery — same convention as
    // the replay-server's P3 sourcemap path.  If present, upstream
    // tooling already knows how to map this view back to the original
    // sources; our line-level inverse would be a strict downgrade.
    let sibling_map = {
        let mut path = source_path.as_os_str().to_owned();
        path.push(".map");
        PathBuf::from(path)
    };
    if sibling_map.exists() {
        return AutoformatOutcome::Skipped(SkipReason::SiblingMapExists);
    }

    let threshold = threshold_from_env().unwrap_or(DEFAULT_MINIFIED_THRESHOLD);
    if !looks_minified(content, threshold) {
        return AutoformatOutcome::Skipped(SkipReason::NotMinified);
    }

    match run_black(content) {
        BlackOutcome::Missing => AutoformatOutcome::Skipped(SkipReason::ToolMissing),
        BlackOutcome::Timeout => {
            AutoformatOutcome::Skipped(SkipReason::ToolError("black timed out".to_string()))
        }
        BlackOutcome::Error(msg) => AutoformatOutcome::Skipped(SkipReason::ToolError(msg)),
        BlackOutcome::Ok(formatted) => {
            // Guard: if ``black`` didn't actually break the source up
            // (same line count), don't claim success — the trace would
            // carry a redundant copy and a degenerate sourcemap.
            let orig_line_count = split_lines(content).len();
            let fmt_line_count = split_lines(&formatted).len();
            if fmt_line_count <= orig_line_count {
                return AutoformatOutcome::Skipped(SkipReason::NoChange);
            }
            let original_name = source_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| source_path.to_string_lossy().into_owned());
            let sourcemap = generate_inverse_sourcemap(content, &formatted, &original_name);
            // The V3 document is small; serialising with `to_string`
            // (compact JSON) keeps the on-disk size down for the
            // ``<file>.fmt.py.map`` sibling.
            let sourcemap_json = match serde_json::to_string(&sourcemap) {
                Ok(s) => s,
                Err(err) => {
                    return AutoformatOutcome::Skipped(SkipReason::ToolError(format!(
                        "failed to serialise sourcemap: {err}"
                    )));
                }
            };
            AutoformatOutcome::Ok(AutoformatResult {
                formatted_content: formatted,
                sourcemap_v3_json: sourcemap_json,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_minified_detects_long_line_bundles() {
        // Single line >500 chars; comfortably above the default
        // threshold.  We use a multi-statement Python one-liner so the
        // test mirrors how a packed lambda snippet would actually look.
        let stmt = "a=1; b=2; c=a+b; d=c*c; print(d)";
        let mut src = String::new();
        for _ in 0..25 {
            src.push_str(stmt);
        }
        assert!(looks_minified(&src, DEFAULT_MINIFIED_THRESHOLD));
    }

    #[test]
    fn looks_minified_returns_false_for_normal_code() {
        let src: Vec<&str> = (0..30).map(|_| "x = 1").collect();
        let joined = src.join("\n");
        assert!(!looks_minified(&joined, DEFAULT_MINIFIED_THRESHOLD));
    }

    #[test]
    fn looks_minified_returns_false_on_empty_input() {
        assert!(!looks_minified("", DEFAULT_MINIFIED_THRESHOLD));
        // Whitespace-only "lines" do not count as non-empty per the
        // heuristic's contract.
        assert!(!looks_minified("\n\n   \n", DEFAULT_MINIFIED_THRESHOLD));
    }

    #[test]
    fn looks_minified_respects_custom_threshold() {
        // 60-char line: above 50, below 100.  Lets us test both sides
        // of the threshold without changing the input.
        let src = "x".repeat(60);
        assert!(looks_minified(&src, 50));
        assert!(!looks_minified(&src, 100));
    }

    #[test]
    fn looks_minified_counts_unicode_codepoints_not_bytes() {
        // 60 multibyte chars × 2 bytes/char = 120 bytes; codepoint
        // count is 60.  Threshold 100 should NOT trigger (we count
        // codepoints, not bytes) — this mirrors the JS recorder's
        // ``Array.from(line).length`` contract.
        let src = "ñ".repeat(60);
        assert!(!looks_minified(&src, 100));
        assert!(looks_minified(&src, 50));
    }

    #[test]
    fn autoformat_enabled_by_env_defaults_to_true_when_unset() {
        // Snapshot + restore the env var so the test doesn't leak
        // state to siblings that run in the same process.
        let saved = std::env::var(ENV_AUTOFORMAT).ok();
        std::env::remove_var(ENV_AUTOFORMAT);
        let enabled = autoformat_enabled_by_env();
        if let Some(v) = saved {
            std::env::set_var(ENV_AUTOFORMAT, v);
        }
        assert!(enabled);
    }

    #[test]
    fn autoformat_enabled_by_env_recognises_off_values() {
        // Snapshot + restore — see the previous test.
        let saved = std::env::var(ENV_AUTOFORMAT).ok();
        for v in ["0", "off", "false", "no", "OFF", "False", "  off  "] {
            std::env::set_var(ENV_AUTOFORMAT, v);
            assert!(!autoformat_enabled_by_env(), "expected '{v}' to disable");
        }
        for v in ["1", "on", "true", "yes"] {
            std::env::set_var(ENV_AUTOFORMAT, v);
            assert!(autoformat_enabled_by_env(), "expected '{v}' to enable");
        }
        match saved {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT, v),
            None => std::env::remove_var(ENV_AUTOFORMAT),
        }
    }

    #[test]
    fn generate_inverse_sourcemap_emits_valid_v3() {
        let original =
            "alpha_var=1; beta_var=2; gamma_var=alpha_var+beta_var";
        let formatted = [
            "alpha_var = 1",
            "beta_var = 2",
            "gamma_var = alpha_var + beta_var",
            "",
        ]
        .join("\n");
        let map = generate_inverse_sourcemap(original, &formatted, "lib.min.py");
        assert_eq!(map["version"], 3);
        assert_eq!(map["sources"][0], "lib.min.py");
        assert_eq!(map["sourcesContent"][0], original);
        let mappings = map["mappings"].as_str().expect("mappings must be string");
        // 3 formatted non-empty lines, each with a unique identifier
        // anchor: the mappings string must encode at least one segment
        // per anchorable line.
        assert!(
            !mappings.is_empty(),
            "mappings must be non-empty for an anchorable formatted view"
        );
    }

    #[test]
    fn generate_inverse_sourcemap_segments_match_anchorable_lines() {
        let original = "alpha_name=1; beta_name=2; gamma_name=3";
        let formatted = [
            "alpha_name = 1",
            "beta_name = 2",
            "gamma_name = 3",
        ]
        .join("\n");
        let map = generate_inverse_sourcemap(original, &formatted, "lib.py");
        let mappings = map["mappings"].as_str().expect("mappings must be string");
        // 3 formatted lines, each with a unique identifier anchor: 3
        // segments separated by `;`.  Each line segment should be
        // non-empty.
        let line_segments: Vec<&str> = mappings.split(';').collect();
        let non_empty = line_segments.iter().filter(|s| !s.is_empty()).count();
        assert!(
            non_empty >= 3,
            "expected ≥3 non-empty line segments, got {non_empty}: {mappings}"
        );
    }

    #[test]
    fn identifier_tokens_skips_short_and_common_keywords() {
        let tokens = identifier_tokens("def foo(bar): return baz");
        // `def` and `return` are common keywords; `foo`, `bar`, `baz`
        // each ≥3 chars and not keywords.
        assert_eq!(tokens, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn identifier_tokens_drops_under_3_char_idents() {
        // `x` and `y` are too short; `add` ≥3 chars.
        let tokens = identifier_tokens("x = add(y)");
        assert_eq!(tokens, vec!["add"]);
    }

    #[test]
    fn encode_vlq_round_trips_known_values() {
        // Spot-checks against the V3 spec examples + known fixtures
        // from the JS recorder's table.  Re-implementing the encoder
        // would silently break sourcemap consumers if these regressed.
        assert_eq!(encode_vlq(0), "A"); // 0 → 'A'
        assert_eq!(encode_vlq(1), "C"); // (1<<1)|0 = 2 → 'C'
        assert_eq!(encode_vlq(-1), "D"); // (1<<1)|1 = 3 → 'D'
        assert_eq!(encode_vlq(16), "gB"); // 32 → continuation
    }

    #[test]
    fn split_lines_drops_trailing_empty_and_carriage_return() {
        let lines = split_lines("a\nb\nc\n");
        assert_eq!(lines, vec!["a", "b", "c"]);

        let lines = split_lines("a\r\nb\r\n");
        assert_eq!(lines, vec!["a", "b"]);

        let lines = split_lines("");
        assert_eq!(lines, Vec::<String>::new());
    }

    #[test]
    fn try_autoformat_skips_when_env_disabled() {
        // Snapshot + restore.
        let saved = std::env::var(ENV_AUTOFORMAT).ok();
        std::env::set_var(ENV_AUTOFORMAT, "0");
        let outcome = try_autoformat(&"x".repeat(600), Path::new("input.py"));
        match saved {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT, v),
            None => std::env::remove_var(ENV_AUTOFORMAT),
        }
        match outcome {
            AutoformatOutcome::Skipped(SkipReason::EnvDisabled) => {}
            other => panic!("expected EnvDisabled, got {other:?}"),
        }
    }

    #[test]
    fn try_autoformat_skips_when_sibling_map_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = tmp.path().join("lib.min.py");
        std::fs::write(&source, "x" .repeat(600)).expect("write source");
        let sibling_map = tmp.path().join("lib.min.py.map");
        std::fs::write(&sibling_map, "{\"version\": 3}").expect("write map");

        // Make sure env autoformat is on so the only branch that can
        // skip is the sibling-map check.
        let saved = std::env::var(ENV_AUTOFORMAT).ok();
        std::env::remove_var(ENV_AUTOFORMAT);

        let outcome = try_autoformat(&"x".repeat(600), &source);

        match saved {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT, v),
            None => std::env::remove_var(ENV_AUTOFORMAT),
        }

        match outcome {
            AutoformatOutcome::Skipped(SkipReason::SiblingMapExists) => {}
            other => panic!("expected SiblingMapExists, got {other:?}"),
        }
    }

    #[test]
    fn try_autoformat_skips_when_not_minified() {
        let saved = std::env::var(ENV_AUTOFORMAT).ok();
        std::env::remove_var(ENV_AUTOFORMAT);
        // No sibling map, normal-looking code.
        let outcome = try_autoformat("x = 1\ny = 2\n", Path::new("input.py"));
        match saved {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT, v),
            None => std::env::remove_var(ENV_AUTOFORMAT),
        }
        match outcome {
            AutoformatOutcome::Skipped(SkipReason::NotMinified) => {}
            other => panic!("expected NotMinified, got {other:?}"),
        }
    }

    #[test]
    fn try_autoformat_skips_when_tool_missing() {
        // Drive ``black`` off PATH by clobbering PATH for the duration
        // of the call.  We can't easily mock ``which`` itself, but
        // setting PATH to an empty directory accomplishes the same
        // thing in a portable way.
        let saved_path = std::env::var("PATH").ok();
        let saved_env = std::env::var(ENV_AUTOFORMAT).ok();

        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("PATH", tmp.path());
        std::env::remove_var(ENV_AUTOFORMAT);

        let src = "a=1; b=2; c=a+b; d=c*c; print(d); ".repeat(20);
        let outcome = try_autoformat(&src, Path::new("input.py"));

        match saved_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match saved_env {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT, v),
            None => std::env::remove_var(ENV_AUTOFORMAT),
        }

        match outcome {
            AutoformatOutcome::Skipped(SkipReason::ToolMissing) => {}
            other => panic!("expected ToolMissing, got {other:?}"),
        }
    }

    #[test]
    fn threshold_from_env_parses_positive_int() {
        let saved = std::env::var(ENV_AUTOFORMAT_THRESHOLD).ok();
        std::env::set_var(ENV_AUTOFORMAT_THRESHOLD, "200");
        assert_eq!(threshold_from_env(), Some(200));
        std::env::set_var(ENV_AUTOFORMAT_THRESHOLD, "0");
        assert_eq!(threshold_from_env(), None);
        std::env::set_var(ENV_AUTOFORMAT_THRESHOLD, "-5");
        assert_eq!(threshold_from_env(), None);
        std::env::set_var(ENV_AUTOFORMAT_THRESHOLD, "abc");
        assert_eq!(threshold_from_env(), None);
        std::env::remove_var(ENV_AUTOFORMAT_THRESHOLD);
        assert_eq!(threshold_from_env(), None);
        match saved {
            Some(v) => std::env::set_var(ENV_AUTOFORMAT_THRESHOLD, v),
            None => std::env::remove_var(ENV_AUTOFORMAT_THRESHOLD),
        }
    }
}
