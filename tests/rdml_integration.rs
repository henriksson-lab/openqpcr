//! Integration test: RDML round-trips through the public `read_path` dispatcher.

use openqpcr::model::{Channel, PlateFormat, QpcrRun, SampleType, Well};
use openqpcr::readers::rdml;

#[test]
fn read_path_routes_rdml_extension() {
    let run = QpcrRun {
        plate: PlateFormat::P96,
        wells: vec![Well {
            row: 0,
            col: 0,
            sample: Some("S1".to_string()),
            sample_type: SampleType::Standard,
            starting_quantity: Some(100.0),
            channels: vec![Channel {
                fluorophore: "FAM".to_string(),
                target: Some("GAPDH".to_string()),
                cq: Some(20.0),
                amplification: vec![1.0, 4.0, 16.0],
                melt: None,
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let path = std::env::temp_dir().join(format!(
        "openqpcr_rdml_integration_{}.rdml",
        std::process::id()
    ));
    rdml::write_rdml(&run, &path).unwrap();

    // Auto-detect via the top-level dispatcher (extension routing).
    let back = openqpcr::read_path(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(back.wells.len(), 1);
    let w = &back.wells[0];
    assert_eq!(w.position(), "A1");
    assert_eq!(w.sample_type, SampleType::Standard);
    assert_eq!(w.starting_quantity, Some(100.0));
    let ch = &w.channels[0];
    assert_eq!(ch.fluorophore, "FAM");
    assert_eq!(ch.target.as_deref(), Some("GAPDH"));
    assert_eq!(ch.cq, Some(20.0));
    assert_eq!(ch.amplification, vec![1.0, 4.0, 16.0]);
}
