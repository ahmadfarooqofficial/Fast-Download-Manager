//! The download list: one owner for every download in the app.
//!
//! `fdm-core` downloads one file when asked. IDM's window is a *list* — rows that
//! queue, pause, resume, fail, retry and survive a restart. That bookkeeping is
//! what this module adds, and it is deliberately a plain Rust crate with no Tauri
//! and no IPC in it, so all of it is testable without building a GUI.
//!
//! Four rules hold the design together:
//!
//! 1. **Exactly one place decides a download has ended.** The spawned task
//!    writes the terminal status; `pause`/`cancel`/`remove` only record an
//!    *intent* and trip the cancel token. Two writers would race to decide
//!    whether a stopped download was paused or failed.
//! 2. **A cancelled engine download keeps its scratch files.** `fdm-core` does
//!    not call `discard_partial` on the cancelled path, so pause costs nothing;
//!    deleting the `.part` and `.fdm` is this module's choice, made only for a
//!    real cancel.
//! 3. **No `await` while the registry lock is held.** The lock is a
//!    `std::sync::Mutex` and every critical section is a few field writes.
//! 4. **Only the current attempt may write to a row.** Cancelling is
//!    asynchronous — `pause` returns long before the engine has stopped — so a
//!    resume issued straight afterwards creates a second task while the first is
//!    still winding down. Each attempt carries a generation, checks it before
//!    every write, and waits on a per-download lock so two attempts never share a
//!    `.part` file. See [`Registry::new_generation`].
//!
//! ## What is not persisted, and why
//!
//! Browser headers — cookies, `Authorization`, `Referer` — are held in memory for
//! the life of the process and never written to `downloads.json`. Any process
//! running as the user could read that file, and a stolen session cookie is worth
//! more than a resumed download. The cost is that resuming a login-protected
//! download *after an app restart* may come back 403; the user re-clicks in the
//! browser and the headers arrive again. Resuming within a session is unaffected.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fdm_core::{
    CancelToken, DownloadRequest, Engine, EngineConfig, HeaderMap, ProgressSnapshot, StartInfo,
};
use tokio::sync::{broadcast, Mutex as AsyncMutex, Semaphore};

use crate::error::{ManagerError, Result};
use crate::model::{now_secs, DownloadEntry, DownloadId, Event, NewDownload, Status};
use crate::store::Store;

/// Simultaneous downloads. IDM defaults to a small number for the same reason we
/// do: eight files sharing a pipe finish no sooner than four, and the connection
/// count *within* a download is where the speed comes from.
pub const DEFAULT_MAX_ACTIVE: usize = 4;

/// Why a running download was asked to stop.
///
/// `Err(Error::Cancelled)` from the engine cannot say whether the user hit pause
/// or delete, and the two demand opposite treatment of the `.part` file. So the
/// caller records the reason before tripping the token, and the task reads it
/// back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    Pause,
    Cancel,
    Remove { delete_file: bool },
}

/// Per-download state that is not shown in the UI and not written to disk.
///
/// One `Runtime` per download, reused across attempts rather than replaced: the
/// run lock has to outlive the attempt that took it, and the scratch paths have to
/// outlive the attempt that learned them, so that a cancel arriving while the
/// previous task is still stopping still knows what to delete.
struct Runtime {
    headers: HeaderMap,
    target_dir: Option<PathBuf>,
    cancel: CancelToken,
    intent: Option<Intent>,
    /// Which attempt owns this download. Bumped by every `add` and `resume`; a
    /// task whose generation no longer matches is a ghost and must write nothing.
    generation: u64,
    /// Held for the whole of an attempt, so the next attempt starts only once the
    /// previous one has actually let go of the `.part` file.
    run: Arc<AsyncMutex<()>>,
    /// Learned from [`StartInfo`] once the engine resolves the destination.
    /// `None` means the download never got past the probe, so there is nothing
    /// on disk to clean up.
    part: Option<PathBuf>,
    control: Option<PathBuf>,
}

impl Runtime {
    fn new(headers: HeaderMap, target_dir: Option<PathBuf>) -> Self {
        Self {
            headers,
            target_dir,
            cancel: CancelToken::new(),
            intent: None,
            // No task ever runs under generation 0, so a restored row cannot be
            // mistaken for a live attempt.
            generation: 0,
            run: Arc::new(AsyncMutex::new(())),
            part: None,
            control: None,
        }
    }
}

struct Registry {
    next_id: DownloadId,
    /// Monotonic across the whole list. Per-download would do, but one counter is
    /// one thing to reason about and 2^64 attempts is not a limit anyone reaches.
    next_gen: u64,
    /// Display order, oldest first. A `HashMap` alone would shuffle the list on
    /// every render.
    order: Vec<DownloadId>,
    entries: HashMap<DownloadId, DownloadEntry>,
    runtime: HashMap<DownloadId, Runtime>,
}

impl Registry {
    fn snapshot(&self, id: DownloadId) -> Option<DownloadEntry> {
        self.entries.get(&id).cloned()
    }

    fn ordered(&self) -> Vec<DownloadEntry> {
        self.order
            .iter()
            .filter_map(|id| self.entries.get(id).cloned())
            .collect()
    }

