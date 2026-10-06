//! Axes: the range a chart's values are drawn in and the ticks along it, with their labels.
//!
//! Numbers step by 1, 2 or 5 times a power of ten; a logarithmic axis by powers of ten. Times
//! step by whole seconds, minutes, hours, days, weeks, months or years, labelled with as much
//! of the date and time as the step needs.

use super::TimeKind;
use super::time::{self, DAY};

/// An axis: from `lo` to `hi`, ticks at `values`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ticks {
    pub lo: f64,
    pub hi: f64,
    /// Between two ticks (0 on a logarithmic axis).
    pub step: f64,
    pub values: Vec<f64>,
    pub log: bool,
}

impl Ticks {
    /// Where `v` is between `lo` (0) and `hi` (1); `None` for a value a logarithmic axis cannot
    /// place (zero or less).
    pub fn at(&self, v: f64) -> Option<f64> {
        if self.log {
            if v <= 0.0 {
                return None;
            }
            let (a, b) = (self.lo.log10(), self.hi.log10());
            return Some((v.log10() - a) / (b - a));
        }
        Some((v - self.lo) / (self.hi - self.lo))
    }

    /// The labels of the ticks, in one format for the whole axis.
    pub fn labels(&self) -> Vec<String> {
        let f = Format::of(self);
        self.values.iter().map(|&v| f.fmt(v)).collect()
    }
}

/// The step `x` rounded up to 1, 2 or 5 times a power of ten.
pub fn nice(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 || !x.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(x.log10().floor());
    let f = x / p;
    let n = if f <= 1.0 + 1e-9 {
        1.0
    } else if f <= 2.0 + 1e-9 {
        2.0
    } else if f <= 5.0 + 1e-9 {
        5.0
    } else {
        10.0
    };
    n * p
}

/// A linear axis over `min..max` with at most `max_ticks` ticks (at least 2), starting and
/// ending on a tick; with `zero` it includes 0 (bars grow from it).
pub fn linear(min: f64, max: f64, max_ticks: usize, zero: bool) -> Ticks {
    let (mut min, mut max) = if min <= max { (min, max) } else { (max, min) };
    if zero {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    if max.partial_cmp(&min) != Some(std::cmp::Ordering::Greater) {
        // One value: a range around it.
        let d = if min == 0.0 { 1.0 } else { min.abs() * 0.1 };
        (min, max) = if zero && min >= 0.0 { (0.0, min + d) } else { (min - d, max + d) };
        if zero && max <= 0.0 && min < 0.0 {
            max = 0.0;
        }
    }
    let n = max_ticks.max(2);
    let mut step = nice((max - min) / (n - 1) as f64);
    loop {
        let (a, b) = ((min / step).floor(), (max / step).ceil());
        if b - a < n as f64 || step > (max - min) * 4.0 {
            let values = (a as i64..=b as i64).map(|k| k as f64 * step).collect();
            return Ticks { lo: a * step, hi: b * step, step, values, log: false };
        }
        step = nice(step * 1.5);
    }
}

/// A logarithmic axis over the positive `min..max`: from the power of ten at or below `min`
/// to the one at or above `max`, a tick on each power (every other one, … when there are more
/// than `max_ticks`).
pub fn log(min: f64, max: f64, max_ticks: usize) -> Ticks {
    let min = if min > 0.0 { min } else { 1.0 };
    let max = max.max(min);
    let a = min.log10().floor() as i32;
    let mut b = max.log10().ceil() as i32;
    if b <= a {
        b = a + 1;
    }
    let every = ((b - a) as usize).div_ceil(max_ticks.max(2) - 1).max(1) as i32;
    let values = (a..=b).filter(|e| (e - a) % every == 0).map(|e| 10f64.powi(e)).collect();
    Ticks { lo: 10f64.powi(a), hi: 10f64.powi(b), step: 0.0, values, log: true }
}

/// How an axis writes its numbers: a unit (`k`, `M`, `G`, `T`) for large ones and as many
/// decimals as the step needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    log: bool,
    unit: f64,
    suffix: &'static str,
    decimals: usize,
}

impl Format {
    pub fn of(t: &Ticks) -> Format {
        let big = t.lo.abs().max(t.hi.abs());
        let (unit, suffix) = match big {
            b if b >= 1e12 => (1e12, "T"),
            b if b >= 1e9 => (1e9, "G"),
            b if b >= 1e6 => (1e6, "M"),
            b if b >= 1e4 => (1e3, "k"),
            _ => (1.0, ""),
        };
        let decimals = if t.log { 0 } else { (-(t.step / unit).log10().floor()).max(0.0) as usize };
        Format { log: t.log, unit, suffix, decimals: decimals.min(9) }
    }

    pub fn fmt(&self, v: f64) -> String {
        if self.log {
            return log_label(v);
        }
        if v == 0.0 && !self.suffix.is_empty() {
            return "0".into();
        }
        let v = v / self.unit;
        let s = format!("{:.*}", self.decimals, v);
        // `-0` reads as a mistake.
        let s = if s.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { s.replace('-', "") } else { s };
        format!("{s}{}", self.suffix)
    }
}

