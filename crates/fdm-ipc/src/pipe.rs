//! The named pipe. Windows only.
//!
//! Everything interesting about a session is in [`crate::session`] and
//! [`crate::client`]; this module only has to be right about the pipe. What that
//! means in practice:
//!
//! - **A new instance before every handler.** A named pipe server serves one
//!   client per instance. The next instance has to exist before the current
//!   connection is handed off, or a client that connects in the gap gets
//!   `ERROR_FILE_NOT_FOUND` and concludes FDM is not running.
//! - **`first_pipe_instance(true)`.** Two purposes at once: nothing else can
//!   squat the name while FDM owns it, and the failure to create the first
//!   instance *is* the single-instance check — see [`BindError::AlreadyRunning`].
//! - **`reject_remote_clients(true)`.** Named pipes are reachable over SMB by
//!   default, as `\\host\pipe\name`. Nothing about this pipe should be reachable
//!   from another machine.
//! - **The user's SID in the name and in the DACL.** See [`crate::security`].

use std::io;
use std::sync::{Arc, OnceLock};

use fdm_manager::Manager;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, PipeMode, ServerOptions};

use crate::client::{Client, ClientError};
use crate::security::{current_user_sid, SecurityDescriptor};
use crate::session;

/// `ERROR_ACCESS_DENIED` — what `FILE_FLAG_FIRST_PIPE_INSTANCE` returns when an
/// instance of the name already exists.
const ERROR_ACCESS_DENIED: i32 = 5;
/// `ERROR_PIPE_BUSY` — the server exists but every instance is taken.
const ERROR_PIPE_BUSY: i32 = 231;

/// How long to keep trying when every pipe instance is momentarily busy.
///
/// The window is small — the server creates the next instance immediately after
/// accepting — but it is real under a burst of "download all links". Retrying is
/// much better than the alternative, which would be `fdm-host` deciding FDM is not
/// running and downloading into its own private list.
const BUSY_RETRIES: u32 = 10;
const BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(50);

/// The pipe FDM listens on, for this user.
///
/// The SID suffix makes "is FDM already running?" a per-user question, which is
/// what it should be: under fast user switching two people have two lists, two
/// `downloads.json` files and two trays, and a shared pipe name would let the
/// second one's downloads land in the first one's window.
pub fn pipe_name() -> io::Result<String> {
    static NAME: OnceLock<String> = OnceLock::new();
    if let Some(name) = NAME.get() {
        return Ok(name.clone());
    }
    let sid = current_user_sid()?;
    let name = format!(r"\\.\pipe\fdm.manager.{sid}");
    Ok(NAME.get_or_init(|| name).clone())
}

/// Why the server could not take the pipe.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// Another FDM already owns the pipe.
    ///
    /// This is the single-instance check, and it is a *feature* rather than a
    /// failure: the caller should hand its command-line arguments to the running
    /// copy, raise its window, and exit — the same thing IDM does when its icon is
    /// double-clicked twice.
    ///
    /// The underlying error is `ERROR_ACCESS_DENIED`, which in theory could also
    /// mean a genuine permission problem. In practice it cannot: any process may
    /// create a pipe in the pipe namespace, and the only name this could collide
    /// with contains the current user's own SID, so a competing creator is either
    /// another FDM or a process already running as this user — inside the trust
    /// boundary either way.
    #[error("FDM is already running")]
    AlreadyRunning,

    #[error("could not create the FDM pipe: {0}")]
    Io(#[from] io::Error),
}

/// A bound pipe with the next instance already created.
pub struct Server {
    name: String,
    security: SecurityDescriptor,
    manager: Arc<Manager>,
    /// The instance waiting for the next client. Always present: an accept
    /// replaces it before the accepted connection is handed to a task.
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl Server {
    /// Take the pipe, or discover that another FDM already has it.
    ///
    /// Must be called from inside a Tokio runtime — the pipe is registered with the
    /// reactor as it is created.
    pub fn bind(manager: Arc<Manager>) -> Result<Self, BindError> {
        Self::bind_named(&pipe_name()?, manager)
    }

    /// [`Server::bind`] on a name of the caller's choosing.
    ///
    /// The name is a parameter so the pipe layer can be tested on a private name
    /// instead of fighting the developer's own running FDM for the real one. It is
    /// also what a future portable mode would need — two copies of FDM on one
    /// machine, deliberately, each with its own list.
    pub fn bind_named(name: &str, manager: Arc<Manager>) -> Result<Self, BindError> {
        let security = SecurityDescriptor::owner_only(&current_user_sid()?)?;

        let next = create(name, &security, true).map_err(|e| {
            if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
                BindError::AlreadyRunning
            } else {
                BindError::Io(e)
            }
        })?;

        tracing::info!(pipe = %name, "listening for FDM clients");
        Ok(Self {
            name: name.to_string(),
            security,
            manager,
            next,
        })
    }