    fn forget(&mut self, id: DownloadId) {
        self.entries.remove(&id);
        self.runtime.remove(&id);
        self.order.retain(|x| *x != id);
    }

    /// Hand the download to a new attempt and return that attempt's generation.
    ///
    /// The old token is latched, so a fresh one goes in — otherwise the new
    /// attempt would cancel itself the first time it looked. The intent is cleared
    /// for the same reason: it described a stop the user has since taken back.
    fn new_generation(&mut self, id: DownloadId) -> u64 {
        self.next_gen += 1;
        let gen = self.next_gen;
        if let Some(rt) = self.runtime.get_mut(&id) {
            rt.generation = gen;
            rt.intent = None;
            rt.cancel = CancelToken::new();
        }
        gen
    }

    /// True while `generation` is still the attempt that owns this download.
    fn is_current(&self, id: DownloadId, generation: u64) -> bool {
        self.runtime.get(&id).is_some_and(|r| r.generation == generation)
    }
}

pub struct Manager {
    engine: Arc<Engine>,
    reg: Arc<Mutex<Registry>>,
    slots: Arc<Semaphore>,
    /// Mirrors the semaphore's permit count so `max_active` can be reported
    /// without `Semaphore` having to expose its configured total.
    max_active: AtomicUsize,
    events: broadcast::Sender<Event>,
    store: Arc<Store>,
}

impl Manager {
    /// Build a manager over an existing engine, restoring whatever the store
    /// holds. Rows that were mid-flight come back `Paused` — see [`Store::load`].
    pub fn new(engine: Engine, store: Store, max_active: usize) -> Self {
        let max_active = max_active.max(1);
        let (next_id, entries) = store.load();

        let order = entries.iter().map(|e| e.id).collect::<Vec<_>>();
        let runtime = entries
            .iter()
            .filter(|e| !e.status.is_finished())
            .map(|e| {
                // Restored rows have no headers: they were never saved. The
                // scratch paths are recoverable from the destination, which is
                // why `resume` reconstructs the request from `entry.path`.
                (e.id, Runtime::new(HeaderMap::new(), None))
            })
            .collect();

        let (events, _) = broadcast::channel(512);

        Self {
            engine: Arc::new(engine),
            reg: Arc::new(Mutex::new(Registry {
                next_id,
                next_gen: 0,
                order,
                entries: entries.into_iter().map(|e| (e.id, e)).collect(),
                runtime,
            })),
            slots: Arc::new(Semaphore::new(max_active)),
            max_active: AtomicUsize::new(max_active),
            events,
            store: Arc::new(store),
        }
    }

    /// Default engine, default store location, [`DEFAULT_MAX_ACTIVE`].
    pub fn with_defaults() -> fdm_core::Result<Self> {
        Ok(Self::new(
            Engine::new(EngineConfig::default())?,
            Store::new(Store::default_path()),
            DEFAULT_MAX_ACTIVE,
        ))
    }

    pub fn engine_config(&self) -> EngineConfig {
        self.engine.config()
    }

    pub fn set_max_connections(&self, conns: u32) {
        self.engine.set_max_connections(conns);
    }

    pub fn set_max_active(&self, max_active: usize) {
        let max_active = max_active.clamp(1, 32);
        self.max_active.store(max_active, Ordering::Relaxed);
    }

    pub fn max_active(&self) -> usize {
        self.max_active.load(Ordering::Relaxed)
    }

    pub fn store_path(&self) -> &std::path::Path {
        self.store.path()
    }

