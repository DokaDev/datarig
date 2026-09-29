//! Summaries of repeated measurements, and what the operating system says about a process
//! (resident memory, CPU time) and the machine (load average).

use serde_json::{Value, json};
#[cfg(not(target_os = "linux"))]
use std::process::Command;
use std::time::Duration;

/// Median, 95th percentile and range of a set of measurements.
#[derive(Clone, Copy, Debug)]
pub struct Summary {
    pub n: usize,
    pub median: f64,
    pub p95: f64,
    pub min: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(xs: &[f64]) -> Summary {
        if xs.is_empty() {
            return Summary { n: 0, median: 0.0, p95: 0.0, min: 0.0, max: 0.0 };
        }
        let mut v = xs.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len();
        let median = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 };
        // Nearest rank: the smallest value with at least 95% of the values at or below it.
        let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
        Summary { n, median, p95: v[rank - 1], min: v[0], max: v[n - 1] }
    }

    pub fn json(&self) -> Value {
        json!({ "n": self.n, "median": round(self.median), "p95": round(self.p95), "min": round(self.min), "max": round(self.max) })
    }

    /// `median / p95 (min..max, n)` with `unit`.
    pub fn line(&self, unit: &str) -> String {
        format!(
            "{:.3}{unit} / p95 {:.3}{unit} (min {:.3}, max {:.3}, n={})",
            self.median, self.p95, self.min, self.max, self.n
        )
    }
}

fn round(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Resident set size of process `pid` in KiB.
pub fn rss_kb(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
        line.split_whitespace().nth(1)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
}

/// CPU time (user + system) process `pid` has used so far, in seconds.
pub fn cpu_secs(pid: u32) -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The fields after the command name, which is in parentheses and may contain spaces.
        let rest = &stat[stat.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
        // USER_HZ is 100 on every Linux the CI runs on.
        Some(ticks / 100.0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output().ok()?;
        parse_cpu_time(String::from_utf8_lossy(&out.stdout).trim())
    }
}

/// `ps -o time` text: `[[dd-]hh:]mm:ss[.cc]`.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn parse_cpu_time(s: &str) -> Option<f64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, s),
    };
    let mut secs = 0.0;
    for part in rest.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + secs)
}

/// The 1, 5 and 15 minute load averages, as the OS prints them.
pub fn load_avg() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/loadavg")
            .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches(['{', '}', ' ']).to_string())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
