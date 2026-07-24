//! Reader trait + registry with **content-based format detection**.
//!
//! Instead of routing purely on a file's extension, [`crate::read_path`] builds a
//! cheap [`Probe`] (extension, first bytes, and — when the file is a ZIP — its
//! member names) and asks every registered [`Reader`] how confidently it can
//! handle the file. The reader with the highest [`Confidence`] wins (ties break
//! toward the earlier entry in [`readers`]).
//!
//! The `zip_entries` hint is what makes routing content-based rather than
//! extension-based: a ZIP archive is recognised as such regardless of its name,
//! so (for example) an `.xlsx` renamed `.dat` still routes correctly, and a
//! future RDML reader can claim a `.pcrd`-named RDML archive by inspecting its
//! members.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use zip::ZipArchive;

use crate::error::{QpcrError, Result};
use crate::model::QpcrRun;
use crate::readers::{export, json, rdml};

/// A cheap, format-agnostic snapshot of a file, used by [`Reader::detect`].
///
/// Building a `Probe` opens the file once for its first bytes and once more to
/// try reading it as a ZIP. It never errors on a non-ZIP file (it simply reports
/// `zip_entries = None`), but it does propagate an [`QpcrError::Io`] when the file
/// cannot be opened at all (e.g. it is missing).
pub struct Probe {
    /// The path that was probed.
    pub path: PathBuf,
    /// Lower-cased file extension (empty if there is none).
    pub extension: String,
    /// The first ~64 bytes of the file, for magic/text sniffing.
    pub head: Vec<u8>,
    /// `Some(entry names)` when the file opens as a ZIP archive, else `None`.
    pub zip_entries: Option<Vec<String>>,
}

impl Probe {
    /// Open `path` and gather detection hints. Cheap; must not fail on non-ZIP
    /// input (a plain text file yields `zip_entries = None`). Fails only when the
    /// file itself cannot be opened (propagated as [`QpcrError::Io`]).
    pub fn open(path: &Path) -> Result<Probe> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        // First ~64 bytes for magic/text sniffing. Opening the file here is what
        // surfaces a missing-file `Io` error, matching the old dispatcher.
        let mut head = Vec::new();
        File::open(path)?.take(64).read_to_end(&mut head)?;

        let zip_entries = read_zip_entries(path);

        Ok(Probe {
            path: path.to_path_buf(),
            extension,
            head,
            zip_entries,
        })
    }
}

/// List a ZIP archive's member names via the raw index (which never decrypts).
/// Returns `None` for anything that does not open as a ZIP.
fn read_zip_entries(path: &Path) -> Option<Vec<String>> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let names = (0..archive.len())
        .filter_map(|i| archive.by_index_raw(i).ok().map(|z| z.name().to_string()))
        .collect();
    Some(names)
}

/// How confidently a [`Reader`] believes it can handle a [`Probe`]. Ordered so
/// the dispatcher can pick the maximum (`No` < `Maybe` < `Strong`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// This reader cannot handle the file.
    No,
    /// Plausible, but another reader may be a better fit.
    Maybe,
    /// A confident, format-specific match.
    Strong,
}

/// A source format reader that populates the shared [`QpcrRun`] model.
pub trait Reader {
    /// A short, stable name for diagnostics.
    fn name(&self) -> &'static str;
    /// Judge how well this reader fits `probe` (no parsing, cheap).
    fn detect(&self, probe: &Probe) -> Confidence;
    /// Parse the probed file into a [`QpcrRun`].
    fn read(&self, probe: &Probe) -> Result<QpcrRun>;
}

/// The registered readers, in tie-break priority order.
///
/// `RdmlReader` is first: it claims a `.rdml`/`.rdm` file or any ZIP whose members
/// include an `<rdml>`-rooted XML document, and `PcrdReader` explicitly defers to
/// it so a `.pcrd`-named RDML archive still routes correctly.
pub fn readers() -> Vec<Box<dyn Reader>> {
    vec![
        Box::new(RdmlReader),
        Box::new(JsonReader),
        Box::new(CfxExportReader),
        Box::new(PcrdReader),
    ]
}

/// Is the probed file an RDML document (by extension, or a ZIP carrying an
/// `<rdml>`-rooted XML member)? Both [`RdmlReader`] and [`PcrdReader`] consult
/// this so exactly one of them claims a ZIP-backed RDML file.
fn probe_is_rdml(probe: &Probe) -> bool {
    matches!(probe.extension.as_str(), "rdml" | "rdm")
        || rdml::is_rdml_zip(&probe.path).unwrap_or(false)
}

/// Does the file's first bytes carry the ZIP local-file-header magic? This
/// matches the historical `pcrd::looks_like_zip` check, which treats the 4-byte
/// magic as "a ZIP archive" even when the archive is truncated/unopenable.
fn looks_like_zip_magic(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04")
}

/// Does `head` look like a CSV/plain-text table (printable text with a comma)?
fn head_looks_like_csv(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    let printable = head
        .iter()
        .all(|&b| b == b'\t' || b == b'\r' || b == b'\n' || (0x20..=0x7e).contains(&b));
    printable && head.contains(&b',')
}

// ---------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------

/// RDML (`.rdml`/`.rdm`, or any ZIP holding an `<rdml>` XML member) reader.
struct RdmlReader;

impl Reader for RdmlReader {
    fn name(&self) -> &'static str {
        "rdml"
    }

    fn detect(&self, probe: &Probe) -> Confidence {
        if probe_is_rdml(probe) {
            Confidence::Strong
        } else {
            Confidence::No
        }
    }

    fn read(&self, probe: &Probe) -> Result<QpcrRun> {
        rdml::read_rdml(&probe.path)
    }
}

/// JSON reader for a previously exported [`QpcrRun`] (the plate editor's native
/// save format). Claims only the `.json` extension.
struct JsonReader;