    /// Live changes to the list. A late subscriber should call [`Manager::list`]
    /// first and then apply events, because the channel only replays 512
    /// messages.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn list(&self) -> Vec<DownloadEntry> {
        self.reg.lock().unwrap().ordered()
    }

    pub fn get(&self, id: DownloadId) -> Option<DownloadEntry> {
        self.reg.lock().unwrap().snapshot(id)
    }

    /// Queue a download and return its id immediately. Must be called from
    /// within a Tokio runtime — it spawns the task that does the work.
    pub fn add(&self, new: NewDownload) -> DownloadId {
        let filename = new
            .filename
            .clone()
            .unwrap_or_else(|| filename_from_url(&new.url));

        let (id, generation, snapshot) = {
            let mut reg = self.reg.lock().unwrap();
            let id = reg.next_id;
            reg.next_id += 1;

            let entry = DownloadEntry::new(id, new.url.as_str(), filename);
            let snapshot = entry.clone();
            reg.entries.insert(id, entry);
            reg.runtime
                .insert(id, Runtime::new(new.headers.clone(), new.target_dir.clone()));
            reg.order.push(id);
            let generation = reg.new_generation(id);
            (id, generation, snapshot)
        };

        let _ = self.events.send(Event::Added(snapshot));
        self.persist();
        self.spawn(id, generation, new.url, new.filename, new.target_dir);
        id
    }

    /// Stop, keep the partial data, stay resumable.
    pub fn pause(&self, id: DownloadId) -> Result<()> {
        let (status, snapshot) = {
            let mut reg = self.reg.lock().unwrap();
            let entry = reg.entries.get(&id).ok_or(ManagerError::NotFound(id))?;
            let status = entry.status;
            if !status.is_active() {
                return Err(ManagerError::WrongState {
                    id,
                    status,
                    action: "pause",
                });
            }

            // Set Paused here rather than waiting for the task, so a download
            // still queued behind a busy slot reports what the user asked for
            // instead of sitting on "Queued" until a permit frees.
            let resumable = match reg.runtime.get_mut(&id) {
                Some(rt) => {
                    rt.intent = Some(Intent::Pause);
                    rt.cancel.cancel();
                    rt.control.as_deref().is_some_and(|c| c.exists())
                }
                None => false,
            };

            let entry = reg.entries.get_mut(&id).unwrap();
            entry.status = Status::Paused;
            entry.speed_bps = 0.0;
            entry.eta_secs = None;
            entry.active_connections = 0;
            entry.resumable = resumable;
            let snapshot = entry.clone();
            (status, snapshot)
        };

        tracing::info!(id, ?status, "paused");
        let _ = self.events.send(Event::Changed(snapshot));
        self.persist();
        Ok(())
    }

    /// Start again from wherever the `.part` file got to.
    ///
    /// For a row restored from disk the request is rebuilt from
    /// [`DownloadEntry::path`]: its parent becomes the target directory and its
    /// file name the filename, which lands the engine on the same `.part` and
    /// `.fdm` pair it left behind. Passing the parent as an explicit target is
    /// correct rather than a shortcut — the saved path already contains the
    /// category folder, so re-sorting it would nest one inside another.
    pub fn resume(&self, id: DownloadId) -> Result<()> {
        let (url, generation, filename, target_dir, snapshot) = {
            let mut reg = self.reg.lock().unwrap();
            let entry = reg.entries.get(&id).ok_or(ManagerError::NotFound(id))?;
            if entry.status.is_active() {
                return Err(ManagerError::WrongState {
                    id,
                    status: entry.status,
                    action: "resume",
                });
            }
            if entry.status == Status::Completed {
                return Err(ManagerError::WrongState {
                    id,
                    status: entry.status,
                    action: "resume",
                });
            }

            let url = url::Url::parse(&entry.url).map_err(|_| ManagerError::BadUrl {
                id,
                url: entry.url.clone(),
            })?;

            // A previous attempt already resolved the destination, so reuse it
            // verbatim. Without it we only know the URL and let the engine decide
            // again.
            let from_path = entry.path.clone().and_then(|p| {
                let dir = p.parent()?.to_path_buf();
                let name = p.file_name()?.to_string_lossy().into_owned();
                Some((dir, name))
            });

            let (target_dir, filename) = match from_path {
                Some((dir, name)) => (Some(dir), Some(name)),
                None => {
                    let rt = reg.runtime.get(&id);
                    (rt.and_then(|r| r.target_dir.clone()), None)
                }
            };

            let entry = reg.entries.get_mut(&id).unwrap();
            entry.status = Status::Queued;
            entry.error = None;
            entry.speed_bps = 0.0;
            entry.eta_secs = None;
            entry.active_connections = 0;
            entry.finished_at = None;
            let snapshot = entry.clone();

            match reg.runtime.get_mut(&id) {
                Some(rt) => rt.target_dir = target_dir.clone(),
                // Restored from disk: the row survived the restart but its runtime
                // state did not, so there are no headers to carry over.
                None => {
                    reg.runtime
                        .insert(id, Runtime::new(HeaderMap::new(), target_dir.clone()));
                }
            }
            // Claims the download for this attempt, which also installs a fresh
            // cancel token and clears the pause that got us here. Any task still
            // finishing under the old generation is now a ghost.
            let generation = reg.new_generation(id);

            (url, generation, filename, target_dir, snapshot)
        };

        tracing::info!(id, generation, "resuming");
        let _ = self.events.send(Event::Changed(snapshot));
        self.persist();
        self.spawn(id, generation, url, filename, target_dir);
        Ok(())
    }

    /// Stop and throw the partial data away. The row stays in the list so the
    /// user can see what happened and restart it.
    pub fn cancel(&self, id: DownloadId) -> Result<()> {
        let snapshot = {
            let mut reg = self.reg.lock().unwrap();
            let entry = reg.entries.get(&id).ok_or(ManagerError::NotFound(id))?;
            let was_active = entry.status.is_active();

            if was_active {
                // The engine still has the `.part` file open, and Windows will
                // not unlink a file with a live handle. So record the intent and
                // let the task delete once it has stopped.
                let rt = reg.runtime.get_mut(&id).ok_or(ManagerError::NotFound(id))?;
                rt.intent = Some(Intent::Cancel);
                rt.cancel.cancel();
            } else {
                let scratch = reg.runtime.get(&id).map(|r| (r.part.clone(), r.control.clone()));
                if let Some((part, control)) = scratch {
                    discard(part.as_deref(), control.as_deref());
                }
            }

            let entry = reg.entries.get_mut(&id).unwrap();
            entry.status = Status::Cancelled;
            entry.speed_bps = 0.0;
            entry.eta_secs = None;
            entry.active_connections = 0;
            entry.resumable = false;
            entry.finished_at = Some(now_secs());
            entry.clone()
        };

        tracing::info!(id, "cancelled");
        let _ = self.events.send(Event::Changed(snapshot));
        self.persist();
        Ok(())
    }

    /// Take the row out of the list.
    ///
    /// `delete_file` also deletes the finished download. Partial data is always
    /// deleted — leaving an orphaned `.part` with no row to resume it from would
    /// be litter the user has no way to find.
    ///
    /// For a running download the row disappears only once the task has stopped,
    /// because the `.part` file cannot be unlinked while the engine holds it
    /// open. The caller gets `Ok(())` immediately; the [`Event::Removed`] follows.
    pub fn remove(&self, id: DownloadId, delete_file: bool) -> Result<()> {
        let removed_now = {
            let mut reg = self.reg.lock().unwrap();
            let entry = reg.entries.get(&id).ok_or(ManagerError::NotFound(id))?;

            if entry.status.is_active() {
                let rt = reg.runtime.get_mut(&id).ok_or(ManagerError::NotFound(id))?;
                rt.intent = Some(Intent::Remove { delete_file });
                rt.cancel.cancel();
                false
            } else {
                let final_path = entry.path.clone();
                let completed = entry.status == Status::Completed;
                let scratch = reg.runtime.get(&id).map(|r| (r.part.clone(), r.control.clone()));

                if let Some((part, control)) = scratch {
                    discard(part.as_deref(), control.as_deref());
                }
                if delete_file && completed {
                    if let Some(p) = final_path {
                        if let Err(e) = std::fs::remove_file(&p) {
                            tracing::warn!(path = %p.display(), error = %e, "could not delete the file");
                        }
                    }
                }
                reg.forget(id);
                true
            }
        };

        if removed_now {
            tracing::info!(id, delete_file, "removed");
            let _ = self.events.send(Event::Removed(id));
            self.persist();
        }
        Ok(())
    }

    /// Pause everything that is running or waiting. Returns how many were
    /// affected. Errors are impossible here by construction, so they are dropped
    /// rather than surfaced: a row that finished between the list scan and the
    /// pause call needs no action.
    pub fn pause_all(&self) -> usize {
        let ids: Vec<_> = self
            .list()
            .into_iter()
            .filter(|e| e.status.is_active())
            .map(|e| e.id)
            .collect();
        ids.iter().filter(|id| self.pause(**id).is_ok()).count()
    }

    /// Resume everything that is paused or failed.
    pub fn resume_all(&self) -> usize {
        let ids: Vec<_> = self
            .list()
            .into_iter()
            .filter(|e| matches!(e.status, Status::Paused | Status::Failed))
            .map(|e| e.id)
            .collect();
        ids.iter().filter(|id| self.resume(**id).is_ok()).count()
    }

    /// Drop every completed, failed and cancelled row. The files themselves are
    /// left alone — this clears the list, it does not delete downloads.
    pub fn clear_finished(&self) -> usize {
        let ids: Vec<_> = self
            .list()
            .into_iter()
            .filter(|e| e.status.is_finished())
            .map(|e| e.id)
            .collect();
        ids.iter().filter(|id| self.remove(**id, false).is_ok()).count()
    }

    fn persist(&self) {
        let (next_id, entries) = {
            let reg = self.reg.lock().unwrap();
            (reg.next_id, reg.ordered())
        };
        self.store.save(next_id, &entries);
    }

    // ------------------------------------------------------------------ the task

    fn spawn(
        &self,
        id: DownloadId,
        generation: u64,
        url: url::Url,
        filename: Option<String>,
        target_dir: Option<PathBuf>,
    ) {
        let engine = self.engine.clone();
        let reg = self.reg.clone();
        let slots = self.slots.clone();
        let events = self.events.clone();
        let store = self.store.clone();

        tokio::spawn(async move {
            // Attempts for one download are serialised. `pause` returns before the
            // engine has stopped, so a resume issued straight afterwards would
            // otherwise put two engines on the same `.part` file — and have them
            // race to move it into place.
            //
            // Taken *before* the semaphore permit, not after: a ghost attempt can
            // be sitting in the queue holding this lock, and it needs a permit to
            // reach the point where it notices it is a ghost and lets go.
            let run = match reg.lock().unwrap().runtime.get(&id) {
                Some(rt) => rt.run.clone(),
                None => return, // removed while this task was being spawned
            };
            let _run = run.lock().await;

            // Resumed again while we waited. That attempt owns the row now.
            if !reg.lock().unwrap().is_current(id, generation) {
                return;
            }

            // Queueing happens here, not in a scheduler: a download that cannot
            // start yet simply waits on the semaphore, which is also what makes
            // `max_active` a single source of truth.
            let permit = match slots.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return, // semaphore closed: the app is shutting down
            };

            let (mut req, cancel) = {
                let mut guard = reg.lock().unwrap();
                if !guard.is_current(id, generation) {
                    return;
                }
                let Some(entry) = guard.entries.get(&id) else {
                    return; // removed while queued
                };
                // pause/cancel/remove while queued already wrote the terminal
                // status. Starting now would contradict the UI.
                if !entry.status.is_active() {
                    finalise_stopped_before_start(&mut guard, id, &events, &store);
                    return;
                }
                let Some(rt) = guard.runtime.get(&id) else {
                    return;
                };
                let mut req = DownloadRequest::new(url.clone()).with_headers(rt.headers.clone());
                if let Some(name) = filename.clone() {
                    req = req.with_filename(name);
                }
                if let Some(dir) = target_dir.clone() {
                    req = req.with_target_dir(dir);
                }
                (req, rt.cancel.clone())
            };

            let max_conns = engine.config().max_connections;
            let result = if is_video_platform(url.as_str()) && find_tool("yt-dlp.exe").is_some() {
                download_video_platform(
                    id,
                    generation,
                    url.as_str(),
                    filename.clone(),
                    target_dir.clone(),
                    max_conns,
                    cancel,
                    reg.clone(),
                    events.clone(),
                    store.clone(),
                ).await
            } else {
                let on_start = {
                    let reg = reg.clone();
                    let events = events.clone();
                    let store = store.clone();
                    move |info: &StartInfo| {
                        let snapshot = {
                            let mut guard = reg.lock().unwrap();
                            if !guard.is_current(id, generation) {
                                return;
                            }
                            if let Some(rt) = guard.runtime.get_mut(&id) {
                                rt.part = Some(info.part.clone());
                                rt.control = Some(info.control.clone());
                            }
                            let Some(entry) = guard.entries.get_mut(&id) else {
                                return;
                            };
                            if !entry.status.is_active() {
                                return;
                            }
                            entry.status = Status::Downloading;
                            entry.path = Some(info.target.clone());
                            if let Some(name) = info.target.file_name() {
                                entry.filename = name.to_string_lossy().into_owned();
                            }
                            entry.total = info.total;
                            entry.category = Some(info.category);
                            entry.resumable = info.used_ranges;
                            entry.clone()
                        };
                        let _ = events.send(Event::Changed(snapshot));
                        persist_now(&reg, &store);
                    }
                };

                let on_progress = {
                    let reg = reg.clone();
                    let events = events.clone();
                    move |p: ProgressSnapshot| {
                        let snapshot = {
                            let mut guard = reg.lock().unwrap();
                            if !guard.is_current(id, generation) {
                                return;
                            }
                            let Some(entry) = guard.entries.get_mut(&id) else {
                                return;
                            };
                            if !entry.status.is_active() {
                                return;
                            }
                            entry.downloaded = p.downloaded;
                            entry.total = p.total;
                            entry.speed_bps = p.speed_bps;
                            entry.eta_secs = p.eta.map(|d| d.as_secs());
                            entry.segments = p.segments;
                            entry.active_connections = p.active_connections;
                            entry.clone()
                        };
                        let _ = events.send(Event::Changed(snapshot));
                    }
                };

                engine
                    .download_observed(req, cancel, on_progress, on_start)
                    .await
            };

            // Release the slot before the bookkeeping, so the next queued
            // download starts while this one is being written down.
            drop(permit);

            let outcome = Terminal::from(result);
            finalise(&reg, id, generation, outcome, &events, &store);
        });
    }
}

