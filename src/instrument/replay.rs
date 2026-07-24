//! src/instrument/replay.rs — `ReplayInstrument`, a hardware-free driver that
//! plays a *recorded* run back as if it were acquiring live.
//!
//! Where [`super::SimulatedInstrument`] synthesizes curves from sample types,
//! `ReplayInstrument` streams the real per-cycle fluorescence of an existing
//! [`QpcrRun`] — typically one decoded from a `.pcrd` — one cycle at a time.
//! That gives the whole control/acquisition/analysis pipeline a deterministic,
//! real-data live source for tests, demos, and GUI screenshots without a device.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::error::{QpcrError, Result};
use crate::model::QpcrRun;
use crate::protocol::ThermalProtocol;

use super::{
    AcquisitionEvent, Capabilities, EventRunHandle, Instrument, RunHandle, RunState,
    WellMeltReading, WellReading,
};
use crate::model::PlateFormat;

/// Default simulated wall-clock per replayed cycle. Small so tests stay fast.
const DEFAULT_TICK: Duration = Duration::from_millis(20);

/// A driver that replays a recorded [`QpcrRun`] as a live acquisition.
#[derive(Debug, Clone)]
pub struct ReplayInstrument {
    connected: bool,
    /// The recorded run whose data is streamed back.
    recorded: Option<QpcrRun>,
    tick: Duration,
}

impl Default for ReplayInstrument {
    fn default() -> Self {
        ReplayInstrument {
            connected: false,
            recorded: None,
            tick: DEFAULT_TICK,
        }
    }
}

impl ReplayInstrument {
    /// Build a replayer around an already-loaded run (e.g. `pcrd::read_file(...)`).
    pub fn from_run(run: QpcrRun) -> Self {
        ReplayInstrument {
            recorded: Some(run),
            ..Default::default()
        }
    }

    /// Override the per-cycle tick.
    pub fn set_tick(&mut self, tick: Duration) {
        self.tick = tick;
    }

    /// Builder-style tick override.
    pub fn with_tick(mut self, tick: Duration) -> Self {
        self.tick = tick;
        self
    }

    /// Number of amplification cycles in the recorded run (the longest channel).
    fn recorded_cycles(&self) -> usize {
        self.recorded
            .as_ref()
            .map(max_amplification_len)
            .unwrap_or(0)
    }
}

/// Longest amplification trace across every well/channel of `run`.
fn max_amplification_len(run: &QpcrRun) -> usize {
    run.wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .map(|c| c.amplification.len())
        .max()
        .unwrap_or(0)
}

/// Longest melt trace across every well/channel of `run`.
fn max_melt_len(run: &QpcrRun) -> usize {
    run.wells
        .iter()
        .flat_map(|w| w.channels.iter())
        .filter_map(|c| c.melt.as_ref())
        .map(|m| m.rfu.len())
        .max()
        .unwrap_or(0)
}

impl Instrument for ReplayInstrument {
    fn name(&self) -> &'static str {
        "ReplayInstrument"
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
        // Advertise exactly the channels present in the recorded run.
        let mut channels: Vec<String> = Vec::new();
        if let Some(run) = &self.recorded {
            for w in &run.wells {
                for c in &w.channels {
                    if !channels.contains(&c.fluorophore) {
                        channels.push(c.fluorophore.clone());
                    }
                }
            }
        }
        let max_plate = self
            .recorded
            .as_ref()
            .map(|r| PlateFormat::from_well_count(r.wells.len()))
            .unwrap_or(PlateFormat::P96);
        Capabilities {
            channels,
            max_plate,
            max_ramp_c_per_s: None,
            supports_gradient: false,
            supports_melt: self
                .recorded
                .as_ref()
                .map(|r| max_melt_len(r) > 0)
                .unwrap_or(false),
        }
    }

    /// Accepts the run to replay. The `protocol` argument is ignored — playback is
    /// driven entirely by the recorded data — but a run passed here overrides any
    /// run supplied via [`ReplayInstrument::from_run`], matching the trait's shape.
    fn load(&mut self, _protocol: &ThermalProtocol, run: &QpcrRun) -> Result<()> {
        self.recorded = Some(run.clone());
        Ok(())
    }

    fn start(&mut self) -> Result<Box<dyn RunHandle>> {
        let run = self
            .recorded
            .clone()
            .ok_or_else(|| QpcrError::Parse("no recorded run to replay".to_string()))?;
        let total = self.recorded_cycles();
        if total == 0 && max_melt_len(&run) == 0 {
            return Err(QpcrError::Parse(
                "recorded run has no amplification or melt data to replay".to_string(),
            ));
        }
        let tick = self.tick;

        let (tx, rx): (Sender<AcquisitionEvent>, Receiver<AcquisitionEvent>) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let abort_worker = Arc::clone(&abort);

        let join = thread::spawn(move || {
            replay_worker(run, tick, tx, abort_worker);
        });

        Ok(Box::new(EventRunHandle::new(rx, abort, total, join)))
    }
}

