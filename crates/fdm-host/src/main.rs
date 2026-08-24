//! `fdm-host` — the process Chrome starts to talk to FDM.
//!
//! # Why this exists
//!
//! A Manifest V3 extension cannot download anything itself: `chrome.downloads`
//! is the browser's own downloader, and the blocking `webRequest` API that made
//! true interception possible was removed in MV3. What an extension *can* do is
//! observe a download, cancel it, and hand the URL plus its cookies to a native
//! program over stdio. That program is this one.
//!
//! # Relay first, local second
//!
//! The preferred path is to hand the download to the running desktop app
//! (`fdm-desktop.exe`) over a named pipe (see [`fdm_ipc`]). That way the
//! download appears in the app's window, benefits from a shared queue, and
//! survives this process exiting.
//!
//! When no desktop app is listening — [`fdm_ipc::ClientError::NotRunning`] — the
//! host falls back to downloading in-process, exactly as it did before the IPC
//! layer existed. Someone with Chrome open and FDM closed still expects a click
//! to download something.
//!
//! # Lifecycle
//!
//! * Chrome starts one host process per `runtime.connectNative()` port.
//! * When the port closes — the extension's service worker was evicted, the tab
//!   went away, Chrome quit — stdin reaches EOF. This host does **not** exit
//!   then. It finishes every in-process download it already owns, reporting to
//!   nobody, and exits when the last one lands.
//! * If it is killed anyway (reboot, Task Manager), `fdm-core`'s `.fdm` control
//!   file makes the transfer resumable from the last durably-written byte. The
//!   worst case is a resumed download, never a corrupt one.
//!
//! # The one rule
//!
//! stdout belongs to the protocol. Anything written there that is not a framed
//! message closes the port with the useless error "Error when communicating with
//! the native messaging host". Every diagnostic in this crate goes to stderr,
//! which Chrome captures. `FDM_LOG=debug fdm-host` also works from a terminal
//! for manual poking, though without framed input it will just sit there.

mod framing;
mod lock;
mod protocol;

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use fdm_core::{CancelToken, Category, DownloadRequest, Engine, EngineConfig, HeaderMap};
use tokio::sync::{mpsc, Mutex, Semaphore};
use tokio::task::JoinSet;
use url::Url;

use protocol::{DownloadCommand, Incoming, Outgoing, PROTOCOL_VERSION};

/// How many downloads run at once. Anything beyond this waits for a slot, which
/// is a queue with no queue code.
///
/// Four rather than one because that is what a download manager is for, and not
/// sixteen because each of those four is itself opening up to `max_connections`
/// sockets — sixty-four simultaneous connections to assorted CDNs is how a
/// client gets rate-limited into looking broken.
const MAX_CONCURRENT_DOWNLOADS: usize = 4;

fn main() {
    // stderr, always. See the module comment.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FDM_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("fdm_host=info")),
        )
        .with_writer(io::stderr)
        .init();

    // Chrome appends its own arguments (the calling extension's origin, and on
    // Windows the native window handle). Logged rather than parsed: the
    // manifest's `allowed_origins` is what actually enforces who may connect,
    // and Chrome enforces it before this process starts.
    tracing::debug!(args = ?std::env::args().skip(1).collect::<Vec<_>>(), "started");

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            // No runtime means no framed reply is possible, so stderr is all
            // there is.
            eprintln!("fdm-host: cannot start async runtime: {e}");
            std::process::exit(1);
        }
    };

    runtime.block_on(run());
}

