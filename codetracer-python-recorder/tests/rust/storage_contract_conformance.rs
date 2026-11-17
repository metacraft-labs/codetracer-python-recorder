//! This recorder's half of the shared trace-storage contract
//! (`codetracer_ctfs::recorder_conformance`): the PyO3 extension binds the
//! shared storage config and sender through `codetracer_ctfs`, and no source
//! file anywhere in this checkout (the Rust extension, the Python package, the
//! pure-Python recorder, the scripts) carries a private storage placement
//! parser or schema type, a private managed sender, or a static config parser
//! of its own.
//!
//! It reads this repository's real sources: no fixture, mock or stub is
//! involved. The check used to run in `codetracer-trace-format`'s suite, which
//! walked every recorder checkout; it lives here so the dependency points from
//! the recorder to the shared contract, never the other way.

use std::fs;
use std::path::{Path, PathBuf};

use codetracer_ctfs::recorder_conformance::{manifest_declares_dependency, scan_recorder_sources};

/// This crate's directory, relative to the repository root.
const CRATE_DIR: &str = "codetracer-python-recorder";

fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The checkout root: `CRATE_DIR` is one directory deep.
fn repository_root() -> PathBuf {
    let root = crate_dir()
        .join("..")
        .canonicalize()
        .expect("resolve the checkout root");
    assert!(
        root.join(CRATE_DIR).join("Cargo.toml").is_file(),
        "{} is not this recorder's checkout root",
        root.display()
    );
    root
}

#[test]
fn the_extension_depends_on_the_shared_storage_contract() {
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml"))
        .expect("read the extension's Cargo.toml");
    assert!(
        manifest_declares_dependency(&manifest, "codetracer_ctfs"),
        "{CRATE_DIR}/Cargo.toml must depend on the shared CTFS storage contract (codetracer_ctfs)"
    );
}

#[test]
fn the_recorder_has_no_private_storage_parser_sender_or_static_config_parser() {
    let root = repository_root();
    let report = scan_recorder_sources(&root)
        .unwrap_or_else(|error| panic!("scan {}: {error}", root.display()));
    assert!(
        report.files_scanned > 0,
        "no source file was scanned under {}",
        root.display()
    );
    let violations: Vec<String> = report.violations.iter().map(ToString::to_string).collect();
    assert!(
        violations.is_empty(),
        "private copies of the shared trace-storage contract:\n{}",
        violations.join("\n")
    );
}
