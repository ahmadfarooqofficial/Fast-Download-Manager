//! Download orchestration.
//!
//! The coordinator owns the segment plan and a fixed pool of connections. When a
//! worker finishes its range, the coordinator hands it either a leftover segment
//! or the back half of whatever segment currently has the most work left. That
//! keeps every connection busy until the very end of the transfer instead of
//! letting one slow server connection define the finish time.
//!
//! Two invariants are load-bearing:
//!
//! 1. `done` only advances after bytes are durably written. A crash therefore
//!    resumes from a byte offset that is genuinely on disk, never ahead of it.
//! 2. A ranged request that comes back anything other than `206` aborts the
//!    whole strategy. Writing a full-resource body at a segment offset yields a
//!    file of exactly the right size that is silently wrong, which is the worst
//!    possible failure for a download manager.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, StreamExt};
use reqwest::header::{HeaderMap, ACCEPT_ENCODING, CONTENT_RANGE, IF_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use url::Url;

use crate::categorize::{self, Category};
use crate::config::EngineConfig;
use crate::error::{Error, Result};
use crate::naming;
use crate::plan::{Plan, Segment};
use crate::probe::{self, RemoteInfo};
use crate::progress::{ProgressSnapshot, SpeedMeter};
use crate::scratch;
use crate::state::DownloadState;
use crate::writer::PositionedFile;

/// Cooperative cancellation. Cheap to clone and share across worker tasks.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: Url,
    /// Headers handed over by the browser extension — cookies, referer,
    /// user-agent. Mandatory for anything behind a login, because the engine
    /// re-issues the request from scratch and would otherwise be an anonymous
    /// client fetching a login page.
    pub headers: HeaderMap,
    /// Override the derived filename.
    pub filename: Option<String>,
    /// Override the destination folder, bypassing type-based organisation.
    pub target_dir: Option<PathBuf>,
}

impl DownloadRequest {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            headers: HeaderMap::new(),
            filename: None,
            target_dir: None,
        }
    }

    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    pub fn with_target_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.target_dir = Some(dir.into());
        self
    }
}

/// What the engine decided, reported once the destination is known and before
/// any bytes move.
///
/// Exists because the destination is only resolvable after the probe — it
/// depends on `Content-Disposition`, the MIME type and the category — yet two
/// callers need it *during* the download rather than after it. A UI has to show
/// a filename in the list from the first frame, the way IDM does, and a caller
/// that cancels has to delete the scratch files, which it cannot name otherwise.
#[derive(Debug, Clone)]
pub struct StartInfo {
    /// Where the finished file will land. Already de-duplicated, so this is the
    /// real path, not a candidate.
    pub target: PathBuf,
    /// The `.part` file that receives the bytes.
    pub part: PathBuf,
    /// The `.fdm` control file that makes the download resumable.
    pub control: PathBuf,
    pub total: Option<u64>,
    /// True when an existing control file was adopted instead of starting over.
    pub resumed: bool,
    pub used_ranges: bool,
    pub category: Category,
}

#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    pub path: PathBuf,
    pub bytes: u64,
    pub elapsed: Duration,
    /// Total segments the plan ended up with, including ones created by
    /// mid-transfer splits.
    pub segments_used: u32,
    pub category: Category,
    pub resumed: bool,
    pub used_ranges: bool,
}

