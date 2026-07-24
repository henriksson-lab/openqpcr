//! src/instrument/mod.rs — instrument-control architecture (driver trait + registry).
//!
//! Write-side analogue of the format readers: an [`Instrument`] driver loads a
//! [`ThermalProtocol`] + a [`QpcrRun`] layout, `start`s a run, and hands back a
//! [`RunHandle`] that streams [`AcquisitionEvent`]s as the run progresses.
//!
//! Two hardware-free drivers live here: [`simulated::SimulatedInstrument`], which
//! synthesizes curves from the plate's sample types, and
//! [`replay::ReplayInstrument`], which plays a *recorded* run (e.g. a decoded
//! `.pcrd`) back cycle-by-cycle as if it were acquiring live. A real vendor driver
//! (e.g. a Chai Open qPCR HTTP client, or a future CFX USB driver) slots in behind
//! the same trait — see [`instruments`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use crate::error::Result;
use crate::model::{PlateFormat, QpcrRun};
use crate::protocol::ThermalProtocol;

pub mod replay;
pub mod simulated;

pub use replay::ReplayInstrument;
pub use simulated::SimulatedInstrument;

/// A run's live lifecycle. Drives UI state.
#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    Idle,
    /// Protocol + plate accepted, not started.
    Loaded,
    Running {
        cycle: usize,
        total: usize,
    },
    Melt,
    Complete,
    Aborted,
    Error(String),
}

/// A snapshot the driver hands back on each poll.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub state: RunState,
    pub block_temp_c: Option<f64>,
    pub lid_temp_c: Option<f64>,
    pub message: Option<String>,
}

/// Incremental data pushed to the UI thread as a run progresses.
#[derive(Debug, Clone)]
pub enum AcquisitionEvent {
    Status(StatusSnapshot),
    /// New per-cycle fluorescence: fold into the matching well/channel.
    Cycle {
        cycle: usize,
        points: Vec<WellReading>,
    },
    /// A melt point batch.
    Melt {
        points: Vec<WellMeltReading>,
    },
    /// Terminal transition: `Complete` | `Aborted` | `Error`.
    Finished(RunState),
}

/// One amplification read: fluorescence for a fluorophore in a well at a cycle.
#[derive(Debug, Clone)]
pub struct WellReading {
    pub row: u8,
    pub col: u8,
    pub fluor: String,
    pub rfu: f64,
}

/// One melt read: fluorescence for a fluorophore in a well at a temperature.
#[derive(Debug, Clone)]
pub struct WellMeltReading {
    pub row: u8,
    pub col: u8,
    pub fluor: String,
    pub temperature: f64,
    pub rfu: f64,
}

/// What an instrument can physically do.
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// Fluorophore channels the optics support.
    pub channels: Vec<String>,
    pub max_plate: PlateFormat,
    pub max_ramp_c_per_s: Option<f64>,
    pub supports_gradient: bool,
    pub supports_melt: bool,
}

/// Handle to a started run; the UI polls/receives events and can abort.
pub trait RunHandle: Send {
    /// Non-blocking drain of any events emitted since the last poll.
    fn poll(&mut self) -> Vec<AcquisitionEvent>;
    /// Request that the run stop; a `Finished(Aborted)` event will follow.
    fn abort(&mut self) -> Result<()>;
    /// Current lifecycle state (updated as events are drained).
    fn state(&self) -> RunState;
}

/// A controllable instrument driver.
pub trait Instrument {
    fn name(&self) -> &'static str;
    /// Probe/connect (e.g. HTTP reachability for a networked device; always-ok for
    /// the simulator).
    fn connect(&mut self) -> Result<()>;
    fn disconnect(&mut self) -> Result<()>;
    /// Channels, plate sizes, max ramp, …
    fn capabilities(&self) -> Capabilities;
    /// Load a program + plate layout; validate against capabilities.
    fn load(&mut self, protocol: &ThermalProtocol, run: &QpcrRun) -> Result<()>;
    /// Start; returns a `Send` handle the worker thread drives.
    fn start(&mut self) -> Result<Box<dyn RunHandle>>;
}

/// Registry of available instrument drivers — same shape as the format-reader
/// registry.
///
/// Only the hardware-free [`SimulatedInstrument`] is registered today. A real driver
/// (e.g. a `ChaiInstrument` HTTP client against the Chai Open qPCR REST API, or a
/// future licensed vendor bridge) implements [`Instrument`] and is added here without
/// touching the rest of the app.
pub fn instruments() -> Vec<Box<dyn Instrument>> {
    vec![
        Box::new(SimulatedInstrument::default()),
        // ReplayInstrument needs a recorded run, so it is constructed on demand
        // (e.g. from a `.pcrd`) rather than registered as a default.
        // v3: Box::new(ChaiInstrument::new(addr)),
    ]
}

/// Shared [`RunHandle`] for the worker-thread drivers ([`SimulatedInstrument`],
/// [`ReplayInstrument`]): it drains an [`AcquisitionEvent`] channel, tracks the
/// derived [`RunState`], and joins the worker on drop.
pub(crate) struct EventRunHandle {
    rx: Receiver<AcquisitionEvent>,
    abort: Arc<AtomicBool>,
    state: RunState,
    total: usize,
    join: Option<JoinHandle<()>>,
}

impl EventRunHandle {
    pub(crate) fn new(
        rx: Receiver<AcquisitionEvent>,
        abort: Arc<AtomicBool>,
        total: usize,
        join: JoinHandle<()>,
    ) -> Self {
        EventRunHandle {
            rx,
            abort,
            state: RunState::Running { cycle: 0, total },
            total,
            join: Some(join),
        }
    }

    fn update_state(&mut self, ev: &AcquisitionEvent) {
        match ev {
            AcquisitionEvent::Status(s) => self.state = s.state.clone(),
            AcquisitionEvent::Cycle { cycle, .. } => {
                self.state = RunState::Running {
                    cycle: *cycle,
                    total: self.total,
                }
            }
            AcquisitionEvent::Melt { .. } => self.state = RunState::Melt,
            AcquisitionEvent::Finished(st) => self.state = st.clone(),
        }
    }
}

impl RunHandle for EventRunHandle {
    fn poll(&mut self) -> Vec<AcquisitionEvent> {
        let mut out = Vec::new();
        // Drain everything buffered; `try_recv` returns Err on both empty and
        // disconnected, either of which ends this poll.
        while let Ok(ev) = self.rx.try_recv() {
            self.update_state(&ev);
            out.push(ev);
        }
        out
    }

    fn abort(&mut self) -> Result<()> {
        self.abort.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn state(&self) -> RunState {
        self.state.clone()
    }
}

impl Drop for EventRunHandle {
    fn drop(&mut self) {
        // Ensure the worker winds down and we don't leak the thread.
        self.abort.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
