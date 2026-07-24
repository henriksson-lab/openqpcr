//! src/instrument/simulated.rs — `SimulatedInstrument`, the first (hardware-free) driver.
//!
//! Generates synthetic amplification + melt data as a run progresses so the entire
//! control & acquisition pipeline can be built and tested with no hardware.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::error::{QpcrError, Result};
use crate::model::{Channel, PlateFormat, QpcrRun, SampleType, Well};
use crate::protocol::{Measure, ThermalProtocol};
use crate::protocol_edit;

use super::{
    AcquisitionEvent, Capabilities, EventRunHandle, Instrument, RunHandle, RunState, StatusSnapshot,
    WellMeltReading, WellReading,
};

/// An optional fault the simulator injects, so the UI and analysis paths can be
/// exercised against error and edge-case runs without hardware.
#[derive(Debug, Clone, PartialEq)]
pub enum SimFault {
    /// The run fails at the start of the given amplification cycle (1-based) with
    /// a terminal [`RunState::Error`] — e.g. a lid or thermal fault mid-run.
    ErrorAtCycle { cycle: usize, message: String },
    /// A single well produces no signal (stays at baseline) for the whole run —
    /// a dead-well / bubble edge case.
    WellDropout { row: u8, col: u8 },
}

/// Default simulated time per amplification cycle. Small so tests run fast; the real
/// value only matters for the "live" feel in the GUI.
const DEFAULT_TICK: Duration = Duration::from_millis(20);

/// A synthetic thermocycler. All `Send`-safe so the worker thread can own the data.
#[derive(Debug, Clone)]
pub struct SimulatedInstrument {
    connected: bool,
    protocol: Option<ThermalProtocol>,
    /// Occupied wells with their per-channel "true Cq" models, derived at `load`.
    wells: Vec<SimWell>,
    /// Simulated wall-clock per amplification cycle.
    tick: Duration,
    /// Optional injected fault for error/edge-case testing.
    fault: Option<SimFault>,
}

impl Default for SimulatedInstrument {
    fn default() -> Self {
        SimulatedInstrument {
            connected: false,
            protocol: None,
            wells: Vec::new(),
            tick: DEFAULT_TICK,
            fault: None,
        }
    }
}

#[derive(Debug, Clone)]
struct SimWell {
    row: u8,
    col: u8,
    channels: Vec<SimChannel>,
}

#[derive(Debug, Clone)]
struct SimChannel {
    fluor: String,
    /// None = never amplifies (NTC / negative / no-RT / empty): stays at baseline.
    true_cq: Option<f64>,
    /// Melt temperature (°C) for the amplified product, if any.
    tm_c: f64,
}

impl SimulatedInstrument {
    /// Override the per-cycle tick (e.g. very small in tests).
    pub fn set_tick(&mut self, tick: Duration) {
        self.tick = tick;
    }

    /// Builder-style tick override.
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Inject a fault to exercise error / edge-case handling. See [`SimFault`].
    pub fn with_fault(mut self, fault: SimFault) -> Self {
        self.fault = Some(fault);
        self
    }
}

// ---- Cq / signal model -----------------------------------------------------

