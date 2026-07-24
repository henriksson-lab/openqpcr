//! Cq (quantification-cycle) determination and melt-peak calling from raw curves.
//!
//! Bio-Rad exports already carry `Cq`, but native `.pcrd` files (and any run
//! acquired live) store only raw per-cycle fluorescence. This module reproduces
//! the standard CFX-style analysis so we can compute Cq ourselves:
//!
//! 1. **Baseline subtraction** — fit an ordinary-least-squares line to an early
//!    "baseline" cycle window and subtract it, flattening the pre-exponential
//!    region to ~0.
//! 2. **Threshold** — a single threshold across the plate (auto-derived from the
//!    baseline-subtracted curves, or supplied).
//! 3. **Cq** — the fractional cycle where a curve crosses the threshold, by
//!    **linear** interpolation between the two bracketing cycles on the
//!    `cycle + 0.5` timebase, requiring the curve to stay above threshold — the
//!    same `xIntersection` logic CFX's `CtDetectionAlgorithm.Threshold` uses.
//!
//! Melt curves get the analogous treatment: a smoothed negative derivative
//! `-dRFU/dT` and local-maximum peak (Tm) detection.
//!
//! The threshold-crossing math, efficiency formula (`10^(-1/slope) - 1`), and
//! melt-derivative shape were reproduced from CFX Manager's `BioRad.PCR.Analysis`.
//! Validated against a real Bio-Rad CFX run: recomputed Cq lands within a mean
//! ~0.65 cycle of CFX's own values (near-zero bias). The residual is because CFX
//! chooses the baseline window and threshold height **adaptively per well** (a
//! Savitzky-Golay curvature analysis); we use a fixed baseline window and a
//! fraction-of-peak threshold, which is close but not bit-exact. Constants live in
//! [`CqParams`] / [`MeltParams`] and can be tuned per analysis mode.

use crate::model::QpcrRun;

/// Parameters for Cq determination. Defaults follow common CFX "Single Threshold"
/// behaviour; adjust to match a particular instrument/analysis mode.
#[derive(Debug, Clone, Copy)]
pub struct CqParams {
    /// First cycle (1-based) of the baseline window.
    pub baseline_start: usize,
    /// Last cycle (1-based) of the baseline window. The window is clamped to the
    /// data and to at least two points.
    pub baseline_end: usize,
    /// If set, use this fixed threshold (in baseline-subtracted RFU). If `None`,
    /// derive one automatically from the plate's curves (see [`auto_threshold`]).
    pub threshold: Option<f64>,
    /// Auto-threshold factor: threshold = `factor` × (max end-point RFU across
    /// curves). A curve must exceed this to be called.
    pub auto_threshold_fraction: f64,
    /// A curve whose peak baseline-subtracted RFU is below this is treated as
    /// non-amplifying (no Cq), guarding against calling noise.
    pub min_amplitude: f64,
}

impl Default for CqParams {
    fn default() -> Self {
        // CFX's default baseline region is auto-selected per well but is bounded to
        // ~cycles 2–8 (`c_PCRBaseLineIndexEndDefault`, with a guard forcing end ≤ 8);
        // 2–8 is CFX's own fallback window and a good fixed default here.
        CqParams {
            baseline_start: 2,
            baseline_end: 8,
            threshold: None,
            // Calibrated against a real Bio-Rad CFX run (biorad_cfx_melt): with a
            // linear crossing on the cycle+0.5 timebase, 0.10 of the peak RFU gives
            // a near-unbiased Cq (mean |ΔCq| ≈ 0.65 vs CFX). Exact per-well parity
            // would need CFX's adaptive curvature-based threshold (see module docs).
            auto_threshold_fraction: 0.10,
            min_amplitude: 20.0,
        }
    }
}

/// Fit a line to the baseline window `[start, end]` (1-based, inclusive) of `rfu`
/// and subtract it from every point, returning the baseline-subtracted trace.
///
/// The window is clamped to the available cycles and to at least two points. A
/// linear (rather than flat-mean) baseline removes optical drift.
pub fn baseline_subtract(rfu: &[f64], start: usize, end: usize) -> Vec<f64> {
    let n = rfu.len();
    if n == 0 {
        return Vec::new();
    }
    // Clamp the window to valid 0-based indices with at least two points.
    let lo = start.saturating_sub(1).min(n.saturating_sub(1));
    let hi = end.saturating_sub(1).min(n.saturating_sub(1)).max(lo + 1).min(n - 1);
    let (slope, intercept) = linear_fit_over(rfu, lo, hi);
    (0..n)
        .map(|i| rfu[i] - (slope * i as f64 + intercept))
        .collect()
}