/// What the task decided, reduced to the three cases the registry cares about.
enum Terminal {
    Done(fdm_core::DownloadOutcome),
    Stopped,
    Failed(fdm_core::Error),
}

impl From<fdm_core::Result<fdm_core::DownloadOutcome>> for Terminal {
    fn from(r: fdm_core::Result<fdm_core::DownloadOutcome>) -> Self {
        match r {
            Ok(o) => Terminal::Done(o),
            Err(fdm_core::Error::Cancelled) => Terminal::Stopped,
            Err(e) => Terminal::Failed(e),
        }
    }
}

/// The one place a download's terminal status is written.
fn finalise(
    reg: &Arc<Mutex<Registry>>,
    id: DownloadId,
    generation: u64,
    outcome: Terminal,
    events: &broadcast::Sender<Event>,
    store: &Store,
) {
    enum Emit {
        Changed(DownloadEntry),
        Removed,
        Nothing,
    }

    let emit = {
        let mut guard = reg.lock().unwrap();
        let intent = guard.runtime.get(&id).and_then(|r| r.intent);
        let scratch = guard
            .runtime
            .get(&id)
            .map(|r| (r.part.clone(), r.control.clone()))
            .unwrap_or((None, None));

        if !guard.entries.contains_key(&id) {
            Emit::Nothing
        } else if !guard.is_current(id, generation) {
            // A ghost: the user resumed while this attempt was still stopping, and
            // a newer one owns the row. Writing "Paused" here is how a download
            // that is running — or has already finished — ends up displayed as
            // stopped. So write nothing.
            tracing::debug!(id, generation, "ignoring the result of a superseded attempt");
            Emit::Nothing
        } else {
            match (outcome, intent) {
                // Removal wins over everything: the row is going away, so its
                // status no longer matters.
                (_, Some(Intent::Remove { delete_file })) => {
                    discard(scratch.0.as_deref(), scratch.1.as_deref());
                    if delete_file {
                        if let Some(p) = guard.entries.get(&id).and_then(|e| e.path.clone()) {
                            let _ = std::fs::remove_file(p);
                        }
                    }
                    guard.forget(id);
                    Emit::Removed
                }

                (Terminal::Done(o), _) => {
                    let entry = guard.entries.get_mut(&id).unwrap();
                    entry.status = Status::Completed;
                    entry.path = Some(o.path.clone());
                    if let Some(name) = o.path.file_name() {
                        entry.filename = name.to_string_lossy().into_owned();
                    }
                    entry.downloaded = o.bytes;
                    entry.total = Some(o.bytes);
                    entry.category = Some(o.category);
                    entry.segments = o.segments_used;
                    entry.speed_bps = 0.0;
                    entry.eta_secs = None;
                    entry.active_connections = 0;
                    entry.error = None;
                    entry.resumable = false;
                    entry.finished_at = Some(now_secs());
                    tracing::info!(id, path = %o.path.display(), bytes = o.bytes, "completed");
                    Emit::Changed(entry.clone())
                }

                (Terminal::Stopped, Some(Intent::Cancel)) => {
                    discard(scratch.0.as_deref(), scratch.1.as_deref());
                    let entry = guard.entries.get_mut(&id).unwrap();
                    // `cancel()` already wrote Cancelled; this only settles the
                    // fields that depended on the engine actually having stopped.
                    entry.status = Status::Cancelled;
                    entry.resumable = false;
                    entry.speed_bps = 0.0;
                    entry.eta_secs = None;
                    entry.active_connections = 0;
                    Emit::Changed(entry.clone())
                }

                // Paused, either explicitly or because the token was tripped
                // without an intent (shutdown). Both keep the partial data.
                (Terminal::Stopped, _) => {
                    let resumable = scratch.1.as_deref().is_some_and(|c| c.exists());
                    let entry = guard.entries.get_mut(&id).unwrap();
                    entry.status = Status::Paused;
                    entry.resumable = resumable;
                    entry.speed_bps = 0.0;
                    entry.eta_secs = None;
                    entry.active_connections = 0;
                    Emit::Changed(entry.clone())
                }

                (Terminal::Failed(e), _) => {
                    // Resumability is a fact about the disk, not about the error:
                    // if a control file survived, the next attempt continues.
                    let resumable = scratch.1.as_deref().is_some_and(|c| c.exists());
                    let entry = guard.entries.get_mut(&id).unwrap();
                    entry.status = Status::Failed;
                    entry.error = Some(e.to_string());
                    entry.resumable = resumable;
                    entry.speed_bps = 0.0;
                    entry.eta_secs = None;
                    entry.active_connections = 0;
                    entry.finished_at = Some(now_secs());
                    tracing::warn!(id, error = %e, resumable, "failed");
                    Emit::Changed(entry.clone())
                }
            }
        }
    };

    match emit {
        Emit::Changed(entry) => {
            let _ = events.send(Event::Changed(entry));
            persist_now(reg, store);
        }
        Emit::Removed => {
            let _ = events.send(Event::Removed(id));
            persist_now(reg, store);
        }
        Emit::Nothing => {}
    }
}

