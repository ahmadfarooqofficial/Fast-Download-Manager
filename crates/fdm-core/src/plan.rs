//! Segment planning and dynamic splitting.
//!
//! This module is where the speed comes from. A naive downloader cuts the file
//! into N fixed pieces up front and waits; when one server connection is slower
//! than the rest you get the classic "stuck at 99% on one chunk" tail, and the
//! download takes as long as its worst segment.
//!
//! IDM's documented fix is to keep splitting during the transfer: as a
//! connection frees up, find the segment with the most work left and cut it in
//! half, handing the back half to the idle connection. We do the same, so a slow
//! segment gets progressively taken apart instead of holding up the download.
//!
//! Segments are shared between the coordinator and the workers, so their mutable
//! fields are atomics: a worker re-reads `end` after every chunk and stops early
//! once the coordinator has shrunk it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// One contiguous byte range of the target file.
///
/// `start` is fixed for the lifetime of the segment. `end` is inclusive and can
/// only ever move *down*, when the coordinator splits the segment.
#[derive(Debug)]
pub struct Segment {
    pub index: u32,
    pub start: u64,
    end: AtomicU64,
    done: AtomicU64,
    active: AtomicBool,
}

impl Segment {
    pub fn new(index: u32, start: u64, end: u64) -> Self {
        Self {
            index,
            start,
            end: AtomicU64::new(end),
            done: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }

    pub fn end(&self) -> u64 {
        self.end.load(Ordering::Acquire)
    }

    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Acquire)
    }

    pub fn set_done(&self, value: u64) {
        self.done.store(value, Ordering::Release);
    }

    pub fn add_done(&self, delta: u64) -> u64 {
        self.done.fetch_add(delta, Ordering::AcqRel) + delta
    }

    /// Absolute offset the next byte should be written to.
    pub fn cursor(&self) -> u64 {
        self.start.saturating_add(self.done())
    }

    /// Declared length of this segment right now, in bytes.
    pub fn len(&self) -> u64 {
        self.end().saturating_sub(self.start).saturating_add(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes still to fetch. Zero once the cursor has passed `end`, which also
    /// covers the case where a split moved `end` behind an already-advanced
    /// cursor.
    pub fn remaining(&self) -> u64 {
        let cursor = self.cursor();
        let end = self.end();
        if cursor > end {
            0
        } else {
            end - cursor + 1
        }
    }

    pub fn is_complete(&self) -> bool {
        self.remaining() == 0
    }

    /// Progress this segment contributes, clamped to its current length.
    ///
    /// After a split the original worker may have already written past its new
    /// `end`; those bytes now belong to the new segment, which will rewrite them
    /// with identical data. Clamping here prevents counting them twice.
    pub fn effective_done(&self) -> u64 {
        self.done().min(self.len())
    }

    fn shrink_end(&self, new_end: u64) {
        self.end.store(new_end, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Claim this segment for a worker. Returns `false` if another worker
    /// already has it.
    pub fn try_activate(&self) -> bool {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn snapshot(&self) -> SegmentSnapshot {
        SegmentSnapshot {
            index: self.index,
            start: self.start,
            end: self.end(),
            done: self.done(),
        }
    }
}

/// Serialisable segment state, persisted to the `.fdm` control file so a
/// download survives a crash, a reboot, or the user quitting mid-transfer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SegmentSnapshot {
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub done: u64,
}

pub struct Plan {
    segments: RwLock<Vec<Arc<Segment>>>,
    next_index: AtomicU32,
    /// `None` when the server never told us the size (no `Content-Length`),
    /// in which case there is exactly one open-ended segment.
    total: Option<u64>,
}

impl Plan {
    /// Cut `[0, total)` into `n` roughly equal segments.
    ///
    /// `n` is reduced when the file is too small to be worth splitting that many
    /// ways — spinning up 16 connections for a 2 MiB file costs more in
    /// handshakes than it saves.
    pub fn split_even(total: u64, n: u32, min_split: u64) -> Self {
        if total == 0 {
            return Self {
                segments: RwLock::new(Vec::new()),
                next_index: AtomicU32::new(0),
                total: Some(0),
            };
        }

        let max_useful = (total / min_split.max(1)).max(1);
        let n = (n as u64).clamp(1, max_useful) as u32;

        let base = total / n as u64;
        let remainder = total % n as u64;

        let mut segments = Vec::with_capacity(n as usize);
        let mut start = 0u64;
        for i in 0..n {
            // Spread the remainder over the first segments so no segment is
            // short by more than a byte.
            let len = base + if (i as u64) < remainder { 1 } else { 0 };
            let end = start + len - 1;
            segments.push(Arc::new(Segment::new(i, start, end)));
            start = end + 1;
        }

        Self {
            segments: RwLock::new(segments),
            next_index: AtomicU32::new(n),
            total: Some(total),
        }
    }

    /// A single segment covering the whole resource. Used when the server
    /// doesn't support `Range`, or when the size is unknown.
    pub fn single(total: Option<u64>) -> Self {
        let end = total.map(|t| t.saturating_sub(1)).unwrap_or(u64::MAX);
        Self {
            segments: RwLock::new(vec![Arc::new(Segment::new(0, 0, end))]),
            next_index: AtomicU32::new(1),
            total,
        }
    }

    /// Rebuild a plan from persisted state to resume a download.
    pub fn from_snapshots(snapshots: &[SegmentSnapshot], total: Option<u64>) -> Self {
        let mut max_index = 0u32;
        let segments: Vec<Arc<Segment>> = snapshots
            .iter()
            .map(|s| {
                max_index = max_index.max(s.index);
                let seg = Segment::new(s.index, s.start, s.end);
                seg.set_done(s.done);
                Arc::new(seg)
            })
            .collect();

        Self {
            segments: RwLock::new(segments),
            next_index: AtomicU32::new(max_index + 1),
            total,
        }
    }

    pub fn total(&self) -> Option<u64> {
        self.total
    }

    pub fn segments(&self) -> Vec<Arc<Segment>> {
        self.segments.read().expect("segment lock poisoned").clone()
    }

    pub fn count(&self) -> u32 {
        self.segments.read().expect("segment lock poisoned").len() as u32
    }

    pub fn active_count(&self) -> u32 {
        self.segments
            .read()
            .expect("segment lock poisoned")
            .iter()
            .filter(|s| s.is_active())
            .count() as u32
    }

    pub fn snapshots(&self) -> Vec<SegmentSnapshot> {
        self.segments
            .read()
            .expect("segment lock poisoned")
            .iter()
            .map(|s| s.snapshot())
            .collect()
    }

    /// Total bytes fetched across all segments.
    pub fn total_done(&self) -> u64 {
        self.segments
            .read()
            .expect("segment lock poisoned")
            .iter()
            .map(|s| s.effective_done())
            .sum()
    }

    pub fn is_complete(&self) -> bool {
        self.segments
            .read()
            .expect("segment lock poisoned")
            .iter()
            .all(|s| s.is_complete())
    }

    /// An incomplete segment nobody is working on. Used on resume, where the
    /// persisted plan already has gaps to fill.
    pub fn claim_idle(&self) -> Option<Arc<Segment>> {
        self.segments
            .read()
            .expect("segment lock poisoned")
            .iter()
            .find(|s| !s.is_complete() && !s.is_active() && s.try_activate())
            .cloned()
    }

    /// Take the segment with the most work remaining and cut it in half,
    /// returning the newly created back half ready to be activated.
    ///
    /// Returns `None` when no segment is big enough to be worth splitting —
    /// splitting below `min_split` trades useful transfer for connection setup.
    pub fn split_largest(&self, min_split: u64) -> Option<Arc<Segment>> {
        let mut segments = self.segments.write().expect("segment lock poisoned");

        let victim = segments
            .iter()
            .filter(|s| s.remaining() >= min_split.saturating_mul(2))
            .max_by_key(|s| s.remaining())?
            .clone();

        // Snapshot under the write lock so the split point can't drift.
        let cursor = victim.cursor();
        let end = victim.end();
        let remaining = end.checked_sub(cursor)?.checked_add(1)?;

        let mid = cursor + remaining / 2;

        // Both halves must stay above the floor, and the split must actually
        // move the boundary.
        if mid <= cursor || mid > end || end - mid + 1 < min_split {
            return None;
        }

        victim.shrink_end(mid - 1);

        let index = self.next_index.fetch_add(1, Ordering::AcqRel);
        let fresh = Arc::new(Segment::new(index, mid, end));
        segments.push(Arc::clone(&fresh));

        tracing::debug!(
            victim = victim.index,
            new = index,
            at = mid,
            "split segment"
        );

        Some(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 1024;

    #[test]
    fn even_split_covers_every_byte_with_no_overlap() {
        let plan = Plan::split_even(1_000_003, 7, MIN);
        let segs = plan.segments();
        assert_eq!(segs.len(), 7);

        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end(), 1_000_002);

        for pair in segs.windows(2) {
            // Contiguous: each segment starts exactly one byte after the last ends.
            assert_eq!(pair[1].start, pair[0].end() + 1);
        }
        let covered: u64 = segs.iter().map(|s| s.len()).sum();
        assert_eq!(covered, 1_000_003);
    }

    #[test]
    fn small_file_is_not_split_into_uselessly_tiny_pieces() {
        // 3 KiB with a 1 KiB floor can support at most 3 segments, not 16.
        let plan = Plan::split_even(3 * 1024, 16, MIN);
        assert_eq!(plan.count(), 3);
    }

    #[test]
    fn tiny_file_gets_exactly_one_segment() {
        let plan = Plan::split_even(500, 16, MIN);
        assert_eq!(plan.count(), 1);
        assert_eq!(plan.segments()[0].len(), 500);
    }

    #[test]
    fn splitting_largest_halves_the_remaining_work() {
        let plan = Plan::split_even(100_000, 1, MIN);
        let original = plan.segments()[0].clone();
        original.set_done(20_000); // cursor at 20_000, 80_000 left

        let fresh = plan.split_largest(MIN).expect("should split");

        assert_eq!(plan.count(), 2);
        assert_eq!(fresh.start, 60_000);
        assert_eq!(fresh.end(), 99_999);
        assert_eq!(original.end(), 59_999);
        // No byte is claimed by both halves.
        assert_eq!(original.end() + 1, fresh.start);
    }

    #[test]
    fn refuses_to_split_below_the_floor() {
        let plan = Plan::split_even(1500, 1, MIN);
        // 1500 bytes remaining is under 2 * 1024, so a split would create a
        // sub-floor piece.
        assert!(plan.split_largest(MIN).is_none());
        assert_eq!(plan.count(), 1);
    }

    #[test]
    fn a_split_never_moves_the_boundary_behind_the_cursor() {
        // The invariant that makes `split_largest` safe: it divides the work
        // that is *left*, never the segment as a whole. Halving the whole
        // segment would hand the new connection bytes already on disk, and
        // could put `end` behind a cursor that has passed the midpoint.
        for done in [0u64, 1, 40_000, 70_000, 90_000, 98_975] {
            let plan = Plan::split_even(100_000, 1, MIN);
            let original = plan.segments()[0].clone();
            original.set_done(done);

            let before = original.cursor();
            if let Some(fresh) = plan.split_largest(MIN) {
                assert!(
                    original.end() >= before,
                    "done={done}: split moved end to {} behind cursor {before}",
                    original.end()
                );
                assert_eq!(original.end() + 1, fresh.start, "done={done}: gap or overlap");
                assert!(
                    original.done() <= original.len(),
                    "done={done}: a split alone must never make done exceed len"
                );
            }
        }
    }

    #[test]
    fn progress_is_not_double_counted_when_a_worker_overruns_a_shrunk_segment() {
        // The one case where `done` can exceed `len`, and the reason
        // `effective_done` clamps. `shrink_end` runs under the coordinator's
        // write lock, but a worker holding the same segment is concurrently
        // writing a chunk it had already pulled off the socket. Its bytes now
        // belong to the new segment, which will fetch them again.
        let plan = Plan::split_even(100_000, 1, MIN);
        let original = plan.segments()[0].clone();
        original.set_done(90_000);

        let fresh = plan.split_largest(MIN).expect("should split");
        assert_eq!(original.end(), 94_999);
        assert_eq!(fresh.start, 95_000);

        // Worker lands a 8 KB chunk it had already read, overrunning the new end.
        original.add_done(8_000);
        assert!(original.done() > original.len());

        assert_eq!(original.effective_done(), original.len());
        assert_eq!(fresh.done(), 0, "the new half re-fetches its range");
        assert_eq!(plan.total_done(), original.len());
        assert!(
            plan.total_done() <= 100_000,
            "progress must never exceed the file size"
        );
    }

    #[test]
    fn segment_is_complete_when_cursor_passes_end() {
        let seg = Segment::new(0, 0, 999);
        assert!(!seg.is_complete());
        seg.set_done(1000);
        assert!(seg.is_complete());
        assert_eq!(seg.remaining(), 0);
    }

    #[test]
    fn an_overrun_segment_reports_complete_not_underflowed() {
        let plan = Plan::split_even(100_000, 1, MIN);
        let original = plan.segments()[0].clone();
        original.set_done(90_000);
        let _ = plan.split_largest(MIN).expect("should split");

        // Same race as above: the worker's in-flight chunk carries the cursor
        // past the end the coordinator just set. `remaining` must saturate at
        // zero rather than wrap around to 18 quintillion.
        original.add_done(8_000);
        assert!(original.cursor() > original.end());
        assert_eq!(original.remaining(), 0);
        assert!(original.is_complete());
    }

    #[test]
    fn resume_round_trips_through_snapshots() {
        let plan = Plan::split_even(50_000, 4, MIN);
        plan.segments()[1].set_done(1234);
        let snaps = plan.snapshots();

        let resumed = Plan::from_snapshots(&snaps, Some(50_000));
        assert_eq!(resumed.count(), 4);
        assert_eq!(resumed.segments()[1].done(), 1234);
        assert_eq!(resumed.total_done(), 1234);
    }

    #[test]
    fn activation_is_exclusive() {
        let seg = Segment::new(0, 0, 99);
        assert!(seg.try_activate());
        assert!(!seg.try_activate(), "second claim must fail");
        seg.deactivate();
        assert!(seg.try_activate());
    }

    #[test]
    fn unknown_size_yields_one_open_ended_segment() {
        let plan = Plan::single(None);
        assert_eq!(plan.count(), 1);
        assert_eq!(plan.segments()[0].end(), u64::MAX);
        assert!(plan.total().is_none());
    }
}
