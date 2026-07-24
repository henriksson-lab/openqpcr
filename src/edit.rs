//! Pure plate-editing core: apply edits, validate, and introspect a selection.
//!
//! All functions operate on a `&mut QpcrRun` plus a selection slice
//! `&[(u8, u8)]` of `(row, col)` coordinates. Per-well edits materialize a
//! [`Well`] for any selected coordinate that is currently empty (the model's
//! `wells` list is sparse), so empty wells can be authored. Nothing here touches
//! the GUI — it is the unit-testable library layer for plate editing.

use crate::model::{Channel, QpcrRun, SampleType, Well};

/// Validation failures for [`set_starting_quantity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// A starting quantity that is zero or negative.
    NonPositiveQuantity,
    /// A starting quantity that is NaN or infinite.
    NotFinite,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::NonPositiveQuantity => {
                write!(f, "starting quantity must be greater than zero")
            }
            EditError::NotFinite => write!(f, "starting quantity must be a finite number"),
        }
    }
}

impl std::error::Error for EditError {}

// ---------------------------------------------------------------------------
// Find-or-insert helpers (mirror readers::export::RunBuilder)
// ---------------------------------------------------------------------------

/// Find the well at `(row, col)`, inserting a fresh empty [`Well`] if absent.
fn well_mut(run: &mut QpcrRun, row: u8, col: u8) -> &mut Well {
    if let Some(idx) = run.wells.iter().position(|w| w.row == row && w.col == col) {
        &mut run.wells[idx]
    } else {
        run.wells.push(Well {
            row,
            col,
            ..Default::default()
        });
        run.wells.last_mut().unwrap()
    }
}

/// Find the channel for `fluor` within `well`, inserting a fresh one if absent.
fn channel_mut<'a>(well: &'a mut Well, fluor: &str) -> &'a mut Channel {
    if let Some(idx) = well.channels.iter().position(|c| c.fluorophore == fluor) {
        &mut well.channels[idx]
    } else {
        well.channels.push(Channel {
            fluorophore: fluor.to_string(),
            ..Default::default()
        });
        well.channels.last_mut().unwrap()
    }
}

// ---------------------------------------------------------------------------
// Per-well edit-apply (each hits every coordinate in the selection)
// ---------------------------------------------------------------------------

/// Set the sample type on every selected well (materializing empties).
pub fn set_sample_type(run: &mut QpcrRun, sel: &[(u8, u8)], value: SampleType) {
    for &(row, col) in sel {
        well_mut(run, row, col).sample_type = value;
    }
}

/// Set the sample name on every selected well. An empty string clears it.
pub fn set_sample_name(run: &mut QpcrRun, sel: &[(u8, u8)], value: Option<String>) {
    let value = normalize(value);
    for &(row, col) in sel {
        well_mut(run, row, col).sample = value.clone();
    }
}

/// Set the biological group on every selected well. An empty string clears it.
pub fn set_biological_group(run: &mut QpcrRun, sel: &[(u8, u8)], value: Option<String>) {
    let value = normalize(value);
    for &(row, col) in sel {
        well_mut(run, row, col).biological_group = value.clone();
    }
}

/// Set the starting quantity on every selected well.
///
/// `None` clears the value. A `Some(q)` is validated: `q` must be finite and
/// strictly greater than zero, otherwise no wells are modified and an
/// [`EditError`] is returned.
pub fn set_starting_quantity(
    run: &mut QpcrRun,
    sel: &[(u8, u8)],
    value: Option<f64>,
) -> Result<(), EditError> {
    if let Some(q) = value {
        if !q.is_finite() {
            return Err(EditError::NotFinite);
        }
        if q <= 0.0 {
            return Err(EditError::NonPositiveQuantity);
        }
    }
    for &(row, col) in sel {
        well_mut(run, row, col).starting_quantity = value;
    }
    Ok(())
}

