//! What the download list must actually do.
//!
//! Every test here runs against a local throttled server (`common::TestServer`)
//! so pause can land mid-transfer and a queue can be caught holding a download
//! back. Nothing touches the network, and nothing writes outside a `tempdir`.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fdm_core::{Engine, EngineConfig};
use fdm_manager::{DownloadEntry, Manager, NewDownload, Status, Store, DEFAULT_MAX_ACTIVE};

use common::{expected_body, TestServer};

/// Body size for the throttled tests. Small enough to keep the suite quick, large
/// enough to split into four segments at `min_split_size`.
const SLOW_TOTAL: usize = 512 * 1024;

/// A transfer that takes a couple of seconds but delivers its first bytes at
/// once, so a test can act on a download that is genuinely mid-flight. Sized
/// against `min_split_size`: 4 segments of 16 chunks, 150 ms apart.
async fn slow_server() -> TestServer {
    TestServer::start(SLOW_TOTAL, 8 * 1024, Duration::from_millis(150))
        .await
        .unwrap()
}

/// Unthrottled, for tests that only care about the finished result.
async fn quick_server(total: usize) -> TestServer {
    TestServer::start(total, 64 * 1024, Duration::ZERO).await.unwrap()
}

fn config(root: &Path, temp: &Path) -> EngineConfig {
    EngineConfig {
        download_root: root.to_path_buf(),
        temp_dir: temp.to_path_buf(),
        max_connections: 4,
        min_split_size: 16 * 1024,
        progress_interval: Duration::from_millis(50),
        ..EngineConfig::default()
    }
}

/// An engine and manager confined to a temp directory, so a test can never write
/// into the developer's real Downloads folder.
struct Harness {
    manager: Manager,
    root: PathBuf,
    temp: PathBuf,
    _dir: tempfile::TempDir,
}

fn harness(max_active: usize) -> Harness {
    harness_with(max_active, |_| {})
}

fn harness_with(max_active: usize, tweak: impl FnOnce(&mut EngineConfig)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Downloads");
    let temp = dir.path().join("Temp");

    let mut cfg = config(&root, &temp);
    tweak(&mut cfg);

    let store = Store::new(dir.path().join("downloads.json"));
    let manager = Manager::new(Engine::new(cfg).unwrap(), store, max_active);

    Harness { manager, root, temp, _dir: dir }
}

/// Run one "app session" on its own runtime, then kill the runtime.
///
/// Necessary because a `tokio::spawn`ed download task outlives the `Manager` that
/// created it: dropping the manager alone leaves the download running, still
/// writing to the same store the next session is about to read. Killing the whole
/// runtime is what quitting — or crashing — actually looks like.
///
/// On the blocking pool rather than a bare `std::thread` + `join()`, because this
/// test's own runtime is single-threaded and is hosting the server the session
/// downloads from. Blocking it would deadlock: the session would wait forever for
/// a connection nothing was left to accept.
async fn app_session<F, Fut, T>(
    cfg: EngineConfig,
    store_path: PathBuf,
    max_active: usize,
    body: F,
) -> T
where
    F: FnOnce(Manager) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T>,
    T: Send + 'static,
{
    let joined = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt.block_on(async move {
            let manager =
                Manager::new(Engine::new(cfg).unwrap(), Store::new(store_path), max_active);
            body(manager).await
        });
        // Deliberately not graceful: every in-flight download dies where it
        // stands, mid-write.
        rt.shutdown_timeout(Duration::ZERO);
        out
    })
    .await;

    match joined {
        Ok(v) => v,
        // Re-raise rather than `unwrap`, so a failed assertion inside the session
        // reports its own message instead of `Any { .. }`.
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => panic!("the app session did not finish: {e}"),
    }
}