/// Deterministic 64-bit hash (FNV-1a) of a well+channel identity — our seeded PRNG.
fn seed(row: u8, col: u8, fluor: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in [row, col] {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for b in fluor.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Derive the "true Cq" for a well/channel from its sample type + starting quantity.
fn derive_cq(
    sample_type: SampleType,
    starting_quantity: Option<f64>,
    row: u8,
    col: u8,
    fluor: &str,
) -> Option<f64> {
    match sample_type {
        // No template / no amplification: flat, never crosses threshold.
        SampleType::Ntc | SampleType::Nrt | SampleType::NegativeControl | SampleType::Empty => None,
        // Standards follow the dilution series: higher SQ → earlier Cq.
        SampleType::Standard => match starting_quantity {
            Some(sq) if sq > 0.0 => Some(40.0 - 3.32 * sq.log10()),
            _ => Some(25.0),
        },
        // A positive control amplifies early and reliably.
        SampleType::PositiveControl => Some(18.0),
        // Unknowns: a deterministic pseudo-random Cq in [20, 32).
        SampleType::Unknown => {
            let h = seed(row, col, fluor);
            Some(20.0 + (h % 1200) as f64 / 100.0)
        }
    }
}

/// Logistic amplification RFU at `cycle` (1-based) for a given true Cq.
fn amp_rfu(cycle: usize, true_cq: Option<f64>, seed_val: u64) -> f64 {
    let baseline = 5.0 + (seed_val % 7) as f64 * 0.1;
    match true_cq {
        None => baseline + tiny_noise(seed_val, cycle),
        Some(cq) => {
            let amplitude = 1000.0;
            let k = 0.45;
            let x = k * (cycle as f64 - cq);
            let logistic = 1.0 / (1.0 + (-x).exp());
            baseline + amplitude * logistic + tiny_noise(seed_val, cycle)
        }
    }
}

/// Melt-curve RFU at temperature `t`: high while double-stranded, dropping at Tm.
fn melt_rfu(t: f64, true_cq: Option<f64>, tm_c: f64, seed_val: u64) -> f64 {
    let baseline = 5.0 + (seed_val % 7) as f64 * 0.1;
    match true_cq {
        // No product → flat low melt signal.
        None => baseline + tiny_noise(seed_val, t as usize),
        Some(_) => {
            let height = 900.0;
            let width = 1.2;
            let frac = 1.0 / (1.0 + ((t - tm_c) / width).exp());
            baseline + height * frac
        }
    }
}

/// Small deterministic, bounded "noise" so traces are not perfectly smooth.
fn tiny_noise(seed_val: u64, step: usize) -> f64 {
    let h = seed_val ^ (step as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    ((h >> 8) % 100) as f64 / 50.0 // 0.0 .. ~2.0
}

// ---- Instrument impl -------------------------------------------------------

impl Instrument for SimulatedInstrument {
    fn name(&self) -> &'static str {
        "SimulatedInstrument"
    }

    fn connect(&mut self) -> Result<()> {
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            channels: ["FAM", "HEX", "VIC", "Cy5", "TexasRed", "ROX"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_plate: PlateFormat::P384,
            max_ramp_c_per_s: Some(5.0),
            supports_gradient: true,
            supports_melt: true,
        }
    }

    fn load(&mut self, protocol: &ThermalProtocol, run: &QpcrRun) -> Result<()> {
        protocol_edit::validate(protocol)
            .map_err(|e| QpcrError::Parse(format!("invalid thermal protocol: {e}")))?;

        let mut wells = Vec::new();
        for w in &run.wells {
            if w.sample_type == SampleType::Empty && w.channels.is_empty() {
                continue;
            }
            let channels = w
                .channels
                .iter()
                .map(|c| {
                    let true_cq = derive_cq(
                        w.sample_type,
                        w.starting_quantity,
                        w.row,
                        w.col,
                        &c.fluorophore,
                    );
                    let tm_c = 78.0 + (seed(w.row, w.col, &c.fluorophore) % 1200) as f64 / 100.0;
                    SimChannel {
                        fluor: c.fluorophore.clone(),
                        true_cq,
                        tm_c,
                    }
                })
                .collect::<Vec<_>>();
            if !channels.is_empty() {
                wells.push(SimWell {
                    row: w.row,
                    col: w.col,
                    channels,
                });
            }
        }
        self.protocol = Some(protocol.clone());
        self.wells = wells;
        Ok(())
    }

    fn start(&mut self) -> Result<Box<dyn RunHandle>> {
        let protocol = self
            .protocol
            .clone()
            .ok_or_else(|| QpcrError::Parse("no protocol loaded".to_string()))?;
        let total = protocol.amplification_cycles();
        let schedule = protocol.schedule();
        let wells = self.wells.clone();
        let tick = self.tick;
        let fault = self.fault.clone();

        let (tx, rx): (Sender<AcquisitionEvent>, Receiver<AcquisitionEvent>) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let abort_worker = Arc::clone(&abort);

        let join = thread::spawn(move || {
            run_worker(schedule, wells, tick, fault, tx, abort_worker);
        });

        Ok(Box::new(EventRunHandle::new(rx, abort, total, join)))
    }
}

/// True when a channel is silenced by a [`SimFault::WellDropout`].
fn is_dropped(fault: &Option<SimFault>, row: u8, col: u8) -> bool {
    matches!(fault, Some(SimFault::WellDropout { row: r, col: c }) if *r == row && *c == col)
}

/// The worker thread body — owns only `Send` types.
fn run_worker(
    schedule: Vec<crate::protocol::ScheduledStep>,
    wells: Vec<SimWell>,
    tick: Duration,
    fault: Option<SimFault>,
    tx: Sender<AcquisitionEvent>,
    abort: Arc<AtomicBool>,
) {
    let mut melt_points: Vec<WellMeltReading> = Vec::new();

    for sched in &schedule {
        if abort.load(Ordering::SeqCst) {
            let _ = tx.send(AcquisitionEvent::Finished(RunState::Aborted));
            return;
        }
        match sched.measure {
            Measure::Real => {
                let cycle = sched.cycle;
                // Injected mid-run fault: emit a terminal error and stop.
                if let Some(SimFault::ErrorAtCycle { cycle: fc, message }) = &fault
                    && cycle == *fc {
                        let _ = tx.send(AcquisitionEvent::Status(StatusSnapshot {
                            state: RunState::Error(message.clone()),
                            block_temp_c: None,
                            lid_temp_c: None,
                            message: Some(message.clone()),
                        }));
                        let _ =
                            tx.send(AcquisitionEvent::Finished(RunState::Error(message.clone())));
                        return;
                    }
                let mut points = Vec::new();
                for w in &wells {
                    for ch in &w.channels {
                        let s = seed(w.row, w.col, &ch.fluor);
                        // A dropped-out well never crosses threshold.
                        let true_cq = if is_dropped(&fault, w.row, w.col) {
                            None
                        } else {
                            ch.true_cq
                        };
                        points.push(WellReading {
                            row: w.row,
                            col: w.col,
                            fluor: ch.fluor.clone(),
                            rfu: amp_rfu(cycle, true_cq, s),
                        });
                    }
                }
                if tx.send(AcquisitionEvent::Cycle { cycle, points }).is_err() {
                    return; // receiver dropped
                }
                thread::sleep(tick);
            }
            Measure::Meltcurve => {
                let t = sched.target_c;
                for w in &wells {
                    for ch in &w.channels {
                        let s = seed(w.row, w.col, &ch.fluor);
                        let true_cq = if is_dropped(&fault, w.row, w.col) {
                            None
                        } else {
                            ch.true_cq
                        };
                        melt_points.push(WellMeltReading {
                            row: w.row,
                            col: w.col,
                            fluor: ch.fluor.clone(),
                            temperature: t,
                            rfu: melt_rfu(t, true_cq, ch.tm_c, s),
                        });
                    }
                }
            }
            Measure::None => {}
        }
    }

    if abort.load(Ordering::SeqCst) {
        let _ = tx.send(AcquisitionEvent::Finished(RunState::Aborted));
        return;
    }

    if !melt_points.is_empty() {
        let _ = tx.send(AcquisitionEvent::Melt {
            points: melt_points,
        });
        thread::sleep(tick);
    }

    let _ = tx.send(AcquisitionEvent::Finished(RunState::Complete));
}

/// Fold an [`AcquisitionEvent::Cycle`]/`Melt` into a [`QpcrRun`] — the same operation
/// the GUI performs on the UI thread. Provided as a pure helper so tests (and the GUI)
/// share one implementation.
pub fn fold_event(run: &mut QpcrRun, ev: &AcquisitionEvent) {
    match ev {
        AcquisitionEvent::Cycle { points, .. } => {
            for p in points {
                let ch = channel_mut(run, p.row, p.col, &p.fluor);
                ch.amplification.push(p.rfu);
            }
        }
        AcquisitionEvent::Melt { points } => {
            for p in points {
                let ch = channel_mut(run, p.row, p.col, &p.fluor);
                let melt = ch.melt.get_or_insert_with(Default::default);
                melt.temperature.push(p.temperature);
                melt.rfu.push(p.rfu);
            }
        }
        _ => {}
    }
}

/// Get (or materialize) the channel for `(row, col, fluor)` in `run`.
fn channel_mut<'a>(run: &'a mut QpcrRun, row: u8, col: u8, fluor: &str) -> &'a mut Channel {
    let widx = match run.wells.iter().position(|w| w.row == row && w.col == col) {
        Some(i) => i,
        None => {
            run.wells.push(Well {
                row,
                col,
                ..Default::default()
            });
            run.wells.len() - 1
        }
    };
    let well = &mut run.wells[widx];
    let cidx = match well.channels.iter().position(|c| c.fluorophore == fluor) {
        Some(i) => i,
        None => {
            well.channels.push(Channel {
                fluorophore: fluor.to_string(),
                ..Default::default()
            });
            well.channels.len() - 1
        }
    };
    &mut well.channels[cidx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standards_are_ordered_by_starting_quantity() {
        // More template → earlier Cq → higher endpoint RFU at a fixed cycle.
        let cq_hi = derive_cq(SampleType::Standard, Some(1e6), 0, 0, "FAM").unwrap();
        let cq_lo = derive_cq(SampleType::Standard, Some(1e2), 0, 1, "FAM").unwrap();
        assert!(cq_hi < cq_lo);
        let end_hi = amp_rfu(40, Some(cq_hi), 1);
        let end_lo = amp_rfu(40, Some(cq_lo), 1);
        assert!(end_hi > end_lo);
    }

    #[test]
    fn ntc_never_amplifies() {
        assert_eq!(derive_cq(SampleType::Ntc, None, 3, 4, "FAM"), None);
        let flat0 = amp_rfu(1, None, 42);
        let flat40 = amp_rfu(40, None, 42);
        assert!((flat0 - flat40).abs() < 5.0); // stays near baseline
    }

    #[test]
    fn unknown_cq_is_deterministic() {
        let a = derive_cq(SampleType::Unknown, None, 5, 6, "FAM");
        let b = derive_cq(SampleType::Unknown, None, 5, 6, "FAM");
        assert_eq!(a, b);
        assert!(a.unwrap() >= 20.0 && a.unwrap() < 32.0);
    }

    fn two_well_run() -> QpcrRun {
        QpcrRun {
            wells: vec![
                Well {
                    row: 0,
                    col: 0,
                    sample_type: SampleType::Unknown,
                    channels: vec![Channel {
                        fluorophore: "FAM".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Well {
                    row: 0,
                    col: 1,
                    sample_type: SampleType::Unknown,
                    channels: vec![Channel {
                        fluorophore: "FAM".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    /// Drive a started run to a terminal state, folding cycle data as we go.
    fn drive(mut handle: Box<dyn RunHandle>, run: &mut QpcrRun) -> RunState {
        for _ in 0..100_000 {
            let mut terminal = None;
            for ev in handle.poll() {
                if let AcquisitionEvent::Finished(st) = &ev {
                    terminal = Some(st.clone());
                }
                fold_event(run, &ev);
            }
            if let Some(st) = terminal {
                return st;
            }
            std::thread::yield_now();
        }
        panic!("run did not terminate");
    }

    #[test]
    fn error_fault_terminates_run() {
        let protocol = crate::protocol::standard_2step_x40();
        let mut inst = SimulatedInstrument::default()
            .with_tick(Duration::from_millis(0))
            .with_fault(SimFault::ErrorAtCycle {
                cycle: 3,
                message: "lid fault".into(),
            });
        inst.load(&protocol, &two_well_run()).unwrap();
        let handle = inst.start().unwrap();
        let mut rebuilt = two_well_run();
        let state = drive(handle, &mut rebuilt);
        assert!(matches!(state, RunState::Error(m) if m == "lid fault"));
        // The run stopped at cycle 3, so only cycles 1..=2 were recorded.
        assert_eq!(rebuilt.wells[0].channels[0].amplification.len(), 2);
    }

    #[test]
    fn well_dropout_stays_flat() {
        let protocol = crate::protocol::standard_2step_x40();
        let mut inst = SimulatedInstrument::default()
            .with_tick(Duration::from_millis(0))
            .with_fault(SimFault::WellDropout { row: 0, col: 0 });
        inst.load(&protocol, &two_well_run()).unwrap();
        let handle = inst.start().unwrap();
        let mut rebuilt = two_well_run();
        let state = drive(handle, &mut rebuilt);
        assert_eq!(state, RunState::Complete);
        let dropped = &rebuilt.wells[0].channels[0].amplification;
        let live = &rebuilt.wells[1].channels[0].amplification;
        // Dropped well never rises; the other well amplifies far above baseline.
        let dropped_rise = dropped.last().unwrap() - dropped.first().unwrap();
        let live_rise = live.last().unwrap() - live.first().unwrap();
        assert!(dropped_rise.abs() < 10.0, "dropped rise {dropped_rise}");
        assert!(live_rise > 100.0, "live rise {live_rise}");
    }

    #[test]
    fn load_rejects_invalid_protocol() {
        let protocol = ThermalProtocol {
            steps: vec![
                crate::protocol::ProtocolStep::Hold(crate::protocol::TemperatureStep::hold(
                    95.0, 10.0,
                )),
                crate::protocol::ProtocolStep::Loop {
                    goto: 0,
                    repeat: 40,
                },
            ],
            ..Default::default()
        };
        let mut instrument = SimulatedInstrument::default();
        let err = instrument
            .load(&protocol, &QpcrRun::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid thermal protocol"));
    }
}
