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
    /// Значения конечны, но статистика по ним переполняет f64 (например, 1e308 и −1e308).
    Overflow,
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatsError::Empty => write!(f, "пустой пакет"),
            StatsError::NotFinite(i) => {
                write!(f, "значение values[{i}] не является конечным числом")
            }
            StatsError::Overflow => write!(
                f,
                "значения слишком велики по модулю: статистика выходит за пределы f64"
            ),
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

    // Масштаб — наибольшее значение по модулю. Промежуточные величины считаются для
    // x / scale ∈ [−1, 1] и не переполняются, даже когда сами значения близки к 1e308:
    // без этого для [1e300, −1e300] квадрат отклонения (≈ 4e600) давал бы inf,
    // хотя ответ (σ ≈ 1.4e300) представим.
    let scale = sorted[0].abs().max(sorted[n - 1].abs());
    let scale = if scale > 0.0 { scale } else { 1.0 };

    // Алгоритм Уэлфорда: устойчив к потере точности на больших суммах.
    let (mut mean, mut m2) = (0.0, 0.0);
    for (i, &x) in values.iter().enumerate() {
        let x = x / scale;
        let delta = x - mean;
        mean += delta / (i + 1) as f64;
        m2 += delta * (x - mean);
    }
    let stddev = if n > 1 {
        (m2 / (n - 1) as f64).sqrt() * scale
    } else {
        0.0
    };

    let stats = Stats {
        count: n,
        sum: values.iter().sum(),
        mean: mean * scale,
        min: sorted[0],
        max: sorted[n - 1],
        median: percentile(&sorted, 0.5),
        p95: percentile(&sorted, 0.95),
        p99: percentile(&sorted, 0.99),
        stddev,
    };
    // Если результат всё же не помещается в f64 (сумма 1e308 + 1e308), serde_json записал
    // бы inf как null, и отчёт с «пустой» статистикой выглядел бы успешным — это ошибка.
    let all_finite = [stats.sum, stats.mean, stats.stddev]
        .iter()
        .all(|v| v.is_finite());
    if !all_finite {
        return Err(StatsError::Overflow);
    }
    Ok((stats, histogram(&sorted, scale)))
}

/// Перцентиль с линейной интерполяцией (как numpy.percentile по умолчанию).
/// Форма lo·(1−f) + hi·f не переполняется даже для lo = −1e308, hi = 1e308.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    if frac == 0.0 {
        sorted[lo]
    } else {
        sorted[lo] * (1.0 - frac) + sorted[hi] * frac
    }
}

/// Гистограмма из HISTOGRAM_BINS равных интервалов от min до max (max входит в последний).
/// Интервалы считаются в масштабированных значениях: max − min может не поместиться в f64.
fn histogram(sorted: &[f64], scale: f64) -> Histogram {
    let (min, max) = (sorted[0], sorted[sorted.len() - 1]);
    let mut counts = vec![0; HISTOGRAM_BINS];
    if min == max {
        counts[0] = sorted.len();
    } else {
        let (smin, smax) = (min / scale, max / scale);
        let width = (smax - smin) / HISTOGRAM_BINS as f64;
        for &v in sorted {
            let bin = (((v / scale - smin) / width) as usize).min(HISTOGRAM_BINS - 1);
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

    #[test]
    fn huge_values_do_not_overflow_intermediates() {
        // Ответы представимы, хотя квадраты отклонений — нет.
        let (s, h) = compute(&[1e300, -1e300]).unwrap();
        assert_eq!(s.mean, 0.0);
        assert!((s.stddev / 1e300 - std::f64::consts::SQRT_2).abs() < 1e-12);
        assert_eq!(h.counts.iter().sum::<usize>(), 2);

        // Разброс 2e308 не помещается в f64, но статистика — помещается.
        let (s, h) = compute(&[-1e308, 1e308]).unwrap();
        assert_eq!((s.sum, s.mean, s.median), (0.0, 0.0, 0.0));
        assert!(s.stddev.is_finite());
        assert_eq!((h.counts[0], h.counts[HISTOGRAM_BINS - 1]), (1, 1));
    }

    #[test]
    fn unrepresentable_result_is_an_error_not_null() {
        // Каждое значение конечно, но сумма 2e308 в f64 не помещается.
        assert_eq!(compute(&[1e308, 1e308]).unwrap_err(), StatsError::Overflow);
    }
}