/// Poll until `pred` holds. Beats a fixed sleep: the assertion that follows knows
/// the state it is asserting about was actually reached.
async fn until(
    manager: &Manager,
    id: u64,
    label: &str,
    pred: impl Fn(&DownloadEntry) -> bool,
) -> DownloadEntry {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        if let Some(e) = manager.get(id) {
            if pred(&e) {
                return e;
            }
            last = Some(e);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "timed out waiting for {label}; last was {:?}",
        last.map(|e| (e.status, e.downloaded, e.error))
    );
}

/// Wait for something the manager does *after* an API call returns — deleting a
/// `.part` file, dropping a row — because those wait on the engine releasing its
/// file handle rather than on the call itself.
async fn eventually(label: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("timed out waiting for {label}");
}

fn scratch_files(temp: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(temp) else {
        return Vec::new();
    };
    let mut v: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    v.sort();
    v
}

fn has_extension(files: &[PathBuf], ext: &str) -> bool {
    files.iter().any(|p| p.extension().is_some_and(|x| x == ext))
}

// --------------------------------------------------------------- the happy path

#[tokio::test]
async fn a_download_completes_and_lands_sorted_by_type() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = quick_server(256 * 1024).await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("archive.zip").parse().unwrap()));

    let done = until(&h.manager, id, "completion", |e| e.status == Status::Completed).await;

    let path = done.path.clone().expect("a completed download must know where it landed");
    // No target_dir was given, so the engine sorts by type — the product
    // requirement the whole `Downloads\FDM\<Category>\` tree exists for.
    assert_eq!(path.parent().unwrap().parent().unwrap(), h.root);
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "Compressed");
    assert_eq!(path.file_name().unwrap(), "archive.zip");

    assert_eq!(std::fs::read(&path).unwrap(), expected_body(256 * 1024));
    assert_eq!(done.downloaded, 256 * 1024);
    assert_eq!(done.total, Some(256 * 1024));
    assert_eq!(done.fraction(), Some(1.0));
    assert!(done.error.is_none());

    // The whole point of a temp directory: nothing of ours is left in it, and
    // nothing of ours was ever in the download folder.
    assert!(scratch_files(&h.temp).is_empty(), "{:?}", scratch_files(&h.temp));
}

#[tokio::test]
async fn partial_data_lives_in_the_temp_dir_not_next_to_the_file() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = slow_server().await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("movie.mp4").parse().unwrap()));
    until(&h.manager, id, "bytes to move", |e| e.downloaded > 0).await;

    let scratch = scratch_files(&h.temp);
    assert!(
        has_extension(&scratch, "part"),
        "expected a .part file in {}, found {scratch:?}",
        h.temp.display()
    );
    assert!(
        has_extension(&scratch, "fdm"),
        "expected a .fdm control file in {}, found {scratch:?}",
        h.temp.display()
    );

    // And the download folder holds nothing yet — this is what the user sees.
    let visible = scratch_files(&h.root.join("Video"));
    assert!(visible.is_empty(), "download folder is not clean: {visible:?}");

    h.manager.cancel(id).unwrap();
}

#[tokio::test]
async fn partial_data_sits_beside_the_file_when_the_temp_dir_is_switched_off() {
    let h = harness_with(DEFAULT_MAX_ACTIVE, |cfg| cfg.use_temp_dir = false);
    let server = quick_server(256 * 1024).await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("notes.pdf").parse().unwrap()));
    let done = until(&h.manager, id, "completion", |e| e.status == Status::Completed).await;

    assert!(done.path.unwrap().exists());
    // The temp directory is never even created when it is not in use.
    assert!(scratch_files(&h.temp).is_empty());
}

// ------------------------------------------------------------- pause and resume

