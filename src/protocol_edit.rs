//! src/protocol_edit.rs — pure editing + validation over a `ThermalProtocol`.
//!
//! Same spirit as `src/edit.rs`: no GUI, no I/O, no hardware — just pure mutations
//! and validation over `&mut ThermalProtocol` that the GUI (or a test) can call and
//! that are directly unit-testable.

use std::error::Error;
use std::fmt;

use crate::protocol::{Measure, ProtocolStep, ThermalProtocol};

/// Append a step to the end of the program.
pub fn add_step(p: &mut ThermalProtocol, step: ProtocolStep) {
    p.steps.push(step);
}

/// Remove the step at `idx`. Returns `true` if a step was removed.
pub fn remove_step(p: &mut ThermalProtocol, idx: usize) -> bool {
    if idx < p.steps.len() {
        p.steps.remove(idx);
        true
    } else {
        false
    }
}

/// Move the step at `from` to position `to` (shifting the rest). Returns `true` on
/// success. A no-op (`from == to`) still succeeds if the index is valid.
pub fn move_step(p: &mut ThermalProtocol, from: usize, to: usize) -> bool {
    let len = p.steps.len();
    if from >= len || to >= len {
        return false;
    }
    let step = p.steps.remove(from);
    p.steps.insert(to, step);
    true
}

/// Set (or clear) the heated-lid setpoint.
pub fn set_lid_temperature(p: &mut ThermalProtocol, lid: Option<f64>) {
    p.lid_temperature = lid;
}

/// A validation failure for a thermal program.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtocolError {
    /// A hold time was zero or negative.
    NonPositiveHold { step: usize },
    /// A melt's start temperature is not below its end temperature.
    MeltStartAfterEnd { step: usize },
    /// A `Loop`'s `goto` target does not exist.
    LoopGotoOutOfRange { step: usize, goto: usize },
    /// A `Loop`'s `goto` does not point strictly before the loop itself.
    LoopGotoNotBackward { step: usize, goto: usize },
    /// A `Loop`'s body never reads the plate (no `Real` measure).
    LoopReadsNoPlate { step: usize },
    /// The lid setpoint is below the hottest block step (risking condensation).
    LidBelowBlockMax { lid: f64, block_max: f64 },
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::NonPositiveHold { step } => {
                write!(f, "step {} has a non-positive hold time", step + 1)
            }
            ProtocolError::MeltStartAfterEnd { step } => write!(
                f,
                "step {} melt start temperature must be below its end temperature",
                step + 1
            ),
            ProtocolError::LoopGotoOutOfRange { step, goto } => write!(
                f,
                "step {} loop goto {} is out of range",
                step + 1,
                goto + 1
            ),
            ProtocolError::LoopGotoNotBackward { step, goto } => write!(
                f,
                "step {} loop goto {} must point to an earlier step",
                step + 1,
                goto + 1
            ),
            ProtocolError::LoopReadsNoPlate { step } => write!(
                f,
                "step {} loop never reads the plate (no real-time measure in its body)",
                step + 1
            ),
            ProtocolError::LidBelowBlockMax { lid, block_max } => write!(
                f,
                "lid temperature {lid} °C is below the hottest block step {block_max} °C"
            ),
        }
    }
}

impl Error for ProtocolError {}

/// Validate a program, returning the first problem found.
pub fn validate(p: &ThermalProtocol) -> Result<(), ProtocolError> {
    for (i, step) in p.steps.iter().enumerate() {
        match step {
            ProtocolStep::Hold(ts) => {
                if matches!(ts.hold_secs, Some(h) if h <= 0.0) {
                    return Err(ProtocolError::NonPositiveHold { step: i });
                }
            }
            ProtocolStep::Gradient(gs) => {
                if matches!(gs.hold_secs, Some(h) if h <= 0.0) {
                    return Err(ProtocolError::NonPositiveHold { step: i });
                }
            }
            ProtocolStep::Melt(ms) => {
                if ms.hold_secs <= 0.0 {
                    return Err(ProtocolError::NonPositiveHold { step: i });
                }
                if ms.start_c >= ms.end_c {
                    return Err(ProtocolError::MeltStartAfterEnd { step: i });
                }
            }
            ProtocolStep::Loop { goto, repeat: _ } => {
                if *goto >= p.steps.len() {
                    return Err(ProtocolError::LoopGotoOutOfRange {
                        step: i,
                        goto: *goto,
                    });
                }
                if *goto >= i {
                    return Err(ProtocolError::LoopGotoNotBackward {
                        step: i,
                        goto: *goto,
                    });
                }
                if !reads_plate(&p.steps[*goto..i]) {
                    return Err(ProtocolError::LoopReadsNoPlate { step: i });
                }
            }
            ProtocolStep::Pause { .. } | ProtocolStep::LidOpen => {}
        }
    }

    if let (Some(lid), Some(block_max)) = (p.lid_temperature, block_max(p))
        && lid < block_max
    {
        return Err(ProtocolError::LidBelowBlockMax { lid, block_max });
    }

    Ok(())
}

