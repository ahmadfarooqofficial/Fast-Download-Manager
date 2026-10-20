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
///
/// "Stable enough to read" is the part that needs care. The coordinator calls
/// [`SpeedMeter::sample`] on every progress tick, which is tens of times a
/// second, and sixty-odd segment workers each land their bytes at their own
/// moments. A rate taken over a few dozen milliseconds is therefore mostly a
/// measure of which workers happened to finish a read inside that slice, and
/// averaging such rates with a fixed weight still jumps around faster than a
/// person can follow (#12). So the meter measures over windows of at least
/// [`SpeedMeter::WINDOW`], and the weight of each new measurement depends on
/// how much time it covers, so the average forgets the past over a steady
/// [`SpeedMeter::TIME_CONSTANT`] however often it is asked.
pub struct SpeedMeter {
    started: Instant,
    last_at: Instant,
    last_bytes: u64,
    ema: f64,
    /// Whether `ema` holds a measurement yet. Separate from `last_bytes == 0`
    /// so a resumed download, whose first sample already carries gigabytes,
    /// is not mistaken for a fresh one on every tick.
    primed: bool,
}

impl SpeedMeter {
    /// The shortest stretch a rate is measured over.
    pub const WINDOW: Duration = Duration::from_millis(250);
    /// How long the average takes to mostly forget an old rate.
    pub const TIME_CONSTANT: Duration = Duration::from_millis(2000);

    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_at: now,
            last_bytes: 0,
            ema: 0.0,
            primed: false,
        }
    }

    /// Feed the cumulative byte count. Returns the smoothed rate in bytes/sec.
    pub fn sample(&mut self, total_bytes: u64) -> f64 {
        self.sample_at(total_bytes, Instant::now())
    }

    fn sample_at(&mut self, total_bytes: u64, now: Instant) -> f64 {
        let dt = now.duration_since(self.last_at).as_secs_f64();

        // Too soon to say anything new. Dividing by a tiny dt produces wild
        // spikes, and even a clean division over a few milliseconds measures
        // worker timing rather than throughput.
        if dt < Self::WINDOW.as_secs_f64() {
            return self.ema;
        }

        let delta = total_bytes.saturating_sub(self.last_bytes) as f64;
        let instant = delta / dt;

        if !self.primed {
            // Nothing to average with yet. The first window in which bytes
            // actually moved is the first estimate; windows before that, such as
            // the connection setup, keep the reading at zero rather than
            // dragging the average down from a start it never had.
            if delta > 0.0 {
                self.primed = true;
                self.ema = instant;
            }
        } else {
            // Weight by the time this measurement covers, so a long gap counts
            // for more than a short one and the forgetting rate is a property
            // of the clock, not of how often the coordinator ticks.
            let alpha = 1.0 - (-dt / Self::TIME_CONSTANT.as_secs_f64()).exp();
            self.ema = alpha * instant + (1.0 - alpha) * self.ema;
        }

        self.last_at = now;
        self.last_bytes = total_bytes;
        self.ema
    }

    /// Start measuring from a byte count that is already on disk, so a resumed
    /// download's first window measures what it transferred, not what it found.
    pub fn starting_at(total_bytes: u64) -> Self {
        let mut meter = Self::new();
        meter.last_bytes = total_bytes;
        meter
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

    /// What the popup shows: a rate that reads as an average, not as whichever
    /// workers happened to land bytes in the last few milliseconds.
    #[test]
    fn the_rate_is_measured_over_a_window_not_per_tick() {
        let start = Instant::now();
        let mut m = SpeedMeter::new();
        m.last_at = start;
        m.started = start;

        // Ticks inside the window change nothing, however lumpy the bytes.
        for (bytes, ms) in [(1_000_000, 16), (2_000_000, 100), (4_000_000, 240)] {
            assert_eq!(m.sample_at(bytes, start + Duration::from_millis(ms)), 0.0);
        }

        // The first full window is the first estimate: 5 MB over 0.5 s.
        let first = m.sample_at(5_000_000, start + Duration::from_millis(500));
        assert!((first - 10_000_000.0).abs() < 1.0, "{first}");

        // A steady stream converges on the steady rate...
        let mut rate = first;
        for i in 1..=20 {
            let at = start + Duration::from_millis(500 + 250 * i);
            rate = m.sample_at(5_000_000 + 2_500_000 * i, at); // 10 MB/s
        }
        assert!((rate - 10_000_000.0).abs() < 1.0, "{rate}");

        // ...and one burst moves it, but not to the burst.
        let bumped = m.sample_at(
            5_000_000 + 2_500_000 * 20 + 25_000_000,
            start + Duration::from_millis(5_750),
        );
        assert!(bumped > 10_000_000.0, "{bumped}");
        assert!(
            bumped < 30_000_000.0,
            "a single 100 MB/s window must not become the reading: {bumped}"
        );
    }

    /// A resumed download has gigabytes on disk before its first byte moves.
    /// Those must not read as an instant transfer.
    #[test]
    fn a_resumed_download_measures_only_what_it_transferred() {
        let start = Instant::now();
        let mut m = SpeedMeter::starting_at(3_000_000_000);
        m.last_at = start;
        m.started = start;

        let rate = m.sample_at(
            3_000_000_000 + 5_000_000,
            start + Duration::from_millis(500),
        );
        assert!((rate - 10_000_000.0).abs() < 1.0, "{rate}");
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