#[tokio::test]
async fn pause_keeps_the_partial_data_and_resume_finishes_the_job() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = slow_server().await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("big.iso").parse().unwrap()));
    until(&h.manager, id, "bytes to move", |e| e.downloaded > 0).await;

    h.manager.pause(id).unwrap();
    let paused = until(&h.manager, id, "pause to settle", |e| {
        e.status == Status::Paused && e.speed_bps == 0.0
    })
    .await;

    assert!(paused.downloaded > 0, "nothing was downloaded before pausing");
    assert!(
        paused.downloaded < SLOW_TOTAL as u64,
        "the download finished before it could pause"
    );
    assert!(paused.resumable, "a paused download with a control file must be resumable");

    // The control file is what makes resume possible; deleting it on pause would
    // be the bug this asserts against.
    let scratch = scratch_files(&h.temp);
    assert!(has_extension(&scratch, "fdm"), "pause deleted the control file: {scratch:?}");

    let gets_before = server.get_count();
    h.manager.resume(id).unwrap();
    let done = until(&h.manager, id, "completion after resume", |e| {
        e.status == Status::Completed
    })
    .await;

    let path = done.path.unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        expected_body(SLOW_TOTAL),
        "the resumed file is not byte-for-byte correct"
    );
    assert_eq!(done.downloaded, SLOW_TOTAL as u64);
    assert!(
        server.get_count() > gets_before,
        "resume did not issue any new request, so it cannot have continued anything"
    );
    assert!(scratch_files(&h.temp).is_empty(), "scratch files survived completion");
}

#[tokio::test]
async fn resuming_the_instant_pause_returns_does_not_lose_the_download() {
    // The race worth having a test for. `pause` returns before the engine has
    // stopped, so the paused attempt finishes *after* the resumed one has started.
    // If that stale task is allowed to write its terminal status, a running — or
    // already finished — download shows up as Paused and never moves again.
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = slow_server().await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("racy.bin").parse().unwrap()));
    until(&h.manager, id, "bytes to move", |e| e.downloaded > 0).await;

    // No settling time on purpose.
    h.manager.pause(id).unwrap();
    h.manager.resume(id).unwrap();

    let done = until(&h.manager, id, "completion despite the race", |e| {
        e.status == Status::Completed
    })
    .await;
    assert_eq!(
        std::fs::read(done.path.unwrap()).unwrap(),
        expected_body(SLOW_TOTAL),
        "two attempts overlapping corrupted the file"
    );

    // And the ghost cannot come back later and undo it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.manager.get(id).unwrap().status, Status::Completed);
}

#[tokio::test]
async fn pausing_a_queued_download_reports_paused_immediately() {
    // One slot, two downloads: the second is stuck behind the first.
    let h = harness(1);
    let server = slow_server().await;

    let first = h
        .manager
        .add(NewDownload::new(server.url("first.bin").parse().unwrap()));
    let second = h
        .manager
        .add(NewDownload::new(server.url("second.bin").parse().unwrap()));

    until(&h.manager, first, "the first to start", |e| e.downloaded > 0).await;
    assert_eq!(
        h.manager.get(second).unwrap().status,
        Status::Queued,
        "max_active is not holding the second download back"
    );

    // The user pressed pause on a row that has not started. It must say Paused
    // right away rather than sitting on Queued until a slot frees.
    h.manager.pause(second).unwrap();
    assert_eq!(h.manager.get(second).unwrap().status, Status::Paused);

    h.manager.pause(first).unwrap();
    // And the queued-then-paused download must not sneak into running once the
    // slot is free.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(h.manager.get(second).unwrap().status, Status::Paused);
    assert_eq!(h.manager.get(second).unwrap().downloaded, 0);
}

#[tokio::test]
async fn pause_is_rejected_on_a_finished_download() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = quick_server(64 * 1024).await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("small.txt").parse().unwrap()));
    until(&h.manager, id, "completion", |e| e.status == Status::Completed).await;

    // The UI should not offer the button; returning an error rather than silently
    // doing nothing is what makes a mistake visible.
    assert!(h.manager.pause(id).is_err());
    assert!(h.manager.resume(id).is_err(), "a completed download has nothing to resume");
    assert!(h.manager.pause(9999).is_err(), "unknown id");
}

// -------------------------------------------------------------------- cancelling