/// Set the `target` on the `fluor` channel of every selected well, creating the
/// channel where a selected well lacks it. An empty string clears the target.
pub fn set_target(run: &mut QpcrRun, sel: &[(u8, u8)], fluor: &str, value: Option<String>) {
    let value = normalize(value);
    for &(row, col) in sel {
        let well = well_mut(run, row, col);
        channel_mut(well, fluor).target = value.clone();
    }
}

/// Remove the [`Well`] entries for every selected coordinate, returning them to
/// the sparse "empty" state.
pub fn clear_wells(run: &mut QpcrRun, sel: &[(u8, u8)]) {
    run.wells
        .retain(|w| !sel.iter().any(|&(row, col)| w.row == row && w.col == col));
}

// ---------------------------------------------------------------------------
// Selection introspection (for populating an editor panel)
// ---------------------------------------------------------------------------

/// The sample type shared by every selected well, or `None` when the selection
/// is empty, mixes types, or contains an unauthored (empty) coordinate.
pub fn common_sample_type(run: &QpcrRun, sel: &[(u8, u8)]) -> Option<SampleType> {
    common(sel, |row, col| {
        find_well(run, row, col).map(|w| w.sample_type)
    })
}

/// The sample name shared by every selected well (as `Some(None)` when they all
/// share "no name"), or `None` when the selection is empty or mixes values.
pub fn common_sample_name(run: &QpcrRun, sel: &[(u8, u8)]) -> Option<Option<String>> {
    common(sel, |row, col| {
        Some(find_well(run, row, col).and_then(|w| w.sample.clone()))
    })
}

/// The sorted union of fluorophores present across the selection.
pub fn fluorophores_in(run: &QpcrRun, sel: &[(u8, u8)]) -> Vec<String> {
    let mut fluors: Vec<String> = Vec::new();
    for &(row, col) in sel {
        if let Some(well) = find_well(run, row, col) {
            for ch in &well.channels {
                if !fluors.contains(&ch.fluorophore) {
                    fluors.push(ch.fluorophore.clone());
                }
            }
        }
    }
    fluors.sort();
    fluors
}

// ---------------------------------------------------------------------------
// Copy layout from a previous run / layout templates
// ---------------------------------------------------------------------------

/// Outcome of [`copy_layout_from`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CopyReport {
    /// Source wells whose layout was copied into the destination.
    pub wells_copied: usize,
    /// Source wells skipped because their coordinate is outside the
    /// destination plate's geometry (e.g. a 384 layout into a 96 plate).
    pub wells_skipped_out_of_bounds: usize,
}

/// Copy **only layout** — never measured data — from `source` into `dest`.
///
/// For each occupied source well whose `(row, col)` fits `dest`'s plate geometry,
/// this copies the per-well sample name, sample type, biological group and
/// starting quantity, plus a *layout-only* channel per source channel (its
/// `fluorophore` and `target` assignment only). It never reads `amplification`,
/// `cq` or `melt` from the source, and it leaves any measured data already present
/// on a destination channel intact — so it is safe to re-layout a plate that has
/// already been acquired.
///
/// Source wells outside the destination geometry are skipped and counted (e.g.
/// copying a 384-well layout into a 96-well plate keeps the A1–H12 corner and
/// reports the rest).
pub fn copy_layout_from(dest: &mut QpcrRun, source: &QpcrRun) -> CopyReport {
    let (rows, cols) = (dest.plate.rows, dest.plate.cols);
    let mut report = CopyReport::default();
    for sw in &source.wells {
        if sw.row >= rows || sw.col >= cols {
            report.wells_skipped_out_of_bounds += 1;
            continue;
        }
        let dw = well_mut(dest, sw.row, sw.col);
        dw.sample = sw.sample.clone();
        dw.sample_type = sw.sample_type;
        dw.biological_group = sw.biological_group.clone();
        dw.starting_quantity = sw.starting_quantity;
        for sc in &sw.channels {
            // Copy only the fluorophore/target assignment; never the trace data.
            channel_mut(dw, &sc.fluorophore).target = sc.target.clone();
        }
        report.wells_copied += 1;
    }
    report
}

