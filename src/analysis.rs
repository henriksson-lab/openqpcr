//! Downstream analyses computed from a parsed [`QpcrRun`].
//!
//! Everything here is pure, model-driven computation (no GUI, no I/O) so it can
//! be unit-tested in isolation. Three families of analysis are provided:
//!
//! * [`gene_expression`] — ΔΔCq relative quantification.
//! * [`allelic_discrimination`] — end-point genotyping scatter + calls.
//! * [`qc_metrics`] — plate-level quality-control summary.

use std::collections::BTreeMap;

use crate::model::{QpcrRun, SampleType};

// ---------------------------------------------------------------------------
// Gene expression (ΔΔCq relative quantification)
// ---------------------------------------------------------------------------

/// Housekeeping / reference-gene names, in preference order. A target whose name
/// contains one of these (case-insensitive) is auto-selected as the reference.
const HOUSEKEEPING: &[&str] = &[
    "GAPDH", "ACTB", "ACTIN", "B2M", "18S", "HPRT", "RPLP0", "TBP",
];

/// One (target, sample) cell of the gene-expression table.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneExprRow {
    pub target: String,
    pub sample: String,
    /// Mean Cq across replicate channels in this (target, sample) group.
    pub mean_cq: f64,
    /// ΔCq = mean_cq(target, sample) − mean_cq(reference target, sample).
    /// `None` when there is no reference target, or it is absent for this sample.
    pub delta_cq: Option<f64>,
    /// Relative expression = 2^(−ΔΔCq). `None` when it cannot be computed
    /// (single target, missing reference, or missing calibrator data).
    pub rel_expr: Option<f64>,
}

/// Result of [`gene_expression`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeneExprResult {
    /// Rows sorted by (target, sample).
    pub rows: Vec<GeneExprRow>,
    /// Auto-selected reference target, if any target exists.
    pub ref_target: Option<String>,
    /// Auto-selected calibrator sample, if any sample exists.
    pub calibrator: Option<String>,
    /// True when only a single distinct target is present (no rel-expr possible).
    pub single_target: bool,
}

/// Compute ΔΔCq relative quantification across the run.
///
/// Channels are grouped by `(target, sample)`; only channels that carry a Cq
/// *and* both a target and a sample name contribute. The reference target is
/// the first housekeeping-gene match (else the alphabetically-first target); the
/// calibrator is the alphabetically-first sample. Relative expression is
/// `2^(−ΔΔCq)` with `ΔΔCq = (Cq_tgt,smp − Cq_ref,smp) − (Cq_tgt,cal − Cq_ref,cal)`.
pub fn gene_expression(run: &QpcrRun) -> GeneExprResult {
    // Accumulate Cq sums per (target, sample) to form means.
    let mut sums: BTreeMap<(String, String), (f64, usize)> = BTreeMap::new();
    for well in &run.wells {
        let Some(sample) = well.sample.as_ref().filter(|s| !s.is_empty()) else {
            continue;
        };
        for ch in &well.channels {
            let (Some(target), Some(cq)) = (ch.target.as_ref(), ch.cq) else {
                continue;
            };
            if target.is_empty() {
                continue;
            }
            let entry = sums
                .entry((target.clone(), sample.clone()))
                .or_insert((0.0, 0));
            entry.0 += cq;
            entry.1 += 1;
        }
    }

    let mut means: BTreeMap<(String, String), f64> = BTreeMap::new();
    for (key, (sum, n)) in &sums {
        means.insert(key.clone(), sum / *n as f64);
    }

    // Distinct targets / samples (BTreeMap keys are already sorted).
    let mut targets: Vec<String> = means.keys().map(|(t, _)| t.clone()).collect();
    targets.sort();
    targets.dedup();
    let mut samples: Vec<String> = means.keys().map(|(_, s)| s.clone()).collect();
    samples.sort();
    samples.dedup();

    if targets.is_empty() {
        return GeneExprResult::default();
    }

    let ref_target = pick_reference_target(&targets);
    let calibrator = samples.first().cloned();
    let single_target = targets.len() < 2;

    // ΔCq(target, sample) helper.
    let dcq = |target: &str, sample: &str| -> Option<f64> {
        let cq_t = means.get(&(target.to_string(), sample.to_string()))?;
        let cq_r = means.get(&(ref_target.clone(), sample.to_string()))?;
        Some(cq_t - cq_r)
    };

    // ΔCq of each target within the calibrator sample (reused per target).
    let mut rows = Vec::with_capacity(means.len());
    for ((target, sample), &mean_cq) in &means {
        let (delta_cq, rel_expr) = if single_target {
            (None, None)
        } else {
            let d_sample = dcq(target, sample);
            let d_cal = calibrator.as_deref().and_then(|cal| dcq(target, cal));
            let rel = match (d_sample, d_cal) {
                (Some(ds), Some(dc)) => Some(2f64.powf(-(ds - dc))),
                _ => None,
            };
            (d_sample, rel)
        };
        rows.push(GeneExprRow {
            target: target.clone(),
            sample: sample.clone(),
            mean_cq,
            delta_cq,
            rel_expr,
        });
    }
    // BTreeMap iteration is already (target, sample) sorted; keep it explicit.
    rows.sort_by(|a, b| a.target.cmp(&b.target).then(a.sample.cmp(&b.sample)));

    GeneExprResult {
        rows,
        ref_target: Some(ref_target),
        calibrator,
        single_target,
    }
}

