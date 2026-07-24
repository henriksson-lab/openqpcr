//! src/protocol.rs — thermal cycling program, modelled on RDML `thermalCyclingConditions`.
//!
//! This is a pure, serde-round-trippable model of a qPCR thermal program. It is the
//! concrete fill-in for the deferred `thermal_protocol` field described in the GUI
//! run-authoring plan.
//!
//! Nothing here touches hardware or the GUI: it is the data model plus two pure,
//! deterministic derivations used by drivers/simulators — [`ThermalProtocol::schedule`]
//! (flatten loops into a linear execution schedule) and
//! [`ThermalProtocol::amplification_cycles`] (count plate-read cycles).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A complete thermal program: an ordered list of steps + a lid temperature.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThermalProtocol {
    pub name: Option<String>,
    pub description: Option<String>,
    /// Heated-lid setpoint (°C). RDML `lidTemperature`. None = instrument default.
    pub lid_temperature: Option<f64>,
    /// Ordered steps (RDML `step`s). Index+1 is the RDML `nr`.
    pub steps: Vec<ProtocolStep>,
}

/// One program step. Exactly one variant per RDML `step`'s single child.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtocolStep {
    /// RDML `temperature`: hold the block at a target for a time.
    Hold(TemperatureStep),
    /// RDML `gradient`: a column temperature gradient (optimization).
    Gradient(GradientStep),
    /// RDML `loop`: jump back to a step and repeat — this is "×N cycles".
    Loop { goto: usize, repeat: u32 },
    /// RDML `pause`: hold until resumed (∞ hold / final storage e.g. 4 °C).
    Pause { temperature: f64 },
    /// RDML `lidOpen`: wait for the user to open the lid, then continue.
    LidOpen,
    /// Convenience macro over RDML: a ramped melt from start→end by increment,
    /// each point measured as `meltcurve`. Lowered to `temperature` steps on write.
    Melt(MeltStep),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemperatureStep {
    pub target_c: f64,
    /// Hold time in seconds. None = "infinite"/until-next for the last step.
    pub hold_secs: Option<f64>,
    /// Max ramp rate to reach target, °C/s. None = maximal (RDML empty `ramp`).
    pub ramp_c_per_s: Option<f64>,
    /// Touchdown: °C added to target each cycle (RDML `temperatureChange`).
    #[serde(default)]
    pub temperature_change: f64,
    /// Seconds added to hold each cycle (RDML `durationChange`).
    #[serde(default)]
    pub duration_change: f64,
    /// Read the plate at this step? Maps to RDML `measure`.
    #[serde(default)]
    pub measure: Measure,
}

impl TemperatureStep {
    /// A plain hold with no read, maximal ramp, no touchdown.
    pub fn hold(target_c: f64, hold_secs: f64) -> TemperatureStep {
        TemperatureStep {
            target_c,
            hold_secs: Some(hold_secs),
            ramp_c_per_s: None,
            temperature_change: 0.0,
            duration_change: 0.0,
            measure: Measure::None,
        }
    }