/// Worker: emit one [`AcquisitionEvent::Cycle`] per recorded cycle, then the melt
/// curve one temperature point at a time, then a terminal `Finished`.
fn replay_worker(
    run: QpcrRun,
    tick: Duration,
    tx: Sender<AcquisitionEvent>,
    abort: Arc<AtomicBool>,
) {
    let cycles = max_amplification_len(&run);
    for i in 0..cycles {
        if abort.load(Ordering::SeqCst) {
            let _ = tx.send(AcquisitionEvent::Finished(RunState::Aborted));
            return;
        }
        let mut points = Vec::new();
        for w in &run.wells {
            for c in &w.channels {
                if let Some(&rfu) = c.amplification.get(i) {
                    points.push(WellReading {
                        row: w.row,
                        col: w.col,
                        fluor: c.fluorophore.clone(),
                        rfu,
                    });
                }
            }
        }
        if !points.is_empty()
            && tx
                .send(AcquisitionEvent::Cycle {
                    cycle: i + 1,
                    points,
                })
                .is_err()
        {
            return; // receiver dropped
        }
        thread::sleep(tick);
    }

    // Melt: one batch per temperature index so the curve animates.
    let melt_len = max_melt_len(&run);
    for i in 0..melt_len {
        if abort.load(Ordering::SeqCst) {
            let _ = tx.send(AcquisitionEvent::Finished(RunState::Aborted));
            return;
        }
        let mut points = Vec::new();
        for w in &run.wells {
            for c in &w.channels {
                if let Some(melt) = &c.melt
                    && let (Some(&t), Some(&rfu)) =
                        (melt.temperature.get(i), melt.rfu.get(i))
                    {
                        points.push(WellMeltReading {
                            row: w.row,
                            col: w.col,
                            fluor: c.fluorophore.clone(),
                            temperature: t,
                            rfu,
                        });
                    }
            }
        }
        if !points.is_empty() && tx.send(AcquisitionEvent::Melt { points }).is_err() {
            return;
        }
        thread::sleep(tick);
    }

    let _ = tx.send(AcquisitionEvent::Finished(RunState::Complete));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::simulated::fold_event;
    use crate::model::{Channel, MeltCurve, Well};

    fn recorded_run() -> QpcrRun {
        // Two wells, one SYBR channel each: A1 rises, A2 flat. A1 has a melt.
        let a1 = Well {
            row: 0,
            col: 0,
            channels: vec![Channel {
                fluorophore: "SYBR".into(),
                amplification: vec![10.0, 20.0, 40.0, 80.0],
                melt: Some(MeltCurve {
                    temperature: vec![65.0, 65.5, 66.0],
                    rfu: vec![900.0, 500.0, 100.0],
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let a2 = Well {
            row: 0,
            col: 1,
            channels: vec![Channel {
                fluorophore: "SYBR".into(),
                amplification: vec![5.0, 5.0, 5.0, 5.0],
                ..Default::default()
            }],
            ..Default::default()
        };
        QpcrRun {
            wells: vec![a1, a2],
            ..Default::default()
        }
    }

    /// Drive the replayer to completion and fold every event into a fresh run;
    /// the reconstruction must match the recorded source exactly.
    #[test]
    fn replays_recorded_run_to_completion() {
        let source = recorded_run();
        let mut inst = ReplayInstrument::from_run(source.clone())
            .with_tick(Duration::from_millis(0));
        inst.connect().unwrap();

        let mut handle = inst.start().unwrap();
        // Seed the fold target with the same empty layout (wells, no data).
        let mut rebuilt = QpcrRun {
            wells: source
                .wells
                .iter()
                .map(|w| Well {
                    row: w.row,
                    col: w.col,
                    channels: w
                        .channels
                        .iter()
                        .map(|c| Channel {
                            fluorophore: c.fluorophore.clone(),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        let mut finished = None;
        // Poll until the worker signals a terminal state.
        for _ in 0..10_000 {
            for ev in handle.poll() {
                if let AcquisitionEvent::Finished(state) = &ev {
                    finished = Some(state.clone());
                }
                fold_event(&mut rebuilt, &ev);
            }
            if finished.is_some() {
                break;
            }
            std::thread::yield_now();
        }

        assert_eq!(finished, Some(RunState::Complete));
        let a1 = &rebuilt.wells[0].channels[0];
        assert_eq!(a1.amplification, vec![10.0, 20.0, 40.0, 80.0]);
        let melt = a1.melt.as_ref().unwrap();
        assert_eq!(melt.temperature, vec![65.0, 65.5, 66.0]);
        assert_eq!(melt.rfu, vec![900.0, 500.0, 100.0]);
        assert_eq!(rebuilt.wells[1].channels[0].amplification, vec![5.0; 4]);
    }

    #[test]
    fn reports_channels_and_cycles() {
        let inst = ReplayInstrument::from_run(recorded_run());
        assert_eq!(inst.recorded_cycles(), 4);
        assert_eq!(inst.capabilities().channels, vec!["SYBR".to_string()]);
        assert!(inst.capabilities().supports_melt);
    }

    #[test]
    fn empty_run_cannot_start() {
        let mut inst = ReplayInstrument::from_run(QpcrRun::default());
        assert!(inst.start().is_err());
    }
}