/// The download was stopped before the engine was ever called, so there is no
/// engine result to interpret. Only `Remove` needs anything doing.
fn finalise_stopped_before_start(
    guard: &mut Registry,
    id: DownloadId,
    events: &broadcast::Sender<Event>,
    store: &Store,
) {
    let intent = guard.runtime.get(&id).and_then(|r| r.intent);
    if let Some(Intent::Remove { delete_file }) = intent {
        let scratch = guard
            .runtime
            .get(&id)
            .map(|r| (r.part.clone(), r.control.clone()))
            .unwrap_or((None, None));
        discard(scratch.0.as_deref(), scratch.1.as_deref());
        if delete_file {
            if let Some(p) = guard.entries.get(&id).and_then(|e| e.path.clone()) {
                let _ = std::fs::remove_file(p);
            }
        }
        guard.forget(id);
        let next_id = guard.next_id;
        let entries = guard.ordered();
        let _ = events.send(Event::Removed(id));
        store.save(next_id, &entries);
    }
}

fn persist_now(reg: &Arc<Mutex<Registry>>, store: &Store) {
    let (next_id, entries) = {
        let guard = reg.lock().unwrap();
        (guard.next_id, guard.ordered())
    };
    store.save(next_id, &entries);
}

/// Delete the scratch pair. Silent on failure by design: a `.part` that will not
/// unlink is a stale-handle problem on the next attempt, not something to report
/// as a download error.
fn discard(part: Option<&std::path::Path>, control: Option<&std::path::Path>) {
    for p in [part, control].into_iter().flatten() {
        match std::fs::remove_file(p) {
            Ok(()) => tracing::debug!(path = %p.display(), "discarded"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(path = %p.display(), error = %e, "could not discard"),
        }
    }
}

/// Last path segment, percent-decoded enough to be recognisable. Only a
/// placeholder: the engine replaces it with the real name after the probe reads
/// `Content-Disposition`.
fn filename_from_url(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back().filter(|s| !s.is_empty()))
        .map(percent_decode)
        .unwrap_or_else(|| "download".to_string())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_video_platform(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("youtube.com/watch")
        || lower.contains("youtube.com/shorts")
        || lower.contains("youtube.com/live")
        || lower.contains("youtu.be/")
        || lower.contains("tiktok.com/")
        || lower.contains("vimeo.com/")
        || lower.contains("instagram.com/p/")
        || lower.contains("instagram.com/reel/")
        || lower.contains("twitter.com/")
        || lower.contains("x.com/")
}

fn parse_speed_str(s: &str) -> f64 {
    let s = s.trim().to_uppercase();
    if s.contains("GIB/S") || s.contains("GB/S") {
        let num_str = s.replace("GIB/S", "").replace("GB/S", "");
        num_str.trim().parse::<f64>().unwrap_or(0.0) * 1024.0 * 1024.0 * 1024.0
    } else if s.contains("MIB/S") || s.contains("MB/S") {
        let num_str = s.replace("MIB/S", "").replace("MB/S", "");
        num_str.trim().parse::<f64>().unwrap_or(0.0) * 1024.0 * 1024.0
    } else if s.contains("KIB/S") || s.contains("KB/S") {
        let num_str = s.replace("KIB/S", "").replace("KB/S", "");
        num_str.trim().parse::<f64>().unwrap_or(0.0) * 1024.0
    } else if s.contains("B/S") {
        let num_str = s.replace("B/S", "");
        num_str.trim().parse::<f64>().unwrap_or(0.0)
    } else {
        0.0
    }
}

fn parse_eta_str(s: &str) -> Option<u64> {
    let s = s.trim();
    if s == "UNKNOWN" || s == "NA" || s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let m: u64 = parts[0].trim().parse().ok()?;
        let s: u64 = parts[1].trim().parse().ok()?;
        Some(m * 60 + s)
    } else if parts.len() == 3 {
        let h: u64 = parts[0].trim().parse().ok()?;
        let m: u64 = parts[1].trim().parse().ok()?;
        let s: u64 = parts[2].trim().parse().ok()?;
        Some(h * 3600 + m * 60 + s)
    } else {
        None
    }
}