/// Pick the reference target: first housekeeping-gene match (case-insensitive,
/// substring), else the alphabetically-first target. `targets` must be non-empty.
fn pick_reference_target(targets: &[String]) -> String {
    for hk in HOUSEKEEPING {
        // Deterministic: among housekeeping matches for this keyword, take the
        // alphabetically-first target (targets is pre-sorted).
        if let Some(t) = targets.iter().find(|t| t.to_ascii_uppercase().contains(hk)) {
            return t.clone();
        }
    }
    targets[0].clone()
}

// ---------------------------------------------------------------------------
// Allelic discrimination (end-point genotyping)
// ---------------------------------------------------------------------------

/// Genotype call for a single well in an allelic-discrimination assay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllelicCall {
    /// Both alleles amplified (heterozygote).
    Both,
    /// Only allele X (the first fluorophore) amplified.
    Allele1,
    /// Only allele Y (the second fluorophore) amplified.
    Allele2,
    /// Neither allele amplified (no-template / no call).
    None,
}

impl AllelicCall {
    /// Human-readable label used in the UI.
    pub fn label(self) -> &'static str {
        match self {
            AllelicCall::Both => "Both",
            AllelicCall::Allele1 => "Allele 1",
            AllelicCall::Allele2 => "Allele 2",
            AllelicCall::None => "NTC/None",
        }
    }
}

/// One well's end-point (x_rfu, y_rfu) coordinate plus its genotype call.
#[derive(Debug, Clone, PartialEq)]
pub struct AllelicPoint {
    pub well_label: String,
    pub x: f64,
    pub y: f64,
    pub call: AllelicCall,
}

/// Result of [`allelic_discrimination`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllelicResult {
    pub points: Vec<AllelicPoint>,
    /// Fluorophore mapped to allele X (first / most frequent).
    pub allele_x: Option<String>,
    /// Fluorophore mapped to allele Y (second most frequent).
    pub allele_y: Option<String>,
}

/// Build an end-point allelic-discrimination scatter.
///
/// The two most-frequently-occurring fluorophores are chosen as allele X and Y
/// (ties broken alphabetically). Each well contributes one point: X/Y are the
/// last amplification RFU of the matching channel; the call is derived from
/// which channels carry a Cq. Runs with fewer than two fluorophores yield no
/// points (but still report whatever single fluorophore exists as allele X).
pub fn allelic_discrimination(run: &QpcrRun) -> AllelicResult {
    // Count fluorophore occurrences across all channels.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for well in &run.wells {
        for ch in &well.channels {
            if !ch.fluorophore.is_empty() {
                *counts.entry(ch.fluorophore.clone()).or_insert(0) += 1;
            }
        }
    }

    // Rank by count desc, then name asc (BTreeMap gives stable name order).
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let allele_x = ranked.first().map(|(f, _)| f.clone());
    let allele_y = ranked.get(1).map(|(f, _)| f.clone());

    let (Some(fx), Some(fy)) = (&allele_x, &allele_y) else {
        // Fewer than two fluorophores: nothing to plot.
        return AllelicResult {
            points: Vec::new(),
            allele_x,
            allele_y,
        };
    };

    let mut points = Vec::new();
    for well in &run.wells {
        let cx = well.channels.iter().find(|c| &c.fluorophore == fx);
        let cy = well.channels.iter().find(|c| &c.fluorophore == fy);
        // A well with neither channel present is not part of this assay.
        if cx.is_none() && cy.is_none() {
            continue;
        }
        let x = cx
            .and_then(|c| c.amplification.last().copied())
            .unwrap_or(0.0);
        let y = cy
            .and_then(|c| c.amplification.last().copied())
            .unwrap_or(0.0);
        let has_x = cx.and_then(|c| c.cq).is_some();
        let has_y = cy.and_then(|c| c.cq).is_some();
        let call = match (has_x, has_y) {
            (true, true) => AllelicCall::Both,
            (true, false) => AllelicCall::Allele1,
            (false, true) => AllelicCall::Allele2,
            (false, false) => AllelicCall::None,
        };
        points.push(AllelicPoint {
            well_label: well.position(),
            x,
            y,
            call,
        });
    }

    AllelicResult {
        points,
        allele_x,
        allele_y,
    }
}

