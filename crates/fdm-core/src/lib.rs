//! FDM engine library.
//!
//! Phase 1 of the build: a segmented, resumable, multi-connection HTTP
//! downloader with no UI and no browser coupling. Keeping it a standalone
//! library means the hard part can be tested on its own before the native
//! messaging host, the Tauri UI, or the installer exist.
//!
//! ```no_run
//! use fdm_core::{CancelToken, DownloadRequest, Engine, EngineConfig};
//!
//! # async fn example() -> fdm_core::Result<()> {
//! let engine = Engine::new(EngineConfig::default())?;
//! let request = DownloadRequest::new("https://example.com/big.iso".parse()?);
//!
//! let outcome = engine
//!     .download(request, CancelToken::new(), |p| {
//!         println!("{} / {:?}", p.downloaded, p.total);
//!     })
//!     .await?;
//!
//! println!("saved to {}", outcome.path.display());
//! # Ok(())
//! # }
//! ```

pub mod categorize;
pub mod config;
pub mod download;
pub mod error;
pub mod headers;
pub mod naming;
pub mod plan;
pub mod probe;
pub mod progress;
pub mod scratch;
pub mod state;
pub mod writer;

pub use categorize::Category;
pub use config::{default_download_root, default_temp_dir, EngineConfig};
pub use download::{CancelToken, DownloadOutcome, DownloadRequest, Engine, StartInfo};
pub use error::{Error, Result};
pub use headers::sanitize as sanitize_headers;
pub use plan::{Plan, Segment, SegmentSnapshot};
pub use probe::RemoteInfo;
pub use progress::{format_bytes, format_duration, format_speed, ProgressSnapshot};
pub use state::DownloadState;

/// Re-exported so callers can populate [`DownloadRequest::headers`] without
/// taking their own `reqwest` dependency. Two crates on different `reqwest`
/// versions get two incompatible `HeaderMap` types, and the resulting error
/// ("expected HeaderMap, found HeaderMap") is famously unhelpful.
pub use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
