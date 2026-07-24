//! openqpcr — read Bio-Rad CFX (Connect / Duet / Opus) real-time PCR data.
//!
//! The crate exposes a single shared [`model::QpcrRun`] data model, and a set of
//! readers that populate it from different sources:
//!
//! * [`readers::pcrd`] — native `.pcrd` archive inspection while parsing is
//!   being reverse-engineered.
//! * [`readers::export`] — CFX CSV / Excel exports.
//!
//! Use [`read_path`] to auto-detect and parse a file by extension/content.

pub mod analysis;
pub mod cq;
pub mod edit;
pub mod error;
pub mod instrument;
pub mod model;
pub mod optics;
pub mod protocol;
pub mod protocol_edit;
pub mod readers;
pub mod stdcurve;

pub use error::{QpcrError, Result};
pub use model::QpcrRun;

use std::path::Path;

/// Auto-detect the format of `path` and parse it into a [`QpcrRun`].
///
/// Detection is **content-based**: a [`readers::registry::Probe`] captures the
/// file's extension, first bytes, and (if it is a ZIP) its member names, and each
/// registered [`readers::registry::Reader`] reports a [`readers::registry::Confidence`].
/// The most confident reader wins (ties break toward the earlier reader), so a
/// misnamed archive is routed by its actual contents rather than by extension.
///
/// Currently:
/// * `.rdml` / `.rdm`, or any ZIP holding an `<rdml>` XML member → the RDML reader.
/// * CSV / Excel CFX exports → the export reader.
/// * `.json` (a previously exported [`QpcrRun`]) → the JSON reader.
/// * `.pcrd` / `.zip` archives (or any file that opens as a ZIP) → unsupported for
///   summary/JSON until the native layout is mapped; use `openqpcr inspect`.
pub fn read_path<P: AsRef<Path>>(path: P) -> Result<QpcrRun> {
    let path = path.as_ref();
    let probe = readers::registry::Probe::open(path)?;

    // Pick the reader with the highest confidence; on a tie the earlier reader in
    // `readers()` wins (strict `>` keeps the first maximum).
    let readers = readers::registry::readers();
    let mut best: Option<usize> = None;
    let mut best_confidence = readers::registry::Confidence::No;
    for (i, reader) in readers.iter().enumerate() {
        let confidence = reader.detect(&probe);
        if confidence > best_confidence {
            best_confidence = confidence;
            best = Some(i);
        }
    }

    match best {
        Some(i) if best_confidence >= readers::registry::Confidence::Maybe => {
            readers[i].read(&probe)
        }
        _ => Err(QpcrError::UnsupportedFormat(format!(
            "{} (unknown extension {:?})",
            path.display(),
            probe.extension
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn read_path_errors_on_truncated_native_archive() {
        // `.pcrd` files are now parsed (decrypt + XML), so a bare ZIP signature
        // with no archive body surfaces as a ZIP error rather than the old
        // "parsing is not implemented" guidance.
        let path =
            std::env::temp_dir().join(format!("openqpcr_native_probe_{}.pcrd", std::process::id()));
        fs::write(&path, b"PK\x03\x04").unwrap();

        let err = read_path(&path).unwrap_err();
        let _ = fs::remove_file(&path);

        assert!(matches!(err, QpcrError::Zip(_) | QpcrError::Io(_)));
    }

    #[test]
    fn read_path_reports_missing_native_archive_as_io_error() {
        let path = std::env::temp_dir().join(format!(
            "openqpcr_missing_native_probe_{}.pcrd",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        let err = read_path(&path).unwrap_err();

        assert!(matches!(err, QpcrError::Io(_)));
    }

    #[test]
    fn read_path_rejects_mislabeled_native_archive_without_inspect_hint() {
        let path = std::env::temp_dir().join(format!(
            "openqpcr_mislabeled_native_probe_{}.pcrd",
            std::process::id()
        ));
        fs::write(&path, b"not a zip").unwrap();

        let err = read_path(&path).unwrap_err().to_string();
        let _ = fs::remove_file(&path);

        assert!(err.contains("not a ZIP-backed .pcrd file"));
        assert!(!err.contains("openqpcr inspect"));
    }
}
