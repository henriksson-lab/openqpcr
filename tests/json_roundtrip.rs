//! Integration test: the native JSON save format round-trips losslessly.
//!
//! Build a `QpcrRun` in code, serialize it with `serde_json::to_string_pretty`,
//! write it to a temp file, read it back with `read_json`, and assert the
//! meaningful fields survive the round trip. This protects the save format.

use std::fs;

use openqpcr::model::{Channel, MeltCurve, PlateFormat, QpcrRun, RunMetadata, SampleType, Well};
use openqpcr::readers::json::read_json;

fn sample_run() -> QpcrRun {
    QpcrRun {
        metadata: RunMetadata {
            instrument: Some("CFX Opus 96".to_string()),
            software_version: Some("openqpcr edit".to_string()),
            cycle_count: Some(2),
            source_file: Some("plate.json".to_string()),
            ..Default::default()
        },
        plate: PlateFormat::P96,
        wells: vec![
            // An unknown sample with an amplification trace and a melt curve.
            Well {
                row: 0,
                col: 0,
                sample: Some("Treated".to_string()),
                sample_type: SampleType::Unknown,
                biological_group: Some("GrpA".to_string()),
                starting_quantity: None,
                channels: vec![Channel {
                    fluorophore: "FAM".to_string(),
                    target: Some("GAPDH".to_string()),
                    cq: Some(24.5),
                    amplification: vec![10.0, 11.5],
                    melt: Some(MeltCurve {
                        temperature: vec![65.0, 66.0],
                        rfu: vec![100.0, 90.0],
                        derivative: vec![1.0, 5.0],
                        peaks: vec![82.5],
                    }),
                }],
            },
            // A standard with a starting quantity.
            Well {
                row: 0,
                col: 1,
                sample: Some("Std1".to_string()),
                sample_type: SampleType::Standard,
                biological_group: None,
                starting_quantity: Some(1000.0),
                channels: vec![Channel {
                    fluorophore: "HEX".to_string(),
                    target: Some("ACTB".to_string()),
                    cq: Some(20.1),
                    amplification: vec![5.0, 6.0, 7.0],
                    melt: None,
                }],
            },
        ],
        protocol: None,
    }
}

#[test]
fn json_save_format_roundtrips() {
    let run = sample_run();

    let json = serde_json::to_string_pretty(&run).expect("serialize");
    let path = std::env::temp_dir().join(format!(
        "openqpcr_json_roundtrip_{}.json",
        std::process::id()
    ));
    fs::write(&path, &json).expect("write temp json");

    let back = read_json(&path).expect("read_json");
    let _ = fs::remove_file(&path);

    // Metadata.
    assert_eq!(back.metadata.instrument.as_deref(), Some("CFX Opus 96"));
    assert_eq!(back.metadata.cycle_count, Some(2));
    assert_eq!(back.metadata.source_file.as_deref(), Some("plate.json"));

    // Plate geometry.
    assert_eq!(back.plate.rows, run.plate.rows);
    assert_eq!(back.plate.cols, run.plate.cols);

    // Wells.
    assert_eq!(back.wells.len(), 2);

    let a1 = back.wells.iter().find(|w| w.position() == "A1").unwrap();
    assert_eq!(a1.sample.as_deref(), Some("Treated"));
    assert_eq!(a1.sample_type, SampleType::Unknown);
    assert_eq!(a1.biological_group.as_deref(), Some("GrpA"));
    assert_eq!(a1.starting_quantity, None);
    assert_eq!(a1.channels.len(), 1);
    let ch = &a1.channels[0];
    assert_eq!(ch.fluorophore, "FAM");
    assert_eq!(ch.target.as_deref(), Some("GAPDH"));
    assert_eq!(ch.cq, Some(24.5));
    assert_eq!(ch.amplification, vec![10.0, 11.5]);
    let melt = ch.melt.as_ref().expect("melt curve");
    assert_eq!(melt.temperature, vec![65.0, 66.0]);
    assert_eq!(melt.rfu, vec![100.0, 90.0]);
    assert_eq!(melt.derivative, vec![1.0, 5.0]);
    assert_eq!(melt.peaks, vec![82.5]);

    let a2 = back.wells.iter().find(|w| w.position() == "A2").unwrap();
    assert_eq!(a2.sample_type, SampleType::Standard);
    assert_eq!(a2.starting_quantity, Some(1000.0));
    assert_eq!(a2.channels[0].fluorophore, "HEX");
    assert!(a2.channels[0].melt.is_none());
}