pub struct Engine {
    client: Client,
    cfg: EngineConfig,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Result<Self> {
        let client = Client::builder()
            .user_agent(cfg.user_agent.clone())
            .connect_timeout(cfg.connect_timeout)
            // The idle pool has to be at least as large as the connection count,
            // or finished segments tear down sockets that the next segment then
            // pays to re-establish. Reusing them is exactly what IDM means by
            // avoiding "additional connect and login stages".
            .pool_max_idle_per_host(cfg.max_connections as usize + 2)
            .pool_idle_timeout(Duration::from_secs(90))
            // Latency matters more than packet efficiency for many small writes.
            .tcp_nodelay(true)
            .build()?;

        Ok(Self { client, cfg })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    /// Probe, then download. Falls back to a single sequential stream if the
    /// server turns out not to honour ranges.
    pub async fn download<F>(
        &self,
        req: DownloadRequest,
        cancel: CancelToken,
        on_progress: F,
    ) -> Result<DownloadOutcome>
    where
        F: FnMut(ProgressSnapshot),
    {
        self.download_observed(req, cancel, on_progress, |_| {}).await
    }

    /// [`Engine::download`], plus a notification carrying the resolved
    /// destination. See [`StartInfo`] for why that is a separate callback rather
    /// than a field on [`ProgressSnapshot`].
    ///
    /// `on_start` fires once per attempt, so it fires a second time if a server
    /// reneges on `Range` support and the download restarts sequentially. Treat
    /// it as "this is the current destination", not as an event count.
    pub async fn download_observed<F, S>(
        &self,
        req: DownloadRequest,
        cancel: CancelToken,
        mut on_progress: F,
        mut on_start: S,
    ) -> Result<DownloadOutcome>
    where
        F: FnMut(ProgressSnapshot),
        S: FnMut(&StartInfo),
    {
        let info = probe::probe(&self.client, &req.url, &req.headers).await?;

        tracing::info!(
            url = %req.url,
            size = ?info.total_size,
            ranges = info.supports_ranges,
            "probed"
        );

        let parallel = info.can_segment();
        match self
            .run(&req, &info, parallel, &cancel, &mut on_progress, &mut on_start)
            .await
        {
            Err(Error::RangeLost { status }) => {
                tracing::warn!(
                    status,
                    "server did not honour Range; discarding partial data and restarting sequentially"
                );
                self.run(&req, &info, false, &cancel, &mut on_progress, &mut on_start)
                    .await
            }
            other => other,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run<F, S>(
        &self,
        req: &DownloadRequest,
        info: &RemoteInfo,
        use_ranges: bool,
        cancel: &CancelToken,
        on_progress: &mut F,
        on_start: &mut S,
    ) -> Result<DownloadOutcome>
    where
        F: FnMut(ProgressSnapshot),
        S: FnMut(&StartInfo),
    {
        let filename = req
            .filename
            .as_deref()
            .map(naming::sanitize_filename)
            .unwrap_or_else(|| info.filename.clone());

        // First-pass classification from name and MIME. Refined by magic bytes
        // after the download completes, if it landed in `Other`.
        let category = categorize::classify(&filename, info.mime.as_deref(), &[]);

        let dir = self.destination_dir(req, category);
        std::fs::create_dir_all(&dir)?;

        let target = naming::unique_path(&dir, &filename);
        let part_path = if self.cfg.use_temp_dir {
            // Partial data goes to the temp directory, so the download folder only
            // ever holds finished files. `scratch::part_path` derives the name from
            // the URL and the intended destination rather than from `target`,
            // because `unique_path` can append " (2)" between attempts and that
            // would orphan the partial data on resume.
            std::fs::create_dir_all(&self.cfg.temp_dir)?;
            scratch::part_path(&self.cfg.temp_dir, req.url.as_str(), &dir, &filename)
        } else {
            let mut s = target.as_os_str().to_owned();
            s.push(".part");
            PathBuf::from(s)
        };
        let control_path = naming::control_path(&part_path);

        // ---- decide fresh vs resume -------------------------------------------------
        let mut resumed = false;
        let plan = if use_ranges {
            let existing = DownloadState::load(&control_path).filter(|s| {
                s.is_resumable_for(req.url.as_str(), info) && part_path.exists()
            });

            match existing {
                Some(state) => {
                    resumed = true;
                    tracing::info!(
                        done = state.bytes_done(),
                        segments = state.segments.len(),
                        "resuming"
                    );
                    Plan::from_snapshots(&state.segments, info.total_size)
                }
                None => {
                    discard_partial(&part_path, &control_path);
                    Plan::split_even(
                        info.total_size.unwrap_or(0),
                        self.cfg.max_connections,
                        self.cfg.min_split_size,
                    )
                }
            }
        } else {
            // Without range support there is nothing to resume from.
            discard_partial(&part_path, &control_path);
            Plan::single(info.total_size)
        };

        let plan = Arc::new(plan);
        let file = Arc::new(PositionedFile::create(
            &naming::to_extended_path(&part_path),
            info.total_size,
        )?);

        let state = DownloadState::new(req.url.as_str(), info, &target, plan.snapshots());
        state.save(&control_path)?;

        // Announce the destination before the first byte. Deliberately after the
        // resume decision, so `resumed` is a fact rather than a guess, and after
        // the control file exists, so a caller that cancels immediately still
        // finds both scratch files where it was told they would be.
        on_start(&StartInfo {
            target: target.clone(),
            part: part_path.clone(),
            control: control_path.clone(),
            total: info.total_size,
            resumed,
            used_ranges: use_ranges,
            category,
        });

        // ---- run the segment pool ---------------------------------------------------
        let mut meter = SpeedMeter::new();
        let started = Instant::now();

        on_progress(ProgressSnapshot {
            downloaded: plan.total_done(),
            total: plan.total(),
            speed_bps: 0.0,
            active_connections: 0,
            segments: plan.count(),
            eta: None,
            elapsed: Duration::ZERO,
        });

        // Internal stop flag. Distinct from the caller's token so a fatal error
        // in one segment can wind the others down without mutating state the
        // caller owns.
        let stop = CancelToken::new();
        let effective_max = if use_ranges { self.cfg.max_connections.max(1) } else { 1 };

        let (tx, mut rx) = mpsc::channel::<Result<u32>>(64);
        let mut in_flight: u32 = 0;

        let client = self.client.clone();
        let cfg = self.cfg.clone();
        let url = info.final_url.clone();
        let headers = req.headers.clone();
        let validator = if use_ranges { info.validator.clone() } else { None };

        macro_rules! spawn_segment {
            ($seg:expr) => {{
                let seg: Arc<Segment> = $seg;
                let tx = tx.clone();
                let client = client.clone();
                let url = url.clone();
                let headers = headers.clone();
                let validator = validator.clone();
                let file = Arc::clone(&file);
                let cfg = cfg.clone();
                let stop = stop.clone();
                tokio::spawn(async move {
                    let index = seg.index;
                    // A panicking worker still has to report in, or the
                    // coordinator's drain loop would wait on a message that is
                    // never sent and hang the download forever.
                    let outcome = std::panic::AssertUnwindSafe(run_segment(
                        &client, &url, &headers, &seg, &file,
                        validator.as_deref(), &cfg, &stop, use_ranges,
                    ))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(Error::other(format!("segment {index} worker panicked")))
                    });
                    seg.deactivate();
                    let _ = tx.send(outcome.map(|()| index)).await;
                });
                in_flight += 1;
            }};
        }

        while in_flight < effective_max {
            match plan.claim_idle() {
                Some(seg) => spawn_segment!(seg),
                None => break,
            }
        }

        let mut ticker = tokio::time::interval(self.cfg.progress_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut last_checkpoint = Instant::now();
        let mut fatal: Option<Error> = None;

        while in_flight > 0 {
            tokio::select! {
                biased;

                received = rx.recv() => {
                    let Some(outcome) = received else { break };
                    in_flight -= 1;

                    if let Err(err) = outcome {
                        fatal = Some(err);
                        stop.cancel();
                        break;
                    }

                    // Top the pool back up: leftover work first, otherwise carve
                    // the back half off whichever segment is furthest behind.
                    while in_flight < effective_max {
                        let next = plan.claim_idle().or_else(|| {
                            plan.split_largest(cfg.min_split_size)
                                .filter(|s| s.try_activate())
                        });
                        match next {
                            Some(seg) => spawn_segment!(seg),
                            None => break,
                        }
                    }
                }

                _ = ticker.tick() => {
                    if cancel.is_cancelled() {
                        fatal = Some(Error::Cancelled);
                        stop.cancel();
                        break;
                    }

                    let downloaded = plan.total_done();
                    let speed = meter.sample(downloaded);
                    on_progress(ProgressSnapshot {
                        downloaded,
                        total: plan.total(),
                        speed_bps: speed,
                        active_connections: plan.active_count(),
                        segments: plan.count(),
                        eta: meter.eta(downloaded, plan.total()),
                        elapsed: started.elapsed(),
                    });

                    if last_checkpoint.elapsed() >= Duration::from_secs(1) {
                        let state = DownloadState::new(
                            req.url.as_str(), info, &target, plan.snapshots(),
                        );
                        if let Err(e) = state.save(&control_path) {
                            tracing::warn!(error = %e, "failed to checkpoint resume state");
                        }
                        last_checkpoint = Instant::now();
                    }
                }
            }
        }

        // Let any still-running workers observe `stop` and finish flushing before
        // we touch the file. Renaming underneath a live writer would lose data.
        while in_flight > 0 {
            match rx.recv().await {
                Some(_) => in_flight -= 1,
                None => break,
            }
        }

        // Persist whatever progress was made, so even a failed attempt resumes.
        let state = DownloadState::new(req.url.as_str(), info, &target, plan.snapshots());
        let _ = state.save(&control_path);

        if let Some(err) = fatal {
            return Err(err);
        }

        // Defensive: `run_segment` only reports success once its range is fully
        // fetched, so this should be unreachable. Finalising an incomplete file
        // would hand the user silent corruption, so check anyway.
        if !plan.is_complete() {
            return Err(Error::other(format!(
                "workers finished with {} of {:?} bytes written",
                plan.total_done(),
                plan.total()
            )));
        }

        self.finalize(
            FinalizeArgs {
                file,
                plan: &plan,
                part_path: &part_path,
                control_path: &control_path,
                filename: &filename,
                dir: &dir,
                category,
                explicit_dir: req.target_dir.is_some(),
            },
            resumed,
            use_ranges,
            started,
        )
    }

    fn destination_dir(&self, req: &DownloadRequest, category: Category) -> PathBuf {
        if let Some(dir) = &req.target_dir {
            return dir.clone();
        }
        if self.cfg.organize_by_type {
            self.cfg.download_root.join(category.folder())
        } else {
            self.cfg.download_root.clone()
        }
    }

    fn finalize(
        &self,
        args: FinalizeArgs<'_>,
        resumed: bool,
        used_ranges: bool,
        started: Instant,
    ) -> Result<DownloadOutcome> {
        let FinalizeArgs {
            file,
            plan,
            part_path,
            control_path,
            filename,
            dir,
            category,
            explicit_dir,
        } = args;

        file.sync()?;

        let bytes = match plan.total() {
            Some(total) => total,
            None => {
                // Size was unknown, so preallocation couldn't happen and the file
                // is exactly as long as what arrived.
                let written = plan.total_done();
                file.truncate_to(written)?;
                written
            }
        };

        // Sniff the signature while the handle is still open. Only used to
        // rescue files that name and MIME failed to classify.
        let mut head = [0u8; 16];
        let head_len = file.read_at(&mut head, 0).unwrap_or(0);

        let mut final_category = category;
        if category == Category::Other {
            if let Some(sniffed) = categorize::from_magic(&head[..head_len]) {
                tracing::debug!(?sniffed, "recovered category from file signature");
                final_category = sniffed;
            }
        }

        // Windows will not rename a file that still has an open handle without
        // FILE_SHARE_DELETE, which Rust does not request. All workers have
        // exited, so this drops the last reference.
        drop(file);

        let final_dir = if !explicit_dir && self.cfg.organize_by_type && final_category != category {
            let better = self.cfg.download_root.join(final_category.folder());
            std::fs::create_dir_all(&better)?;
            better
        } else {
            dir.to_path_buf()
        };

        let final_target = naming::unique_path(&final_dir, filename);
        // Not `rename`: the temp directory is usually on a different volume from
        // the download folder, and `rename` cannot cross one.
        scratch::move_into_place(part_path, &final_target)?;
        DownloadState::delete(control_path);

        tracing::info!(path = %final_target.display(), bytes, "download complete");

        Ok(DownloadOutcome {
            path: final_target,
            bytes,
            elapsed: started.elapsed(),
            segments_used: plan.count(),
            category: final_category,
            resumed,
            used_ranges,
        })
    }
}

struct FinalizeArgs<'a> {
    file: Arc<PositionedFile>,
    plan: &'a Arc<Plan>,
    part_path: &'a Path,
    control_path: &'a Path,
    filename: &'a str,
    dir: &'a Path,
    category: Category,
    explicit_dir: bool,
}

fn discard_partial(part_path: &Path, control_path: &Path) {
    let _ = std::fs::remove_file(part_path);
    DownloadState::delete(control_path);
}

/// Fetch one segment, retrying transient failures from wherever it got to.
#[allow(clippy::too_many_arguments)]
async fn run_segment(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    seg: &Arc<Segment>,
    file: &Arc<PositionedFile>,
    validator: Option<&str>,
    cfg: &EngineConfig,
    stop: &CancelToken,
    use_ranges: bool,
) -> Result<()> {
    let mut attempt = 0u32;

    loop {
        if stop.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if seg.is_complete() {
            return Ok(());
        }

        let outcome =
            stream_segment(client, url, headers, seg, file, validator, cfg, stop, use_ranges).await;

        match outcome {
            Ok(()) if seg.is_complete() => return Ok(()),
            Ok(()) => {
                // The body ended before the requested range was satisfied.
                // Usually a dropped connection; retry from the new cursor.
                attempt += 1;
                if attempt > cfg.max_retries {
                    return Err(Error::RetriesExhausted(format!(
                        "segment {} stalled at offset {} with {} bytes left",
                        seg.index,
                        seg.cursor(),
                        seg.remaining()
                    )));
                }
                tracing::debug!(segment = seg.index, attempt, "body ended early, retrying");
                backoff(attempt).await;
            }
            Err(err) if err.is_retryable() && attempt < cfg.max_retries => {
                attempt += 1;
                tracing::debug!(segment = seg.index, attempt, error = %err, "retrying segment");
                if !use_ranges {
                    // No ranges means no resume: the only correct retry is from
                    // byte zero.
                    seg.set_done(0);
                }
                backoff(attempt).await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Exponential backoff, capped so a long outage doesn't stall for minutes.
async fn backoff(attempt: u32) {
    let millis = 200u64.saturating_mul(1u64 << attempt.min(6));
    tokio::time::sleep(Duration::from_millis(millis.min(10_000))).await;
}

/// One HTTP request covering the segment's remaining range.
#[allow(clippy::too_many_arguments)]
async fn stream_segment(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    seg: &Arc<Segment>,
    file: &Arc<PositionedFile>,
    validator: Option<&str>,
    cfg: &EngineConfig,
    stop: &CancelToken,
    use_ranges: bool,
) -> Result<()> {
    let start_at = seg.cursor();
    let seg_end = seg.end();
    if start_at > seg_end {
        return Ok(());
    }

    let mut builder = client
        .get(url.clone())
        .headers(headers.clone())
        // Any content coding would decouple wire bytes from file offsets and
        // make the whole segment scheme meaningless.
        .header(ACCEPT_ENCODING, "identity");

    if use_ranges {
        builder = builder.header(RANGE, format!("bytes={start_at}-{seg_end}"));
        if let Some(validator) = validator {
            // If the file changed since we started, the server answers 200 with
            // the whole resource instead of 206, which we detect below.
            builder = builder.header(IF_RANGE, validator);
        }
    } else if start_at > 0 {
        return Err(Error::other(
            "cannot resume mid-file without server range support",
        ));
    }

    let response = builder.send().await?;
    let status = response.status();

    if use_ranges {
        match status {
            StatusCode::PARTIAL_CONTENT => {
                if let Some(header) = response
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                {
                    if let Some(actual) = probe::parse_content_range_start(header) {
                        if actual != start_at {
                            return Err(Error::other(format!(
                                "server returned range starting at {actual}, asked for {start_at}"
                            )));
                        }
                    }
                }
            }
            // The dangerous case: a full body that must never be written at an
            // offset. Surfaces to the caller, which restarts sequentially.
            StatusCode::OK => return Err(Error::RangeLost { status: 200 }),
            StatusCode::RANGE_NOT_SATISFIABLE => {
                return Err(Error::RangeNotSatisfiable { offset: start_at })
            }
            s if s.is_success() => return Err(Error::RangeLost { status: s.as_u16() }),
            s => return Err(Error::Status(s.as_u16())),
        }
    } else if !status.is_success() {
        return Err(Error::Status(status.as_u16()));
    }

    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(cfg.write_buffer);
    let mut buf_start = start_at;
    let mut pos = start_at;

    loop {
        if stop.is_cancelled() {
            flush(file, &mut buf, buf_start, seg).await?;
            return Err(Error::Cancelled);
        }

        let next = tokio::time::timeout(cfg.read_timeout, stream.next()).await;

        let chunk = match next {
            Err(_elapsed) => {
                // Keep what we have — the retry resumes from the new cursor.
                flush(file, &mut buf, buf_start, seg).await?;
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no data received within read timeout",
                )));
            }
            Ok(None) => break,
            Ok(Some(Err(err))) => {
                flush(file, &mut buf, buf_start, seg).await?;
                return Err(Error::Http(err));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };

        // Re-read the end each iteration: the coordinator may have split this
        // segment and handed our tail to another connection.
        let end = seg.end();
        if pos > end {
            break;
        }

        let room = end.saturating_sub(pos).saturating_add(1);
        let take = (chunk.len() as u64).min(room) as usize;
        buf.extend_from_slice(&chunk[..take]);
        pos += take as u64;

        if buf.len() >= cfg.write_buffer {
            buf_start += flush(file, &mut buf, buf_start, seg).await?;
        }

        if take < chunk.len() {
            // Range satisfied; the rest of this chunk belongs to another segment.
            break;
        }
    }

    flush(file, &mut buf, buf_start, seg).await?;
    Ok(())
}

/// Write the buffer at `at` and only then advance the segment cursor.
///
/// The ordering is the resume guarantee: `done` must never describe bytes that
/// aren't on disk, or a resumed download will skip a hole and produce a corrupt
/// file that passes a size check.
async fn flush(
    file: &Arc<PositionedFile>,
    buf: &mut Vec<u8>,
    at: u64,
    seg: &Arc<Segment>,
) -> Result<u64> {
    if buf.is_empty() {
        return Ok(0);
    }

    // Hand the bytes off but leave an equally-sized buffer behind. `mem::take`
    // would leave a zero-capacity Vec, so every subsequent MiB would be rebuilt
    // through ~20 doubling reallocations — per segment, for the whole transfer.
    let cap = buf.capacity();
    let data = std::mem::replace(buf, Vec::with_capacity(cap));
    let len = data.len() as u64;
    let handle = Arc::clone(file);

    // Blocking positional write moved off the async runtime.
    tokio::task::spawn_blocking(move || handle.write_all_at(&data, at))
        .await
        .map_err(|e| Error::other(format!("write task failed: {e}")))??;

    seg.add_done(len);
    Ok(len)
}
