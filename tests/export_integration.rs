//! Integration test: parse a full multi-file CFX export directory.

use openqpcr::model::SampleType;
use openqpcr::readers::export;
use std::fs;
use std::path::Path;

#[test]
fn reads_full_export_dir() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cfx_export");
    let run = export::read_export_dir(&dir).expect("parse export dir");

    // Three occupied wells: A1, A2, B1.
    assert_eq!(run.wells.len(), 3);
    assert_eq!(run.metadata.cycle_count, Some(5));

    let a1 = run.wells.iter().find(|w| w.position() == "A1").unwrap();
    assert_eq!(a1.sample.as_deref(), Some("Treated"));
    assert_eq!(a1.biological_group.as_deref(), Some("GrpA"));
    let ch = &a1.channels[0];
    assert_eq!(ch.fluorophore, "SYBR");
    assert_eq!(ch.target.as_deref(), Some("GAPDH"));
    assert_eq!(ch.cq, Some(28.3));
    assert_eq!(ch.amplification.len(), 5);
    // Melt derivative was merged into the same channel.
    let melt = ch.melt.as_ref().expect("melt curve");
    assert_eq!(melt.derivative.len(), 4);
    assert_eq!(melt.temperature.len(), 4);

    // A2 is an NTC with no Cq called.
    let a2 = run.wells.iter().find(|w| w.position() == "A2").unwrap();
    assert_eq!(a2.sample_type, SampleType::Ntc);
    assert_eq!(a2.channels[0].cq, None);
}

#[test]
fn export_dir_rejects_unrecognised_csv() {
    let dir = std::env::temp_dir().join(format!("openqpcr_bad_export_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(dir.join("notes.csv"), "this,is,not,a,cfx,table\n").unwrap();

    let err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);
    assert!(err.contains("failed to parse one or more CFX export CSVs"));
    assert!(err.contains("notes.csv"));
}

#[test]
fn export_dir_ignores_empty_optional_side_tables() {
    let dir = std::env::temp_dir().join(format!(
        "openqpcr_optional_empty_export_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("Melt Peak Results_SYBR.csv"),
        "Well,Fluor,Melt Temperature\nA1,SYBR,\n",
    )
    .unwrap();
    fs::write(
        dir.join("PlateLayout.csv"),
        "Well,Sample,Content,Target\nA1,,,\n",
    )
    .unwrap();

    let run = export::read_export_dir(&dir).expect("parse export dir");
    let peak_err = export::read_csv(&dir.join("Melt Peak Results_SYBR.csv"))
        .unwrap_err()
        .to_string();
    let layout_err = export::read_csv(&dir.join("PlateLayout.csv"))
        .unwrap_err()
        .to_string();
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(run.wells.len(), 1);
    assert!(peak_err.contains("melt peaks table has no data rows"));
    assert!(layout_err.contains("plate layout table has no data rows"));
}

#[test]
fn export_dir_ignores_header_only_optional_side_tables() {
    let dir = std::env::temp_dir().join(format!(
        "openqpcr_header_only_optional_export_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("Melt Peak Results_SYBR.csv"),
        "Well,Fluor,Melt Temperature\n",
    )
    .unwrap();
    fs::write(dir.join("PlateLayout.csv"), "Well,Sample,Content,Target\n").unwrap();

    let run = export::read_export_dir(&dir).expect("parse export dir");
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(run.wells.len(), 1);
}

#[test]
fn export_dir_rejects_malformed_optional_side_tables() {
    let dir = std::env::temp_dir().join(format!(
        "openqpcr_malformed_optional_export_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("Melt Peak Results_SYBR.csv"),
        "Well,Fluor\nA1,SYBR\n",
    )
    .unwrap();

    let err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);

    assert!(err.contains("Melt Peak Results_SYBR.csv"));
    assert!(err.contains("melt peaks table missing Melt Temperature column"));
}

#[test]
fn export_dir_rejects_invalid_optional_peak_values() {
    let dir = std::env::temp_dir().join(format!(
        "openqpcr_invalid_peak_optional_export_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("Melt Peak Results_SYBR.csv"),
        "Well,Fluor,Melt Temperature\nA1,SYBR,not-a-number\n",
    )
    .unwrap();

    let err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);

    assert!(err.contains("Melt Peak Results_SYBR.csv"));
    assert!(err.contains("invalid Melt Temperature value"));
}

#[test]
fn export_dir_rejects_optional_side_tables_without_valid_wells() {
    let dir = std::env::temp_dir().join(format!(
        "openqpcr_invalid_well_optional_export_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("Quantification Cq Results.csv"),
        "Well,Fluor,Target,Content,Sample,Cq\nA1,SYBR,GAPDH,Unkn,S1,25.0\n",
    )
    .unwrap();
    fs::write(
        dir.join("PlateLayout.csv"),
        "Well,Sample,Content,Target\nnot-a-well,S1,Unkn,GAPDH\n",
    )
    .unwrap();

    let err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);

    assert!(err.contains("PlateLayout.csv"));
    assert!(err.contains("plate layout table has no valid Well rows"));
}

#[test]
fn rejects_empty_csv_inputs() {
    let dir = std::env::temp_dir().join(format!("openqpcr_empty_export_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let csv = dir.join("empty.csv");
    fs::write(&csv, "").unwrap();

    let single_err = export::read_csv(&csv).unwrap_err().to_string();
    let dir_err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);

    assert!(single_err.contains("empty export table"));
    assert!(dir_err.contains("empty export table"));
}

#[test]
fn rejects_run_info_only_exports() {
    let dir = std::env::temp_dir().join(format!("openqpcr_run_info_only_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let csv = dir.join("Run Information.csv");
    fs::write(
        &csv,
        "Run Information,\nInstrument,CFX Opus\nPlate Format,96 Wells\n",
    )
    .unwrap();

    let single_err = export::read_csv(&csv).unwrap_err().to_string();
    let dir_err = export::read_export_dir(&dir).unwrap_err().to_string();
    let _ = fs::remove_dir_all(&dir);

    assert!(single_err.contains("no data-bearing CFX export table"));
    assert!(dir_err.contains("no data-bearing CFX export CSVs"));
}