/// Least-squares line `y = slope*x + intercept` over indices `[lo, hi]` of `y`,
/// with `x` the (0-based) index. Falls back to a flat mean if `x` is degenerate.
fn linear_fit_over(y: &[f64], lo: usize, hi: usize) -> (f64, f64) {
    let idx: Vec<usize> = (lo..=hi.min(y.len().saturating_sub(1))).collect();
    let n = idx.len() as f64;
    if n < 2.0 {
        let mean = idx.iter().map(|&i| y[i]).sum::<f64>() / n.max(1.0);
        return (0.0, mean);
    }
    let mean_x = idx.iter().map(|&i| i as f64).sum::<f64>() / n;
    let mean_y = idx.iter().map(|&i| y[i]).sum::<f64>() / n;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for &i in &idx {
        let dx = i as f64 - mean_x;
        sxx += dx * dx;
        sxy += dx * (y[i] - mean_y);
    }
    if sxx <= f64::EPSILON {
        return (0.0, mean_y);
    }
    let slope = sxy / sxx;
    (slope, mean_y - slope * mean_x)
}

/// Number of points a crossing must stay above threshold to count (CFX
/// `c_ThresholdOverCount`), rejecting single-point noise spikes.
const THRESHOLD_OVER_COUNT: usize = 2;

/// CFX x-axis offset: a data point at 0-based cycle index `j` sits at
/// `x = j + 0.5` (`c_dbTimeBaseOffset`). The reported Cq is in this coordinate.
const TIMEBASE_OFFSET: f64 = 0.5;

/// The fractional cycle at which `curve` rises through `threshold`, matching
/// CFX's `Utils.xIntersection`: **linear** interpolation between the two
/// bracketing points on the baseline-subtracted curve, on the `cycle + 0.5`
/// timebase, requiring the curve to stay above threshold for
/// [`THRESHOLD_OVER_COUNT`] points. Returns `None` if it never crosses upward.
pub fn threshold_cq(curve: &[f64], threshold: f64) -> Option<f64> {
    if threshold <= 0.0 {
        return None;
    }
    let n = curve.len();
    for i in 1..n {
        let (a, b) = (curve[i - 1], curve[i]);
        // Upward crossing: previous point below threshold, this one at/above.
        if a < threshold && b >= threshold {
            // Must remain above threshold for the required run length.
            let stays = (i..(i + THRESHOLD_OVER_COUNT).min(n)).all(|k| curve[k] >= threshold);
            if !stays {
                continue;
            }
            // Linear interpolation (NOT log) between the bracketing points.
            let frac = if (b - a).abs() < f64::EPSILON {
                0.0
            } else {
                (threshold - a) / (b - a)
            };
            // Point i-1 sits at x = (i-1)+0.5; the crossing is `frac` of the way
            // to point i at x = i+0.5.
            return Some((i - 1) as f64 + TIMEBASE_OFFSET + frac);
        }
    }
    None
}

/// Derive a single plate-wide threshold from baseline-subtracted `curves`:
/// `fraction` × the maximum end-point RFU across all curves. Returns `None` when
/// there is no positive signal to key off.
pub fn auto_threshold(curves: &[Vec<f64>], fraction: f64) -> Option<f64> {
    let max_end = curves
        .iter()
        .filter_map(|c| c.last().copied())
        .fold(f64::MIN, f64::max);
    if max_end.is_finite() && max_end > 0.0 {
        Some(max_end * fraction)
    } else {
        None
    }
}

/// Compute Cq for one raw amplification trace with the given parameters and a
/// pre-computed `threshold` (baseline-subtracted RFU units). Returns `None` when
/// the curve does not amplify above `min_amplitude` or never crosses.
pub fn cq_for_trace(rfu: &[f64], threshold: f64, params: &CqParams) -> Option<f64> {
    let bs = baseline_subtract(rfu, params.baseline_start, params.baseline_end);
    let peak = bs.iter().copied().fold(f64::MIN, f64::max);
    if !peak.is_finite() || peak < params.min_amplitude {
        return None;
    }
    threshold_cq(&bs, threshold)
}

