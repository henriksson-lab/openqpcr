//! Writer for a CFX-style "Quantification Cq Results" CSV.
//!
//! Produces the tall, per-(well, channel) table that CFX Maestro / CFX Manager
//! emit, so the output can be round-tripped back through
//! [`crate::readers::export::read_csv`].

use std::io::Write;
use std::path::Path;

use crate::error::Result;
use crate::model::{QpcrRun, SampleType};

/// Write `run` as a CFX-style "Quantification Cq Results" CSV to `path`.
///
/// One header row followed by one row per (well, channel):
/// `Well,Fluor,Target,Content,Sample,Biological Set Name,Cq,Starting Quantity (SQ)`.
pub fn write_cfx_csv(run: &QpcrRun, path: &Path) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    writeln!(
        file,
        "Well,Fluor,Target,Content,Sample,Biological Set Name,Cq,Starting Quantity (SQ)"
    )?;
    for well in &run.wells {
        let position = well.position();
        let content = content_code(well.sample_type);
        let sample = well.sample.as_deref().unwrap_or("");
        let bio = well.biological_group.as_deref().unwrap_or("");
        let sq = well
            .starting_quantity
            .map(|q| format!("{q}"))
            .unwrap_or_default();
        for channel in &well.channels {
            let target = channel.target.as_deref().unwrap_or("");
            let cq = channel
                .cq
                .map(|c| format!("{c:.2}"))
                .unwrap_or_else(|| "N/A".to_string());
            writeln!(
                file,
                "{},{},{},{},{},{},{},{}",
                csv_field(&position),
                csv_field(&channel.fluorophore),
                csv_field(target),
                csv_field(content),
                csv_field(sample),
                csv_field(bio),
                csv_field(&cq),
                csv_field(&sq),
            )?;
        }
    }
    Ok(())
}

/// CFX "Content" code for a sample type (parses back via [`SampleType::parse`]).
fn content_code(ty: SampleType) -> &'static str {
    match ty {
        SampleType::Unknown => "Unk",
        SampleType::Standard => "Std",
        SampleType::Ntc => "NTC",
        SampleType::Nrt => "NRT",
        SampleType::PositiveControl => "Pos",
        SampleType::NegativeControl => "Neg",
        SampleType::Empty => "",
    }
}

/// Quote a CSV field if it contains a comma, quote, or newline.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, PlateFormat, Well};
    use crate::readers::export;

    #[test]
    fn round_trips_key_fields() {
        let run = QpcrRun {
            metadata: Default::default(),
            plate: PlateFormat::P96,
            protocol: None,
            wells: vec![
                Well {
                    row: 0,
                    col: 0,
                    sample: Some("Ctrl".to_string()),
                    sample_type: SampleType::Unknown,
                    biological_group: Some("Grp1".to_string()),
                    starting_quantity: None,
                    channels: vec![Channel {
                        fluorophore: "FAM".to_string(),
                        target: Some("GAPDH".to_string()),
                        cq: Some(24.5),
                        amplification: Vec::new(),
                        melt: None,
                    }],
                },
                Well {
                    row: 0,
                    col: 1,
                    sample: None,
                    sample_type: SampleType::Ntc,
                    biological_group: None,
                    starting_quantity: None,
                    channels: vec![Channel {
                        fluorophore: "FAM".to_string(),
                        target: Some("GAPDH".to_string()),
                        cq: None,
                        amplification: Vec::new(),
                        melt: None,
                    }],
                },
            ],
        };

        let dir = std::env::temp_dir();
        let path = dir.join(format!("openqpcr-csv-writer-test-{}.csv", std::process::id()));
        write_cfx_csv(&run, &path).unwrap();
        let parsed = export::read_csv(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let a1 = parsed.wells.iter().find(|w| w.position() == "A1").unwrap();
        assert_eq!(a1.sample.as_deref(), Some("Ctrl"));
        assert_eq!(a1.sample_type, SampleType::Unknown);
        assert_eq!(a1.biological_group.as_deref(), Some("Grp1"));
        assert_eq!(a1.channels[0].fluorophore, "FAM");
        assert_eq!(a1.channels[0].target.as_deref(), Some("GAPDH"));
        assert_eq!(a1.channels[0].cq, Some(24.5));

        let a2 = parsed.wells.iter().find(|w| w.position() == "A2").unwrap();
        assert_eq!(a2.sample_type, SampleType::Ntc);
        assert_eq!(a2.channels[0].cq, None);
    }
}