/// Does any step in `steps` perform a real-time plate read?
fn reads_plate(steps: &[ProtocolStep]) -> bool {
    steps.iter().any(|s| match s {
        ProtocolStep::Hold(ts) => ts.measure == Measure::Real,
        ProtocolStep::Gradient(gs) => gs.measure == Measure::Real,
        _ => false,
    })
}

/// The hottest block temperature commanded anywhere in the program, if any.
fn block_max(p: &ThermalProtocol) -> Option<f64> {
    let mut max: Option<f64> = None;
    let mut bump = |t: f64| {
        max = Some(match max {
            Some(m) if m >= t => m,
            _ => t,
        });
    };
    for step in &p.steps {
        match step {
            ProtocolStep::Hold(ts) => bump(ts.target_c),
            ProtocolStep::Gradient(gs) => bump(gs.high_c),
            ProtocolStep::Pause { temperature } => bump(*temperature),
            ProtocolStep::Melt(ms) => bump(ms.end_c),
            ProtocolStep::Loop { .. } | ProtocolStep::LidOpen => {}
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MeltStep, TemperatureStep, standard_2step_x40};

    #[test]
    fn add_remove_move() {
        let mut p = ThermalProtocol::default();
        add_step(
            &mut p,
            ProtocolStep::Hold(TemperatureStep::hold(95.0, 30.0)),
        );
        add_step(
            &mut p,
            ProtocolStep::Hold(TemperatureStep::read(60.0, 30.0)),
        );
        add_step(&mut p, ProtocolStep::LidOpen);
        assert_eq!(p.steps.len(), 3);

        // move the LidOpen (idx 2) to the front.
        assert!(move_step(&mut p, 2, 0));
        assert_eq!(p.steps[0], ProtocolStep::LidOpen);
        assert!(!move_step(&mut p, 9, 0));

        assert!(remove_step(&mut p, 0));
        assert_eq!(p.steps.len(), 2);
        assert!(!remove_step(&mut p, 9));
    }

    #[test]
    fn set_lid() {
        let mut p = ThermalProtocol::default();
        set_lid_temperature(&mut p, Some(105.0));
        assert_eq!(p.lid_temperature, Some(105.0));
        set_lid_temperature(&mut p, None);
        assert_eq!(p.lid_temperature, None);
    }

    #[test]
    fn builtin_is_valid() {
        assert!(validate(&standard_2step_x40()).is_ok());
    }

    #[test]
    fn rejects_non_positive_hold() {
        let p = ThermalProtocol {
            steps: vec![ProtocolStep::Hold(TemperatureStep::hold(95.0, 0.0))],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::NonPositiveHold { step: 0 })
        );
    }

    #[test]
    fn rejects_melt_start_after_end() {
        let p = ThermalProtocol {
            steps: vec![ProtocolStep::Melt(MeltStep {
                start_c: 95.0,
                end_c: 65.0,
                increment_c: 0.5,
                hold_secs: 5.0,
            })],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::MeltStartAfterEnd { step: 0 })
        );
    }

    #[test]
    fn rejects_loop_goto_out_of_range() {
        let p = ThermalProtocol {
            steps: vec![
                ProtocolStep::Hold(TemperatureStep::read(60.0, 30.0)),
                ProtocolStep::Loop {
                    goto: 9,
                    repeat: 40,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::LoopGotoOutOfRange { step: 1, goto: 9 })
        );
    }

    #[test]
    fn rejects_loop_goto_not_backward() {
        // goto == its own index is not backward.
        let p = ThermalProtocol {
            steps: vec![
                ProtocolStep::Hold(TemperatureStep::read(60.0, 30.0)),
                ProtocolStep::Loop {
                    goto: 1,
                    repeat: 40,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::LoopGotoNotBackward { step: 1, goto: 1 })
        );
    }

    #[test]
    fn rejects_loop_that_reads_no_plate() {
        let p = ThermalProtocol {
            steps: vec![
                ProtocolStep::Hold(TemperatureStep::hold(95.0, 10.0)),
                ProtocolStep::Hold(TemperatureStep::hold(60.0, 30.0)),
                ProtocolStep::Loop {
                    goto: 0,
                    repeat: 40,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::LoopReadsNoPlate { step: 2 })
        );
    }

    #[test]
    fn rejects_lid_below_block_max() {
        let p = ThermalProtocol {
            lid_temperature: Some(90.0),
            steps: vec![ProtocolStep::Hold(TemperatureStep::hold(95.0, 30.0))],
            ..Default::default()
        };
        assert_eq!(
            validate(&p),
            Err(ProtocolError::LidBelowBlockMax {
                lid: 90.0,
                block_max: 95.0
            })
        );
    }

    #[test]
    fn error_displays() {
        let msg = ProtocolError::LidBelowBlockMax {
            lid: 90.0,
            block_max: 95.0,
        }
        .to_string();
        assert!(msg.contains("lid temperature"));
    }
}
