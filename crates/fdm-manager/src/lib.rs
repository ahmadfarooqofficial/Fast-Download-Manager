//! FDM's download list.
//!
//! `fdm-core` is a downloader: give it a URL, get a file. This crate is the thing
//! IDM's main window actually shows — a persistent list of downloads that can be
//! queued, paused, resumed, retried and removed, with events for a UI to follow.
//!
//! It is a plain library on purpose. Every rule about pausing, cancelling and
//! cleaning up scratch files is testable here without compiling a GUI, which is
//! the difference between a download list that has been proven and one that has
//! only been clicked.
//!
//! ```no_run
//! use fdm_manager::{Manager, NewDownload};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let manager = Manager::with_defaults()?;
//! let mut events = manager.subscribe();
//!
//! let id = manager.add(NewDownload::new("https://example.com/big.iso".parse()?));
//!
//! while let Ok(event) = events.recv().await {
//!     println!("{event:?}");
//! }
//!
//! manager.pause(id)?;
//! manager.resume(id)?;
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod manager;
pub mod model;
pub mod store;

pub use error::{ManagerError, Result};
pub use manager::{Manager, DEFAULT_MAX_ACTIVE};
pub use model::{DownloadEntry, DownloadId, Event, NewDownload, Status};
pub use store::Store;

/// Re-exported so a UI crate can configure the engine without adding its own
/// `fdm-core` dependency.
pub use fdm_core::{Category, EngineConfig, HeaderMap};
