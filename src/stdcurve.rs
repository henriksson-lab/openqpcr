//! Standard-curve regression for qPCR quantification.
//!
//! qPCR standards relate a known starting quantity (SQ) to a measured
//! quantification cycle (Cq). By convention the curve is fit as
//! `Cq = slope * log10(SQ) + intercept`, and the amplification efficiency is
//! derived from the slope as `efficiency = 10^(-1/slope) - 1` (fractional; a
//! perfect assay has slope ≈ -3.3219 → efficiency ≈ 1.0 = 100%).

/// A single point on the standard curve in the fitted coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StdCurvePoint {
    /// log10 of the starting quantity.
    pub log_sq: f64,
    /// Measured quantification cycle.
    pub cq: f64,
}

/// The result of a least-squares fit over standard points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StdCurveFit {
    /// Slope of `Cq = slope * log10(SQ) + intercept`.
    pub slope: f64,
    /// Intercept (extrapolated Cq at SQ = 1).
    pub intercept: f64,
    /// Coefficient of determination (0..=1).
    pub r_squared: f64,
    /// Fractional amplification efficiency, `10^(-1/slope) - 1`.
    pub efficiency: f64,
    /// Number of points used in the fit.
    pub n: usize,
}

/// Fit a standard curve to `(starting_quantity, cq)` pairs.
///
/// Points with `sq <= 0` (or non-finite) are ignored. Returns `None` if fewer
/// than 2 valid points remain, or if the x values (log10 SQ) are degenerate
/// (zero variance).
pub fn fit(points: &[(f64, f64)]) -> Option<StdCurveFit> {
    let pts: Vec<StdCurvePoint> = points
        .iter()
        .filter(|(sq, cq)| *sq > 0.0 && sq.is_finite() && cq.is_finite())
        .map(|(sq, cq)| StdCurvePoint {
            log_sq: sq.log10(),
            cq: *cq,
        })
        .collect();

    let n = pts.len();
    if n < 2 {
        return None;
    }

    let n_f = n as f64;
    let mean_x = pts.iter().map(|p| p.log_sq).sum::<f64>() / n_f;
    let mean_y = pts.iter().map(|p| p.cq).sum::<f64>() / n_f;

    let mut sxx = 0.0;
    let mut sxy = 0.0;
    let mut syy = 0.0;
    for p in &pts {
        let dx = p.log_sq - mean_x;
        let dy = p.cq - mean_y;
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }

    // Degenerate: no spread in x means we can't determine a slope.
    if sxx <= f64::EPSILON {
        return None;
    }

    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;

    // R² from the ratio of explained to total variance. If y has no variance
    // (all Cq equal) the fit is a flat line through the points → R² = 1.
    let r_squared = if syy <= f64::EPSILON {
        1.0
    } else {
        (sxy * sxy) / (sxx * syy)
    };

    let efficiency = 10f64.powf(-1.0 / slope) - 1.0;

    Some(StdCurveFit {
        slope,
        intercept,
        r_squared,
        efficiency,
        n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_clean_synthetic_curve() {
        // Known line: Cq = -3.5 * log10(SQ) + 40.
        let slope = -3.5;
        let intercept = 40.0;
        let sqs: [f64; 5] = [1.0, 10.0, 100.0, 1000.0, 10000.0];
        let points: Vec<(f64, f64)> = sqs
            .iter()
            .map(|&sq| (sq, slope * sq.log10() + intercept))
            .collect();

        let fit = fit(&points).expect("should fit");
        assert_eq!(fit.n, 5);
        assert!((fit.slope - slope).abs() < 1e-9, "slope = {}", fit.slope);
        assert!(
            (fit.intercept - intercept).abs() < 1e-9,
            "intercept = {}",
            fit.intercept
        );
        assert!(
            (fit.r_squared - 1.0).abs() < 1e-9,
            "r_squared = {}",
            fit.r_squared
        );
        // efficiency = 10^(-1/-3.5) - 1
        let expected_eff = 10f64.powf(-1.0 / slope) - 1.0;
        assert!(
            (fit.efficiency - expected_eff).abs() < 1e-9,
            "efficiency = {}",
            fit.efficiency
        );
    }

    #[test]
    fn perfect_efficiency_from_slope() {
        // slope = -log10(2) * 10 ... actually the canonical 100%-efficiency
        // slope is -3.3219; efficiency should be ~1.0 (100%).
        let slope = -std::f64::consts::LOG2_10;
        let intercept = 35.0;
        let sqs: [f64; 4] = [1.0, 10.0, 100.0, 1000.0];
        let points: Vec<(f64, f64)> = sqs
            .iter()
            .map(|&sq| (sq, slope * sq.log10() + intercept))
            .collect();

        let fit = fit(&points).expect("should fit");
        assert!(
            (fit.efficiency - 1.0).abs() < 1e-5,
            "efficiency = {}",
            fit.efficiency
        );
    }

    #[test]
    fn too_few_points_returns_none() {
        assert!(fit(&[]).is_none());
        assert!(fit(&[(10.0, 20.0)]).is_none());
        // Non-positive SQ values are filtered out, leaving too few points.
        assert!(fit(&[(0.0, 20.0), (-5.0, 21.0), (100.0, 22.0)]).is_none());
    }

    #[test]
    fn degenerate_zero_x_variance_returns_none() {
        // All points share the same SQ → zero x-variance → no slope.
        let points = [(10.0, 20.0), (10.0, 25.0), (10.0, 30.0)];
        assert!(fit(&points).is_none());
    }

    #[test]
    fn noisy_curve_has_reasonable_r_squared() {
        // A near-linear set with small perturbations should still fit well.
        let points = [
            (1.0, 40.05),
            (10.0, 36.48),
            (100.0, 33.02),
            (1000.0, 29.49),
            (10000.0, 25.98),
        ];
        let fit = fit(&points).expect("should fit");
        assert!(fit.r_squared > 0.999, "r_squared = {}", fit.r_squared);
        assert!(fit.slope < 0.0);
    }
}
