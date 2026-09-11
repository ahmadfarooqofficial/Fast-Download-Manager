//! Does a video download actually report progress?
//!
//! This exists because it did not, and nothing caught it. `--print` implies
//! `--quiet` in yt-dlp, quiet suppresses the progress lines, and the whole
//! progress pipeline downstream was therefore fed nothing: the popup sat on
//! "Connecting to server…" for an entire download and then jumped to complete.
//! Unit tests over the parser could not see it, because the parser was fine —
//! the bytes never arrived.
//!
//! So this test runs the real yt-dlp against the real YouTube and asserts that
//! bytes and an ETA reach the registry. It needs the network and the tools
//! folder, so it is `#[ignore]`d and run deliberately:
//!
//!     cargo test -p fdm-manager --test video_progress -- --ignored --nocapture

use std::time::{Duration, Instant};

use fdm_core::{Engine, EngineConfig};
use fdm_manager::{Manager, NewDownload, Status, Store};

/// "Me at the zoo" — 19 seconds long, and the oldest still-playable video on
/// the site, so it is about as stable a fixture as YouTube offers.
const SHORT_VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits the network and needs yt-dlp in the tools folder"]
async fn a_video_download_reports_bytes_and_an_eta() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = EngineConfig {
        download_root: dir.path().to_path_buf(),
        temp_dir: dir.path().join("tmp"),
        // Keep the fixture small and quick; quality is not what is under test.
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg).unwrap();
    let store = Store::new(dir.path().join("downloads.json"));
    let manager = Manager::new(engine, store, 2);

    let mut events = manager.subscribe();

    let url = url::Url::parse(SHORT_VIDEO).unwrap();
    let id = manager.add(NewDownload::new(url));

    let mut saw_bytes = false;
    let mut saw_eta = false;
    let mut saw_stage = false;
    let mut finished = false;

    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline && !(finished && saw_bytes) {
        let remaining = deadline - Instant::now();
        let Ok(Ok(event)) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        let entry = match event {
            fdm_manager::Event::Added(e) | fdm_manager::Event::Changed(e) => e,
            fdm_manager::Event::Removed(_) => continue,
        };
        if entry.id != id {
            continue;
        }
        if entry.stage.is_some() {
            saw_stage = true;
        }
        if entry.downloaded > 0 {
            saw_bytes = true;
        }
        if entry.eta_secs.is_some() {
            saw_eta = true;
        }
        if matches!(entry.status, Status::Completed | Status::Failed) {
            finished = true;
            assert_eq!(
                entry.status,
                Status::Completed,
                "download failed: {:?}",
                entry.error
            );
        }
    }

    // The regression: bytes must arrive *during* the download, not only as the
    // final size written once it is over.
    assert!(
        saw_bytes,
        "no progress ever reached the registry — yt-dlp is being run in a mode \
         that suppresses it (this is the --print/--quiet bug)"
    );
    assert!(
        saw_eta,
        "bytes arrived but no ETA did — the progress line's own colons are \
         being mis-split again"
    );
    assert!(
        saw_stage,
        "no extraction stage was reported, so the UI has nothing to show while \
         yt-dlp resolves the video"
    );
    assert!(finished, "download never reached a terminal state");
}