#[tokio::test]
async fn cancel_throws_the_partial_data_away() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = slow_server().await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("junk.bin").parse().unwrap()));
    until(&h.manager, id, "bytes to move", |e| e.downloaded > 0).await;
    assert!(!scratch_files(&h.temp).is_empty());

    h.manager.cancel(id).unwrap();
    assert_eq!(h.manager.get(id).unwrap().status, Status::Cancelled);

    // Deletion happens once the engine has released the file handle, which is why
    // this waits instead of asserting straight away: Windows will not unlink a
    // file that is still open.
    eventually("the scratch files to be deleted", || {
        scratch_files(&h.temp).is_empty()
    })
    .await;

    let entry = h.manager.get(id).unwrap();
    assert_eq!(entry.status, Status::Cancelled);
    assert!(!entry.resumable, "there is nothing left to resume from");
    assert_eq!(entry.speed_bps, 0.0);
}

// ---------------------------------------------------------------------- removing

#[tokio::test]
async fn removing_a_finished_download_can_keep_or_delete_the_file() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = quick_server(64 * 1024).await;

    let keep = h
        .manager
        .add(NewDownload::new(server.url("keep.zip").parse().unwrap()));
    let kept = until(&h.manager, keep, "completion", |e| e.status == Status::Completed).await;
    let kept_path = kept.path.unwrap();

    h.manager.remove(keep, false).unwrap();
    assert!(h.manager.get(keep).is_none(), "the row is gone");
    assert!(kept_path.exists(), "remove(false) must not delete the download");

    let drop = h
        .manager
        .add(NewDownload::new(server.url("drop.zip").parse().unwrap()));
    let dropped = until(&h.manager, drop, "completion", |e| e.status == Status::Completed).await;
    let dropped_path = dropped.path.unwrap();

    h.manager.remove(drop, true).unwrap();
    assert!(h.manager.get(drop).is_none());
    assert!(!dropped_path.exists(), "remove(true) must delete the download");
}

#[tokio::test]
async fn removing_a_running_download_waits_for_the_file_handle() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = slow_server().await;

    let id = h
        .manager
        .add(NewDownload::new(server.url("gone.bin").parse().unwrap()));
    until(&h.manager, id, "bytes to move", |e| e.downloaded > 0).await;

    // Returns immediately; the row disappears once the engine has stopped.
    h.manager.remove(id, true).unwrap();
    eventually("the row to disappear", || h.manager.get(id).is_none()).await;

    assert!(
        scratch_files(&h.temp).is_empty(),
        "an orphaned .part with no row to resume it is litter the user cannot find: {:?}",
        scratch_files(&h.temp)
    );
}

// ------------------------------------------------------------------ the list API

#[tokio::test]
async fn the_list_keeps_the_order_downloads_were_added_in() {
    let h = harness(1);
    let server = quick_server(32 * 1024).await;

    let ids: Vec<_> = (0..5)
        .map(|i| {
            h.manager
                .add(NewDownload::new(server.url(&format!("f{i}.bin")).parse().unwrap()))
        })
        .collect();

    assert_eq!(
        h.manager.list().iter().map(|e| e.id).collect::<Vec<_>>(),
        ids,
        "a HashMap iteration order would shuffle the user's list on every render"
    );

    for id in &ids {
        until(&h.manager, *id, "completion", |e| e.status == Status::Completed).await;
    }
    assert_eq!(h.manager.clear_finished(), 5);
    assert!(h.manager.list().is_empty());
}