async fn run() {
    let (inbox_tx, mut inbox) = mpsc::channel::<Vec<u8>>(64);
    let outbox = Outbox::spawn();

    // stdin is read on its own OS thread. Chrome's end of the pipe blocks, and
    // tokio's async stdin on Windows is a blocking read on a pool thread anyway,
    // so this is the same cost with none of the ambiguity.
    std::thread::Builder::new()
        .name("fdm-host-stdin".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                match framing::read_message(&mut stdin) {
                    Ok(Some(msg)) => {
                        // blocking_send is correct here: this is a real thread,
                        // and back-pressure is desirable if we ever fall behind.
                        if inbox_tx.blocking_send(msg).is_err() {
                            break; // async side is gone
                        }
                    }
                    Ok(None) => {
                        tracing::debug!("port closed by the browser");
                        break;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "malformed message stream; stopping reader");
                        break;
                    }
                }
            }
            // Dropping inbox_tx closes the channel, which is how `run` learns
            // the port is gone.
        })
        .expect("spawning one thread");

    // The engine is created lazily — only when the desktop app is not running
    // and we need to download in-process.
    let ctx = Arc::new(Context {
        engine: tokio::sync::OnceCell::new(),
        outbox: outbox.clone(),
        slots: Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS)),
        running: Arc::new(Mutex::new(HashMap::new())),
        lock_dir: state_dir().join("locks"),
    });

    let mut downloads = JoinSet::new();

    while let Some(raw) = inbox.recv().await {
        match serde_json::from_slice::<Incoming>(&raw) {
            Ok(msg) => dispatch(msg, &ctx, &mut downloads).await,
            Err(e) => {
                // Recover the id if we can, so the extension fails one row
                // rather than assuming the whole host is broken.
                let id = serde_json::from_slice::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64));
                ctx.outbox
                    .send(Outgoing::error(id, format!("unreadable message: {e}")));
            }
        }
    }

    // The port is closed. Anything still running is now running for nobody, and
    // that is deliberate: see the module comment. Progress messages from here on
    // go into a closed pipe and are dropped by the writer thread.
    if !downloads.is_empty() {
        tracing::info!(
            count = downloads.len(),
            "browser disconnected; finishing downloads already in flight"
        );
        while downloads.join_next().await.is_some() {}
    }
}

struct Context {
    /// Lazily created only when the desktop app is not running and we need to
    /// download in-process.
    engine: tokio::sync::OnceCell<Arc<Engine>>,
    outbox: Outbox,
    slots: Arc<Semaphore>,
    /// Correlation id -> its cancel token, for `cancel` and `status`.
    running: Arc<Mutex<HashMap<u64, CancelToken>>>,
    lock_dir: PathBuf,
}

impl Context {
    /// Get or create the in-process engine.
    async fn engine(&self) -> Result<&Arc<Engine>, String> {
        self.engine
            .get_or_try_init(|| async {
                Engine::new(EngineConfig::default())
                    .map(Arc::new)
                    .map_err(|e| format!("FDM's download engine failed to start: {e}"))
            })
            .await
    }
}

async fn dispatch(msg: Incoming, ctx: &Arc<Context>, downloads: &mut JoinSet<()>) {
    match msg {
        Incoming::Ping { id, protocol } => {
            if let Some(theirs) = protocol {
                if theirs != PROTOCOL_VERSION {
                    ctx.outbox.send(Outgoing::version_mismatch(id, theirs));
                    return;
                }
            }
            handle_ping(id, ctx).await;
        }

        Incoming::Status { id } => {
            let active = ctx.running.lock().await.keys().copied().collect();
            ctx.outbox.send(Outgoing::Status { id, active });
        }

        Incoming::Cancel { id } => {
            match ctx.running.lock().await.get(&id) {
                // The download task notices the token and reports `cancelled`
                // itself, so there is exactly one place that decides a download
                // has ended.
                Some(token) => token.cancel(),
                None => ctx
                    .outbox
                    .send(Outgoing::error(Some(id), "no such download is running")),
            }
        }

        Incoming::Download(cmd) => {
            if let Some(theirs) = cmd.protocol {
                if theirs != PROTOCOL_VERSION {
                    ctx.outbox
                        .send(Outgoing::version_mismatch(Some(cmd.id), theirs));
                    return;
                }
            }
            let ctx = Arc::clone(ctx);
            downloads.spawn(async move { handle_download(*cmd, ctx).await });
        }
    }
}

/// Build a `Pong` from whatever config source we have. Helper to avoid
/// repeating the struct literal in three places.
fn pong_from(id: Option<u64>, download_root: String, max_connections: u32) -> Outgoing {
    Outgoing::Pong {
        id,
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION"),
        host_path: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".into()),
        download_root,
        max_connections,
        categories: Category::ALL.iter().map(|c| c.folder()).collect(),
    }
}