// ---------------------------------------------------------------------------
// QC metrics
// ---------------------------------------------------------------------------

/// Compute plate-level QC metrics as an ordered list of (label, value) pairs,
/// ready to drop into a key/value table.
pub fn qc_metrics(run: &QpcrRun) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();

    let total = run.plate.well_count();
    out.push(("Total wells".into(), total.to_string()));
    out.push(("Occupied wells".into(), run.wells.len().to_string()));

    // Counts per sample type.
    let mut type_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for well in &run.wells {
        *type_counts
            .entry(sample_type_label(well.sample_type))
            .or_insert(0) += 1;
    }
    for (label, n) in &type_counts {
        out.push((format!("  {label}"), n.to_string()));
    }

    // Call rate: fraction of channels with a Cq.
    let mut n_channels = 0usize;
    let mut n_called = 0usize;
    for well in &run.wells {
        for ch in &well.channels {
            n_channels += 1;
            if ch.cq.is_some() {
                n_called += 1;
            }
        }
    }
    let call_rate = if n_channels > 0 {
        n_called as f64 / n_channels as f64 * 100.0
    } else {
        0.0
    };
    out.push((
        "Call rate".into(),
        format!("{call_rate:.1}% ({n_called}/{n_channels})"),
    ));

    // Control check: NTC / NRT wells that unexpectedly amplified (carry a Cq).
    let mut bad_controls: Vec<String> = Vec::new();
    for well in &run.wells {
        if matches!(well.sample_type, SampleType::Ntc | SampleType::Nrt)
            && well.channels.iter().any(|c| c.cq.is_some())
        {
            bad_controls.push(well.position());
        }
    }
    let control_summary = if bad_controls.is_empty() {
        "PASS (no NTC/NRT amplification)".to_string()
    } else {
        format!(
            "FAIL: {} amplified [{}]",
            bad_controls.len(),
            bad_controls.join(", ")
        )
    };
    out.push(("Control check".into(), control_summary));

    // Replicate CV%: for each (sample, target) group with >=2 Cq values.
    let cv = replicate_cv(run);
    if cv.n_groups == 0 {
        out.push(("Replicate CV".into(), "n/a (no replicate groups)".into()));
    } else {
        out.push((
            "Mean replicate CV".into(),
            format!("{:.2}% ({} groups)", cv.mean_cv_pct, cv.n_groups),
        ));
        out.push(("Groups > 0.5 Cq SD".into(), cv.n_high_sd.to_string()));
    }

    out
}

/// Aggregated replicate-precision statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplicateCv {
    /// Number of (sample, target) groups with >= 2 Cq values.
    pub n_groups: usize,
    /// Mean of per-group CV% (coefficient of variation on Cq).
    pub mean_cv_pct: f64,
    /// Count of groups whose Cq standard deviation exceeds 0.5.
    pub n_high_sd: usize,
}

