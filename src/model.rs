//! Common data model for a qPCR run.
//!
//! Every reader (native `.pcrd`, CSV/Excel export, …) parses into this shared
//! model so the CLI and GUI never have to care where the data came from.

use serde::{Deserialize, Serialize};

/// A single real-time PCR run: metadata plus one entry per physical well.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QpcrRun {
    pub metadata: RunMetadata,
    /// Plate geometry (e.g. 96 or 384 wells).
    pub plate: PlateFormat,
    /// One entry per occupied well. Sparse: empty wells may be omitted.
    pub wells: Vec<Well>,
    /// Thermal cycling program that produced (or will produce) this run.
    /// Additive and omitted from JSON when absent, so existing runs are
    /// unaffected. See [`crate::protocol::ThermalProtocol`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<crate::protocol::ThermalProtocol>,
}

/// Run-level metadata. All optional — different sources populate different fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunMetadata {
    /// Instrument model, e.g. "CFX Opus 96", "CFX Connect", "CFX Duet".
    pub instrument: Option<String>,
    pub serial_number: Option<String>,
    /// Software that produced the file, e.g. "CFX Maestro 2.3".
    pub software_version: Option<String>,
    /// Run start timestamp, kept as the source's raw string (formats vary).
    pub run_started: Option<String>,
    pub run_ended: Option<String>,
    /// Operator / user name.
    pub operator: Option<String>,
    /// Plate barcode or ID.
    pub plate_id: Option<String>,
    /// Physical plate type, e.g. "BR White", "BR Clear".
    pub plate_type: Option<String>,
    /// Free-text notes.
    pub notes: Option<String>,
    /// Number of amplification cycles, if known.
    pub cycle_count: Option<usize>,
    /// Original file name / path this run was read from.
    pub source_file: Option<String>,
}

/// Plate geometry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlateFormat {
    pub rows: u8,
    pub cols: u8,
}

impl Default for PlateFormat {
    fn default() -> Self {
        // 96-well is by far the most common CFX format.
        PlateFormat { rows: 8, cols: 12 }
    }
}

impl PlateFormat {
    pub const P96: PlateFormat = PlateFormat { rows: 8, cols: 12 };
    pub const P384: PlateFormat = PlateFormat { rows: 16, cols: 24 };

    pub fn well_count(&self) -> usize {
        self.rows as usize * self.cols as usize
    }

    /// Infer a standard plate format from a well count.
    pub fn from_well_count(n: usize) -> PlateFormat {
        match n {
            0..=96 => PlateFormat::P96,
            _ => PlateFormat::P384,
        }
    }
}

/// A physical well and everything measured in it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Well {
    /// Zero-based row index (A = 0).
    pub row: u8,
    /// Zero-based column index (well "1" = 0).
    pub col: u8,
    /// User-facing sample name.
    pub sample: Option<String>,
    /// What kind of well this is.
    pub sample_type: SampleType,
    /// Biological grouping / replicate group label.
    pub biological_group: Option<String>,
    /// Starting quantity for standards (used to build the standard curve).
    pub starting_quantity: Option<f64>,
    /// One entry per fluorophore/target measured in this well (multiplex → many).
    pub channels: Vec<Channel>,
}

impl Well {
    /// Human-readable position label, e.g. "A1", "H12".
    pub fn position(&self) -> String {
        format!("{}{}", row_label(self.row), self.col as usize + 1)
    }
}

/// One fluorophore/target's data within a well.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Channel {
    /// Fluorophore/dye, e.g. "FAM", "HEX", "VIC", "Cy5", "TexasRed".
    pub fluorophore: String,
    /// Assay target / gene name, if assigned.
    pub target: Option<String>,
    /// Quantification cycle (Cq / Ct). None if not called (e.g. no amplification).
    pub cq: Option<f64>,
    /// Raw amplification trace: fluorescence (RFU) indexed by cycle (1-based cycle = index 0).
    pub amplification: Vec<f64>,
    /// Melt curve data, if a melt step was run.
    pub melt: Option<MeltCurve>,
}

/// Melt (dissociation) curve for one channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeltCurve {
    /// Temperature axis (°C), parallel to `rfu` and `derivative`.
    pub temperature: Vec<f64>,
    /// Raw fluorescence (RFU) at each temperature.
    pub rfu: Vec<f64>,
    /// Negative derivative -d(RFU)/dT at each temperature (the classic melt peak plot).
    pub derivative: Vec<f64>,
    /// Detected melt peak temperatures (Tm, °C).
    pub peaks: Vec<f64>,
}

/// Well content classification, mirroring CFX "Sample Type".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SampleType {
    #[default]
    Unknown,
    Standard,
    /// No-template control.
    Ntc,
    /// No-reverse-transcriptase control.
    Nrt,
    PositiveControl,
    NegativeControl,
    /// Explicitly empty / not used.
    Empty,
}

impl SampleType {
    /// Parse the free-text "Content"/"Type" label used in CFX exports.
    pub fn parse(s: &str) -> SampleType {
        let t = s.trim().to_ascii_lowercase();
        // CFX export "Content" uses codes like "Unkn", "Std", "NTC", "NRT",
        // "Pos Ctrl", "Neg Ctrl". Match generously.
        match t.as_str() {
            _ if t.starts_with("unk") => SampleType::Unknown,
            _ if t.starts_with("std") || t.contains("standard") => SampleType::Standard,
            _ if t.starts_with("ntc") || t.contains("no template") => SampleType::Ntc,
            _ if t.starts_with("nrt") || t.contains("no rt") => SampleType::Nrt,
            _ if t.contains("pos") => SampleType::PositiveControl,
            _ if t.contains("neg") => SampleType::NegativeControl,
            "" | "empty" | "none" => SampleType::Empty,
            _ => SampleType::Unknown,
        }
    }
}

/// Convert a zero-based row index to its letter label (0→A, 25→Z, 26→AA…).
pub fn row_label(row: u8) -> String {
    // 384-well tops out at row 15 (P), so a single letter suffices in practice,
    // but handle overflow gracefully anyway.
    let mut n = row as usize;
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

/// Parse a well label like "A1" / "H12" / "P24" into zero-based (row, col).
pub fn parse_well_label(label: &str) -> Option<(u8, u8)> {
    let label = label.trim();
    let split = label.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = label.split_at(split);
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let mut row: usize = 0;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        row = row * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    let row = row.checked_sub(1)?;
    let col: usize = digits.parse().ok()?;
    let col = col.checked_sub(1)?;
    Some((u8::try_from(row).ok()?, u8::try_from(col).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_labels() {
        assert_eq!(row_label(0), "A");
        assert_eq!(row_label(7), "H");
        assert_eq!(row_label(15), "P");
    }

    #[test]
    fn well_labels_roundtrip() {
        for (label, rc) in [("A1", (0, 0)), ("H12", (7, 11)), ("P24", (15, 23))] {
            assert_eq!(parse_well_label(label), Some(rc));
            let (r, c) = rc;
            let w = Well {
                row: r,
                col: c,
                ..Default::default()
            };
            assert_eq!(w.position(), label);
        }
    }

    #[test]
    fn sample_type_parse() {
        assert_eq!(SampleType::parse("Unkn"), SampleType::Unknown);
        assert_eq!(SampleType::parse("Std"), SampleType::Standard);
        assert_eq!(SampleType::parse("NTC"), SampleType::Ntc);
        assert_eq!(SampleType::parse("Pos Ctrl"), SampleType::PositiveControl);
    }
}
