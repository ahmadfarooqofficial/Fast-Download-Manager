//! The pipe itself — the one layer `tests/session.rs` cannot reach.
//!
//! Everything here binds a real named pipe on a private name. What is being tested
//! is not the protocol (that is covered over `duplex` in `tests/session.rs`) but
//! the four things only a real pipe can be wrong about:
//!
//! 1. Nothing listening reads as "FDM is not running" rather than as a failure.
//! 2. A second server on the same name discovers the first — the single-instance
//!    check.
//! 3. The pipe keeps answering after the first client, which depends on the next
//!    instance being created before the current connection is served.
//! 4. Two clients at once are two conversations, not one interleaved mess.
//!
//! The name is private per test because the real name is per user, and a test that
//! took it would fight the developer's own running FDM — and lose in a way that
//! looked like a bug in the code.

#![cfg(windows)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fdm_core::{Engine, EngineConfig};
use fdm_ipc::pipe::{connect_to, BindError, Server};
use fdm_ipc::wire::AddRequest;
use fdm_ipc::ClientError;
use fdm_manager::{Manager, Store};

const PATIENCE: Duration = Duration::from_secs(5);

/// A pipe name no other test — or other run of this suite — will collide with.
fn private_name(what: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\fdm.test.{}.{what}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

struct Harness {
    manager: Arc<Manager>,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let cfg = EngineConfig {
        download_root: dir.path().join("Downloads"),
        temp_dir: dir.path().join("Temp"),
        max_retries: 0,
        connect_timeout: Duration::from_millis(200),
        ..EngineConfig::default()
    };
    let store = Store::new(dir.path().join("downloads.json"));
    Harness {
        manager: Arc::new(Manager::new(Engine::new(cfg).unwrap(), store, 4)),
        _dir: dir,
    }
}

/// A URL that fails to connect at once. These tests want rows, not bytes.
fn dead(name: &str) -> AddRequest {
    AddRequest {
        url: format!("http://127.0.0.1:9/{name}"),
        ..AddRequest::default()
    }
}

/// Bind and start serving, returning the name to connect to.
fn serving(h: &Harness, what: &str) -> String {
    let name = private_name(what);
    let server = Server::bind_named(&name, Arc::clone(&h.manager)).expect("bind");
    assert_eq!(server.name(), name);
    tokio::spawn(server.run());
    name
}

#[tokio::test]
async fn nothing_listening_means_fdm_is_not_running() {
    // The case `fdm-host` depends on. Anything other than `NotRunning` here and a
    // click in the browser fails outright instead of falling back to downloading
    // in-process.
    let err = connect_to(&private_name("absent"))
        .await
        .expect_err("nothing is listening on that name");
    assert!(
        matches!(err, ClientError::NotRunning),
        "expected NotRunning, got {err:?}"
    );
}

#[tokio::test]
async fn a_second_server_on_the_same_name_finds_the_first() {
    // This is the single-instance check. Double-clicking the FDM icon must raise
    // the existing window rather than start a second app with a second list.
    let h = harness();
    let name = private_name("single-instance");

    let _first = Server::bind_named(&name, Arc::clone(&h.manager)).expect("the first one binds");

    let second = Server::bind_named(&name, Arc::clone(&h.manager));
    match second {
        Err(BindError::AlreadyRunning) => {}
        Err(other) => panic!("expected AlreadyRunning, got {other:?}"),
        Ok(_) => panic!("two servers must not be able to own one name"),
    }
}

#[tokio::test]
async fn the_name_frees_up_when_the_server_goes_away() {
    // Quit and restart. If the name did not free up, a restarted FDM would report
    // itself as already running and refuse to start — the classic stale-lock bug.
    let h = harness();
    let name = private_name("restart");

    let first = Server::bind_named(&name, Arc::clone(&h.manager)).expect("bind");
    drop(first);

    Server::bind_named(&name, Arc::clone(&h.manager)).expect("the name must be free again");
}

#[tokio::test]
async fn a_download_handed_over_a_real_pipe_lands_in_the_list() {
    // The end-to-end shape of `fdm-host` handing a URL to the running app.
    let h = harness();
    let name = serving(&h, "handover");

    let mut client = connect_to(&name).await.expect("connect");
    assert_eq!(client.server().pid, std::process::id());

    let id = client.add(dead("handed-over.bin")).await.expect("add");

    let entry = h.manager.get(id).expect("the row is in the app's list");
    assert!(entry.url.contains("handed-over.bin"));
}

#[tokio::test]
async fn the_pipe_keeps_answering_after_the_first_client() {
    // The ordering bug this guards against is invisible with one client: if the
    // server served the accepted connection *before* creating the next instance,
    // the pipe would briefly have no listener and the second client would be told
    // the pipe does not exist — indistinguishable from FDM being closed.
    let h = harness();
    let name = serving(&h, "sequential");

    for i in 0..5 {
        let mut client = connect_to(&name)
            .await
            .unwrap_or_else(|e| panic!("client {i} could not connect: {e:?}"));
        client.add(dead(&format!("{i}.bin"))).await.unwrap();
        // Dropped here, so each connection is fully finished before the next
        // starts. That is the arrangement that exercises instance turnover.
    }

    assert_eq!(h.manager.list().len(), 5);
}

#[tokio::test]
async fn two_clients_at_once_are_two_conversations() {
    // A shared write half would let a reply and an event interleave inside one
    // length-prefixed frame, and the symptom would be exactly this test failing
    // to parse something.
    let h = harness();
    let name = serving(&h, "concurrent");

    let mut tasks = Vec::new();
    for i in 0..4 {
        let name = name.clone();
        tasks.push(tokio::spawn(async move {
            let mut client = connect_to(&name).await.expect("connect");
            let mut ids = Vec::new();
            for n in 0..3 {
                ids.push(client.add(dead(&format!("{i}-{n}.bin"))).await.expect("add"));
            }
            // Every id this client was given must be visible to it afterwards.
            for id in &ids {
                assert!(client.get(*id).await.expect("get").is_some());
            }
            ids
        }));
    }

    let mut all = Vec::new();
    for task in tasks {
        all.extend(
            tokio::time::timeout(PATIENCE, task)
                .await
                .expect("no client should hang")
                .expect("no client should panic"),
        );
    }

    // Twelve distinct rows: no id handed out twice, nothing lost.
    all.sort_unstable();
    let before = all.len();
    all.dedup();
    assert_eq!(before, 12);
    assert_eq!(all.len(), 12, "an id was handed to two clients");
    assert_eq!(h.manager.list().len(), 12);
}

#[tokio::test]
async fn a_subscriber_on_a_real_pipe_sees_another_clients_work() {
    // The desktop window watching while the browser hands over a download, over
    // the actual transport rather than a duplex pair.
    let h = harness();
    let name = serving(&h, "watch");

    let mut watcher = connect_to(&name).await.expect("connect");
    watcher.subscribe().await.expect("subscribe");

    let mut adder = connect_to(&name).await.expect("connect");
    let id = adder.add(dead("watched.bin")).await.expect("add");

    let found = tokio::time::timeout(PATIENCE, async {
        loop {
            match watcher.next_event().await.expect("event stream") {
                Some(fdm_ipc::EventMessage::Added(d)) if d.id == id => return true,
                Some(_) => continue,
                None => return false,
            }
        }
    })
    .await
    .expect("the event must arrive");
    assert!(found, "the server closed the stream instead of sending it");
}
