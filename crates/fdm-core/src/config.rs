use std::path::PathBuf;
use std::time::Duration;

/// Tuning knobs for the download engine. Every field here is intended to be
/// surfaced in the app's Settings screen — an IDM alternative is judged partly
/// on how much the user is allowed to control.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum simultaneous connections for a single download.
    ///
    /// IDM caps at 32. We default lower and grow adaptively, because a lot of
    /// CDNs throttle or 403 aggressive clients and a static high number is the
    /// wrong answer for both fast and hostile hosts.
    pub max_connections: u32,

    /// Never create a segment smaller than this. Below a few MiB the connection
    /// setup cost outweighs any parallelism gain.
    pub min_split_size: u64,

    /// Bytes each worker buffers in memory before issuing one positioned write.
    /// Coalescing writes keeps the disk from becoming the bottleneck at high
    /// segment counts and reduces file fragmentation.
    pub write_buffer: usize,

    /// Retry budget per segment before the whole download fails.
    pub max_retries: u32,

    pub connect_timeout: Duration,

    /// Idle timeout while streaming a body. Not a total-download timeout.
    pub read_timeout: Duration,

    pub user_agent: String,

    /// Root of the organized download tree, e.g. `C:\Users\me\Downloads\FDM`.
    pub download_root: PathBuf,

    /// Sort finished files into per-type subfolders under `download_root`.
    pub organize_by_type: bool,

    /// Where `.part` and `.fdm` files live while a download is running.
    ///
    /// Separated from `download_root` the way IDM separates its temporary
    /// directory: the download folder then only ever contains finished files.
    /// Surfaced in Settings, because a user with a small SSD and a large data
    /// drive has a real reason to move it.
    pub temp_dir: PathBuf,

    /// When false, partial data sits beside the finished file as
    /// `<name>.part` instead of in `temp_dir`.
    ///
    /// Worth keeping as an option: moving a finished 40 GB file off a different
    /// volume means copying all 40 GB, and someone downloading to an external
    /// disk may prefer to skip that.
    pub use_temp_dir: bool,

    /// How often progress callbacks fire, and how often the `.fdm` resume
    /// state is flushed to disk.
    pub progress_interval: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_connections: 64,
            min_split_size: 256 * 1024,
            write_buffer: 64 * 1024,
            max_retries: 10,
            connect_timeout: Duration::from_secs(8),
            read_timeout: Duration::from_secs(20),
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 FDM/0.1.0".into(),
            download_root: default_download_root(),
            organize_by_type: true,
            temp_dir: default_temp_dir(),
            use_temp_dir: true,
            progress_interval: Duration::from_millis(16),
        }
    }
}

/// `%USERPROFILE%\Downloads\FDM` on Windows, `~/Downloads/FDM` elsewhere.
/// Falls back to the current directory if the home directory can't be found.
pub fn default_download_root() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);

    match home {
        Some(h) => h.join("Downloads").join("FDM"),
        None => PathBuf::from("."),
    }
}

/// `%LOCALAPPDATA%\FDM\Temp` on Windows — on the C: drive, per-user, and
/// writable without elevation.
///
/// Not the install directory. FDM installs to `C:\Program Files\FDM`, which a
/// standard user cannot write to, so putting live download data there would make
/// every download fail unless the app ran elevated. IDM does not do it either: its
/// default temporary directory is under AppData. The path is a setting, so anyone
/// who wants it somewhere else — including inside the install folder on a machine
/// where that is writable — can say so in Settings.
///
/// LocalAppData rather than Roaming, because a `.part` file refers to bytes on
/// this machine's disk and syncing the path to another machine would only produce
/// downloads that cannot resume.
pub fn default_temp_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".cache"))
        });

    match base {
        Some(b) => b.join("FDM").join("Temp"),
        None => std::env::temp_dir().join("FDM"),
    }
}
