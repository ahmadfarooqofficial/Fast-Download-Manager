//! The server side of one connection.
//!
//! Generic over the stream rather than tied to a named pipe, for two reasons. The
//! smaller one is that a pipe handle is awkward to fake. The larger one is that
//! *all* of the interesting behaviour is here — the handshake, the dispatch table,
//! the event fan-out, what happens when a client stops reading — and it can be
//! tested in full over `tokio::io::duplex`, on any platform, in milliseconds. The
//! named pipe in `pipe.rs` is then a thin thing that only has to be right about
//! the pipe.
//!
//! # Shape of a session
//!
//! ```text
//!  reader loop ──► dispatch ──► manager ──┐
//!                                         ├──► outbox (mpsc) ──► writer task ──► stream
//!  broadcast ──► forwarder task ──────────┘
//! ```
//!
//! One task owns the write half. It has to be exactly one: a reply and a pushed
//! event that interleave halfway through a length-prefixed frame produce a stream
//! neither end can resynchronise.

use std::sync::Arc;

use fdm_manager::{Manager, NewDownload};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::frame;
use crate::wire::{
    AddRequest, Call, ErrorKind, EventMessage, Reply, Request, ServerMessage, PROTOCOL_VERSION,
};

/// How many messages may be queued for one client before the server gives up on
/// it.
///
/// Progress events arrive several times a second per download, so a client that
/// has stopped reading — a hung UI, a debugger paused on a breakpoint — would
/// otherwise grow this queue without limit. Dropping the slow client is the right
/// answer: the download list is not the client's memory, and it can reconnect and
/// ask for a fresh snapshot.
const OUTBOX_DEPTH: usize = 256;

/// Serve one connection until the client disconnects.
///
/// Returns `Ok(())` for every ordinary ending, including a client that hangs up
/// mid-request. An `Err` means the stream itself misbehaved — a frame that could
/// not be parsed as a length prefix — which is worth a log line and nothing more.
pub async fn serve<S>(stream: S, manager: Arc<Manager>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (outbox, mut inbox) = mpsc::channel::<ServerMessage>(OUTBOX_DEPTH);

    // The single writer. Ends when every sender is dropped, or when the client
    // stops reading — at which point there is no one to report the failure to, so
    // it just stops.
    let pump = tokio::spawn(async move {
        while let Some(msg) = inbox.recv().await {
            if let Err(e) = frame::write_json(&mut writer, &msg).await {
                if !frame::is_disconnect(&e) {
                    tracing::debug!(error = %e, "could not write to the client");
                }
                break;
            }
        }
        // Drain, so nothing blocks on a send into a channel nobody reads.
        inbox.close();
        while inbox.recv().await.is_some() {}
    });

    let result = converse(&mut reader, &outbox, &manager).await;

    // Dropping the last sender is what tells the writer to finish.
    drop(outbox);
    let _ = pump.await;
    result
}

