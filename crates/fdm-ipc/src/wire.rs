//! The JSON messages that travel over the pipe.
//!
//! Both ends of this wire are Rust in the same repository, shipped by the same
//! installer, so the shapes are plain structs rather than anything negotiated.
//! What they are *not* is unversioned: the installer replaces `fdm-desktop.exe`
//! while the old copy is still running, so a freshly-installed `fdm-host.exe` can
//! and will meet a server from the previous version. [`PROTOCOL_VERSION`] turns
//! that into "restart FDM" instead of a parse error.
//!
//! The client sends [`Call`]s and receives [`ServerMessage`]s. Replies carry the
//! `seq` they answer, so a client may have several in flight; pushed events carry
//! none, because they were not asked for.

use std::collections::BTreeMap;
use std::path::PathBuf;

use fdm_manager::{DownloadEntry, DownloadId, Event, ManagerError};
use serde::{Deserialize, Serialize};

/// Bumped only on a breaking change to the shapes in this module.
///
/// Separate from `fdm-host`'s extension protocol version on purpose: that one is
/// a contract with a Chrome Web Store artefact that updates on its own schedule,
/// this one is a contract between two files in the same installer. They move for
/// entirely different reasons.
pub const PROTOCOL_VERSION: u32 = 1;

/// One request, with the sequence number its reply will carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub seq: u64,
    pub request: Request,
}

/// Everything a client may ask the download list to do.
///
/// This is deliberately the same vocabulary as `fdm_manager::Manager`'s public
/// methods, one variant per method. A request that does not map onto a method is
/// a sign that logic is leaking into the transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// Must be the first message on a connection. Nothing else is answered until
    /// the versions agree.
    Hello { protocol: u32 },

    Add(Box<AddRequest>),

    Pause { id: DownloadId },
    Resume { id: DownloadId },
    Cancel { id: DownloadId },

    #[serde(rename_all = "camelCase")]
    Remove {
        id: DownloadId,
        /// Delete the file on disk as well as the row. The finished-file case is
        /// the one that matters: removing a row from the list is not the same
        /// gesture as throwing the download away.
        delete_file: bool,
    },

    PauseAll,
    ResumeAll,
    ClearFinished,

    List,
    Get { id: DownloadId },

    /// Start receiving [`ServerMessage::Event`] on this connection.
    ///
    /// Replies with [`Reply::List`] — the snapshot and the subscription are one
    /// request on purpose. `List` then `Subscribe` would drop anything that
    /// happened in between; `Subscribe` then `List` can resurrect a row that was
    /// removed in between. Doing both at once, with the subscription taken first,
    /// means a duplicate at worst, and every event is an idempotent "replace the
    /// row with this id".
    Subscribe,
}

/// A new download, as a client describes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub url: String,

    /// `Cookie`, `Referer` and `User-Agent` as the browser saw them.
    ///
    /// These do travel over the pipe, which is the entire point of the pipe: a
    /// download behind a login only works if the engine can present the same
    /// credentials the browser had. That is consistent with them never being
    /// written to `downloads.json` — the pipe is restricted to this user's own
    /// processes and holds a value for milliseconds; a JSON file holds it until
    /// somebody deletes it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    /// Left `None` so the engine can derive the name from `Content-Disposition`,
    /// which it parses more carefully than any caller will.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    /// Set only when the user picked a folder. `None` is what makes the engine
    /// sort into `Downloads\FDM\<Category>\`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<PathBuf>,

    /// Separate audio stream URL for YouTube adaptive formats.
    /// When set, the manager downloads video+audio in parallel and merges
    /// with ffmpeg instead of spawning yt-dlp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
}

/// Anything the server sends.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServerMessage {
    Reply { seq: u64, reply: Reply },
    /// Unsolicited. Only sent after a successful [`Request::Subscribe`].
    Event(EventMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "camelCase")]
pub enum Reply {
    /// Answer to `Hello`. Carries what a client needs in order to describe the
    /// running app without guessing — `fdm-host` forwards most of this straight
    /// into the extension's `pong`, and it must describe the process that owns
    /// the downloads, not the process that happened to answer the browser.
    #[serde(rename_all = "camelCase")]
    Welcome {
        protocol: u32,
        /// `fdm-desktop`'s crate version.
        version: String,
        /// Which process owns the list. Purely diagnostic, and the fastest way to
        /// tell a stale server from a fresh one in a bug report.
        pid: u32,
        download_root: PathBuf,
        temp_dir: PathBuf,
        use_temp_dir: bool,
        max_active: usize,
        max_connections: u32,
    },

    Added { id: DownloadId },

    /// Acknowledgement for a request with nothing to return.
    Done,

    /// How many rows a bulk request touched.
    Count { n: usize },

    List { downloads: Vec<DownloadEntry> },

    /// `None` rather than an error: "is this download still in the list?" is a
    /// question, and `None` is a legitimate answer to it.
    Entry { download: Option<DownloadEntry> },

    #[serde(rename_all = "camelCase")]
    Error {
        kind: ErrorKind,
        /// Already human-readable. A client may show it verbatim.
        message: String,
    },
}

/// Why a request failed, in the coarse categories a caller can actually act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// The two halves of FDM are from different builds. Nothing works until one
    /// of them restarts, so a client must not retry.
    VersionMismatch,
    /// A request arrived before `Hello`.
    NotReady,
    NotFound,
    /// The row exists but is in the wrong state — pausing something already
    /// finished, for instance. A UI that gets this offered a button it should
    /// have disabled.
    WrongState,
    /// The request itself was unusable: an unparseable URL, a scheme that is not
    /// a download.
    Invalid,
    Internal,
}