/// Return a layout-only copy of `run`: identical plate, per-well sample metadata,
/// channel fluorophore/target assignments and (once present) thermal protocol,
/// but with all measured data (`amplification`, `cq`, `melt`) stripped. Suitable
/// for saving as a reusable plate template via the JSON writer.
///
/// Implemented by cloning and stripping, so any fields added to the run/well model
/// later (e.g. the thermal `protocol`) are preserved automatically.
pub fn layout_template(run: &QpcrRun) -> QpcrRun {
    let mut template = run.clone();
    for well in &mut template.wells {
        for channel in &mut well.channels {
            channel.amplification.clear();
            channel.cq = None;
            channel.melt = None;
        }
    }
    template
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Map an empty string to `None`; keep any non-empty value as-is.
fn normalize(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

fn find_well(run: &QpcrRun, row: u8, col: u8) -> Option<&Well> {
    run.wells.iter().find(|w| w.row == row && w.col == col)
}

/// Return the value shared by every coordinate in `sel`, or `None` when the
/// selection is empty or any coordinate disagrees. `f` yields the per-coord
/// value; a coordinate for which `f` returns `None` (e.g. an unauthored well)
/// only agrees if every coordinate returns the same `None`-derived value.
fn common<T, F>(sel: &[(u8, u8)], mut f: F) -> Option<T>
where
    T: PartialEq,
    F: FnMut(u8, u8) -> Option<T>,
{
    let mut iter = sel.iter();
    let &(row, col) = iter.next()?;
    let first = f(row, col)?;
    for &(row, col) in iter {
        match f(row, col) {
            Some(v) if v == first => {}
            _ => return None,
        }
    }
    Some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(wells: Vec<Well>) -> QpcrRun {
        QpcrRun {
            wells,
            ..Default::default()
        }
    }

    fn well_at(run: &QpcrRun, row: u8, col: u8) -> Option<&Well> {
        find_well(run, row, col)
    }

    #[test]
    fn set_per_well_fields_hit_exactly_the_selection() {
        let mut run = run_with(vec![]);
        let sel = &[(0, 0), (0, 1), (3, 5)];
        set_sample_type(&mut run, sel, SampleType::Standard);
        set_sample_name(&mut run, sel, Some("S1".into()));
        set_biological_group(&mut run, sel, Some("G1".into()));

        assert_eq!(run.wells.len(), 3);
        for &(r, c) in sel {
            let w = well_at(&run, r, c).unwrap();
            assert_eq!(w.sample_type, SampleType::Standard);
            assert_eq!(w.sample.as_deref(), Some("S1"));
            assert_eq!(w.biological_group.as_deref(), Some("G1"));
        }
        // An unselected well is untouched (none was created for it).
        assert!(well_at(&run, 1, 1).is_none());
    }

    #[test]
    fn set_sample_name_empty_clears_to_none() {
        let mut run = run_with(vec![Well {
            row: 0,
            col: 0,
            sample: Some("old".into()),
            ..Default::default()
        }]);
        set_sample_name(&mut run, &[(0, 0)], Some(String::new()));
        assert_eq!(well_at(&run, 0, 0).unwrap().sample, None);
    }

    #[test]
    fn set_biological_group_empty_clears_to_none() {
        let mut run = run_with(vec![Well {
            row: 0,
            col: 0,
            biological_group: Some("old".into()),
            ..Default::default()
        }]);
        set_biological_group(&mut run, &[(0, 0)], Some(String::new()));
        assert_eq!(well_at(&run, 0, 0).unwrap().biological_group, None);
    }

    #[test]
    fn editing_empty_coord_materializes_well() {
        let mut run = run_with(vec![]);
        assert!(run.wells.is_empty());
        set_sample_type(&mut run, &[(2, 3)], SampleType::Ntc);
        let w = well_at(&run, 2, 3).unwrap();
        assert_eq!(w.row, 2);
        assert_eq!(w.col, 3);
        assert_eq!(w.sample_type, SampleType::Ntc);
    }

    #[test]
    fn clear_wells_removes_entries() {
        let mut run = run_with(vec![]);
        let sel = &[(0, 0), (0, 1)];
        set_sample_type(&mut run, sel, SampleType::Standard);
        assert_eq!(run.wells.len(), 2);
        // Clear only one of them plus an unauthored coord (no-op for the latter).
        clear_wells(&mut run, &[(0, 0), (5, 5)]);
        assert!(well_at(&run, 0, 0).is_none());
        assert!(well_at(&run, 0, 1).is_some());
        assert_eq!(run.wells.len(), 1);
    }

    #[test]
    fn starting_quantity_accepts_positive() {
        let mut run = run_with(vec![]);
        assert!(set_starting_quantity(&mut run, &[(0, 0)], Some(10.0)).is_ok());
        assert_eq!(well_at(&run, 0, 0).unwrap().starting_quantity, Some(10.0));
    }

    #[test]
    fn starting_quantity_empty_clears() {
        let mut run = run_with(vec![Well {
            row: 0,
            col: 0,
            starting_quantity: Some(5.0),
            ..Default::default()
        }]);
        assert!(set_starting_quantity(&mut run, &[(0, 0)], None).is_ok());
        assert_eq!(well_at(&run, 0, 0).unwrap().starting_quantity, None);
    }

    #[test]
    fn starting_quantity_rejects_zero_negative_and_nonfinite() {
        let mut run = run_with(vec![]);
        assert_eq!(
            set_starting_quantity(&mut run, &[(0, 0)], Some(0.0)),
            Err(EditError::NonPositiveQuantity)
        );
        assert_eq!(
            set_starting_quantity(&mut run, &[(0, 0)], Some(-1.0)),
            Err(EditError::NonPositiveQuantity)
        );
        assert_eq!(
            set_starting_quantity(&mut run, &[(0, 0)], Some(f64::NAN)),
            Err(EditError::NotFinite)
        );
        assert_eq!(
            set_starting_quantity(&mut run, &[(0, 0)], Some(f64::INFINITY)),
            Err(EditError::NotFinite)
        );
        // Rejected input must not have materialized or mutated any well.
        assert!(run.wells.is_empty());
    }

    #[test]
    fn set_target_sets_and_creates_channel() {
        // A1 already has a FAM channel; A2 is empty.
        let mut run = run_with(vec![Well {
            row: 0,
            col: 0,
            channels: vec![Channel {
                fluorophore: "FAM".into(),
                ..Default::default()
            }],
            ..Default::default()
        }]);
        set_target(&mut run, &[(0, 0), (0, 1)], "FAM", Some("GAPDH".into()));

        let a1 = well_at(&run, 0, 0).unwrap();
        assert_eq!(a1.channels.len(), 1);
        assert_eq!(a1.channels[0].target.as_deref(), Some("GAPDH"));

        // A2 was materialized and given a FAM channel.
        let a2 = well_at(&run, 0, 1).unwrap();
        assert_eq!(a2.channels.len(), 1);
        assert_eq!(a2.channels[0].fluorophore, "FAM");
        assert_eq!(a2.channels[0].target.as_deref(), Some("GAPDH"));
    }

    #[test]
    fn set_target_multi_fluorophore_selection() {
        let mut run = run_with(vec![
            Well {
                row: 0,
                col: 0,
                channels: vec![Channel {
                    fluorophore: "FAM".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Well {
                row: 0,
                col: 1,
                channels: vec![Channel {
                    fluorophore: "HEX".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]);
        let sel = &[(0, 0), (0, 1)];
        // Setting HEX target should touch HEX on both, creating it on A1.
        set_target(&mut run, sel, "HEX", Some("ACTB".into()));

        let a1 = well_at(&run, 0, 0).unwrap();
        assert_eq!(a1.channels.len(), 2);
        assert_eq!(
            a1.channels
                .iter()
                .find(|c| c.fluorophore == "HEX")
                .and_then(|c| c.target.as_deref()),
            Some("ACTB")
        );
        // FAM on A1 was untouched.
        assert!(
            a1.channels
                .iter()
                .find(|c| c.fluorophore == "FAM")
                .unwrap()
                .target
                .is_none()
        );

        let a2 = well_at(&run, 0, 1).unwrap();
        assert_eq!(
            a2.channels
                .iter()
                .find(|c| c.fluorophore == "HEX")
                .and_then(|c| c.target.as_deref()),
            Some("ACTB")
        );
    }

    #[test]
    fn set_target_empty_clears() {
        let mut run = run_with(vec![Well {
            row: 0,
            col: 0,
            channels: vec![Channel {
                fluorophore: "FAM".into(),
                target: Some("GAPDH".into()),
                ..Default::default()
            }],
            ..Default::default()
        }]);
        set_target(&mut run, &[(0, 0)], "FAM", Some(String::new()));
        assert_eq!(well_at(&run, 0, 0).unwrap().channels[0].target, None);
    }

    #[test]
    fn common_sample_type_shared_and_mixed() {
        let mut run = run_with(vec![]);
        let sel = &[(0, 0), (0, 1)];
        set_sample_type(&mut run, sel, SampleType::Standard);
        assert_eq!(common_sample_type(&run, sel), Some(SampleType::Standard));

        // Make them disagree.
        set_sample_type(&mut run, &[(0, 1)], SampleType::Ntc);
        assert_eq!(common_sample_type(&run, sel), None);

        // Empty selection => None.
        assert_eq!(common_sample_type(&run, &[]), None);
    }

    #[test]
    fn common_sample_type_none_when_selection_has_empty_coord() {
        let mut run = run_with(vec![]);
        set_sample_type(&mut run, &[(0, 0)], SampleType::Standard);
        // (0,1) is unauthored → not all coords agree.
        assert_eq!(common_sample_type(&run, &[(0, 0), (0, 1)]), None);
    }

    #[test]
    fn common_sample_name_shared_and_mixed() {
        let mut run = run_with(vec![]);
        let sel = &[(0, 0), (0, 1)];
        set_sample_name(&mut run, sel, Some("S1".into()));
        assert_eq!(common_sample_name(&run, sel), Some(Some("S1".to_string())));

        set_sample_name(&mut run, &[(0, 1)], Some("S2".into()));
        assert_eq!(common_sample_name(&run, sel), None);
    }

    #[test]
    fn common_sample_name_shared_none() {
        // Two authored wells, neither with a name → they agree on "no name".
        let run = run_with(vec![
            Well {
                row: 0,
                col: 0,
                ..Default::default()
            },
            Well {
                row: 0,
                col: 1,
                ..Default::default()
            },
        ]);
        assert_eq!(common_sample_name(&run, &[(0, 0), (0, 1)]), Some(None));
    }

    #[test]
    fn fluorophores_in_sorted_union() {
        let run = run_with(vec![
            Well {
                row: 0,
                col: 0,
                channels: vec![
                    Channel {
                        fluorophore: "HEX".into(),
                        ..Default::default()
                    },
                    Channel {
                        fluorophore: "FAM".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            Well {
                row: 0,
                col: 1,
                channels: vec![Channel {
                    fluorophore: "FAM".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ]);
        assert_eq!(
            fluorophores_in(&run, &[(0, 0), (0, 1)]),
            vec!["FAM".to_string(), "HEX".to_string()]
        );
        // Unauthored coords contribute nothing.
        assert!(fluorophores_in(&run, &[(9, 9)]).is_empty());
    }

    // ----- copy_layout_from / layout_template -----

    use crate::model::{MeltCurve, PlateFormat};

    /// A well carrying full measured data (used to prove data is never copied).
    fn acquired_well(row: u8, col: u8, sample: &str, target: &str) -> Well {
        Well {
            row,
            col,
            sample: Some(sample.into()),
            sample_type: SampleType::Unknown,
            starting_quantity: None,
            channels: vec![Channel {
                fluorophore: "FAM".into(),
                target: Some(target.into()),
                cq: Some(22.0),
                amplification: vec![1.0, 2.0, 3.0],
                melt: Some(MeltCurve {
                    rfu: vec![10.0],
                    ..Default::default()
                }),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn copy_layout_copies_layout_never_data() {
        let source = run_with(vec![acquired_well(0, 0, "S1", "GAPDH")]);
        let mut dest = run_with(vec![]);
        let report = copy_layout_from(&mut dest, &source);
        assert_eq!(report.wells_copied, 1);
        assert_eq!(report.wells_skipped_out_of_bounds, 0);

        let w = well_at(&dest, 0, 0).unwrap();
        assert_eq!(w.sample.as_deref(), Some("S1"));
        let ch = &w.channels[0];
        assert_eq!(ch.fluorophore, "FAM");
        assert_eq!(ch.target.as_deref(), Some("GAPDH"));
        // Measured data was NOT transferred.
        assert!(ch.amplification.is_empty());
        assert_eq!(ch.cq, None);
        assert!(ch.melt.is_none());
    }

    #[test]
    fn copy_layout_384_into_96_skips_out_of_bounds() {
        let source = QpcrRun {
            plate: PlateFormat::P384,
            wells: vec![
                acquired_well(0, 0, "in", "T"),    // fits a 96 plate
                acquired_well(15, 23, "out", "T"), // P24: out of 8×12 bounds
            ],
            ..Default::default()
        };
        let mut dest = QpcrRun {
            plate: PlateFormat::P96,
            ..Default::default()
        };
        let report = copy_layout_from(&mut dest, &source);
        assert_eq!(report.wells_copied, 1);
        assert_eq!(report.wells_skipped_out_of_bounds, 1);
        assert!(well_at(&dest, 0, 0).is_some());
        assert!(well_at(&dest, 15, 23).is_none());
    }

    #[test]
    fn copy_layout_96_into_384_preserves_coordinates() {
        let source = QpcrRun {
            plate: PlateFormat::P96,
            wells: vec![acquired_well(7, 11, "H12", "T")],
            ..Default::default()
        };
        let mut dest = QpcrRun {
            plate: PlateFormat::P384,
            ..Default::default()
        };
        let report = copy_layout_from(&mut dest, &source);
        assert_eq!(report.wells_copied, 1);
        assert_eq!(report.wells_skipped_out_of_bounds, 0);
        assert_eq!(
            well_at(&dest, 7, 11).unwrap().sample.as_deref(),
            Some("H12")
        );
    }

    #[test]
    fn copy_layout_over_existing_well_keeps_its_traces() {
        // Destination already acquired: A1 has a FAM channel with data.
        let mut dest = run_with(vec![acquired_well(0, 0, "old", "OLD")]);
        // Source re-lays A1 with a new target.
        let source = run_with(vec![acquired_well(0, 0, "new", "NEW")]);
        copy_layout_from(&mut dest, &source);

        let ch = &well_at(&dest, 0, 0).unwrap().channels[0];
        // Layout (sample, target) is overwritten...
        assert_eq!(well_at(&dest, 0, 0).unwrap().sample.as_deref(), Some("new"));
        assert_eq!(ch.target.as_deref(), Some("NEW"));
        // ...but the destination's own measured data is left intact.
        assert_eq!(ch.amplification, vec![1.0, 2.0, 3.0]);
        assert_eq!(ch.cq, Some(22.0));
        assert!(ch.melt.is_some());
    }

    #[test]
    fn layout_template_strips_measured_data() {
        let run = run_with(vec![acquired_well(0, 0, "S1", "GAPDH")]);
        let template = layout_template(&run);
        let ch = &template.wells[0].channels[0];
        // Layout preserved.
        assert_eq!(template.wells[0].sample.as_deref(), Some("S1"));
        assert_eq!(ch.fluorophore, "FAM");
        assert_eq!(ch.target.as_deref(), Some("GAPDH"));
        // Data stripped.
        assert!(ch.amplification.is_empty());
        assert_eq!(ch.cq, None);
        assert!(ch.melt.is_none());
    }
}
