//! What the pipe must actually do, tested without a pipe.
//!
//! Every test here runs a real [`Manager`] over a real `tokio::io::duplex` pair —
//! the same [`serve`](fdm_ipc::session::serve) and the same [`Client`] the named
//! pipe uses, with only the two bytes-in-bytes-out halves swapped out. That is the
//! whole reason both sides are generic over the stream: the handshake, the dispatch
//! table, the event fan-out and the "one client stops reading" case are all
//! reachable here, in milliseconds, on any platform.
//!
//! Nothing here touches the network. Downloads are pointed at port 9 (discard),
//! which refuses instantly, because these tests are about the transport and not
//! about the engine — `crates/fdm-manager/tests/list.rs` is where transfers are
//! tested for real.

use std::sync::Arc;
use std::time::Duration;

use fdm_core::{Engine, EngineConfig};
use fdm_ipc::client::Client;
use fdm_ipc::wire::{AddRequest, Call, ErrorKind, EventMessage, Reply, Request, ServerMessage};
use fdm_ipc::{frame, session, ClientError, PROTOCOL_VERSION};
use fdm_manager::{Manager, Store};
use tokio::io::DuplexStream;

/// Enough for several frames without a partial write, small enough that a client
/// which stops reading actually backs the server up.
const PIPE_BUFFER: usize = 64 * 1024;

/// Long enough that a slow machine is not a failing machine, short enough that a
/// genuinely missing event fails the test instead of hanging the suite.
const PATIENCE: Duration = Duration::from_secs(5);

/// A URL that fails to connect immediately. These tests need rows in the list, not
/// bytes on disk.
fn dead_url(name: &str) -> String {
    format!("http://127.0.0.1:9/{name}")
}

struct Harness {
    manager: Arc<Manager>,
    root: std::path::PathBuf,
    temp: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Downloads");
    let temp = dir.path().join("Temp");

    let cfg = EngineConfig {
        download_root: root.clone(),
        temp_dir: temp.clone(),
        max_connections: 4,
        // A connection to a closed port should be a failed row straight away, not
        // a row that retries for the length of the test.
        max_retries: 0,
        connect_timeout: Duration::from_millis(200),
        ..EngineConfig::default()
    };
    let store = Store::new(dir.path().join("downloads.json"));
    let manager = Arc::new(Manager::new(Engine::new(cfg).unwrap(), store, 4));

    Harness {
        manager,
        root,
        temp,
        _dir: dir,
    }
}

/// A connected, greeted client talking to `manager` over a duplex pair.
async fn connect(manager: &Arc<Manager>) -> Client<DuplexStream> {
    let (theirs, ours) = tokio::io::duplex(PIPE_BUFFER);
    tokio::spawn(session::serve(ours, Arc::clone(manager)));
    Client::handshake(theirs).await.expect("handshake")
}

/// A raw stream with the server attached but no handshake performed, for the tests
/// that are about the handshake itself.
fn raw(manager: &Arc<Manager>) -> DuplexStream {
    let (theirs, ours) = tokio::io::duplex(PIPE_BUFFER);
    tokio::spawn(session::serve(ours, Arc::clone(manager)));
    theirs
}

async fn reply_to(stream: &mut DuplexStream, seq: u64, request: Request) -> ServerMessage {
    frame::write_json(stream, &Call { seq, request })
        .await
        .unwrap();
    tokio::time::timeout(PATIENCE, frame::read_json::<_, ServerMessage>(stream))
        .await
        .expect("the server must answer")
        .unwrap()
        .expect("the server must not just hang up")
}

/// Wait for an event matching `want`, ignoring the progress and status churn of
/// downloads failing against a closed port.
async fn wait_for<F>(client: &mut Client<DuplexStream>, want: F) -> EventMessage
where
    F: Fn(&EventMessage) -> bool,
{
    tokio::time::timeout(PATIENCE, async {
        loop {
            match client.next_event().await.expect("event stream") {
                Some(event) if want(&event) => return event,
                Some(_) => continue,
                None => panic!("the server closed the connection before the event arrived"),
            }
        }
    })
    .await
    .expect("the event never arrived")
}