#[tokio::test]
async fn events_describe_every_transition() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    let server = TestServer::start(128 * 1024, 32 * 1024, Duration::from_millis(20))
        .await
        .unwrap();
    let mut events = h.manager.subscribe();

    let id = h
        .manager
        .add(NewDownload::new(server.url("watched.zip").parse().unwrap()));

    let mut saw_added = false;
    let mut saw_downloading = false;
    let mut saw_completed = false;
    let mut saw_removed = false;

    let watch = async {
        while let Ok(event) = events.recv().await {
            match event {
                fdm_manager::Event::Added(e) => {
                    assert_eq!(e.id, id);
                    assert_eq!(e.status, Status::Queued);
                    saw_added = true;
                }
                fdm_manager::Event::Changed(e) => match e.status {
                    Status::Downloading => {
                        // The filename must be known by now: a row with no name
                        // while bytes move is the thing StartInfo exists to fix.
                        assert_eq!(e.filename, "watched.zip");
                        assert!(e.path.is_some());
                        saw_downloading = true;
                    }
                    Status::Completed => saw_completed = true,
                    _ => {}
                },
                fdm_manager::Event::Removed(gone) => {
                    assert_eq!(gone, id);
                    saw_removed = true;
                    return;
                }
            }
            if saw_completed && !saw_removed {
                h.manager.remove(id, true).unwrap();
            }
        }
    };

    tokio::time::timeout(Duration::from_secs(30), watch)
        .await
        .expect("timed out watching events");

    assert!(saw_added, "no Added event");
    assert!(saw_downloading, "no Downloading event");
    assert!(saw_completed, "no Completed event");
    assert!(saw_removed, "no Removed event");
}

// ---------------------------------------------------------------- the queue

#[tokio::test]
async fn max_active_bounds_how_many_run_at_once() {
    let h = harness(2);
    let server = TestServer::start(128 * 1024, 8 * 1024, Duration::from_millis(50))
        .await
        .unwrap();

    for i in 0..6 {
        h.manager
            .add(NewDownload::new(server.url(&format!("q{i}.bin")).parse().unwrap()));
    }

    // Sample repeatedly: a single check could miss an over-run that happens
    // between two transitions.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut peak = 0usize;
    let mut finished = 0usize;
    while tokio::time::Instant::now() < deadline {
        let list = h.manager.list();
        let running = list
            .iter()
            .filter(|e| matches!(e.status, Status::Connecting | Status::Downloading))
            .count();
        peak = peak.max(running);
        finished = list.iter().filter(|e| e.status == Status::Completed).count();
        assert!(running <= 2, "{running} downloads running with max_active = 2");
        if finished == 6 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    assert_eq!(finished, 6, "not every queued download finished");
    assert!(peak >= 2, "the queue never used both slots (peak {peak})");
}

#[tokio::test]
async fn pause_all_then_resume_all_finishes_everything() {
    let h = harness(2);
    let server = slow_server().await;

    let ids: Vec<_> = (0..4)
        .map(|i| {
            h.manager
                .add(NewDownload::new(server.url(&format!("all{i}.bin")).parse().unwrap()))
        })
        .collect();

    until(&h.manager, ids[0], "the first to start", |e| e.downloaded > 0).await;

    assert_eq!(h.manager.pause_all(), 4);
    assert!(h.manager.list().iter().all(|e| e.status == Status::Paused));

    assert_eq!(h.manager.resume_all(), 4);
    for id in &ids {
        let done = until(&h.manager, *id, "completion", |e| e.status == Status::Completed).await;
        assert_eq!(
            std::fs::read(done.path.unwrap()).unwrap(),
            expected_body(SLOW_TOTAL),
            "a file paused and resumed in bulk is corrupt"
        );
    }
    assert!(scratch_files(&h.temp).is_empty());
}

// ------------------------------------------------------------------ persistence

#[tokio::test]
async fn the_list_survives_a_restart_and_resumes_where_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Downloads");
    let temp = dir.path().join("Temp");
    let store_path = dir.path().join("downloads.json");

    let server = slow_server().await;
    let url = server.url("survivor.bin");

    // ---- first session: start it, pause it, quit the app ----
    let id = {
        let url = url.clone();
        app_session(
            config(&root, &temp),
            store_path.clone(),
            DEFAULT_MAX_ACTIVE,
            move |manager| async move {
                let id = manager.add(NewDownload::new(url.parse().unwrap()));
                until(&manager, id, "bytes to move", |e| e.downloaded > 0).await;
                manager.pause(id).unwrap();
                until(&manager, id, "pause", |e| e.status == Status::Paused).await;
                id
            },
        )
        .await
    };

    // ---- second session: a fresh manager over the same store ----
    let manager = Manager::new(
        Engine::new(config(&root, &temp)).unwrap(),
        Store::new(&store_path),
        DEFAULT_MAX_ACTIVE,
    );

    let restored = manager.get(id).expect("the download was not restored");
    assert_eq!(restored.status, Status::Paused);
    assert_eq!(restored.url, url);
    assert_eq!(restored.filename, "survivor.bin");
    assert!(
        restored.path.is_some(),
        "without the saved path a restored row cannot find its .part file"
    );
    assert!(restored.downloaded > 0 && restored.downloaded < SLOW_TOTAL as u64);

    // The headers were deliberately not saved, so this is an anonymous retry —
    // fine for this server, and the documented trade-off for not writing cookies
    // to disk.
    manager.resume(id).unwrap();
    let done = until(&manager, id, "completion after restart", |e| {
        e.status == Status::Completed
    })
    .await;

    assert_eq!(
        std::fs::read(done.path.unwrap()).unwrap(),
        expected_body(SLOW_TOTAL),
        "a download resumed after a restart is corrupt"
    );
    assert!(scratch_files(&temp).is_empty());
}

