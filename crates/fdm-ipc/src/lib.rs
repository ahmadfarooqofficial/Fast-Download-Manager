//! Talking to the running FDM.
//!
//! FDM is two programs that have to agree about one list. `fdm-desktop.exe` owns
//! the downloads — the list, the engine, `downloads.json` — and `fdm-host.exe` is
//! the small process Chrome starts to hand over a URL. Without an IPC layer the
//! host would download into its own private world: a file would appear on disk,
//! but the app's window would never know about it, and pausing it from the UI
//! would be impossible. This crate is the pipe between them.
//!
//! ```text
//!  Chrome ──stdio──► fdm-host.exe ──named pipe──► fdm-desktop.exe
//!                          │                          (Manager)
//!                          └── no answer? download in-process
//! ```
//!
//! That fallback is not a nicety. Someone whose browser is open and whose FDM is
//! closed still expects a click to download something, so `fdm-host` treats
//! [`ClientError::NotRunning`] as "do it myself", not as a failure.
//!
//! # What is in here
//!
//! - [`wire`] — the JSON messages, and the protocol version that makes a
//!   half-upgraded install say "restart FDM" instead of failing to parse.
//! - [`frame`] — length-prefixed framing. A pipe is a byte stream even in message
//!   mode, so the prefix is the only thing that says where a message ends.
//! - [`session`] — the server side of one connection: handshake, dispatch, event
//!   fan-out. Generic over the stream, so all of it is tested over
//!   `tokio::io::duplex` rather than against a real pipe.
//! - [`client`] — the other side of the same conversation, generic for the same
//!   reason.
//! - [`pipe`] — Windows only. The named pipe itself, and the single-instance check
//!   that falls out of owning its name.
//!
//! # Who may connect
//!
//! The pipe carries `Cookie` headers and can start a download to any folder the
//! user can write to, so it is restricted to this user's own processes: a DACL
//! naming the user's SID and `SYSTEM`, `reject_remote_clients(true)` so it is not
//! reachable over SMB, and the user's SID in the pipe name so two people logged
//! into one machine get two pipes. See [`pipe`] for the details.

pub mod client;
pub mod frame;
pub mod session;
pub mod wire;

#[cfg(windows)]
mod security;

#[cfg(windows)]
pub mod pipe;

pub use client::{Client, ClientError, Welcome};
pub use frame::MAX_FRAME;
pub use wire::{AddRequest, ErrorKind, EventMessage, Reply, Request, PROTOCOL_VERSION};

#[cfg(windows)]
pub use pipe::{connect, pipe_name, serve_forever, BindError, Server};
