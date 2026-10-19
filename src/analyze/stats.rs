//! Lightweight descriptive stats (no heavy deps).

use crate::normalize::metric_value_accepted;

fn all_accepted(values: &[f64]) -> bool {
    values.iter().copied().all(metric_value_accepted)
}

pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() || !all_accepted(values) {
        return None;
    }
    // A finite sample overflows to ±inf, never NaN. Callers that compare
    // against the mean (HRV drop, activity drop-off) still see that inf.
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

pub fn std_dev(values: &[f64]) -> Option<f64> {
    if values.len() < 2 || !all_accepted(values) {
        return None;
    }
    let m = mean(values)?;
    let var = values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (values.len() as f64 - 1.0);
    let s = var.sqrt();
    if s.is_nan() {
        None
    } else {
        Some(s)
    }
}

/// Population-style z-score of `value` against sample `values` (excludes value itself if desired by caller).
///
/// Returns `None` when the sample is empty, any input is non-finite, the
/// standard deviation is below `1e-9` (including zero), or the quotient is
/// itself non-finite (overflow).
pub fn z_score(value: f64, values: &[f64]) -> Option<f64> {
    if !metric_value_accepted(value) || !all_accepted(values) {
        return None;
    }
    let m = mean(values)?;
    let s = std_dev(values)?;
    if !s.is_finite() || s < 1e-9 {
        return None;
    }
    let z = (value - m) / s;
    if z.is_finite() {
        Some(z)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_stats() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((mean(&v).unwrap() - 3.0).abs() < 1e-9);
        assert!(std_dev(&v).unwrap() > 1.0);
        assert!(z_score(5.0, &v).unwrap() > 1.0);
    }

    #[test]
    fn non_finite_inputs_and_overflow_do_not_return_nan() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(mean(&[bad]).is_none());
            assert!(mean(&[1.0, bad, 2.0]).is_none());
            assert!(std_dev(&[1.0, bad]).is_none());
            assert!(std_dev(&[bad, bad, bad]).is_none());
            assert!(z_score(bad, &[1.0, 2.0, 3.0, 4.0]).is_none());
            assert!(z_score(0.0, &[1.0, bad, 3.0, 4.0]).is_none());
        }
        // Identical samples have zero stddev: no z-score, not a NaN score.
        assert!(z_score(0.0, &[60.0; 6]).is_none());
        // Finite overflow (sum or quotient) must not surface as NaN.
        assert!(!mean(&[1e308, 1e308]).is_some_and(|m| m.is_nan()));
        assert!(!std_dev(&[1e308, 1e308]).is_some_and(|s| s.is_nan()));
        assert!(!z_score(0.0, &[1e308; 7]).is_some_and(|z| z.is_nan()));
    }
}