#[tokio::test]
async fn a_download_killed_mid_flight_comes_back_paused_not_downloading() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Downloads");
    let temp = dir.path().join("Temp");
    let store_path = dir.path().join("downloads.json");

    let server = slow_server().await;
    let url = server.url("crash.bin");

    // No pause and no cancel: the runtime is killed under a running download,
    // which is what a crash or a taskkill looks like to the store.
    let id = {
        let url = url.clone();
        app_session(
            config(&root, &temp),
            store_path.clone(),
            DEFAULT_MAX_ACTIVE,
            move |manager| async move {
                let id = manager.add(NewDownload::new(url.parse().unwrap()));
                until(&manager, id, "bytes to move", |e| e.downloaded > 0).await;
                id
            },
        )
        .await
    };

    let manager = Manager::new(
        Engine::new(config(&root, &temp)).unwrap(),
        Store::new(&store_path),
        DEFAULT_MAX_ACTIVE,
    );

    let restored = manager.get(id).unwrap();
    assert_eq!(
        restored.status,
        Status::Paused,
        "a row that claims to be downloading with no task behind it shows a frozen \
         speed and a stop button that stops nothing"
    );
    assert_eq!(restored.speed_bps, 0.0);
    assert_eq!(restored.active_connections, 0);

    // And it is genuinely recoverable, which is the only reason Paused is the
    // honest status to show.
    manager.resume(id).unwrap();
    let done = until(&manager, id, "completion after the crash", |e| {
        e.status == Status::Completed
    })
    .await;
    assert_eq!(
        std::fs::read(done.path.unwrap()).unwrap(),
        expected_body(SLOW_TOTAL),
        "a download recovered from a crash is corrupt"
    );
}

// ------------------------------------------------------------------- failure

#[tokio::test]
async fn a_failed_download_records_why_and_can_be_retried() {
    let h = harness(DEFAULT_MAX_ACTIVE);
    // Nothing is listening on this port.
    let id = h
        .manager
        .add(NewDownload::new("http://127.0.0.1:1/nope.bin".parse().unwrap()));

    let failed = until(&h.manager, id, "failure", |e| e.status == Status::Failed).await;
    assert!(
        failed.error.is_some_and(|e| !e.is_empty()),
        "a failed row with no reason gives the user nothing to act on"
    );
    assert_eq!(failed.speed_bps, 0.0);

    // Retry is offered even when it will fail again: the alternative is a dead
    // row for what is usually a transient network problem.
    assert!(h.manager.resume(id).is_ok());
    until(&h.manager, id, "the retry to fail too", |e| e.status == Status::Failed).await;
}