impl Reader for JsonReader {
    fn name(&self) -> &'static str {
        "json"
    }

    fn detect(&self, probe: &Probe) -> Confidence {
        if probe.extension == "json" {
            Confidence::Strong
        } else {
            Confidence::No
        }
    }

    fn read(&self, probe: &Probe) -> Result<QpcrRun> {
        json::read_json(&probe.path)
    }
}

/// Native Bio-Rad `.pcrd` / generic ZIP archive reader (parsing not yet
/// implemented — it reports the same guidance the old dispatcher did).
struct PcrdReader;

impl Reader for PcrdReader {
    fn name(&self) -> &'static str {
        "pcrd"
    }

    fn detect(&self, probe: &Probe) -> Confidence {
        // Defer RDML archives to `RdmlReader` (a `.pcrd`-named RDML routes there).
        if probe_is_rdml(probe) {
            return Confidence::No;
        }
        // The historical dispatcher treated the 4-byte ZIP magic as "a ZIP
        // archive", even for a truncated archive `ZipArchive` cannot open.
        let looks_zip = probe.zip_entries.is_some() || looks_like_zip_magic(&probe.head);
        if looks_zip {
            // A ZIP archive: strongest when it also carries the `.pcrd` name.
            if probe.extension == "pcrd" {
                Confidence::Strong
            } else {
                Confidence::Maybe
            }
        } else if probe.extension == "pcrd" || probe.extension == "zip" {
            // A mislabeled archive (native extension but not a ZIP): claim it so
            // `read` can emit the specific "not a ZIP-backed .pcrd file" error.
            Confidence::Maybe
        } else {
            Confidence::No
        }
    }

    fn read(&self, probe: &Probe) -> Result<QpcrRun> {
        if probe.zip_entries.is_some() || looks_like_zip_magic(&probe.head) {
            // Decrypt with the global key and parse the inner `<experimentalData2>`
            // XML. `read_file` returns `QpcrError::Encrypted` only for files that
            // additionally carry a user-set open password.
            crate::readers::pcrd::read_file(&probe.path)
        } else {
            Err(QpcrError::UnsupportedFormat(format!(
                "{} has a native archive extension but is not a ZIP-backed .pcrd file",
                probe.path.display()
            )))
        }
    }
}

/// Bio-Rad CFX CSV / Excel export reader.
struct CfxExportReader;

impl Reader for CfxExportReader {
    fn name(&self) -> &'static str {
        "cfx-export"
    }

    fn detect(&self, probe: &Probe) -> Confidence {
        match probe.extension.as_str() {
            "csv" => Confidence::Strong,
            "xlsx" | "xls" => Confidence::Strong,
            _ => {
                // Content sniff: a CSV-looking text file with no telling
                // extension. Never hijack archive files.
                let archive_ext = matches!(
                    probe.extension.as_str(),
                    "pcrd" | "zip" | "rdml" | "rdm" | "json"
                );
                if !archive_ext && probe.zip_entries.is_none() && head_looks_like_csv(&probe.head) {
                    Confidence::Maybe
                } else {
                    Confidence::No
                }
            }
        }
    }

    fn read(&self, probe: &Probe) -> Result<QpcrRun> {
        match probe.extension.as_str() {
            "xlsx" | "xls" => export::read_xlsx(&probe.path),
            // `.csv` and content-sniffed text both parse as CSV.
            _ => export::read_csv(&probe.path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("openqpcr_registry_{}_{}", std::process::id(), name))
    }

    #[test]
    fn confidence_orders_maybe_below_strong() {
        assert!(Confidence::No < Confidence::Maybe);
        assert!(Confidence::Maybe < Confidence::Strong);
    }

    #[test]
    fn probe_reports_none_for_non_zip() {
        let path = tmp("plain.txt");
        std::fs::write(&path, b"hello,world\n1,2\n").unwrap();
        let probe = Probe::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(probe.zip_entries.is_none());
        assert_eq!(probe.extension, "txt");
        // A CSV-looking text file with no telling extension is a soft match.
        assert_eq!(CfxExportReader.detect(&probe), Confidence::Maybe);
    }

    #[test]
    fn probe_lists_zip_entries_regardless_of_extension() {
        // A ZIP archive is recognised by content even when misnamed `.dat`.
        let path = tmp("archive.dat");
        {
            let file = File::create(&path).unwrap();
            let mut zip = ZipWriter::new(file);
            zip.start_file("run.bin", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"\x00\x01\x02").unwrap();
            zip.finish().unwrap();
        }
        let probe = Probe::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(probe.zip_entries.is_some());
        // No `.pcrd`/`.zip` name, but it is a real archive → soft pcrd match.
        assert_eq!(PcrdReader.detect(&probe), Confidence::Maybe);
        assert_eq!(CfxExportReader.detect(&probe), Confidence::No);
    }

    #[test]
    fn plain_pcrd_zip_is_strong_for_pcrd() {
        let path = tmp("native.pcrd");
        {
            let file = File::create(&path).unwrap();
            let mut zip = ZipWriter::new(file);
            zip.start_file("run.bin", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"\x00\x01\x02").unwrap();
            zip.finish().unwrap();
        }
        let probe = Probe::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(PcrdReader.detect(&probe), Confidence::Strong);
        assert_eq!(CfxExportReader.detect(&probe), Confidence::No);
    }

    #[test]
    fn csv_extension_is_strong_for_cfx() {
        let path = tmp("export.csv");
        std::fs::write(&path, b"Well,Cq\nA1,24.5\n").unwrap();
        let probe = Probe::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(CfxExportReader.detect(&probe), Confidence::Strong);
        assert_eq!(PcrdReader.detect(&probe), Confidence::No);
    }
}