async fn download_video_platform(
    id: DownloadId,
    generation: u64,
    url: &str,
    filename: Option<String>,
    target_dir: Option<PathBuf>,
    max_conns: u32,
    cancel: CancelToken,
    reg: Arc<Mutex<Registry>>,
    events: broadcast::Sender<Event>,
    _store: Arc<Store>,
) -> fdm_core::Result<fdm_core::DownloadOutcome> {
    let ytdlp_path = find_tool("yt-dlp.exe").ok_or_else(|| fdm_core::Error::other("yt-dlp.exe not found"))?;
    let deno = find_tool("deno.exe");
    let ffmpeg = find_tool("ffmpeg.exe");

    let is_audio = filename.as_ref().map(|f| f.contains("(Audio)") || f.ends_with(".mp3") || f.ends_with(".m4a")).unwrap_or(false);

    let mut format_arg = "bestvideo[ext=mp4]+bestaudio[ext=m4a]/bestvideo+bestaudio/best".to_string();
    if is_audio {
        format_arg = "bestaudio[ext=m4a]/bestaudio/best".to_string();
    } else if let Some(ref name) = filename {
        for res in [2160, 1440, 1080, 720, 480, 360, 240, 144] {
            if name.contains(&format!("{}p", res)) || name.contains(&format!("{}P", res)) {
                format_arg = format!(
                    "bestvideo[height<={}][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<={}]+bestaudio/best[height<={}]/bestvideo+bestaudio/best",
                    res, res, res
                );
                break;
            }
        }
    }

    let default_dir = target_dir.unwrap_or_else(|| {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join("Downloads").join("FDM").join(if is_audio { "Music" } else { "Video" })
    });
    let _ = std::fs::create_dir_all(&default_dir);

    let output_template = format!("{}/%(title)s.%(ext)s", default_dir.display());

    // Mark as downloading
    {
        let mut guard = reg.lock().unwrap();
        if let Some(entry) = guard.entries.get_mut(&id) {
            entry.status = Status::Downloading;
            entry.category = Some(if is_audio {
                fdm_core::categorize::Category::Music
            } else {
                fdm_core::categorize::Category::Video
            });
            let snapshot = entry.clone();
            let _ = events.send(Event::Changed(snapshot));
        }
    }

    let started = std::time::Instant::now();
    let url_owned = url.to_string();
    let reg_c = reg.clone();
    let events_c = events.clone();
    let cancel_c = cancel.clone();

    let outcome = tokio::task::spawn_blocking(move || -> fdm_core::Result<PathBuf> {
        let mut cmd = std::process::Command::new(ytdlp_path);
        cmd.args(&[
            "--newline",
            "--progress-template",
            "download:FDM_PROG:%(progress.downloaded_bytes)s:%(progress.total_bytes)s:%(progress._speed_str)s:%(progress._eta_str)s",
            "--no-playlist",
            "--no-warnings",
            "-N",
            &max_conns.to_string(),
            "-f",
            &format_arg,
            "-o",
            &output_template,
        ]);

        if let Some(deno_path) = deno {
            cmd.arg("--js-runtimes").arg(format!("deno:{}", deno_path.display()));
        }
        if let Some(ffmpeg_path) = ffmpeg {
            if let Some(parent) = ffmpeg_path.parent() {
                cmd.arg("--ffmpeg-location").arg(parent);
            }
        }
        cmd.arg(&url_owned);

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = cmd.spawn().map_err(|e| fdm_core::Error::other(e.to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| fdm_core::Error::other("Failed to capture stdout"))?;

        use std::io::{BufRead, BufReader};
        let reader = BufReader::new(stdout);
        let mut last_progress = std::time::Instant::now();
        let mut base_downloaded: u64 = 0;
        let mut prev_track_downloaded: u64 = 0;
        let mut prev_track_total: u64 = 0;
        let mut cumulative_total: u64 = 0;

        for line in reader.lines().flatten() {
            if cancel_c.is_cancelled() {
                let _ = child.kill();
                return Err(fdm_core::Error::Cancelled);
            }

            if line.starts_with("FDM_PROG:") {
                let raw = line.trim_start_matches("FDM_PROG:").trim();
                let parts: Vec<&str> = raw.split(':').collect();
                if parts.len() >= 4 {
                    let curr_downloaded: u64 = parts[0].trim().parse().unwrap_or(0);
                    let curr_total: Option<u64> = parts[1].trim().parse().ok().filter(|&t| t > 0);
                    let speed_str = parts[2].trim();
                    let eta_str = parts[3].trim();

                    // Detect transition from Video track to Audio track without progress resetting
                    if curr_downloaded < prev_track_downloaded && prev_track_downloaded > 0 {
                        base_downloaded += prev_track_downloaded;
                        prev_track_downloaded = 0;
                    } else {
                        prev_track_downloaded = curr_downloaded;
                    }

                    if let Some(t) = curr_total {
                        if t != prev_track_total {
                            cumulative_total = base_downloaded + t;
                            prev_track_total = t;
                        }
                    }

                    let display_downloaded = base_downloaded + curr_downloaded;
                    let display_total = if cumulative_total > 0 {
                        Some(cumulative_total.max(display_downloaded))
                    } else {
                        curr_total.map(|t| base_downloaded + t)
                    };

                    if last_progress.elapsed() >= std::time::Duration::from_millis(50) {
                        last_progress = std::time::Instant::now();
                        let mut guard = reg_c.lock().unwrap();
                        if guard.is_current(id, generation) {
                            if let Some(entry) = guard.entries.get_mut(&id) {
                                entry.downloaded = display_downloaded;
                                if display_total.is_some() {
                                    entry.total = display_total;
                                }
                                entry.speed_bps = parse_speed_str(speed_str);
                                entry.eta_secs = parse_eta_str(eta_str);
                                entry.active_connections = max_conns;
                                let snapshot = entry.clone();
                                let _ = events_c.send(Event::Changed(snapshot));
                            }
                        }
                    }
                }
            }
        }

        let status = child.wait().map_err(|e| fdm_core::Error::other(e.to_string()))?;
        if !status.success() {
            if cancel_c.is_cancelled() {
                return Err(fdm_core::Error::Cancelled);
            }
            return Err(fdm_core::Error::other("Video download failed"));
        }

        let mut final_path = default_dir.clone();
        if let Ok(entries) = std::fs::read_dir(&default_dir) {
            let mut newest: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                    if ext == "mp4" || ext == "m4a" || ext == "webm" || ext == "mkv" {
                        if let Ok(meta) = entry.metadata() {
                            if let Ok(modified) = meta.modified() {
                                if newest.as_ref().map(|(_, t)| modified > *t).unwrap_or(true) {
                                    newest = Some((path, modified));
                                }
                            }
                        }
                    }
                }
            }
            if let Some((p, _)) = newest {
                final_path = p;
            }
        }

        Ok(final_path)
    }).await.map_err(|e| fdm_core::Error::other(e.to_string()))??;

    let file_size = std::fs::metadata(&outcome).map(|m| m.len()).unwrap_or(0);

    {
        let mut guard = reg.lock().unwrap();
        if let Some(entry) = guard.entries.get_mut(&id) {
            entry.path = Some(outcome.clone());
            entry.downloaded = file_size;
            entry.total = Some(file_size);
            if let Some(name) = outcome.file_name() {
                entry.filename = name.to_string_lossy().into_owned();
            }
        }
    }

    Ok(fdm_core::DownloadOutcome {
        path: outcome,
        bytes: file_size,
        elapsed: started.elapsed(),
        segments_used: max_conns,
        category: if is_audio {
            fdm_core::categorize::Category::Music
        } else {
            fdm_core::categorize::Category::Video
        },
        resumed: false,
        used_ranges: true,
    })
}

fn find_tool(name: &str) -> Option<std::path::PathBuf> {
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

    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let in_appdata = std::path::PathBuf::from(local_appdata).join("FDM").join("tools").join(name);
        if in_appdata.exists() {
            return Some(in_appdata);
        }
    }

    let static_paths = [
        std::path::PathBuf::from(r"D:\Code\FDM\target\release\tools").join(name),
        std::path::PathBuf::from(r"D:\Code\FDM\target\installer-staging\tools").join(name),
        std::path::PathBuf::from(r"C:\Program Files\FDM\tools").join(name),
    ];
    for p in static_paths {
        if p.exists() {
            return Some(p);
        }
    }

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
