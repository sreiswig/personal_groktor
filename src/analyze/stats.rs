//! Lightweight descriptive stats (no heavy deps).

pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

pub fn std_dev(values: &[f64]) -> Option<f64> {
    if values.len() < 2 {
        return None;
    }
    let m = mean(values)?;
    let var = values.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (values.len() as f64 - 1.0);
    Some(var.sqrt())
}

/// Population-style z-score of `value` against sample `values` (excludes value itself if desired by caller).
pub fn z_score(value: f64, values: &[f64]) -> Option<f64> {
    let m = mean(values)?;
    let s = std_dev(values)?;
    if s < 1e-9 {
        return None;
    }
    Some((value - m) / s)
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
}