/// Fill in `Channel.cq` for every channel that has raw amplification but no Cq,
/// using a single plate-wide threshold per fluorophore. Channels that already
/// carry a Cq (e.g. from a CFX export) are left untouched. Returns the number of
/// Cq values computed.
///
/// A separate threshold is derived per fluorophore, because different dyes sit at
/// different fluorescence scales.
pub fn annotate_run(run: &mut QpcrRun, params: &CqParams) -> usize {
    // Collect the fluorophores that need calling.
    let fluors: Vec<String> = {
        let mut fs: Vec<String> = Vec::new();
        for w in &run.wells {
            for c in &w.channels {
                if c.cq.is_none() && !c.amplification.is_empty() && !fs.contains(&c.fluorophore) {
                    fs.push(c.fluorophore.clone());
                }
            }
        }
        fs
    };

    let mut count = 0;
    for fluor in &fluors {
        // Baseline-subtract every curve of this fluorophore to pick a threshold.
        let curves: Vec<Vec<f64>> = run
            .wells
            .iter()
            .flat_map(|w| w.channels.iter())
            .filter(|c| &c.fluorophore == fluor && !c.amplification.is_empty())
            .map(|c| baseline_subtract(&c.amplification, params.baseline_start, params.baseline_end))
            .collect();
        let threshold = match params.threshold.or_else(|| {
            auto_threshold(&curves, params.auto_threshold_fraction)
        }) {
            Some(t) if t > 0.0 => t,
            _ => continue,
        };

        for w in &mut run.wells {
            for c in &mut w.channels {
                if &c.fluorophore == fluor && c.cq.is_none() && !c.amplification.is_empty()
                    && let Some(cq) = cq_for_trace(&c.amplification, threshold, params) {
                        c.cq = Some(cq);
                        count += 1;
                    }
            }
        }
    }
    count
}

// ---- Melt-curve analysis ---------------------------------------------------

/// Parameters for melt-peak (Tm) calling.
#[derive(Debug, Clone, Copy)]
pub struct MeltParams {
    /// Half-width (in points) of the moving-average smoothing applied before
    /// differentiating. `0` disables smoothing.
    pub smooth_half_width: usize,
    /// A peak in `-dRFU/dT` must reach at least this fraction of the largest peak
    /// to be reported (rejects shoulders/noise).
    pub min_peak_fraction: f64,
}

impl Default for MeltParams {
    fn default() -> Self {
        MeltParams {
            smooth_half_width: 1,
            min_peak_fraction: 0.10,
        }
    }
}

/// Smoothed negative derivative `-dRFU/dT`, aligned to `temperature`. The melt
/// peak (Tm) is a local maximum of this curve. Uses a centred finite difference
/// after an optional moving-average smooth.
pub fn negative_derivative(temperature: &[f64], rfu: &[f64], half_width: usize) -> Vec<f64> {
    let n = temperature.len().min(rfu.len());
    if n < 3 {
        return vec![0.0; n];
    }
    let smoothed = moving_average(&rfu[..n], half_width);
    let mut out = vec![0.0; n];
    for i in 1..n - 1 {
        let dt = temperature[i + 1] - temperature[i - 1];
        if dt.abs() > f64::EPSILON {
            out[i] = -(smoothed[i + 1] - smoothed[i - 1]) / dt;
        }
    }
    out[0] = out[1];
    out[n - 1] = out[n - 2];
    out
}

fn moving_average(v: &[f64], half_width: usize) -> Vec<f64> {
    if half_width == 0 {
        return v.to_vec();
    }
    let n = v.len();
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half_width);
            let hi = (i + half_width).min(n - 1);
            let win = &v[lo..=hi];
            win.iter().sum::<f64>() / win.len() as f64
        })
        .collect()
}