/// Compute replicate CV across (sample, target) groups with >= 2 Cq values.
pub fn replicate_cv(run: &QpcrRun) -> ReplicateCv {
    let mut groups: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for well in &run.wells {
        let sample = well.sample.clone().unwrap_or_default();
        for ch in &well.channels {
            if let Some(cq) = ch.cq {
                let target = ch.target.clone().unwrap_or_else(|| ch.fluorophore.clone());
                groups.entry((sample.clone(), target)).or_default().push(cq);
            }
        }
    }

    let mut n_groups = 0usize;
    let mut cv_sum = 0.0;
    let mut n_high_sd = 0usize;
    for values in groups.values() {
        if values.len() < 2 {
            continue;
        }
        let (mean, sd) = mean_std(values);
        n_groups += 1;
        if mean.abs() > f64::EPSILON {
            cv_sum += sd / mean * 100.0;
        }
        if sd > 0.5 {
            n_high_sd += 1;
        }
    }

    ReplicateCv {
        n_groups,
        mean_cv_pct: if n_groups > 0 {
            cv_sum / n_groups as f64
        } else {
            0.0
        },
        n_high_sd,
    }
}

/// Mean and *sample* standard deviation (n−1 denominator) of `values`.
/// `values` must be non-empty; a single value yields sd = 0.
fn mean_std(values: &[f64]) -> (f64, f64) {
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    if n < 2 {
        return (mean, 0.0);
    }
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    (mean, var.sqrt())
}

