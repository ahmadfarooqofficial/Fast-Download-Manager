//! `fdm` — command-line harness for the download engine.
//!
//! This is not the product; it exists so the engine can be exercised and the
//! Phase 1 exit test can be run before any UI, extension, or installer is
//! written:
//!
//! ```text
//! fdm get <url> --sequential --sha256      # baseline, one connection
//! fdm get <url> -n 16 --sha256             # segmented; hashes must match
//! ```
//!
//! Kill the segmented run partway through (Ctrl-C, or just terminate the
//! process) and re-run the identical command — it should resume from the `.fdm`
//! control file and still land on the same hash.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use fdm_core::{
    default_download_root, format_bytes, format_duration, format_speed, CancelToken, Category,
    DownloadRequest, Engine, EngineConfig,
};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "fdm", version, about = "FDM download engine (test harness)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a URL.
    Get {
        url: String,

        /// Maximum simultaneous connections.
        #[arg(short = 'n', long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=64))]
        connections: u32,

        /// Download root. Defaults to %USERPROFILE%\Downloads\FDM.
        #[arg(short, long)]
        out: Option<PathBuf>,

        /// Override the derived filename.
        #[arg(long)]
        name: Option<String>,

        /// One connection only — the baseline to compare a segmented run against.
        #[arg(long, conflicts_with = "connections")]
        sequential: bool,

        /// Write straight into the output directory instead of sorting into
        /// Documents/, Video/, Music/, ... subfolders.
        #[arg(long)]
        flat: bool,

        /// Smallest segment the planner will create, in MiB.
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u64).range(1..=1024))]
        min_split_mb: u64,

        /// Print the SHA-256 of the finished file.
        #[arg(long)]
        sha256: bool,

        /// Extra request header. Repeatable: -H "Cookie: a=b" -H "Referer: ..."
        #[arg(short = 'H', long = "header", value_name = "NAME: VALUE")]
        headers: Vec<String>,
    },

    /// SHA-256 a local file, so a segmented download can be compared against a
    /// known-good copy.
    Hash { path: PathBuf },

    /// Create the download root and its category subfolders, then print the
    /// root path. The installer runs this as the logged-on user, so the folders
    /// land in the right profile rather than the elevating administrator's.
    ///
    /// Deliberately shares `fdm_core`'s own path logic instead of the installer
    /// guessing at it — one source of truth for where downloads go.
    InitFolders {
        /// Root to create under. Defaults to %USERPROFILE%\Downloads\FDM.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FDM_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("fdm_core=info,fdm=info")),
        )
        // Progress rendering owns stdout; logs go to stderr.
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Hash { path } => {
            let digest = sha256_file(&path)?;
            println!("{digest}  {}", path.display());
            Ok(())
        }
        Command::InitFolders { out } => init_folders(out),
        command => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to start the tokio runtime")?;
            runtime.block_on(run(command))
        }
    }
}

async fn run(command: Command) -> Result<()> {
    let Command::Get {
        url,
        connections,
        out,
        name,
        sequential,
        flat,
        min_split_mb,
        sha256,
        headers,
    } = command
    else {
        unreachable!("Hash and InitFolders are handled without a runtime");
    };

    let url = url
        .parse()
        .with_context(|| format!("`{url}` is not a valid URL"))?;

    let mut cfg = EngineConfig {
        max_connections: if sequential { 1 } else { connections },
        min_split_size: min_split_mb * 1024 * 1024,
        organize_by_type: !flat,
        ..EngineConfig::default()
    };
    if let Some(dir) = out {
        cfg.download_root = dir;
    }

    let mut request = DownloadRequest::new(url).with_headers(parse_headers(&headers)?);
    if let Some(name) = name {
        request = request.with_filename(name);
    }

    let engine = Engine::new(cfg)?;

    // Ctrl-C asks the engine to stop. It flushes the control file and leaves the
    // `.part` intact, so the same command resumes.
    let cancel = CancelToken::new();
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\ninterrupted — flushing resume state");
                cancel.cancel();
            }
        });
    }

    let bar = ProgressBar::new_spinner();
    bar.enable_steady_tick(Duration::from_millis(120));
    bar.set_style(spinner_style());
    let mut sized = false;

    let outcome = engine
        .download(request, cancel, |p| {
            if let (Some(total), false) = (p.total, sized) {
                bar.set_style(bar_style());
                bar.set_length(total);
                sized = true;
            }
            bar.set_position(p.downloaded);
            bar.set_message(format!(
                "{} · {} of {} seg active{}",
                format_speed(p.speed_bps),
                p.active_connections,
                p.segments,
                p.eta
                    .map(|e| format!(" · ETA {}", format_duration(e)))
                    .unwrap_or_default(),
            ));
        })
        .await;

    let outcome = match outcome {
        Ok(outcome) => {
            bar.finish_and_clear();
            outcome
        }
        Err(fdm_core::Error::Cancelled) => {
            bar.finish_and_clear();
            println!("paused — re-run the same command to resume from the .part file");
            return Ok(());
        }
        Err(err) => {
            bar.abandon();
            return Err(err.into());
        }
    };

    let avg = if outcome.elapsed.as_secs_f64() > 0.0 {
        outcome.bytes as f64 / outcome.elapsed.as_secs_f64()
    } else {
        0.0
    };

    println!("{}", outcome.path.display());
    println!(
        "  {} in {} ({} average)",
        format_bytes(outcome.bytes),
        format_duration(outcome.elapsed),
        format_speed(avg),
    );
    println!(
        "  {} segments · {} · ranges {} · {}",
        outcome.segments_used,
        outcome.category.folder(),
        if outcome.used_ranges { "yes" } else { "no" },
        if outcome.resumed {
            "resumed"
        } else {
            "fresh download"
        },
    );

    if sha256 {
        println!("  sha256 {}", sha256_file(&outcome.path)?);
    }

    Ok(())
}

/// Create the download root and every category subfolder up front.
///
/// The installer calls this as the logged-on user. Doing it here rather than in
/// the installer means the folder set can never drift from `Category::ALL` —
/// add a category to the engine and the installer starts creating it for free.
fn init_folders(out: Option<PathBuf>) -> Result<()> {
    let root = out.unwrap_or_else(default_download_root);

    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create download root {}", root.display()))?;

    for category in Category::ALL {
        let dir = root.join(category.folder());
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }

    // stdout is the contract: the installer captures this line to show the user
    // where their downloads will go.
    println!("{}", root.display());
    Ok(())
}

fn parse_headers(raw: &[String]) -> Result<HeaderMap> {
    let mut map = HeaderMap::with_capacity(raw.len());
    for entry in raw {
        let Some((name, value)) = entry.split_once(':') else {
            bail!("header `{entry}` is missing a colon; expected \"Name: value\"");
        };
        let name: HeaderName = name
            .trim()
            .parse()
            .with_context(|| format!("`{name}` is not a valid header name"))?;
        let value = HeaderValue::from_str(value.trim())
            .with_context(|| format!("`{value}` is not a valid value for `{name}`"))?;
        map.append(name, value);
    }
    Ok(map)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("cannot open {} to hash", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];

    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Red fill on a dark track — the same accent the UI will use.
fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{bar:32.red/black} {percent:>3}%  {bytes}/{total_bytes}  {msg}",
    )
    .expect("static template")
    .progress_chars("█▉ ")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.red} {bytes}  {msg}").expect("static template")
}