    /// A hold that reads the plate as real-time amplification.
    pub fn read(target_c: f64, hold_secs: f64) -> TemperatureStep {
        TemperatureStep {
            measure: Measure::Real,
            ..TemperatureStep::hold(target_c, hold_secs)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GradientStep {
    pub high_c: f64,
    pub low_c: f64,
    pub hold_secs: Option<f64>,
    pub ramp_c_per_s: Option<f64>,
    #[serde(default)]
    pub measure: Measure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeltStep {
    pub start_c: f64,
    pub end_c: f64,
    pub increment_c: f64,
    /// Seconds held at each increment.
    pub hold_secs: f64,
}

/// RDML `measure`: whether/how a step reads the optics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    /// No read.
    #[default]
    None,
    /// Real-time amplification read (RDML `real`).
    Real,
    /// Continuous melt read (RDML `meltcurve`).
    Meltcurve,
}

/// One entry in the flattened execution schedule produced by
/// [`ThermalProtocol::schedule`]. Loops are expanded; each occurrence of a step is
/// one entry, and `Melt` steps expand to one entry per temperature increment.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledStep {
    /// Index into `ThermalProtocol::steps` this entry came from.
    pub step_index: usize,
    /// Running amplification-cycle counter (1-based for `Real` reads; for non-`Real`
    /// entries it is the number of amplification cycles completed so far).
    pub cycle: usize,
    /// How this scheduled point reads the optics.
    pub measure: Measure,
    /// Target block temperature (°C) at this occurrence (touchdown applied; for a
    /// gradient it is the mean of high/low; for a melt point it is that increment).
    pub target_c: f64,
    /// Hold time (s) for this occurrence, if bounded.
    pub hold_secs: Option<f64>,
}

// Safety cap so a malformed loop can never spin forever.
const MAX_SCHEDULE_STEPS: usize = 10_000_000;

impl ThermalProtocol {
    /// Total plate-read (amplification) cycles = number of `Real`-measuring reads in
    /// the flattened schedule, honoring every `Loop`'s repeat count. Used to size the
    /// live amplification x-axis and to set `QpcrRun.metadata.cycle_count`.
    pub fn amplification_cycles(&self) -> usize {
        self.schedule()
            .iter()
            .filter(|s| s.measure == Measure::Real)
            .count()
    }

    /// Flatten loops into a linear execution schedule of [`ScheduledStep`]s for a
    /// simulator/driver to execute. Pure and deterministic.
    pub fn schedule(&self) -> Vec<ScheduledStep> {
        let mut out = Vec::new();
        // How many times each step index has executed (drives touchdown).
        let mut occurrence: HashMap<usize, u32> = HashMap::new();
        // How many back-jumps each loop index has performed so far.
        let mut loop_jumps: HashMap<usize, u32> = HashMap::new();
        let mut cycle: usize = 0;
        let mut pc: usize = 0;
        let mut guard: usize = 0;

        while pc < self.steps.len() {
            guard += 1;
            if guard > MAX_SCHEDULE_STEPS {
                break;
            }

            match &self.steps[pc] {
                ProtocolStep::Loop { goto, repeat } => {
                    let jumps = *loop_jumps.entry(pc).or_insert(0);
                    // `repeat` = total iterations of the block; one iteration already
                    // ran via linear flow, so we jump back `repeat - 1` more times.
                    if *repeat >= 2 && jumps < *repeat - 1 {
                        loop_jumps.insert(pc, jumps + 1);
                        // Reset any nested loops inside this block so they repeat fully.
                        let (g, cur) = (*goto, pc);
                        loop_jumps.retain(|&k, _| !(k > g && k < cur));
                        pc = *goto;
                    } else {
                        // Done: clear our counter so an outer loop can re-enter cleanly.
                        loop_jumps.remove(&pc);
                        pc += 1;
                    }
                }
                step => {
                    let occ = *occurrence.entry(pc).or_insert(0);
                    occurrence.insert(pc, occ + 1);
                    let k = occ as f64;
                    self.emit(step, pc, k, &mut cycle, &mut out);
                    pc += 1;
                }
            }
        }
        out
    }

    fn emit(
        &self,
        step: &ProtocolStep,
        step_index: usize,
        k: f64,
        cycle: &mut usize,
        out: &mut Vec<ScheduledStep>,
    ) {
        match step {
            ProtocolStep::Hold(ts) => {
                let target = ts.target_c + ts.temperature_change * k;
                let hold = ts.hold_secs.map(|h| h + ts.duration_change * k);
                if ts.measure == Measure::Real {
                    *cycle += 1;
                }
                out.push(ScheduledStep {
                    step_index,
                    cycle: *cycle,
                    measure: ts.measure,
                    target_c: target,
                    hold_secs: hold,
                });
            }
            ProtocolStep::Gradient(gs) => {
                if gs.measure == Measure::Real {
                    *cycle += 1;
                }
                out.push(ScheduledStep {
                    step_index,
                    cycle: *cycle,
                    measure: gs.measure,
                    target_c: (gs.high_c + gs.low_c) / 2.0,
                    hold_secs: gs.hold_secs,
                });
            }
            ProtocolStep::Pause { temperature } => out.push(ScheduledStep {
                step_index,
                cycle: *cycle,
                measure: Measure::None,
                target_c: *temperature,
                hold_secs: None,
            }),
            ProtocolStep::LidOpen => out.push(ScheduledStep {
                step_index,
                cycle: *cycle,
                measure: Measure::None,
                target_c: 0.0,
                hold_secs: None,
            }),
            ProtocolStep::Melt(ms) => {
                if ms.increment_c <= 0.0 {
                    out.push(ScheduledStep {
                        step_index,
                        cycle: *cycle,
                        measure: Measure::Meltcurve,
                        target_c: ms.start_c,
                        hold_secs: Some(ms.hold_secs),
                    });
                    return;
                }
                let mut t = ms.start_c;
                while t <= ms.end_c + 1e-9 {
                    out.push(ScheduledStep {
                        step_index,
                        cycle: *cycle,
                        measure: Measure::Meltcurve,
                        target_c: t,
                        hold_secs: Some(ms.hold_secs),
                    });
                    t += ms.increment_c;
                }
            }
            ProtocolStep::Loop { .. } => unreachable!("loops handled by schedule()"),
        }
    }
}

/// A handful of standard programs, constructed in code (no file I/O). These seed the
/// editor's "New from template".
pub fn builtins() -> Vec<ThermalProtocol> {
    vec![standard_2step_x40(), standard_3step_x40(), sybr_with_melt()]
}

/// "Standard 2-step ×40": initial denature, then (denature / anneal+read) ×40.
pub fn standard_2step_x40() -> ThermalProtocol {
    ThermalProtocol {
        name: Some("Standard 2-step ×40".to_string()),
        description: Some("Initial denaturation, then 2-step cycling ×40.".to_string()),
        lid_temperature: Some(105.0),
        steps: vec![
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 180.0)),
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 10.0)),
            ProtocolStep::Hold(TemperatureStep::read(60.0, 30.0)),
            ProtocolStep::Loop {
                goto: 1,
                repeat: 40,
            },
        ],
    }
}