    /// The pipe's full name, for logs and for a bug report.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Accept connections until something goes wrong with the pipe itself.
    ///
    /// Never returns `Ok`. An `Err` means a new instance could not be created,
    /// which is fatal to the IPC layer but *not* to FDM: the app keeps its
    /// downloads and its window, it merely stops hearing from the browser
    /// extension. The caller should log it and carry on rather than exit.
    pub async fn run(mut self) -> io::Result<std::convert::Infallible> {
        loop {
            self.next.connect().await?;

            // The next instance has to exist before this connection is served.
            // Doing it the other way round leaves a window in which the pipe has
            // no listening instance, and a client that arrives in that window is
            // told the pipe does not exist — indistinguishable, from its side,
            // from FDM not running at all.
            let connected = std::mem::replace(
                &mut self.next,
                create(&self.name, &self.security, false)?,
            );

            let manager = Arc::clone(&self.manager);
            tokio::spawn(async move {
                if let Err(e) = session::serve(connected, manager).await {
                    tracing::debug!(error = %e, "ipc session ended badly");
                }
            });
        }
    }
}

fn create(
    name: &str,
    security: &SecurityDescriptor,
    first: bool,
) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let mut attrs = security.attributes();
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        // Byte mode, not message mode. Message mode would not remove the need for
        // the length prefix — a read can still return a partial message — so it
        // would be a second framing scheme layered under the one that actually
        // does the work.
        .pipe_mode(PipeMode::Byte);

    // SAFETY: `attrs` is a valid `SECURITY_ATTRIBUTES` whose descriptor is owned by
    // `security`, which outlives this call. `CreateNamedPipe` copies what it needs.
    unsafe {
        options.create_with_security_attributes_raw(name, std::ptr::addr_of_mut!(attrs).cast())
    }
}

/// Bind and serve, for a caller that has nothing else to say about it.
pub async fn serve_forever(manager: Arc<Manager>) -> Result<std::convert::Infallible, BindError> {
    Ok(Server::bind(manager)?.run().await?)
}

/// Connect to the running FDM and complete the handshake.
///
/// [`ClientError::NotRunning`] is the ordinary case, not an error to report: it
/// means no desktop app is listening, and the caller should do the work itself.
/// `fdm-host` depends on that — someone with Chrome open and FDM closed still
/// expects a click to download something.
pub async fn connect() -> Result<Client<NamedPipeClient>, ClientError> {
    connect_to(&pipe_name().map_err(ClientError::Io)?).await
}

/// [`connect`] to a name of the caller's choosing. See [`Server::bind_named`].
pub async fn connect_to(name: &str) -> Result<Client<NamedPipeClient>, ClientError> {
    let mut attempt = 0;
    let stream = loop {
        match ClientOptions::new().open(name) {
            Ok(stream) => break stream,
            // No server. Not worth retrying — the app is closed.
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ClientError::NotRunning)
            }
            // Every instance busy: the server is there, so wait for it.
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < BUSY_RETRIES => {
                attempt += 1;
                tokio::time::sleep(BUSY_WAIT).await;
            }
            Err(e) => return Err(ClientError::Io(e)),
        }
    };

    Client::handshake(stream).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_name_is_per_user_and_stable() {
        let first = pipe_name().unwrap();
        assert!(first.starts_with(r"\\.\pipe\fdm.manager.S-1-"), "{first}");
        // Both halves of FDM call this function, so the second call has to agree
        // with the first — including across the `OnceLock` cache.
        assert_eq!(first, pipe_name().unwrap());
    }

    /// The security boundary, checked against the kernel rather than against the
    /// struct that was passed in.
    ///
    /// Everything else in this crate could be right and this still wrong — a
    /// descriptor that failed to apply would leave a pipe with the *default* ACL,
    /// which grants Everyone read access. That would not break a single test that
    /// only ever connects as the current user, which is precisely why it is worth
    /// asking the object itself.
    #[tokio::test]
    async fn the_pipe_carries_the_dacl_that_was_asked_for() {
        use std::os::windows::io::AsRawHandle;

        let sid = current_user_sid().unwrap();
        let security = SecurityDescriptor::owner_only(&sid).unwrap();
        let name = format!(r"\\.\pipe\fdm.dacl-test.{}", std::process::id());
        let pipe = create(&name, &security, true).expect("create the pipe");

        // SAFETY: the handle belongs to `pipe`, which is alive for the call, and
        // the creator of a pipe holds READ_CONTROL on it.
        let sddl = unsafe {
            crate::security::object_dacl_sddl(pipe.as_raw_handle().cast()).expect("read the DACL")
        };

        assert!(
            sddl.contains(&sid),
            "the pipe's DACL must name this user: {sddl}"
        );
        // `WD` is Everyone and `AU` is Authenticated Users. Either one appearing
        // means the descriptor did not take and the pipe fell back to the default
        // ACL, which grants Everyone read.
        assert!(!sddl.contains(";WD)"), "Everyone must not be on the ACL: {sddl}");
        assert!(
            !sddl.contains(";AU)"),
            "Authenticated Users must not be on the ACL: {sddl}"
        );
        // Protected, so nothing is inherited into it either.
        assert!(sddl.starts_with("D:P"), "{sddl}");
    }
}