/// A power of ten on a logarithmic axis: `0.01`, `1`, `100`, `1k`, `10M`.
fn log_label(v: f64) -> String {
    match v {
        v if v >= 1e12 => format!("{}T", plain(v / 1e12)),
        v if v >= 1e9 => format!("{}G", plain(v / 1e9)),
        v if v >= 1e6 => format!("{}M", plain(v / 1e6)),
        v if v >= 1e3 => format!("{}k", plain(v / 1e3)),
        v => plain(v),
    }
}

/// A value as a number to read or paste: whole numbers in full, others with up to 12
/// significant digits (sums of decimals lose their binary noise).
pub fn plain(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let digits = 12 - (v.abs().log10().floor() as i32 + 1);
    let s = format!("{:.*}", digits.clamp(0, 17) as usize, v);
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// A value as a short number for a bar's end: three significant digits with a unit.
pub fn short(v: f64) -> String {
    let a = v.abs();
    let (unit, suffix) = match a {
        a if a >= 1e12 => (1e12, "T"),
        a if a >= 1e9 => (1e9, "G"),
        a if a >= 1e6 => (1e6, "M"),
        a if a >= 1e4 => (1e3, "k"),
        _ => (1.0, ""),
    };
    let x = v / unit;
    let decimals = match x.abs() {
        x if x >= 100.0 || (x.fract() == 0.0 && suffix.is_empty()) => 0,
        x if x >= 10.0 => 1,
        _ => 2,
    };
    let s = format!("{x:.decimals$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    format!("{s}{suffix}")
}

/// Steps of a time axis that count seconds.
const STEPS: &[f64] = &[
    1.0,
    2.0,
    5.0,
    10.0,
    15.0,
    30.0,
    60.0,
    120.0,
    300.0,
    600.0,
    900.0,
    1800.0,
    3600.0,
    7200.0,
    10800.0,
    21600.0,
    43200.0,
    DAY,
    2.0 * DAY,
    7.0 * DAY,
    14.0 * DAY,
];

/// A time axis over `min..max` (seconds, as [`time::parse`] gives them) with at most
/// `max_ticks` ticks: their places and labels.
pub fn times(min: f64, max: f64, max_ticks: usize, kind: TimeKind) -> Vec<(f64, String)> {
    let n = max_ticks.max(2) as f64;
    let span = (max - min).max(1.0);
    if let Some(&step) = STEPS.iter().find(|&&s| span / s <= n - 1.0) {
        // Weeks start on Mondays (1970-01-01 was a Thursday).
        let monday = if step >= 7.0 * DAY { 4.0 * DAY } else { 0.0 };
        let first = ((min - monday) / step).ceil() * step + monday;
        let days = kind != TimeKind::Time && max - min >= DAY;
        let mut out = Vec::new();
        let mut k = 0.0;
        while first + k * step <= max + 1e-6 {
            let v = first + k * step;
            out.push((v, time_label(v, step, days, kind)));
            k += 1.0;
        }
        return out;
    }
    // Months and years: on the first of a month.
    let ((y0, m0, _), _) = time::parts(min);
    let ((y1, m1, _), _) = time::parts(max);
    let months = (y1 - y0) * 12 + i64::from(m1) - i64::from(m0) + 1;
    let every = [1i64, 2, 3, 6, 12, 24, 60, 120, 240, 600, 1200, 6000]
        .into_iter()
        .find(|&e| months / e < n as i64)
        .unwrap_or(12_000);
    let mut out = Vec::new();
    let mut i = (y0 * 12 + i64::from(m0) - 1).div_euclid(every) * every;
    loop {
        let (y, m) = (i.div_euclid(12), (i.rem_euclid(12) + 1) as u32);
        let v = time::days_from_civil(y, m, 1) as f64 * DAY;
        if v > max + 1e-6 {
            break;
        }
        if v >= min - 1e-6 {
            out.push((v, if every >= 12 { format!("{y}") } else { format!("{y}-{m:02}") }));
        }
        i += every;
    }
    out
}

/// The label of a tick at `v` on a time axis stepping by `step` seconds: the date, the time
/// or both (`days`: the axis spans days).
fn time_label(v: f64, step: f64, days: bool, kind: TimeKind) -> String {
    let ((y, mo, d), (h, mi, s)) = time::parts(v);
    let clock = if step < 60.0 { format!("{h:02}:{mi:02}:{s:02}") } else { format!("{h:02}:{mi:02}") };
    match kind {
        TimeKind::Time => clock,
        _ if step >= DAY => format!("{y}-{mo:02}-{d:02}"),
        _ if days => format!("{mo:02}-{d:02} {clock}"),
        _ => clock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps_are_one_two_or_five_times_a_power_of_ten() {
        assert_eq!(nice(0.7), 1.0);
        assert_eq!(nice(1.3), 2.0);
        assert_eq!(nice(3.0), 5.0);
        assert_eq!(nice(7.0), 10.0);
        assert_eq!(nice(1234.0), 2000.0);
        assert!((nice(0.023) - 0.05).abs() < 1e-12);
        assert_eq!(nice(0.0), 1.0);
        assert_eq!(nice(f64::NAN), 1.0);
    }

    #[test]
    fn a_linear_axis_covers_the_values_with_round_ticks() {
        let t = linear(3.0, 97.0, 6, true);
        assert_eq!((t.lo, t.hi, t.step), (0.0, 100.0, 20.0));
        assert_eq!(t.labels(), ["0", "20", "40", "60", "80", "100"]);
        let t = linear(-12.0, 30.0, 5, true);
        assert!(t.lo <= -12.0 && t.hi >= 30.0 && t.values.len() <= 5, "{t:?}");
        assert!(t.values.contains(&0.0));
        // Lines need no zero.
        let t = linear(1000.0, 1010.0, 6, false);
        assert_eq!((t.lo, t.hi), (1000.0, 1010.0));
        // One value: a range around it.
        let t = linear(5.0, 5.0, 5, true);
        assert!(t.lo == 0.0 && t.hi > 5.0);
        let t = linear(-5.0, -5.0, 5, true);
        assert!(t.lo < -5.0 && t.hi == 0.0, "{t:?}");
        let t = linear(0.0, 0.0, 5, false);
        assert!(t.lo < 0.0 && t.hi > 0.0);
        assert_eq!(t.at(t.lo), Some(0.0));
        assert_eq!(t.at(t.hi), Some(1.0));
    }

    #[test]
    fn large_and_small_numbers_get_units_and_decimals() {
        let t = linear(0.0, 2_400_000.0, 5, true);
        assert_eq!(t.labels(), ["0", "1M", "2M", "3M"]);
        let t = linear(0.0, 48_000.0, 6, true);
        assert_eq!(t.labels(), ["0", "10k", "20k", "30k", "40k", "50k"]);
        let t = linear(0.0, 25_000_000.0, 6, true);
        assert_eq!(t.labels(), ["0", "5M", "10M", "15M", "20M", "25M"]);
        let t = linear(0.0, 0.5, 6, true);
        assert_eq!(t.labels(), ["0.0", "0.1", "0.2", "0.3", "0.4", "0.5"]);
        let t = linear(-1.0, 1.0, 3, false);
        assert_eq!(t.labels(), ["-1", "0", "1"]);
        assert_eq!(plain(1234.0), "1234");
        assert_eq!(plain(0.1 + 0.2), "0.3");
        assert_eq!(plain(-2.5), "-2.5");
        assert_eq!(plain(9_007_199_254_740_993.0), "9007199254740992");
        assert_eq!(short(1234.0), "1234");
        assert_eq!(short(12_345.0), "12.3k");
        assert_eq!(short(2_500_000.0), "2.5M");
        assert_eq!(short(0.126), "0.13");
        assert_eq!(short(-42.0), "-42");
    }

    #[test]
    fn a_log_axis_ticks_on_powers_of_ten() {
        let t = log(3.0, 45_000.0, 10);
        assert_eq!((t.lo, t.hi), (1.0, 100_000.0));
        assert_eq!(t.labels(), ["1", "10", "100", "1k", "10k", "100k"]);
        assert_eq!(t.at(1000.0).map(|p| (p * 5.0).round()), Some(3.0));
        assert_eq!(t.at(0.0), None);
        assert_eq!(t.at(-3.0), None);
        let t = log(1.0, 1e12, 4);
        assert!(t.values.len() <= 4, "{t:?}");
        let t = log(0.02, 0.5, 6);
        assert_eq!(t.labels(), ["0.01", "0.1", "1"]);
    }

    #[test]
    fn time_axes_step_by_calendar_units_with_fitting_labels() {
        let day = |d: &str| time::parse(d).unwrap().secs;
        let ticks = times(day("2026-10-01"), day("2026-10-31"), 6, TimeKind::Date);
        assert_eq!(
            ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            ["2026-10-05", "2026-10-12", "2026-10-19", "2026-10-26"],
            "weeks from Monday"
        );
        let ticks = times(day("2026-10-07 09:00"), day("2026-10-07 17:00"), 5, TimeKind::DateTime);
        assert_eq!(ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(), ["10:00", "12:00", "14:00", "16:00"]);
        let ticks = times(day("2026-10-06 18:00"), day("2026-10-08 06:00"), 4, TimeKind::DateTime);
        assert_eq!(
            ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            ["10-07 00:00", "10-07 12:00", "10-08 00:00"]
        );
        let ticks = times(day("2024-01-15"), day("2026-10-01"), 4, TimeKind::Date);
        assert_eq!(ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(), ["2025", "2026"]);
        let ticks = times(day("2026-01-01"), day("2026-12-31"), 5, TimeKind::Date);
        assert_eq!(
            ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            ["2026-01", "2026-04", "2026-07", "2026-10"]
        );
        let ticks = times(0.0, 50.0, 6, TimeKind::Time);
        assert_eq!(
            ticks.iter().map(|t| t.1.as_str()).collect::<Vec<_>>(),
            ["00:00:00", "00:00:10", "00:00:20", "00:00:30", "00:00:40", "00:00:50"]
        );
        assert!(ticks.len() <= 6);
    }
}