fn add(url: &str) -> AddRequest {
    AddRequest {
        url: url.to_string(),
        ..AddRequest::default()
    }
}

// ---------------------------------------------------------------- the handshake

#[tokio::test]
async fn the_welcome_describes_the_app_that_owns_the_downloads() {
    // `fdm-host` forwards these straight into the extension's `pong`, so they have
    // to come from the running app rather than from whatever defaults the client
    // was built with.
    let h = harness();
    let client = connect(&h.manager).await;
    let welcome = client.server();

    assert_eq!(welcome.download_root, h.root);
    assert_eq!(welcome.temp_dir, h.temp);
    assert_eq!(welcome.max_active, 4);
    assert_eq!(welcome.max_connections, 4);
    assert_eq!(welcome.pid, std::process::id());
    assert!(!welcome.version.is_empty());
}

#[tokio::test]
async fn nothing_is_answered_before_hello() {
    let h = harness();
    let mut stream = raw(&h.manager);

    let ServerMessage::Reply { seq, reply } = reply_to(&mut stream, 7, Request::List).await else {
        panic!("expected a reply");
    };
    assert_eq!(seq, 7, "a refusal still has to answer the request it refuses");
    let Reply::Error { kind, message } = reply else {
        panic!("a listing before hello must be refused");
    };
    assert_eq!(kind, ErrorKind::NotReady);
    assert!(message.contains("hello"), "{message}");
}

#[tokio::test]
async fn a_client_from_a_different_build_is_told_what_to_do() {
    // The half-upgraded install: the installer replaced `fdm-host.exe` while the
    // old `fdm-desktop.exe` was still running.
    let h = harness();
    let mut stream = raw(&h.manager);

    let ServerMessage::Reply { reply, .. } =
        reply_to(&mut stream, 1, Request::Hello { protocol: 999 }).await
    else {
        panic!("expected a reply");
    };
    let Reply::Error { kind, message } = reply else {
        panic!("a version mismatch must be refused");
    };
    assert_eq!(kind, ErrorKind::VersionMismatch);
    assert!(message.contains("start it again"), "{message}");
}

#[tokio::test]
async fn the_client_reports_a_version_mismatch_as_a_refusal() {
    // The same case from the other side: `Client::handshake` must fail rather than
    // hand back a client that will misbehave three requests later.
    let h = harness();
    let (theirs, ours) = tokio::io::duplex(PIPE_BUFFER);
    tokio::spawn(session::serve(ours, Arc::clone(&h.manager)));

    // Speak the protocol by hand, because the real client cannot be made to lie
    // about its version — which is the point.
    let mut stream = theirs;
    frame::write_json(
        &mut stream,
        &Call {
            seq: 1,
            request: Request::Hello { protocol: 999 },
        },
    )
    .await
    .unwrap();
    let msg: ServerMessage = frame::read_json(&mut stream).await.unwrap().unwrap();
    assert!(matches!(
        msg,
        ServerMessage::Reply {
            reply: Reply::Error {
                kind: ErrorKind::VersionMismatch,
                ..
            },
            ..
        }
    ));
}

#[tokio::test]
async fn a_second_hello_is_answered_rather_than_fatal() {
    let h = harness();
    let mut client = connect(&h.manager).await;

    let reply = client
        .request(Request::Hello {
            protocol: PROTOCOL_VERSION,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Welcome { .. }));

    // And the connection is still good afterwards.
    assert!(client.list().await.unwrap().is_empty());
}

// ------------------------------------------------------------------- the list

#[tokio::test]
async fn a_download_added_over_the_wire_lands_in_the_shared_list() {
    // The whole reason this crate exists: one list, reachable from another process.
    let h = harness();
    let mut client = connect(&h.manager).await;

    let id = client.add(add(&dead_url("one.bin"))).await.unwrap();

    let entry = h.manager.get(id).expect("the row must be in the manager's list");
    assert_eq!(entry.id, id);
    assert!(entry.url.contains("one.bin"));
    assert_eq!(h.manager.list().len(), 1);

    // And the client sees the same row the manager does.
    let over_the_wire = client.get(id).await.unwrap().expect("visible over IPC too");
    assert_eq!(over_the_wire.id, id);
}