/// Melt-peak temperatures (Tm), as local maxima of the negative derivative that
/// clear `min_peak_fraction` of the tallest peak. The peak temperature is refined
/// by a 3-point parabolic interpolation around each maximum.
pub fn melt_peaks(temperature: &[f64], neg_deriv: &[f64], params: &MeltParams) -> Vec<f64> {
    let n = temperature.len().min(neg_deriv.len());
    if n < 3 {
        return Vec::new();
    }
    let max_peak = neg_deriv[1..n - 1].iter().copied().fold(f64::MIN, f64::max);
    if !max_peak.is_finite() || max_peak <= 0.0 {
        return Vec::new();
    }
    let cutoff = max_peak * params.min_peak_fraction;
    let mut peaks = Vec::new();
    for i in 1..n - 1 {
        let (l, c, r) = (neg_deriv[i - 1], neg_deriv[i], neg_deriv[i + 1]);
        if c > l && c >= r && c >= cutoff {
            // Parabolic vertex offset in index units, mapped to temperature.
            let denom = l - 2.0 * c + r;
            let delta = if denom.abs() > f64::EPSILON {
                0.5 * (l - r) / denom
            } else {
                0.0
            };
            let step = temperature[i + 1] - temperature[i - 1];
            let tm = temperature[i] + delta * step / 2.0;
            peaks.push(tm);
        }
    }
    peaks
}