/// Respond to a ping. If the desktop app is running, report its config so the
/// extension describes the process that owns the downloads, not this relay.
async fn handle_ping(id: Option<u64>, ctx: &Arc<Context>) {
    // Try the desktop app first.
    match fdm_ipc::connect().await {
        Ok(client) => {
            let s = client.server();
            ctx.outbox
                .send(pong_from(id, s.download_root.display().to_string(), s.max_connections));
            return;
        }
        Err(fdm_ipc::ClientError::NotRunning) => {
            tracing::debug!("ping: desktop app not running, using local config");
        }
        Err(e) => {
            tracing::warn!(error = %e, "ping: IPC failed, using local config");
        }
    }

    // Fallback to the in-process engine's config.
    match ctx.engine().await {
        Ok(engine) => {
            let cfg = engine.config();
            ctx.outbox.send(pong_from(
                id,
                cfg.download_root.display().to_string(),
                cfg.max_connections,
            ));
        }
        Err(e) => ctx.outbox.send(Outgoing::error(id, e)),
    }
}

async fn handle_download(mut cmd: DownloadCommand, ctx: Arc<Context>) {
    // Resolve video streaming links (YouTube, TikTok, Vimeo, etc.) to authentic direct media streams via yt-dlp
    let (resolved_url, resolved_filename) = resolve_video_page(&cmd.url, cmd.filename.clone()).await;
    cmd.url = resolved_url;
    cmd.filename = resolved_filename;

    let id = cmd.id;

    // --- Try relaying to the running desktop app first ---
    match try_relay(&cmd, &ctx).await {
        RelayResult::Handed => {
            // The desktop app has the download. Nothing more to do here —
            // progress and completion are the desktop app's responsibility now.
            // The extension will not see progress from us, but the download is
            // in the app's list and will complete.
            return;
        }
        RelayResult::NotRunning => {
            tracing::debug!(id, "desktop app not running; downloading in-process");
        }
        RelayResult::Failed(e) => {
            tracing::warn!(id, error = %e, "IPC relay failed; downloading in-process");
        }
    }

    // --- Fallback: download in-process (pre-IPC behaviour) ---

    let url = match validate_url(&cmd.url) {
        Ok(url) => url,
        Err(message) => {
            ctx.outbox.send(Outgoing::Failed {
                id,
                message,
                resumable: false,
            });
            return;
        }
    };

    let engine = match ctx.engine().await {
        Ok(e) => Arc::clone(e),
        Err(e) => {
            ctx.outbox.send(Outgoing::error(Some(id), e));
            return;
        }
    };

    // Claimed before anything else, and held until this function returns.
    let _claim = match lock::acquire(&ctx.lock_dir, url.as_str()) {
        Ok(Some(claim)) => claim,
        Ok(None) => {
            ctx.outbox.send(Outgoing::Failed {
                id,
                message: "FDM is already downloading this URL.".into(),
                resumable: false,
            });
            return;
        }
        Err(e) => {
            // Not fatal to the product, but proceeding would risk two writers on
            // one file, which is precisely what must never happen.
            ctx.outbox.send(Outgoing::Failed {
                id,
                message: format!("could not claim this download: {e}"),
                resumable: true,
            });
            return;
        }
    };

    ctx.outbox.send(Outgoing::Accepted {
        id,
        url: url.to_string(),
    });

    // `expected` is the size the browser thought it was getting. Worth logging
    // because a large disagreement with the engine's own probe is the signature
    // of a token-gated URL that now answers with an error page instead of the
    // file, which is otherwise a baffling bug report.
    tracing::info!(id, url = %url, expected = ?cmd.total_bytes, "taking over download (in-process)");

    // Registered before queueing so a download waiting for a slot can still be
    // cancelled.
    let cancel = CancelToken::new();
    ctx.running.lock().await.insert(id, cancel.clone());

    // Held for the duration of the transfer. Dropping it lets the next queued
    // download start.
    let _slot = match Arc::clone(&ctx.slots).acquire_owned().await {
        Ok(slot) => slot,
        Err(_) => {
            ctx.running.lock().await.remove(&id);
            ctx.outbox.send(Outgoing::Failed {
                id,
                message: "FDM is shutting down.".into(),
                resumable: true,
            });
            return;
        }
    };

    if cancel.is_cancelled() {
        // Cancelled while queued.
        ctx.running.lock().await.remove(&id);
        ctx.outbox.send(Outgoing::Cancelled { id });
        return;
    }

    let mut request = DownloadRequest::new(url).with_headers(browser_headers(&cmd));
    if let Some(name) = cmd.filename.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        request = request.with_filename(name);
    }
    if let Some(dir) = cmd.target_dir.as_deref().filter(|d| !d.is_empty()) {
        request = request.with_target_dir(dir);
    }

    let outbox = ctx.outbox.clone();
    let started = std::time::Instant::now();
    let outcome = engine
        .download(request, cancel.clone(), move |p| {
            outbox.send(Outgoing::Progress {
                id,
                downloaded: p.downloaded,
                total: p.total,
                speed_bps: p.speed_bps,
                eta_seconds: p.eta.map(|d| d.as_secs()),
                segments: p.segments,
                active_connections: p.active_connections,
            });
        })
        .await;

    ctx.running.lock().await.remove(&id);

    match outcome {
        Ok(o) => {
            tracing::info!(path = %o.path.display(), bytes = o.bytes, "completed");
            ctx.outbox.send(Outgoing::Completed {
                id,
                path: o.path.display().to_string(),
                bytes: o.bytes,
                seconds: o.elapsed.as_secs_f64(),
                category: o.category.folder().to_string(),
                segments: o.segments_used,
                resumed: o.resumed,
                used_ranges: o.used_ranges,
            });
        }
        Err(fdm_core::Error::Cancelled) => {
            tracing::info!(id, "cancelled");
            ctx.outbox.send(Outgoing::Cancelled { id });
        }
        Err(e) => {
            tracing::warn!(id, error = %e, elapsed = ?started.elapsed(), "failed");
            ctx.outbox.send(Outgoing::Failed {
                id,
                message: e.to_string(),
                resumable: is_resumable(&e),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// IPC relay
// ---------------------------------------------------------------------------

enum RelayResult {
    /// The desktop app accepted the download.
    Handed,
    /// No desktop app is running; the caller should download in-process.
    NotRunning,
    /// Something went wrong talking to the desktop app.
    Failed(String),
}

/// Try to hand a download to the running desktop app via IPC.
///
/// Returns [`RelayResult::Handed`] only when the desktop app acknowledged the
/// add request. The extension will *not* see progress from this host in that
/// case — the download is now the desktop app's responsibility, and the popup
/// already talks to the app's list.
async fn try_relay(cmd: &DownloadCommand, ctx: &Arc<Context>) -> RelayResult {
    let mut client = match fdm_ipc::connect().await {
        Ok(c) => c,
        Err(fdm_ipc::ClientError::NotRunning) => {
            let mut connected = None;
            if let Ok(current_exe) = std::env::current_exe() {
                if let Some(dir) = current_exe.parent() {
                    let desktop_exe = dir.join("fdm-desktop.exe");
                    if desktop_exe.exists() {
                        if let Ok(_child) = std::process::Command::new(&desktop_exe).spawn() {
                            for _ in 0..15 {
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                if let Ok(c) = fdm_ipc::connect().await {
                                    connected = Some(c);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            match connected {
                Some(c) => c,
                None => return RelayResult::NotRunning,
            }
        }
        Err(e) => return RelayResult::Failed(e.to_string()),
    };

    let add = fdm_ipc::AddRequest {
        url: cmd.url.clone(),
        headers: cmd.headers.clone(),
        filename: cmd.filename.clone(),
        target_dir: cmd.target_dir.as_ref().map(PathBuf::from),
    };

    match client.add(add).await {
        Ok(manager_id) => {
            tracing::info!(
                browser_id = cmd.id,
                manager_id,
                "relayed download to desktop app"
            );
            // Tell the extension the download was accepted, so it stops
            // worrying about the cancelled browser download.
            ctx.outbox.send(Outgoing::Accepted {
                id: cmd.id,
                url: cmd.url.clone(),
            });
            RelayResult::Handed
        }
        Err(e) => RelayResult::Failed(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn resolve_video_page(url: &str, filename: Option<String>) -> (String, Option<String>) {
    let lower = url.to_lowercase();
    let is_video_site = lower.contains("youtube.com/watch")
        || lower.contains("youtube.com/shorts")
        || lower.contains("youtube.com/live")
        || lower.contains("youtu.be/")
        || lower.contains("tiktok.com/")
        || lower.contains("vimeo.com/")
        || lower.contains("instagram.com/p/")
        || lower.contains("instagram.com/reel/")
        || lower.contains("twitter.com/")
        || lower.contains("x.com/");

    if !is_video_site {
        return (url.to_string(), filename);
    }

    let Some(ytdlp_path) = find_tool("yt-dlp.exe") else {
        tracing::debug!("yt-dlp.exe not found; passing raw video link to engine");
        return (url.to_string(), filename);
    };

    let deno = find_tool("deno.exe");

    let mut args = Vec::new();
    if let Some(deno_path) = deno {
        args.push("--js-runtimes".to_string());
        args.push(format!("deno:{}", deno_path.display()));
    }
    args.push("-g".to_string());
    args.push("--get-filename".to_string());
    args.push("-o".to_string());
    args.push("%(title)s.%(ext)s".to_string());
    args.push("-f".to_string());
    args.push("b/best".to_string());
    args.push(url.to_string());

    let url_owned = url.to_string();
    let filename_owned = filename.clone();

    let resolved = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(ytdlp_path);
        cmd.args(&args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<&str> = stdout.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
                if lines.len() >= 2 {
                    let direct_url = lines[0].to_string();
                    let real_filename = lines[1].to_string();
                    return Some((direct_url, Some(real_filename)));
                } else if lines.len() == 1 {
                    let direct_url = lines[0].to_string();
                    return Some((direct_url, filename_owned));
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(%stderr, "yt-dlp stream resolution failed");
            }
        }
        None
    }).await.unwrap_or(None);

    if let Some((direct_url, real_filename)) = resolved {
        tracing::info!(%direct_url, ?real_filename, "resolved direct video stream via yt-dlp");
        return (direct_url, real_filename);
    }

    (url.to_string(), filename)
}

fn find_tool(name: &str) -> Option<PathBuf> {
    // 1. In tools directory relative to the current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let in_tools = dir.join("tools").join(name);
            if in_tools.exists() {
                return Some(in_tools);
            }
            let next_to = dir.join(name);
            if next_to.exists() {
                return Some(next_to);
            }
        }
    }

    // 2. In %LOCALAPPDATA%\FDM\tools
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let in_appdata = PathBuf::from(local_appdata).join("FDM").join("tools").join(name);
        if in_appdata.exists() {
            return Some(in_appdata);
        }
    }

    // 3. In system PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for p in std::env::split_paths(&path_var) {
            let candidate = p.join(name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Whether retrying this download later can plausibly succeed, and therefore
/// whether the UI should offer "resume" or "this cannot be downloaded".
///
/// Distinct from `Error::is_retryable`, which asks a narrower question about
/// retrying one HTTP request immediately. A 404 is not retryable *and* not
/// resumable; an exhausted retry budget is not retryable but very much
/// resumable, because the network may simply have been down.
fn is_resumable(e: &fdm_core::Error) -> bool {
    use fdm_core::Error;
    match e {
        Error::RetriesExhausted(_) | Error::Http(_) | Error::Io(_) => true,
        // The remote file changed or shrank. The bytes on disk are stale, so
        // "resume" is the wrong word for what has to happen next.
        Error::ResourceChanged | Error::RangeNotSatisfiable { .. } => false,
        Error::Status(s) => *s == 408 || *s == 429 || (500..600).contains(s),
        _ => false,
    }
}

fn validate_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("not a valid URL: {e}"))?;

    // The extension filters these too, but the host is the security boundary:
    // it is what runs outside the browser sandbox, and it must not become a
    // general-purpose "open any URL scheme" service for whatever can reach the
    // port. `file:` is the one that matters — it would turn a download request
    // into an arbitrary local file read.
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "FDM only handles http and https downloads, not {other}:"
            ))
        }
    }
    if !url.has_host() {
        return Err("that URL has no host".into());
    }
    Ok(url)
}

/// Turn the extension's header map into a `HeaderMap`.
///
/// The rule itself lives in `fdm_core::headers` because it is an engine
/// constraint, not a browser one — the IPC path in `fdm-ipc` has to apply exactly
/// the same denylist, and `Accept-Encoding` slipping through either route
/// produces the same corrupt file. This function exists only to name the browser
/// as the source.
fn browser_headers(cmd: &DownloadCommand) -> HeaderMap {
    fdm_core::sanitize_headers(&cmd.headers)
}

/// `%LOCALAPPDATA%\FDM`, or the temp directory if the environment is too broken
/// to say. Per-user on purpose — a lock directory shared between accounts would
/// let one user's download block another's.
fn state_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("FDM")
}

/// The only writer to stdout.
///
/// A dedicated thread rather than a mutex around `Stdout`, because a write to a
/// pipe blocks whenever the reader is not draining it, and blocking a tokio
/// worker thread inside a progress callback would stall unrelated downloads.
/// Send is fire-and-forget; a full or closed pipe drops messages rather than
/// propagating an error into the download path, which is right for progress
/// updates and harmless for the rest, since a closed port means nobody is
/// listening to the answer anyway.
#[derive(Clone)]
struct Outbox(mpsc::UnboundedSender<Outgoing>);

impl Outbox {
    fn spawn() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Outgoing>();

        std::thread::Builder::new()
            .name("fdm-host-stdout".into())
            .spawn(move || {
                let mut stdout = io::stdout().lock();
                while let Some(msg) = rx.blocking_recv() {
                    let body = match serde_json::to_vec(&msg) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(error = %e, "could not serialise a reply");
                            continue;
                        }
                    };
                    if let Err(e) = framing::write_message(&mut stdout, &body) {
                        // Almost always the port having closed. Keep draining so
                        // senders never block, but stop trying to write.
                        tracing::debug!(error = %e, "stdout closed; discarding replies");
                        break;
                    }
                }
                // Drain whatever is left so no task blocks on send.
                while rx.blocking_recv().is_some() {}
                let _ = stdout.flush();
            })
            .expect("spawning one thread");

        Self(tx)
    }

    fn send(&self, msg: Outgoing) {
        let _ = self.0.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cmd_with(headers: &[(&str, &str)]) -> DownloadCommand {
        let json = serde_json::json!({
            "type": "download",
            "id": 1,
            "url": "https://example.com/a.bin",
            "headers": headers.iter().copied().collect::<BTreeMap<_, _>>(),
        });
        match serde_json::from_value::<Incoming>(json).unwrap() {
            Incoming::Download(c) => *c,
            _ => unreachable!(),
        }
    }

    #[test]
    fn keeps_the_headers_that_make_authenticated_downloads_work() {
        let map = browser_headers(&cmd_with(&[
            ("Cookie", "session=abc123"),
            ("Referer", "https://example.com/page"),
            ("User-Agent", "Mozilla/5.0"),
        ]));
        assert_eq!(map.get("cookie").unwrap(), "session=abc123");
        assert_eq!(map.get("referer").unwrap(), "https://example.com/page");
        assert_eq!(map.get("user-agent").unwrap(), "Mozilla/5.0");
    }

    #[test]
    fn drops_accept_encoding_whatever_case_it_arrives_in() {
        // The important one: a gzipped body makes every byte range refer to
        // compressed offsets, so segment boundaries land in the wrong place.
        let map = browser_headers(&cmd_with(&[
            ("Accept-Encoding", "gzip, deflate, br"),
            ("accept-encoding", "gzip"),
            ("ACCEPT-ENCODING", "br"),
        ]));
        assert!(map.get("accept-encoding").is_none());
    }

    #[test]
    fn drops_connection_level_headers() {
        let map = browser_headers(&cmd_with(&[
            ("Host", "evil.example"),
            ("Content-Length", "0"),
            ("Range", "bytes=0-10"),
            ("Transfer-Encoding", "chunked"),
            ("Cookie", "keep=me"),
        ]));
        assert_eq!(map.len(), 1, "only Cookie should survive");
        assert!(map.get("cookie").is_some());
    }

    #[test]
    fn one_malformed_cookie_does_not_lose_the_others() {
        let map = browser_headers(&cmd_with(&[
            ("Cookie", "fine=yes"),
            ("X-Bad", "line\nbreak"),
            ("Referer", "https://example.com/"),
        ]));
        assert!(map.get("x-bad").is_none());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn rejects_schemes_that_are_not_downloads() {
        for bad in [
            "file:///C:/Windows/System32/config/SAM",
            "data:text/plain,hello",
            "blob:https://example.com/1234",
            "chrome-extension://abc/page.html",
            "ftp://example.com/x",
            "javascript:alert(1)",
        ] {
            assert!(validate_url(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn accepts_ordinary_download_urls() {
        assert!(validate_url("https://example.com/file.iso").is_ok());
        assert!(validate_url("http://example.com/file.iso?token=x").is_ok());
    }

    #[test]
    fn a_404_is_not_worth_resuming_but_a_dead_network_is() {
        use fdm_core::Error;
        assert!(!is_resumable(&Error::Status(404)));
        assert!(!is_resumable(&Error::ResourceChanged));
        assert!(is_resumable(&Error::Status(503)));
        assert!(is_resumable(&Error::RetriesExhausted("timeout".into())));
    }
}
