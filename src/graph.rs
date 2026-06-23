//! Helpers for braille trend charts.

/// Map a value series to (x, y) chart points indexed 0..n.
pub fn points(values: &[f64]) -> Vec<(f64, f64)> {
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| (i as f64, v))
        .collect()
}

/// Largest value (>= 0) for a y-axis bound; returns 1.0 for an empty/all-zero series.
pub fn max_y(values: &[f64]) -> f64 {
    let m = values.iter().cloned().fold(0.0_f64, f64::max);
    if m > 0.0 {
        m
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_index() {
        assert_eq!(points(&[2.0, 5.0]), vec![(0.0, 2.0), (1.0, 5.0)]);
    }

    #[test]
    fn max_y_floor() {
        assert_eq!(max_y(&[]), 1.0);
        assert_eq!(max_y(&[0.0, 0.0]), 1.0);
        assert_eq!(max_y(&[1.0, 3.0, 2.0]), 3.0);
    }
}