/// Handshake, then requests until the client goes away.
async fn converse<R>(
    reader: &mut R,
    outbox: &mpsc::Sender<ServerMessage>,
    manager: &Arc<Manager>,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    // Forwards broadcast events onto this connection. `None` until the client
    // subscribes, because most clients — `fdm-host` handing over one download —
    // never do.
    let mut forwarder: Option<tokio::task::JoinHandle<()>> = None;
    let mut greeted = false;

    let outcome = loop {
        let call: Call = match frame::read_json(reader).await {
            Ok(Some(call)) => call,
            // Clean disconnect.
            Ok(None) => break Ok(()),
            Err(e) if frame::is_disconnect(&e) => break Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                // Unreadable JSON inside a well-formed frame. The framing is
                // still in step, so answer and keep the connection: one bad
                // request should not cost a client its subscription. There is no
                // `seq` to answer with, so 0 is used — a client correlating
                // strictly will ignore it, and a client that just wants the log
                // line gets it.
                let _ = outbox
                    .send(ServerMessage::Reply {
                        seq: 0,
                        reply: Reply::error(ErrorKind::Invalid, format!("unreadable request: {e}")),
                    })
                    .await;
                continue;
            }
            // A length prefix that made no sense: the stream is out of step and
            // cannot be recovered.
            Err(e) => break Err(e),
        };

        let seq = call.seq;

        // Nothing is answered before the versions agree. A build mismatch that
        // showed up as a confusing `NotFound` three requests later would be far
        // harder to diagnose than a refusal on the first one.
        if !greeted {
            match call.request {
                Request::Hello { protocol } if protocol == PROTOCOL_VERSION => {
                    greeted = true;
                    let reply = welcome(manager);
                    if !send(outbox, seq, reply).await {
                        break Ok(());
                    }
                    continue;
                }
                Request::Hello { protocol } => {
                    tracing::warn!(theirs = protocol, ours = PROTOCOL_VERSION, "ipc version mismatch");
                    send(outbox, seq, Reply::version_mismatch(protocol)).await;
                    break Ok(());
                }
                _ => {
                    send(
                        outbox,
                        seq,
                        Reply::error(
                            ErrorKind::NotReady,
                            "send hello before anything else on this connection",
                        ),
                    )
                    .await;
                    break Ok(());
                }
            }
        }

        let reply = dispatch(call.request, manager, outbox, &mut forwarder).await;
        if !send(outbox, seq, reply).await {
            break Ok(());
        }
    };

    // The subscription outlives neither the connection nor this function. Its
    // only sender is `outbox`, and `outbox` is owned by the caller, so the task
    // will not notice the client has gone on its own — it has to be told.
    if let Some(handle) = forwarder {
        handle.abort();
    }
    outcome
}

/// Queue a reply for the writer task.
///
/// `false` means the client is gone. The undelivered message is dropped rather
/// than handed back, because nothing upstream can do anything with a reply that has
/// no one to read it — and carrying a whole `ServerMessage` back through every
/// caller's error path just to drop it there is worse than dropping it here.
async fn send(outbox: &mpsc::Sender<ServerMessage>, seq: u64, reply: Reply) -> bool {
    outbox
        .send(ServerMessage::Reply { seq, reply })
        .await
        .is_ok()
}

fn welcome(manager: &Manager) -> Reply {
    let cfg = manager.engine_config();
    Reply::Welcome {
        protocol: PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        download_root: cfg.download_root.clone(),
        temp_dir: cfg.temp_dir.clone(),
        use_temp_dir: cfg.use_temp_dir,
        max_active: manager.max_active(),
        max_connections: cfg.max_connections,
    }
}

async fn dispatch(
    request: Request,
    manager: &Arc<Manager>,
    outbox: &mpsc::Sender<ServerMessage>,
    forwarder: &mut Option<tokio::task::JoinHandle<()>>,
) -> Reply {
    match request {
        // A second hello is harmless and answering it is friendlier than
        // dropping the connection over it.
        Request::Hello { protocol } if protocol == PROTOCOL_VERSION => welcome(manager),
        Request::Hello { protocol } => Reply::version_mismatch(protocol),

        Request::Add(add) => match new_download(*add) {
            Ok(new) => Reply::Added {
                id: manager.add(new),
            },
            Err(message) => Reply::error(ErrorKind::Invalid, message),
        },

        Request::Pause { id } => ack(manager.pause(id)),
        Request::Resume { id } => ack(manager.resume(id)),
        Request::Cancel { id } => ack(manager.cancel(id)),
        Request::Remove { id, delete_file } => ack(manager.remove(id, delete_file)),

        Request::PauseAll => Reply::Count {
            n: manager.pause_all(),
        },
        Request::ResumeAll => Reply::Count {
            n: manager.resume_all(),
        },
        Request::ClearFinished => Reply::Count {
            n: manager.clear_finished(),
        },

        Request::List => Reply::List {
            downloads: manager.list(),
        },
        Request::Get { id } => Reply::Entry {
            download: manager.get(id),
        },

        Request::Subscribe => {
            // Subscribed *before* the snapshot is taken, so nothing can slip
            // through the gap. See `Request::Subscribe`'s documentation.
            let events = manager.subscribe();
            let downloads = manager.list();

            if let Some(old) = forwarder.take() {
                // Asked twice. Replace rather than add: two forwarders would
                // deliver every event twice, and the client's second snapshot
                // makes the first subscription redundant anyway.
                old.abort();
            }
            let snapshot = {
                let manager = Arc::clone(manager);
                move || manager.list()
            };
            *forwarder = Some(tokio::spawn(forward_events(
                events,
                outbox.clone(),
                snapshot,
            )));

            Reply::List { downloads }
        }
    }
}