impl Reply {
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self::Error {
            kind,
            message: message.into(),
        }
    }

    pub fn version_mismatch(theirs: u32) -> Self {
        Self::error(
            ErrorKind::VersionMismatch,
            format!(
                "This copy of FDM speaks IPC v{PROTOCOL_VERSION} but the running FDM speaks \
                 v{theirs}. Quit FDM from its tray icon and start it again — the installer \
                 replaced the program while the old one was still running."
            ),
        )
    }
}

impl From<ManagerError> for Reply {
    fn from(e: ManagerError) -> Self {
        let kind = match &e {
            ManagerError::NotFound(_) => ErrorKind::NotFound,
            ManagerError::WrongState { .. } => ErrorKind::WrongState,
            // Only reachable when `downloads.json` was hand-edited, so it is a
            // bad row rather than a bad request — but the caller's options are
            // the same either way: report it and stop.
            ManagerError::BadUrl { .. } => ErrorKind::Invalid,
        };
        Self::error(kind, e.to_string())
    }
}

/// The list changed. Mirrors `fdm_manager::Event`.
///
/// A separate type rather than `serde` derives on `Event` itself: the manager is
/// a library that should not grow a wire format because one of its consumers
/// happens to be a pipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum EventMessage {
    Added(DownloadEntry),
    Changed(DownloadEntry),
    Removed { id: DownloadId },

    /// The server fell behind this client and dropped events, so it is sending
    /// the whole list instead. Replace local state with this wholesale.
    ///
    /// Not cosmetic: a missed `Changed` is corrected by the next one, but a missed
    /// `Removed` leaves a row on screen that no longer exists and never updates
    /// again. Progress events arrive several times a second per download, so a
    /// client that stalls briefly — a window being dragged, a renderer being
    /// throttled in the background — really does overrun the channel.
    Resync { downloads: Vec<DownloadEntry> },
}

impl From<Event> for EventMessage {
    fn from(e: Event) -> Self {
        match e {
            Event::Added(entry) => Self::Added(entry),
            Event::Changed(entry) => Self::Changed(entry),
            Event::Removed(id) => Self::Removed { id },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        serde_json::from_slice(&serde_json::to_vec(value).unwrap()).unwrap()
    }

    #[test]
    fn a_call_survives_the_wire() {
        let call = Call {
            seq: 42,
            request: Request::Remove {
                id: 7,
                delete_file: true,
            },
        };
        let back = round_trip(&call);
        assert_eq!(back.seq, 42);
        assert!(matches!(
            back.request,
            Request::Remove {
                id: 7,
                delete_file: true
            }
        ));
    }

    #[test]
    fn an_add_request_keeps_the_headers_that_make_logins_work() {
        let mut headers = BTreeMap::new();
        headers.insert("Cookie".into(), "session=abc".into());
        let back = round_trip(&Call {
            seq: 1,
            request: Request::Add(Box::new(AddRequest {
                url: "https://example.com/a.iso".into(),
                headers,
                ..AddRequest::default()
            })),
        });
        let Request::Add(add) = back.request else {
            panic!("expected an add");
        };
        assert_eq!(add.headers.get("Cookie").unwrap(), "session=abc");
        assert!(add.filename.is_none());
    }

    #[test]
    fn requests_without_fields_still_parse() {
        // `pauseAll` and friends carry nothing at all. An externally tagged enum
        // would need `{"pauseAll":null}` here; the internal tag is what keeps
        // this readable in a log.
        let raw = br#"{"seq":3,"request":{"op":"pauseAll"}}"#;
        let call: Call = serde_json::from_slice(raw).unwrap();
        assert!(matches!(call.request, Request::PauseAll));
    }

    #[test]
    fn omitted_optional_fields_are_not_a_parse_error() {
        // What `fdm-host` actually sends: a URL, headers, and nothing else.
        let raw = br#"{"seq":1,"request":{"op":"add","url":"https://example.com/x.bin"}}"#;
        let call: Call = serde_json::from_slice(raw).unwrap();
        let Request::Add(add) = call.request else {
            panic!("expected an add");
        };
        assert!(add.headers.is_empty());
        assert!(add.target_dir.is_none());
    }

    #[test]
    fn a_reply_is_distinguishable_from_a_pushed_event() {
        let reply = serde_json::to_string(&ServerMessage::Reply {
            seq: 9,
            reply: Reply::Added { id: 4 },
        })
        .unwrap();
        assert!(reply.contains(r#""kind":"reply""#));
        assert!(reply.contains(r#""seq":9"#));

        let event = serde_json::to_string(&ServerMessage::Event(EventMessage::Removed { id: 4 }))
            .unwrap();
        assert!(event.contains(r#""kind":"event""#));
        // No seq: nobody asked for it, so there is nothing to correlate against.
        assert!(!event.contains("seq"));
    }

    #[test]
    fn manager_errors_keep_their_category() {
        let not_found: Reply = ManagerError::NotFound(3).into();
        assert!(matches!(
            not_found,
            Reply::Error {
                kind: ErrorKind::NotFound,
                ..
            }
        ));

        let wrong: Reply = ManagerError::WrongState {
            id: 3,
            status: fdm_manager::Status::Completed,
            action: "pause",
        }
        .into();
        let Reply::Error { kind, message } = wrong else {
            panic!("expected an error");
        };
        assert_eq!(kind, ErrorKind::WrongState);
        // The message has to be worth showing, not just worth matching on.
        assert!(message.contains("Completed"), "{message}");
    }

    #[test]
    fn a_version_mismatch_says_what_to_do_about_it() {
        let Reply::Error { kind, message } = Reply::version_mismatch(99) else {
            panic!("expected an error");
        };
        assert_eq!(kind, ErrorKind::VersionMismatch);
        assert!(message.contains("start it again"), "{message}");
    }
}