#[tokio::test]
async fn a_url_that_is_not_a_download_is_refused_without_adding_a_row() {
    let h = harness();
    let mut client = connect(&h.manager).await;

    let err = client
        .add(add("file:///C:/Windows/System32/config/SAM"))
        .await
        .expect_err("a local file read must not be reachable over the pipe");
    match err {
        ClientError::Refused { kind, .. } => assert_eq!(kind, ErrorKind::Invalid),
        other => panic!("expected a refusal, got {other:?}"),
    }

    // The refusal has to happen before the row exists, not after.
    assert!(h.manager.list().is_empty());
}

#[tokio::test]
async fn asking_about_a_row_that_is_gone_is_a_question_not_an_error() {
    let h = harness();
    let mut client = connect(&h.manager).await;
    assert!(client.get(4242).await.unwrap().is_none());
}

#[tokio::test]
async fn acting_on_a_row_that_is_gone_says_so() {
    let h = harness();
    let mut client = connect(&h.manager).await;

    for err in [
        client.pause(4242).await.unwrap_err(),
        client.resume(4242).await.unwrap_err(),
        client.cancel(4242).await.unwrap_err(),
    ] {
        match err {
            ClientError::Refused { kind, .. } => assert_eq!(kind, ErrorKind::NotFound),
            other => panic!("expected not-found, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_bulk_request_reports_how_many_rows_it_touched() {
    let h = harness();
    let mut client = connect(&h.manager).await;

    for i in 0..3 {
        client.add(add(&dead_url(&format!("{i}.bin")))).await.unwrap();
    }

    let Reply::Count { n } = client.request(Request::PauseAll).await.unwrap() else {
        panic!("pauseAll must report a count");
    };
    // How many were still pausable is a race against three downloads failing
    // against a closed port, so the assertion is on the shape, not the number.
    assert!(n <= 3, "cannot have paused more rows than exist: {n}");
}

// ------------------------------------------------------------------- the events

#[tokio::test]
async fn the_snapshot_and_the_subscription_are_one_request() {
    // A row added before subscribing must be in the snapshot: a UI that started
    // late still has to show the downloads that were already running.
    let h = harness();
    let mut client = connect(&h.manager).await;
    let id = client.add(add(&dead_url("early.bin"))).await.unwrap();

    let snapshot = client.subscribe().await.unwrap();
    assert!(
        snapshot.iter().any(|d| d.id == id),
        "the subscription's snapshot must include rows that already existed"
    );
}

#[tokio::test]
async fn a_subscriber_sees_what_another_client_does() {
    // Two connections, which is the real arrangement: the desktop window watching
    // while `fdm-host` hands over a download from the browser.
    let h = harness();
    let mut watcher = connect(&h.manager).await;
    let mut adder = connect(&h.manager).await;

    watcher.subscribe().await.unwrap();
    let id = adder.add(add(&dead_url("watched.bin"))).await.unwrap();

    let event = wait_for(&mut watcher, |e| {
        matches!(e, EventMessage::Added(d) if d.id == id)
    })
    .await;
    let EventMessage::Added(entry) = event else {
        unreachable!()
    };
    assert!(entry.url.contains("watched.bin"));
}

#[tokio::test]
async fn a_removal_reaches_every_subscriber() {
    // The event that cannot be recovered from by waiting: a missed `Removed` leaves
    // a row on screen for ever.
    let h = harness();
    let mut first = connect(&h.manager).await;
    let mut second = connect(&h.manager).await;
    let mut adder = connect(&h.manager).await;

    first.subscribe().await.unwrap();
    second.subscribe().await.unwrap();

    let id = adder.add(add(&dead_url("doomed.bin"))).await.unwrap();
    adder.remove(id, false).await.unwrap();

    for client in [&mut first, &mut second] {
        wait_for(client, |e| matches!(e, EventMessage::Removed { id: got } if *got == id)).await;
    }
}

#[tokio::test]
async fn subscribing_twice_does_not_double_every_event() {
    let h = harness();
    let mut client = connect(&h.manager).await;

    client.subscribe().await.unwrap();
    client.subscribe().await.unwrap();

    let id = client.add(add(&dead_url("once.bin"))).await.unwrap();
    wait_for(&mut client, |e| {
        matches!(e, EventMessage::Added(d) if d.id == id)
    })
    .await;

    // A second `Added` for the same id would mean two forwarders are running. Any
    // other event is fine — a failing download produces plenty.
    let more = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            match client.next_event().await.unwrap() {
                Some(EventMessage::Added(d)) if d.id == id => return true,
                Some(_) => continue,
                None => return false,
            }
        }
    })
    .await;
    assert!(
        more.is_err() || more == Ok(false),
        "the same row was announced twice, so two forwarders are running"
    );
}

