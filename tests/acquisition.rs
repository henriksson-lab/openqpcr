//! End-to-end acquisition test — drive `SimulatedInstrument` with NO hardware.
//!
//! Builds a small `QpcrRun` layout + a `ThermalProtocol` (2-step ×N + a melt), then
//! `load` → `start` → drain `RunHandle::poll` until `Finished`, folding events back
//! into the run exactly as the GUI would.

use std::time::Duration;

use openqpcr::instrument::simulated::{SimulatedInstrument, fold_event};
use openqpcr::instrument::{AcquisitionEvent, Instrument, RunState};
use openqpcr::model::{Channel, PlateFormat, QpcrRun, SampleType, Well};
use openqpcr::protocol::{Measure, MeltStep, ProtocolStep, TemperatureStep, ThermalProtocol};

fn fam_well(row: u8, col: u8, st: SampleType, sq: Option<f64>) -> Well {
    Well {
        row,
        col,
        sample_type: st,
        starting_quantity: sq,
        channels: vec![Channel {
            fluorophore: "FAM".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn small_run() -> QpcrRun {
    QpcrRun {
        plate: PlateFormat::P96,
        wells: vec![
            fam_well(0, 0, SampleType::Standard, Some(1e6)),
            fam_well(0, 1, SampleType::Standard, Some(1e4)),
            fam_well(0, 2, SampleType::Standard, Some(1e2)),
            fam_well(0, 3, SampleType::Ntc, None),
            fam_well(1, 0, SampleType::Unknown, None),
            fam_well(1, 1, SampleType::Unknown, None),
        ],
        ..Default::default()
    }
}

/// A 2-step ×N amplification followed by a short melt.
fn protocol(cycles: u32) -> ThermalProtocol {
    ThermalProtocol {
        name: Some("test".to_string()),
        lid_temperature: Some(105.0),
        steps: vec![
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 180.0)),
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 5.0)),
            ProtocolStep::Hold(TemperatureStep::read(60.0, 15.0)),
            ProtocolStep::Loop {
                goto: 1,
                repeat: cycles,
            },
            ProtocolStep::Melt(MeltStep {
                start_c: 70.0,
                end_c: 90.0,
                increment_c: 1.0,
                hold_secs: 2.0,
            }),
        ],
        ..Default::default()
    }
}

/// Drain until `Finished`, folding events into `run`. Returns the terminal state.
fn drive(handle: &mut Box<dyn openqpcr::instrument::RunHandle>, run: &mut QpcrRun) -> RunState {
    loop {
        let events = handle.poll();
        let mut finished = None;
        for ev in &events {
            fold_event(run, ev);
            if let AcquisitionEvent::Finished(st) = ev {
                finished = Some(st.clone());
            }
        }
        if let Some(st) = finished {
            return st;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn full_run_folds_into_qpcrrun() {
    let cycles = 30u32;
    let proto = protocol(cycles);
    let mut run = small_run();

    let mut inst = SimulatedInstrument::default().with_tick(Duration::from_millis(1));
    inst.connect().unwrap();
    inst.load(&proto, &run).unwrap();
    let mut handle = inst.start().unwrap();

    let terminal = drive(&mut handle, &mut run);
    assert_eq!(terminal, RunState::Complete);

    // Every occupied well accrued exactly `amplification_cycles()` RFU points.
    let expected = proto.amplification_cycles();
    assert_eq!(expected, cycles as usize);
    for w in &run.wells {
        let ch = &w.channels[0];
        assert_eq!(
            ch.amplification.len(),
            expected,
            "well {} accrued wrong point count",
            w.position()
        );
    }

    // Standards' end-point RFU is ordered by starting quantity (more → higher).
    let endpoint = |row: u8, col: u8| -> f64 {
        let w = run
            .wells
            .iter()
            .find(|w| w.row == row && w.col == col)
            .unwrap();
        *w.channels[0].amplification.last().unwrap()
    };
    let hi = endpoint(0, 0); // 1e6
    let mid = endpoint(0, 1); // 1e4
    let lo = endpoint(0, 2); // 1e2
    assert!(
        hi > mid && mid > lo,
        "standards not ordered: {hi} {mid} {lo}"
    );

    // A melt curve was produced for an amplified well.
    let std_well = run.wells.iter().find(|w| w.row == 0 && w.col == 0).unwrap();
    let melt = std_well.channels[0].melt.as_ref().expect("melt curve");
    assert!(!melt.temperature.is_empty());
    assert_eq!(melt.temperature.len(), melt.rfu.len());

    // The NTC stayed flat (no amplification): its span is tiny vs a standard's.
    let ntc = run.wells.iter().find(|w| w.row == 0 && w.col == 3).unwrap();
    let ntc_amp = &ntc.channels[0].amplification;
    let ntc_span = ntc_amp.iter().cloned().fold(f64::MIN, f64::max)
        - ntc_amp.iter().cloned().fold(f64::MAX, f64::min);
    let std_span = hi - std_well.channels[0].amplification[0];
    assert!(ntc_span < 20.0, "NTC not flat: span {ntc_span}");
    assert!(
        std_span > 100.0,
        "standard did not amplify: span {std_span}"
    );
}

#[test]
fn abort_mid_run_yields_aborted_and_partial_trace() {
    let proto = protocol(40);
    let mut run = small_run();

    // Slow enough that we can abort before it completes.
    let mut inst = SimulatedInstrument::default().with_tick(Duration::from_millis(10));
    inst.connect().unwrap();
    inst.load(&proto, &run).unwrap();
    let mut handle = inst.start().unwrap();

    // Let a few cycles accrue, then abort.
    let mut collected = 0usize;
    while collected < 3 {
        for ev in handle.poll() {
            fold_event(&mut run, &ev);
            if matches!(ev, AcquisitionEvent::Cycle { .. }) {
                collected += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    handle.abort().unwrap();

    let terminal = drive(&mut handle, &mut run);
    assert_eq!(terminal, RunState::Aborted);

    // Partial trace: some but not all cycles were recorded.
    let w = &run.wells[0];
    let n = w.channels[0].amplification.len();
    assert!(n >= 3, "expected partial trace, got {n}");
    assert!(
        n < proto.amplification_cycles(),
        "abort did not stop early: {n}"
    );
}

#[test]
fn measure_enum_is_used_by_protocol() {
    // Sanity: the read step is a Real measure (guards against a refactor regression).
    let proto = protocol(5);
    let real_reads = proto
        .schedule()
        .iter()
        .filter(|s| s.measure == Measure::Real)
        .count();
    assert_eq!(real_reads, 5);
}
