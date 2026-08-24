//! Errors the download list can return.
//!
//! Deliberately small. Almost everything that can go wrong with a download is an
//! `fdm_core::Error` recorded on the row, not a failure of the call that started
//! it — `add` cannot fail, and `pause` can only fail by naming a row that is not
//! there or not running.

use crate::model::{DownloadId, Status};

pub type Result<T> = std::result::Result<T, ManagerError>;

#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("no download with id {0}")]
    NotFound(DownloadId),

    /// The UI should not have offered the button. Returned rather than ignored so
    /// a mistake shows up in tests instead of as a control that quietly does
    /// nothing.
    #[error("cannot {action} download {id}: it is {status:?}")]
    WrongState {
        id: DownloadId,
        status: Status,
        action: &'static str,
    },

    /// A stored row whose URL no longer parses — only reachable if the JSON was
    /// hand-edited.
    #[error("download {id} has an unusable url: {url}")]
    BadUrl { id: DownloadId, url: String },
}