/// Turn a `Result<(), ManagerError>` into a reply, since most requests either
/// worked or named a row that could not do what was asked.
fn ack(result: fdm_manager::Result<()>) -> Reply {
    match result {
        Ok(()) => Reply::Done,
        Err(e) => e.into(),
    }
}

/// Pump the manager's broadcast onto one connection.
///
/// Takes a `snapshot` closure rather than the `Manager` itself. The only thing
/// this needs the manager for is the resync below, and asking for "a way to get the
/// current list" instead of "the download list" is both narrower and testable: the
/// lag path can be exercised with a canned snapshot and a two-slot channel, which
/// is the only realistic way to reach it on purpose.
async fn forward_events<F>(
    mut events: tokio::sync::broadcast::Receiver<fdm_manager::Event>,
    outbox: mpsc::Sender<ServerMessage>,
    snapshot: F,
) where
    F: Fn() -> Vec<fdm_manager::DownloadEntry> + Send + 'static,
{
    use tokio::sync::broadcast::error::RecvError;

    loop {
        let message = match events.recv().await {
            Ok(event) => EventMessage::from(event),
            Err(RecvError::Lagged(missed)) => {
                // This client was too slow and the broadcast overwrote events it
                // had not read. Send the whole list rather than pretending
                // nothing happened: the dangerous loss is a `Removed`, which no
                // later event will ever correct.
                tracing::debug!(missed, "client lagged; sending a full resync");
                EventMessage::Resync {
                    downloads: snapshot(),
                }
            }
            // The manager is gone, which means the process is shutting down.
            Err(RecvError::Closed) => return,
        };

        if outbox.send(ServerMessage::Event(message)).await.is_err() {
            return; // connection closed
        }
    }
}

