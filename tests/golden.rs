//! No-regression golden test for the reader refactor.
//!
//! Parses `examples/demo_export` through the CFX export reader and asserts the
//! serialized [`QpcrRun`] is byte-identical to a committed fixture. This proves
//! the Phase 0 reader/registry refactor does not change parsed output. It also
//! checks that content-routed `read_path` still parses a single CSV.
//!
//! `source_file` (an absolute, machine-specific provenance string) is cleared
//! before serializing so the fixture stays portable.

use std::path::Path;

use openqpcr::readers::export;

fn demo_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo_export")
}

fn golden_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_export.golden.json")
}

/// Parse the demo export and serialize it with provenance stripped.
fn demo_export_json() -> String {
    let mut run = export::read_export_dir(&demo_dir()).expect("demo export must parse");
    run.metadata.source_file = None;
    serde_json::to_string_pretty(&run).expect("run must serialize")
}

#[test]
fn demo_export_matches_golden() {
    let actual = demo_export_json();

    // Set OPENQPCR_BLESS=1 to (re)generate the fixture after an intentional change.
    if std::env::var_os("OPENQPCR_BLESS").is_some() {
        std::fs::write(golden_path(), format!("{actual}\n")).expect("write golden");
    }

    let expected = std::fs::read_to_string(golden_path()).expect("golden fixture must exist");
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "parsed demo export drifted from tests/fixtures/demo_export.golden.json; \
         if this change is intentional, re-run with OPENQPCR_BLESS=1"
    );
}

#[test]
fn read_path_parses_single_cq_csv() {
    let csv = demo_dir().join("demo - Quantification Cq Results.csv");
    let run = openqpcr::read_path(&csv).expect("single CSV must parse via read_path");
    assert!(!run.wells.is_empty(), "expected at least one well");
}
