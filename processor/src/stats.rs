//! Статистика по пакету измерений.

use serde::Serialize;

pub const HISTOGRAM_BINS: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub count: usize,
    pub sum: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    /// Выборочное стандартное отклонение (делитель n − 1); для одного значения — 0.
    pub stddev: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Histogram {
    pub min: f64,
    pub max: f64,
    pub counts: Vec<usize>,
}

#[derive(Debug, PartialEq)]
pub enum StatsError {
    Empty,
    NotFinite(usize),
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatsError::Empty => write!(f, "пустой пакет"),
            StatsError::NotFinite(i) => {
                write!(f, "значение values[{i}] не является конечным числом")
            }
        }
    }
}

/// Считает статистику. Сортирует копию данных один раз: O(n log n).
pub fn compute(values: &[f64]) -> Result<(Stats, Histogram), StatsError> {
    if values.is_empty() {
        return Err(StatsError::Empty);
    }
    if let Some(i) = values.iter().position(|v| !v.is_finite()) {
        return Err(StatsError::NotFinite(i));
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();

    // Алгоритм Уэлфорда: устойчив к потере точности на больших суммах.
    let (mut mean, mut m2) = (0.0, 0.0);
    for (i, &x) in values.iter().enumerate() {
        let delta = x - mean;
        mean += delta / (i + 1) as f64;
        m2 += delta * (x - mean);
    }
    let stddev = if n > 1 {
        (m2 / (n - 1) as f64).sqrt()
    } else {
        0.0
    };

    let stats = Stats {
        count: n,
        sum: values.iter().sum(),
        mean,
        min: sorted[0],
        max: sorted[n - 1],
        median: percentile(&sorted, 0.5),
        p95: percentile(&sorted, 0.95),
        p99: percentile(&sorted, 0.99),
        stddev,
    };
    Ok((stats, histogram(&sorted)))
}

/// Перцентиль с линейной интерполяцией (как numpy.percentile по умолчанию).
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Гистограмма из HISTOGRAM_BINS равных интервалов от min до max (max входит в последний).
fn histogram(sorted: &[f64]) -> Histogram {
    let (min, max) = (sorted[0], sorted[sorted.len() - 1]);
    let mut counts = vec![0; HISTOGRAM_BINS];
    if min == max {
        counts[0] = sorted.len();
    } else {
        let width = (max - min) / HISTOGRAM_BINS as f64;
        for &v in sorted {
            let bin = (((v - min) / width) as usize).min(HISTOGRAM_BINS - 1);
            counts[bin] += 1;
        }
    }
    Histogram { min, max, counts }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn basic_stats() {
        let (s, h) = compute(&[4.0, 1.0, 3.0, 2.0, 5.0]).unwrap();
        assert_eq!(s.count, 5);
        assert!(close(s.sum, 15.0) && close(s.mean, 3.0));
        assert!(close(s.min, 1.0) && close(s.max, 5.0) && close(s.median, 3.0));
        // Выборочное σ для 1..5 = sqrt(2.5)
        assert!(close(s.stddev, 2.5f64.sqrt()));
        // numpy.percentile([1..5], 95) = 4.8
        assert!(close(s.p95, 4.8));
        assert_eq!(h.counts.iter().sum::<usize>(), 5);
        assert_eq!(
            h.counts[HISTOGRAM_BINS - 1],
            1,
            "max попадает в последний интервал"
        );
    }

    #[test]
    fn single_value_and_constant() {
        let (s, h) = compute(&[7.0]).unwrap();
        assert_eq!((s.median, s.p99, s.stddev), (7.0, 7.0, 0.0));
        assert_eq!(h.counts[0], 1);

        let (_, h) = compute(&[2.0; 4]).unwrap();
        assert_eq!(h.counts[0], 4);
    }

    #[test]
    fn even_count_median() {
        let (s, _) = compute(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        assert!(close(s.median, 2.5));
    }

    #[test]
    fn welford_is_numerically_stable() {
        // Наивная формула Σx² − n·mean² здесь теряет все значащие цифры.
        let values: Vec<f64> = (0..1000).map(|i| 1e9 + (i % 2) as f64).collect();
        let (s, _) = compute(&values).unwrap();
        assert!((s.stddev - 0.50025).abs() < 1e-4, "stddev = {}", s.stddev);
    }

    #[test]
    fn errors() {
        assert_eq!(compute(&[]).unwrap_err(), StatsError::Empty);
        assert_eq!(
            compute(&[1.0, f64::NAN]).unwrap_err(),
            StatsError::NotFinite(1)
        );
        assert_eq!(
            compute(&[f64::INFINITY]).unwrap_err(),
            StatsError::NotFinite(0)
        );
    }
}
