//! Progress accounting and speed measurement.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Smoothed transfer rate in bytes per second.
    pub speed_bps: f64,
    /// Segments currently streaming.
    pub active_connections: u32,
    /// Total segments, including finished ones.
    pub segments: u32,
    pub eta: Option<Duration>,
    pub elapsed: Duration,
}

impl ProgressSnapshot {
    pub fn fraction(&self) -> Option<f64> {
        self.total.filter(|t| *t > 0).map(|t| self.downloaded as f64 / t as f64)
    }
}

/// Exponentially-weighted moving average of transfer rate.
///
/// A plain `bytes / elapsed` average is useless in a UI: it barely moves once a
/// download has been running a while, so the number stops reflecting what the
/// connection is actually doing. The EMA responds to change while still being
/// stable enough to read.
pub struct SpeedMeter {
    started: Instant,
    last_at: Instant,
    last_bytes: u64,
    ema: f64,
    alpha: f64,
}

impl SpeedMeter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_at: now,
            last_bytes: 0,
            ema: 0.0,
            alpha: 0.3,
        }
    }

    /// Feed the cumulative byte count. Returns the smoothed rate in bytes/sec.
    pub fn sample(&mut self, total_bytes: u64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).as_secs_f64();

        // Ignore samples taken too close together — dividing by a tiny dt
        // produces wild spikes.
        if dt < 0.05 {
            return self.ema;
        }

        let delta = total_bytes.saturating_sub(self.last_bytes) as f64;
        let instant = delta / dt;

        self.ema = if self.last_bytes == 0 && self.ema == 0.0 {
            instant
        } else {
            self.alpha * instant + (1.0 - self.alpha) * self.ema
        };

        self.last_at = now;
        self.last_bytes = total_bytes;
        self.ema
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn eta(&self, downloaded: u64, total: Option<u64>) -> Option<Duration> {
        let total = total?;
        if self.ema < 1.0 || downloaded >= total {
            return None;
        }
        let remaining = (total - downloaded) as f64;
        Some(Duration::from_secs_f64((remaining / self.ema).min(86_400.0 * 30.0)))
    }
}

impl Default for SpeedMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Human-readable byte count, e.g. `1.43 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn format_speed(bps: f64) -> String {
    format!("{}/s", format_bytes(bps as u64))
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m {s:02}s")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_byte_scales() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KiB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.50 GiB");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 02m 05s");
    }

    #[test]
    fn fraction_is_none_without_total() {
        let p = ProgressSnapshot {
            downloaded: 100,
            total: None,
            speed_bps: 0.0,
            active_connections: 1,
            segments: 1,
            eta: None,
            elapsed: Duration::ZERO,
        };
        assert!(p.fraction().is_none());
    }
}
