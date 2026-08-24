//! The client side of one connection.
//!
//! Generic over the stream, like [`crate::session`], so the whole request/reply
//! and event-stream behaviour is exercised over `tokio::io::duplex` in the tests
//! and only the pipe itself is Windows-specific.
//!
//! Deliberately **not** a connection pool or a background actor. A client is one
//! stream owned by one caller, and `request` is `&mut self`: the reply to a
//! request is the next reply frame on the wire, and that is only true if one
//! caller is asking at a time. Anything wanting concurrent access should hold
//! several clients, or put this one behind a mutex and accept the serialisation
//! it implies.

use tokio::io::{AsyncRead, AsyncWrite};

use crate::frame;
use crate::wire::{
    AddRequest, Call, ErrorKind, EventMessage, Reply, Request, ServerMessage, PROTOCOL_VERSION,
};
use fdm_manager::{DownloadEntry, DownloadId};

/// Everything that can go wrong talking to the download list.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No FDM is listening. The caller's cue to fall back to doing the work
    /// itself — for `fdm-host`, that means downloading in-process.
    #[error("FDM is not running")]
    NotRunning,

    #[error("the connection to FDM failed: {0}")]
    Io(#[from] std::io::Error),

    /// FDM answered, and the answer was no.
    #[error("{message}")]
    Refused { kind: ErrorKind, message: String },

    /// A reply arrived that does not answer the request that was sent. Only
    /// reachable if two callers share one client, which is exactly what the
    /// `&mut self` on [`Client::request`] exists to prevent.
    #[error("FDM replied out of turn (expected #{expected}, got #{got})")]
    OutOfTurn { expected: u64, got: u64 },

    /// The reply was well-formed but the wrong kind — a `Count` where an `Added`
    /// was due. A protocol bug on one side or the other.
    #[error("FDM replied with {got} where {expected} was expected")]
    Unexpected {
        expected: &'static str,
        got: &'static str,
    },

    #[error("FDM closed the connection without answering")]
    Closed,
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// A connected, greeted client.
pub struct Client<S> {
    stream: S,
    seq: u64,
    /// What the server said about itself in reply to `Hello`.
    welcome: Welcome,
}

/// Hand-written rather than derived, because the stream type has no reason to be
/// `Debug` and requiring it would leak into every caller. What is worth printing is
/// which server this is talking to anyway.
impl<S> std::fmt::Debug for Client<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("server_pid", &self.welcome.pid)
            .field("server_version", &self.welcome.version)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

/// The server's self-description, kept from the handshake so callers do not have
/// to ask again.
#[derive(Debug, Clone)]
pub struct Welcome {
    pub version: String,
    pub pid: u32,
    pub download_root: std::path::PathBuf,
    pub temp_dir: std::path::PathBuf,
    pub use_temp_dir: bool,
    pub max_active: usize,
    pub max_connections: u32,
}

impl<S> Client<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Perform the handshake and return a client ready to use.
    ///
    /// The handshake is not optional and not deferred: a version mismatch has to
    /// surface here, before a caller has made a decision — such as telling the
    /// browser a download was accepted — that it cannot take back.
    pub async fn handshake(mut stream: S) -> Result<Self> {
        let call = Call {
            seq: 1,
            request: Request::Hello {
                protocol: PROTOCOL_VERSION,
            },
        };
        frame::write_json(&mut stream, &call).await?;

        let mut client = Self {
            stream,
            seq: 1,
            // Replaced below. Present only because the read borrows `self`.
            welcome: Welcome {
                version: String::new(),
                pid: 0,
                download_root: std::path::PathBuf::new(),
                temp_dir: std::path::PathBuf::new(),
                use_temp_dir: false,
                max_active: 0,
                max_connections: 0,
            },
        };

        match client.await_reply(1).await? {
            Reply::Welcome {
                version,
                pid,
                download_root,
                temp_dir,
                use_temp_dir,
                max_active,
                max_connections,
                ..
            } => {
                client.welcome = Welcome {
                    version,
                    pid,
                    download_root,
                    temp_dir,
                    use_temp_dir,
                    max_active,
                    max_connections,
                };
                Ok(client)
            }
            Reply::Error { kind, message } => Err(ClientError::Refused { kind, message }),
            other => Err(ClientError::Unexpected {
                expected: "welcome",
                got: name_of(&other),
            }),
        }
    }

    pub fn server(&self) -> &Welcome {
        &self.welcome
    }

    /// Send a request and wait for its reply, skipping any pushed events.
    ///
    /// Events that arrive while a reply is outstanding are dropped, not buffered.
    /// A caller that wants events uses [`Client::subscribe`] and then
    /// [`Client::next_event`]; a caller that just wants to add a download does not
    /// want a queue of progress updates growing behind its back.
    pub async fn request(&mut self, request: Request) -> Result<Reply> {
        self.seq += 1;
        let seq = self.seq;
        frame::write_json(&mut self.stream, &Call { seq, request }).await?;
        self.await_reply(seq).await
    }

    /// Read frames until the reply to `seq` arrives.
    async fn await_reply(&mut self, seq: u64) -> Result<Reply> {
        loop {
            match frame::read_json::<_, ServerMessage>(&mut self.stream).await {
                Ok(Some(ServerMessage::Reply { seq: got, reply })) if got == seq => {
                    return Ok(reply)
                }
                Ok(Some(ServerMessage::Reply { seq: got, .. })) => {
                    return Err(ClientError::OutOfTurn { expected: seq, got })
                }
                Ok(Some(ServerMessage::Event(_))) => continue,
                Ok(None) => return Err(ClientError::Closed),
                Err(e) if frame::is_disconnect(&e) => return Err(ClientError::Closed),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Add a download to the list and return its id.
    pub async fn add(&mut self, add: AddRequest) -> Result<DownloadId> {
        match self.request(Request::Add(Box::new(add))).await? {
            Reply::Added { id } => Ok(id),
            other => Err(expected("added", other)),
        }
    }

    pub async fn pause(&mut self, id: DownloadId) -> Result<()> {
        self.done(Request::Pause { id }).await
    }

    pub async fn resume(&mut self, id: DownloadId) -> Result<()> {
        self.done(Request::Resume { id }).await
    }

    pub async fn cancel(&mut self, id: DownloadId) -> Result<()> {
        self.done(Request::Cancel { id }).await
    }

    pub async fn remove(&mut self, id: DownloadId, delete_file: bool) -> Result<()> {
        self.done(Request::Remove { id, delete_file }).await
    }

    pub async fn list(&mut self) -> Result<Vec<DownloadEntry>> {
        match self.request(Request::List).await? {
            Reply::List { downloads } => Ok(downloads),
            other => Err(expected("list", other)),
        }
    }

    pub async fn get(&mut self, id: DownloadId) -> Result<Option<DownloadEntry>> {
        match self.request(Request::Get { id }).await? {
            Reply::Entry { download } => Ok(download),
            other => Err(expected("entry", other)),
        }
    }

    /// Ask for the event stream. Returns the snapshot it starts from — see
    /// [`Request::Subscribe`] for why those are one request.
    pub async fn subscribe(&mut self) -> Result<Vec<DownloadEntry>> {
        match self.request(Request::Subscribe).await? {
            Reply::List { downloads } => Ok(downloads),
            other => Err(expected("list", other)),
        }
    }

    /// Wait for the next pushed event.
    ///
    /// `Ok(None)` means the server closed the connection, which for a subscriber
    /// is how a session ends rather than an error. Only meaningful after
    /// [`Client::subscribe`]; before it, this waits forever, because the server
    /// has nothing unsolicited to say.
    pub async fn next_event(&mut self) -> Result<Option<EventMessage>> {
        loop {
            match frame::read_json::<_, ServerMessage>(&mut self.stream).await {
                Ok(Some(ServerMessage::Event(event))) => return Ok(Some(event)),
                // A stray reply, from a request whose caller stopped waiting.
                // Nothing here can use it.
                Ok(Some(ServerMessage::Reply { .. })) => continue,
                Ok(None) => return Ok(None),
                Err(e) if frame::is_disconnect(&e) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn done(&mut self, request: Request) -> Result<()> {
        match self.request(request).await? {
            Reply::Done => Ok(()),
            Reply::Error { kind, message } => Err(ClientError::Refused { kind, message }),
            other => Err(expected("done", other)),
        }
    }
}

fn expected(what: &'static str, got: Reply) -> ClientError {
    match got {
        Reply::Error { kind, message } => ClientError::Refused { kind, message },
        other => ClientError::Unexpected {
            expected: what,
            got: name_of(&other),
        },
    }
}

fn name_of(reply: &Reply) -> &'static str {
    match reply {
        Reply::Welcome { .. } => "welcome",
        Reply::Added { .. } => "added",
        Reply::Done => "done",
        Reply::Count { .. } => "count",
        Reply::List { .. } => "list",
        Reply::Entry { .. } => "entry",
        Reply::Error { .. } => "error",
    }
}