fn sample_type_label(t: SampleType) -> &'static str {
    match t {
        SampleType::Unknown => "Unknown",
        SampleType::Standard => "Standard",
        SampleType::Ntc => "NTC",
        SampleType::Nrt => "NRT",
        SampleType::PositiveControl => "Positive Control",
        SampleType::NegativeControl => "Negative Control",
        SampleType::Empty => "Empty",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, PlateFormat, Well};

    fn well(row: u8, col: u8, sample: &str, st: SampleType, channels: Vec<Channel>) -> Well {
        Well {
            row,
            col,
            sample: Some(sample.to_string()),
            sample_type: st,
            biological_group: None,
            starting_quantity: None,
            channels,
        }
    }

    fn ch(fluor: &str, target: Option<&str>, cq: Option<f64>, amp: Vec<f64>) -> Channel {
        Channel {
            fluorophore: fluor.to_string(),
            target: target.map(|s| s.to_string()),
            cq,
            amplification: amp,
            melt: None,
        }
    }

    // ----- Gene expression -----

    #[test]
    fn ddcq_known_one_cq_difference_gives_two_and_half() {
        // Reference GAPDH constant at Cq 20 in both samples.
        // Target GENE: calibrator (Ctrl) Cq 25, treated (Treat) Cq 24 → 1 Cq less.
        //   ΔCq(Treat) = 24 - 20 = 4 ; ΔCq(Ctrl) = 25 - 20 = 5
        //   ΔΔCq(Treat) = 4 - 5 = -1 → rel = 2^1 = 2.0
        //   ΔΔCq(Ctrl) = 0 → rel = 1.0
        // Also verify the opposite target that is 1 Cq HIGHER → rel 0.5.
        let run = QpcrRun {
            wells: vec![
                well(
                    0,
                    0,
                    "Ctrl",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(25.0), vec![])],
                ),
                well(
                    0,
                    1,
                    "Ctrl",
                    SampleType::Unknown,
                    vec![ch("HEX", Some("GAPDH"), Some(20.0), vec![])],
                ),
                well(
                    0,
                    2,
                    "Treat",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(24.0), vec![])],
                ),
                well(
                    0,
                    3,
                    "Treat",
                    SampleType::Unknown,
                    vec![ch("HEX", Some("GAPDH"), Some(20.0), vec![])],
                ),
            ],
            ..Default::default()
        };
        let res = gene_expression(&run);
        assert_eq!(res.ref_target.as_deref(), Some("GAPDH"));
        assert_eq!(res.calibrator.as_deref(), Some("Ctrl")); // alphabetically first
        assert!(!res.single_target);

        let treat = res
            .rows
            .iter()
            .find(|r| r.target == "GENE" && r.sample == "Treat")
            .unwrap();
        assert!(
            (treat.rel_expr.unwrap() - 2.0).abs() < 1e-9,
            "got {:?}",
            treat.rel_expr
        );

        let ctrl = res
            .rows
            .iter()
            .find(|r| r.target == "GENE" && r.sample == "Ctrl")
            .unwrap();
        assert!((ctrl.rel_expr.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ddcq_one_cq_higher_gives_half() {
        // Target 1 Cq HIGHER in treated → half the expression.
        let run = QpcrRun {
            wells: vec![
                well(
                    0,
                    0,
                    "Ctrl",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(25.0), vec![])],
                ),
                well(
                    0,
                    1,
                    "Ctrl",
                    SampleType::Unknown,
                    vec![ch("HEX", Some("GAPDH"), Some(20.0), vec![])],
                ),
                well(
                    0,
                    2,
                    "Treat",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(26.0), vec![])],
                ),
                well(
                    0,
                    3,
                    "Treat",
                    SampleType::Unknown,
                    vec![ch("HEX", Some("GAPDH"), Some(20.0), vec![])],
                ),
            ],
            ..Default::default()
        };
        let res = gene_expression(&run);
        let treat = res
            .rows
            .iter()
            .find(|r| r.target == "GENE" && r.sample == "Treat")
            .unwrap();
        assert!(
            (treat.rel_expr.unwrap() - 0.5).abs() < 1e-9,
            "got {:?}",
            treat.rel_expr
        );
    }

    #[test]
    fn housekeeping_gene_is_preferred_over_alphabetical() {
        // "ZZZ" would win alphabetically, but ACTB is housekeeping.
        let targets = vec!["ACTB".to_string(), "AAA".to_string(), "ZZZ".to_string()];
        assert_eq!(pick_reference_target(&targets), "ACTB");

        // Case-insensitive substring match ("hGapdh" contains GAPDH).
        let targets2 = vec!["hGapdh".to_string(), "MyGene".to_string()];
        assert_eq!(pick_reference_target(&targets2), "hGapdh");

        // No housekeeping → alphabetically first (input is pre-sorted per contract).
        let targets3 = vec!["Bar".to_string(), "Foo".to_string()];
        assert_eq!(pick_reference_target(&targets3), "Bar");
    }

    #[test]
    fn single_target_reports_mean_cq_only() {
        let run = QpcrRun {
            wells: vec![
                well(
                    0,
                    0,
                    "A",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(20.0), vec![])],
                ),
                well(
                    0,
                    1,
                    "A",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("GENE"), Some(22.0), vec![])],
                ),
            ],
            ..Default::default()
        };
        let res = gene_expression(&run);
        assert!(res.single_target);
        assert_eq!(res.rows.len(), 1);
        assert!((res.rows[0].mean_cq - 21.0).abs() < 1e-9);
        assert!(res.rows[0].rel_expr.is_none());
        assert!(res.rows[0].delta_cq.is_none());
    }

    #[test]
    fn no_targets_yields_empty_result() {
        let run = QpcrRun {
            wells: vec![well(
                0,
                0,
                "A",
                SampleType::Unknown,
                vec![ch("FAM", None, Some(20.0), vec![])],
            )],
            ..Default::default()
        };
        let res = gene_expression(&run);
        assert!(res.rows.is_empty());
        assert!(res.ref_target.is_none());
    }

    // ----- Allelic discrimination -----

    #[test]
    fn allelic_classifies_four_cases() {
        let run = QpcrRun {
            wells: vec![
                // both
                well(
                    0,
                    0,
                    "s1",
                    SampleType::Unknown,
                    vec![
                        ch("FAM", None, Some(22.0), vec![1.0, 900.0]),
                        ch("VIC", None, Some(23.0), vec![1.0, 850.0]),
                    ],
                ),
                // only X (FAM)
                well(
                    0,
                    1,
                    "s2",
                    SampleType::Unknown,
                    vec![
                        ch("FAM", None, Some(22.0), vec![1.0, 900.0]),
                        ch("VIC", None, None, vec![1.0, 10.0]),
                    ],
                ),
                // only Y (VIC)
                well(
                    0,
                    2,
                    "s3",
                    SampleType::Unknown,
                    vec![
                        ch("FAM", None, None, vec![1.0, 12.0]),
                        ch("VIC", None, Some(23.0), vec![1.0, 850.0]),
                    ],
                ),
                // neither
                well(
                    0,
                    3,
                    "s4",
                    SampleType::Unknown,
                    vec![
                        ch("FAM", None, None, vec![1.0, 8.0]),
                        ch("VIC", None, None, vec![1.0, 9.0]),
                    ],
                ),
            ],
            ..Default::default()
        };
        let res = allelic_discrimination(&run);
        // FAM and VIC both appear 4 times; tie broken alphabetically → FAM = X.
        assert_eq!(res.allele_x.as_deref(), Some("FAM"));
        assert_eq!(res.allele_y.as_deref(), Some("VIC"));
        assert_eq!(res.points.len(), 4);
        assert_eq!(res.points[0].call, AllelicCall::Both);
        assert_eq!(res.points[1].call, AllelicCall::Allele1);
        assert_eq!(res.points[2].call, AllelicCall::Allele2);
        assert_eq!(res.points[3].call, AllelicCall::None);
        // End-point RFU = last amplification value.
        assert!((res.points[0].x - 900.0).abs() < 1e-9);
        assert!((res.points[0].y - 850.0).abs() < 1e-9);
        assert_eq!(res.points[0].call.label(), "Both");
    }

    #[test]
    fn allelic_handles_single_fluorophore() {
        let run = QpcrRun {
            wells: vec![well(
                0,
                0,
                "s1",
                SampleType::Unknown,
                vec![ch("FAM", None, Some(22.0), vec![900.0])],
            )],
            ..Default::default()
        };
        let res = allelic_discrimination(&run);
        assert_eq!(res.allele_x.as_deref(), Some("FAM"));
        assert!(res.allele_y.is_none());
        assert!(res.points.is_empty());
    }

    // ----- QC / CV -----

    #[test]
    fn cv_computation_matches_hand_value() {
        // Two replicates 20.0 and 22.0: mean 21, sample SD sqrt(2)=1.41421,
        // CV = 1.41421 / 21 * 100 = 6.734%. SD > 0.5 → high-SD group.
        let run = QpcrRun {
            wells: vec![
                well(
                    0,
                    0,
                    "s1",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("G"), Some(20.0), vec![])],
                ),
                well(
                    0,
                    1,
                    "s1",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("G"), Some(22.0), vec![])],
                ),
            ],
            ..Default::default()
        };
        let cv = replicate_cv(&run);
        assert_eq!(cv.n_groups, 1);
        assert!(
            (cv.mean_cv_pct - 6.7343).abs() < 1e-3,
            "got {}",
            cv.mean_cv_pct
        );
        assert_eq!(cv.n_high_sd, 1);
    }

    #[test]
    fn mean_std_sample_denominator() {
        let (mean, sd) = mean_std(&[20.0, 22.0]);
        assert!((mean - 21.0).abs() < 1e-12);
        assert!((sd - std::f64::consts::SQRT_2).abs() < 1e-12);
        // Single value → sd 0.
        let (m1, s1) = mean_std(&[5.0]);
        assert_eq!(m1, 5.0);
        assert_eq!(s1, 0.0);
    }

    #[test]
    fn qc_metrics_control_check_flags_ntc_amplification() {
        let run = QpcrRun {
            plate: PlateFormat::P96,
            wells: vec![
                well(
                    0,
                    0,
                    "unk",
                    SampleType::Unknown,
                    vec![ch("FAM", Some("G"), Some(20.0), vec![])],
                ),
                // NTC that unexpectedly amplified:
                well(
                    0,
                    1,
                    "ntc",
                    SampleType::Ntc,
                    vec![ch("FAM", Some("G"), Some(35.0), vec![])],
                ),
            ],
            ..Default::default()
        };
        let m = qc_metrics(&run);
        let total = m.iter().find(|(k, _)| k == "Total wells").unwrap();
        assert_eq!(total.1, "96");
        let control = m.iter().find(|(k, _)| k == "Control check").unwrap();
        assert!(control.1.starts_with("FAIL"), "got {}", control.1);
        assert!(control.1.contains("A2"));
        // Call rate = 2/2 = 100%.
        let call = m.iter().find(|(k, _)| k == "Call rate").unwrap();
        assert!(call.1.starts_with("100.0%"));
    }

    #[test]
    fn qc_metrics_empty_run_does_not_panic() {
        let run = QpcrRun::default();
        let m = qc_metrics(&run);
        assert!(m.iter().any(|(k, _)| k == "Total wells"));
        let control = m.iter().find(|(k, _)| k == "Control check").unwrap();
        assert!(control.1.starts_with("PASS"));
    }
}