/// "3-step ×40": initial denature, then (denature / anneal / extend+read) ×40.
pub fn standard_3step_x40() -> ThermalProtocol {
    ThermalProtocol {
        name: Some("3-step ×40".to_string()),
        description: Some("Initial denaturation, then 3-step cycling ×40.".to_string()),
        lid_temperature: Some(105.0),
        steps: vec![
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 180.0)),
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 15.0)),
            ProtocolStep::Hold(TemperatureStep::hold(58.0, 30.0)),
            ProtocolStep::Hold(TemperatureStep::read(72.0, 30.0)),
            ProtocolStep::Loop {
                goto: 1,
                repeat: 40,
            },
        ],
    }
}

/// "SYBR + melt": 2-step ×40 amplification followed by a high-resolution melt.
pub fn sybr_with_melt() -> ThermalProtocol {
    ThermalProtocol {
        name: Some("SYBR + melt".to_string()),
        description: Some("2-step ×40 amplification, then a 65→95 °C melt.".to_string()),
        lid_temperature: Some(105.0),
        steps: vec![
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 180.0)),
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 10.0)),
            ProtocolStep::Hold(TemperatureStep::read(60.0, 30.0)),
            ProtocolStep::Loop {
                goto: 1,
                repeat: 40,
            },
            ProtocolStep::Melt(MeltStep {
                start_c: 65.0,
                end_c: 95.0,
                increment_c: 0.5,
                hold_secs: 5.0,
            }),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_step_x40_has_40_amplification_cycles() {
        let p = standard_2step_x40();
        assert_eq!(p.amplification_cycles(), 40);
    }

    #[test]
    fn three_step_x40_has_40_amplification_cycles() {
        let p = standard_3step_x40();
        assert_eq!(p.amplification_cycles(), 40);
    }

    #[test]
    fn schedule_length_and_order_for_two_step() {
        let p = standard_2step_x40();
        let sched = p.schedule();
        // 1 initial hold + 40 × (denature + read) = 81 entries; loop is control flow.
        assert_eq!(sched.len(), 1 + 40 * 2);
        // The first entry is the initial denature at 95 °C, no read.
        assert_eq!(sched[0].step_index, 0);
        assert_eq!(sched[0].measure, Measure::None);
        // Real reads are numbered 1..=40 in order.
        let reads: Vec<usize> = sched
            .iter()
            .filter(|s| s.measure == Measure::Real)
            .map(|s| s.cycle)
            .collect();
        assert_eq!(reads, (1..=40).collect::<Vec<_>>());
    }

    #[test]
    fn schedule_expands_melt_into_meltcurve_points() {
        let p = sybr_with_melt();
        let sched = p.schedule();
        let melt_pts: Vec<&ScheduledStep> = sched
            .iter()
            .filter(|s| s.measure == Measure::Meltcurve)
            .collect();
        // 65 → 95 by 0.5 inclusive = 61 points.
        assert_eq!(melt_pts.len(), 61);
        // Melt comes after all 40 amplification cycles.
        assert_eq!(melt_pts[0].cycle, 40);
        assert!((melt_pts[0].target_c - 65.0).abs() < 1e-9);
        assert!((melt_pts.last().unwrap().target_c - 95.0).abs() < 1e-9);
        // The melt does not add amplification cycles.
        assert_eq!(p.amplification_cycles(), 40);
    }

    #[test]
    fn touchdown_lowers_target_each_cycle() {
        let p = ThermalProtocol {
            steps: vec![
                ProtocolStep::Hold(TemperatureStep {
                    temperature_change: -1.0,
                    ..TemperatureStep::read(65.0, 15.0)
                }),
                ProtocolStep::Loop { goto: 0, repeat: 3 },
            ],
            ..Default::default()
        };
        let sched = p.schedule();
        let targets: Vec<f64> = sched.iter().map(|s| s.target_c).collect();
        assert_eq!(targets, vec![65.0, 64.0, 63.0]);
    }

    #[test]
    fn nested_loops_multiply() {
        // inner block reads once, inner loop ×2, outer loop ×3 → 6 reads.
        let p = ThermalProtocol {
            steps: vec![
                ProtocolStep::Hold(TemperatureStep::read(60.0, 10.0)),
                ProtocolStep::Loop { goto: 0, repeat: 2 },
                ProtocolStep::Loop { goto: 0, repeat: 3 },
            ],
            ..Default::default()
        };
        assert_eq!(p.amplification_cycles(), 6);
    }

    #[test]
    fn serde_round_trip() {
        let p = sybr_with_melt();
        let json = serde_json::to_string(&p).unwrap();
        let back: ThermalProtocol = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        // Spot-check the internally-tagged step encoding.
        assert!(json.contains("\"kind\":\"hold\""));
        assert!(json.contains("\"kind\":\"loop\""));
        assert!(json.contains("\"kind\":\"melt\""));
    }
}