// ----------------------------------------------------------------- misbehaviour

#[tokio::test]
async fn one_unreadable_request_does_not_cost_a_client_its_connection() {
    // A garbled request inside a well-formed frame leaves the framing in step, so
    // the connection is still usable. Dropping it would take a subscriber's event
    // stream with it.
    let h = harness();
    let (mut stream, ours) = tokio::io::duplex(PIPE_BUFFER);
    tokio::spawn(session::serve(ours, Arc::clone(&h.manager)));

    frame::write_json(
        &mut stream,
        &Call {
            seq: 1,
            request: Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
        },
    )
    .await
    .unwrap();
    let _welcome: ServerMessage = frame::read_json(&mut stream).await.unwrap().unwrap();

    frame::write_frame(&mut stream, b"{ this is not a request }")
        .await
        .unwrap();
    let ServerMessage::Reply { reply, .. } = frame::read_json(&mut stream).await.unwrap().unwrap()
    else {
        panic!("expected a reply");
    };
    assert!(matches!(
        reply,
        Reply::Error {
            kind: ErrorKind::Invalid,
            ..
        }
    ));

    // Still talking.
    let ServerMessage::Reply { seq, reply } = reply_to(&mut stream, 3, Request::List).await else {
        panic!("expected a reply");
    };
    assert_eq!(seq, 3);
    assert!(matches!(reply, Reply::List { .. }));
}

#[tokio::test]
async fn a_client_that_hangs_up_mid_conversation_is_not_the_servers_problem() {
    let h = harness();
    let (theirs, ours) = tokio::io::duplex(PIPE_BUFFER);
    let server = tokio::spawn(session::serve(ours, Arc::clone(&h.manager)));

    let mut client = Client::handshake(theirs).await.unwrap();
    client.subscribe().await.unwrap();
    drop(client);

    // The session has to finish on its own — including the forwarder task it
    // spawned, which has no other way of learning the client is gone.
    let outcome = tokio::time::timeout(PATIENCE, server)
        .await
        .expect("the session must end when the client does")
        .expect("and must not panic");
    assert!(outcome.is_ok(), "a hang-up is an ordinary ending: {outcome:?}");

    // And the manager is untouched and still usable by everyone else.
    let mut next = connect(&h.manager).await;
    assert!(next.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_subscriber_that_stops_reading_does_not_stall_anyone_else() {
    // The hung-UI case. A client that subscribes and then never reads must not be
    // able to hold up the download list or another connection.
    let h = harness();
    let (deaf, ours) = tokio::io::duplex(1024);
    tokio::spawn(session::serve(ours, Arc::clone(&h.manager)));
    let mut deaf = Client::handshake(deaf).await.unwrap();
    deaf.subscribe().await.unwrap();

    let mut working = connect(&h.manager).await;
    working.subscribe().await.unwrap();

    // Enough traffic to fill the silent client's outbox several times over.
    let mut last = 0;
    for i in 0..40 {
        last = working.add(add(&dead_url(&format!("{i}.bin")))).await.unwrap();
    }

    // The other client still gets its events, and the manager still answers.
    wait_for(&mut working, |e| {
        matches!(e, EventMessage::Added(d) if d.id == last)
    })
    .await;
    assert_eq!(h.manager.list().len(), 40);

    // Deliberately not dropped earlier: the point is that it was still connected
    // and still not reading for the whole test.
    drop(deaf);
}