/// Populate every channel's melt `derivative` and `peaks` from its raw melt
/// `temperature`/`rfu`, where present and not already filled. Returns the number
/// of melt curves annotated.
pub fn annotate_melt(run: &mut QpcrRun, params: &MeltParams) -> usize {
    let mut count = 0;
    for w in &mut run.wells {
        for c in &mut w.channels {
            if let Some(melt) = &mut c.melt
                && melt.rfu.len() >= 3 && (melt.derivative.is_empty() || melt.peaks.is_empty()) {
                    let deriv = negative_derivative(&melt.temperature, &melt.rfu, params.smooth_half_width);
                    let peaks = melt_peaks(&melt.temperature, &deriv, params);
                    if melt.derivative.is_empty() {
                        melt.derivative = deriv;
                    }
                    if melt.peaks.is_empty() {
                        melt.peaks = peaks;
                    }
                    count += 1;
                }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, MeltCurve, QpcrRun, Well};

    /// A logistic amplification curve with a flat baseline and known midpoint.
    fn sigmoid(cycles: usize, cq: f64, amplitude: f64, baseline: f64) -> Vec<f64> {
        (1..=cycles)
            .map(|c| {
                let x = 0.5 * (c as f64 - cq);
                baseline + amplitude / (1.0 + (-x).exp())
            })
            .collect()
    }

    #[test]
    fn baseline_subtract_flattens_baseline() {
        // Curve with a sloped baseline: y = 3 + 0.5*i over the baseline window.
        let rfu: Vec<f64> = (0..30).map(|i| 3.0 + 0.5 * i as f64).collect();
        let bs = baseline_subtract(&rfu, 2, 10);
        // A purely linear input is fully removed → ~0 everywhere.
        for v in &bs {
            assert!(v.abs() < 1e-6, "residual {v}");
        }
    }

    #[test]
    fn threshold_crossing_is_fractional_and_ordered() {
        // Two curves with different Cq → earlier Cq crosses first. Both Cq values
        // sit well past the baseline window (cycles 2–10), so baseline subtraction
        // is clean and the crossing offset is preserved exactly.
        let early = sigmoid(45, 22.0, 1000.0, 5.0);
        let late = sigmoid(45, 32.0, 1000.0, 5.0);
        let p = CqParams::default();
        let curves = vec![
            baseline_subtract(&early, p.baseline_start, p.baseline_end),
            baseline_subtract(&late, p.baseline_start, p.baseline_end),
        ];
        let thr = auto_threshold(&curves, p.auto_threshold_fraction).unwrap();
        let cq_early = cq_for_trace(&early, thr, &p).unwrap();
        let cq_late = cq_for_trace(&late, thr, &p).unwrap();
        assert!(cq_early < cq_late);
        // Two identically-shaped curves offset by 10 cycles must yield a Cq
        // difference of ~10, whatever the (shared) threshold — the core property.
        assert!(
            (cq_late - cq_early - 10.0).abs() < 0.5,
            "ΔCq = {} (expected ~10)",
            cq_late - cq_early
        );
        assert!(cq_early.fract() != 0.0, "expected a fractional cycle");
    }

    #[test]
    fn threshold_crossing_matches_cfx_linear_timebase() {
        // Baseline-subtracted curve: flat 0 then a linear ramp.
        // idx:   0  1  2  3  4   5   6   7    8
        let curve = [0.0, 0.0, 0.0, 0.0, 0.0, 40.0, 80.0, 120.0, 160.0];
        // Threshold 60 crosses between idx5 (40) and idx6 (80): linear frac 0.5.
        // Cq = (6-1) + 0.5 (timebase) + 0.5 (frac) = 6.0.
        let cq = threshold_cq(&curve, 60.0).unwrap();
        assert!((cq - 6.0).abs() < 1e-9, "cq = {cq}");
        // A single-point spike that drops back must NOT be called (stay-above rule).
        let spike = [0.0, 0.0, 100.0, 0.0, 0.0, 0.0];
        assert_eq!(threshold_cq(&spike, 60.0), None);
    }

    #[test]
    fn flat_curve_gets_no_cq() {
        let flat: Vec<f64> = (0..40).map(|i| 5.0 + 0.01 * i as f64).collect();
        let p = CqParams::default();
        // Threshold from a real amplifier so it's a sane positive value.
        let amp = sigmoid(40, 20.0, 1000.0, 5.0);
        let thr = auto_threshold(
            &[baseline_subtract(&amp, p.baseline_start, p.baseline_end)],
            p.auto_threshold_fraction,
        )
        .unwrap();
        assert_eq!(cq_for_trace(&flat, thr, &p), None);
    }

    #[test]
    fn annotate_run_fills_missing_cq_only() {
        let mut run = QpcrRun {
            wells: vec![
                Well {
                    row: 0,
                    col: 0,
                    channels: vec![Channel {
                        fluorophore: "SYBR".into(),
                        amplification: sigmoid(40, 20.0, 1000.0, 5.0),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Well {
                    row: 0,
                    col: 1,
                    channels: vec![Channel {
                        fluorophore: "SYBR".into(),
                        cq: Some(99.0), // pre-existing: must be preserved
                        amplification: sigmoid(40, 25.0, 1000.0, 5.0),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let n = annotate_run(&mut run, &CqParams::default());
        assert_eq!(n, 1);
        assert!(run.wells[0].channels[0].cq.is_some());
        assert_eq!(run.wells[1].channels[0].cq, Some(99.0));
    }

    #[test]
    fn melt_peak_recovers_tm() {
        // A single melt transition centred at 82 °C: RFU drops sigmoidally, so the
        // negative derivative peaks at 82.
        let temperature: Vec<f64> = (0..=60).map(|i| 65.0 + i as f64 * 0.5).collect();
        let rfu: Vec<f64> = temperature
            .iter()
            .map(|&t| 1000.0 / (1.0 + ((t - 82.0) / 0.8).exp()))
            .collect();
        let params = MeltParams::default();
        let deriv = negative_derivative(&temperature, &rfu, params.smooth_half_width);
        let peaks = melt_peaks(&temperature, &deriv, &params);
        assert_eq!(peaks.len(), 1, "peaks: {peaks:?}");
        assert!((peaks[0] - 82.0).abs() < 0.6, "Tm = {}", peaks[0]);
    }

    #[test]
    fn annotate_melt_fills_derivative_and_peaks() {
        let temperature: Vec<f64> = (0..=60).map(|i| 65.0 + i as f64 * 0.5).collect();
        let rfu: Vec<f64> = temperature
            .iter()
            .map(|&t| 1000.0 / (1.0 + ((t - 80.0) / 0.8).exp()))
            .collect();
        let mut run = QpcrRun {
            wells: vec![Well {
                channels: vec![Channel {
                    fluorophore: "SYBR".into(),
                    melt: Some(MeltCurve {
                        temperature,
                        rfu,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let n = annotate_melt(&mut run, &MeltParams::default());
        assert_eq!(n, 1);
        let melt = run.wells[0].channels[0].melt.as_ref().unwrap();
        assert!(!melt.derivative.is_empty());
        assert_eq!(melt.peaks.len(), 1);
        assert!((melt.peaks[0] - 80.0).abs() < 0.6);
    }
}