/// Validate an [`AddRequest`] and turn it into what the manager takes.
///
/// The scheme check is the security boundary and is not a duplicate of the one in
/// `fdm-host`: this is the path a *different process* can reach, so it cannot
/// assume anything about what already validated the request. `file:` is the one
/// that matters — accepting it would turn "add a download" into an arbitrary
/// local file read, with the result written wherever the caller asked.
fn new_download(add: AddRequest) -> Result<NewDownload, String> {
    let url: url::Url = add
        .url
        .parse()
        .map_err(|e| format!("not a valid URL: {e}"))?;

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("FDM only handles http and https, not {other}:")),
    }
    if !url.has_host() {
        return Err("that URL has no host".into());
    }

    let mut new = NewDownload::new(url).with_headers(fdm_core::sanitize_headers(&add.headers));
    if let Some(name) = add.filename.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        new = new.with_filename(name);
    }
    if let Some(dir) = add.target_dir.filter(|d| !d.as_os_str().is_empty()) {
        new = new.with_target_dir(dir);
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_schemes_that_are_not_downloads() {
        // The same list `fdm-host` refuses, checked again here because this is a
        // separate entry point and a separate process can reach it.
        for bad in [
            "file:///C:/Windows/System32/config/SAM",
            "data:text/plain,hello",
            "blob:https://example.com/1234",
            "ftp://example.com/x",
            "javascript:alert(1)",
        ] {
            let add = AddRequest {
                url: bad.into(),
                ..AddRequest::default()
            };
            assert!(new_download(add).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn strips_headers_the_engine_owns() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("Cookie".to_string(), "keep=me".to_string());
        // The one that silently corrupts a segmented download.
        headers.insert("Accept-Encoding".to_string(), "gzip".to_string());

        let new = new_download(AddRequest {
            url: "https://example.com/a.bin".into(),
            headers,
            ..AddRequest::default()
        })
        .unwrap();

        assert!(new.headers.get("accept-encoding").is_none());
        assert_eq!(new.headers.get("cookie").unwrap(), "keep=me");
    }

    #[test]
    fn a_blank_filename_is_the_same_as_none() {
        // What a caller sends when the browser had no name for it. Passing " "
        // through would make the engine skip its own `Content-Disposition`
        // parsing and save a file called nothing.
        let new = new_download(AddRequest {
            url: "https://example.com/a.bin".into(),
            filename: Some("   ".into()),
            ..AddRequest::default()
        })
        .unwrap();
        assert!(new.filename.is_none());
    }

    #[test]
    fn an_explicit_target_dir_survives() {
        let new = new_download(AddRequest {
            url: "https://example.com/a.bin".into(),
            target_dir: Some("D:\\somewhere".into()),
            ..AddRequest::default()
        })
        .unwrap();
        assert_eq!(new.target_dir.unwrap(), std::path::Path::new("D:\\somewhere"));
    }

    /// The path that only happens when a client stalls, and the one that would be
    /// invisible if it were wrong.
    ///
    /// Forced deterministically rather than by racing: the receiver is created
    /// first, five events are pushed through a two-slot channel while nothing is
    /// reading, and only then is the forwarder started. Its first `recv` cannot be
    /// anything but `Lagged`.
    #[tokio::test]
    async fn a_client_that_fell_behind_is_sent_the_whole_list() {
        use fdm_manager::Event;

        let (tx, rx) = tokio::sync::broadcast::channel::<Event>(2);
        for id in 1..=5 {
            tx.send(Event::Removed(id)).unwrap();
        }

        let (outbox, mut inbox) = mpsc::channel(8);
        // The snapshot the resync should carry. A closure, so no Manager is needed
        // to test this.
        tokio::spawn(forward_events(rx, outbox, Vec::new));

        let first = inbox.recv().await.expect("the forwarder must say something");
        let ServerMessage::Event(EventMessage::Resync { .. }) = first else {
            panic!("expected a resync, got {first:?}");
        };

        // And then it carries on rather than giving up on the client.
        //
        // Not necessarily the *next* message: after a lag, a broadcast receiver
        // resumes at the oldest value still retained, so the resync is followed by
        // whatever survived in the channel. That is harmless — every event is an
        // idempotent "replace the row with this id" — but it is why this waits
        // rather than asserting on one message.
        tx.send(Event::Removed(9)).unwrap();
        loop {
            match inbox.recv().await.expect("the forwarder must keep going") {
                ServerMessage::Event(EventMessage::Removed { id: 9 }) => break,
                ServerMessage::Event(EventMessage::Removed { .. }) => continue,
                other => panic!("unexpected message after a resync: {other:?}"),
            }
        }
    }

    /// Closing the broadcast is how process shutdown reaches the forwarder.
    #[tokio::test]
    async fn the_forwarder_stops_when_the_manager_goes_away() {
        let (tx, rx) = tokio::sync::broadcast::channel::<fdm_manager::Event>(4);
        let (outbox, mut inbox) = mpsc::channel(4);
        let task = tokio::spawn(forward_events(rx, outbox, Vec::new));

        drop(tx);
        task.await.expect("the forwarder must return, not panic");
        // The outbox sender was dropped with the task, so the channel is closed
        // rather than merely empty — which is what lets `serve` finish.
        assert!(inbox.recv().await.is_none());
    }
}
