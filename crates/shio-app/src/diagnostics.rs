use shio_core::AppConfig;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const LOG_FILE: &str = "shio.log";
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const RETAINED_LOGS: u8 = 3;

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn init_logging() {
    let log_dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("failed to create log directory {}: {e}", log_dir.display());
    }
    let log_path = log_dir.join(LOG_FILE);
    if let Err(e) = rotate_logs(&log_path) {
        eprintln!("failed to rotate logs {}: {e}", log_path.display());
    }

    let filter = std::env::var("SHIO_LOG").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "shio=debug,shio_core=debug,warn".to_string()
        } else {
            "shio=info,shio_core=info,warn".to_string()
        }
    });

    let writer_path = log_path;
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(move || -> Box<dyn std::io::Write + Send> {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&writer_path)
            {
                Ok(file) => Box::new(file),
                Err(_) => Box::new(std::io::stderr()),
            }
        })
        .finish();

    if tracing::subscriber::set_global_default(subscriber).is_err() {
        eprintln!("failed to install tracing subscriber");
    }
}

pub(crate) fn log_dir() -> PathBuf {
    LOG_DIR
        .get_or_init(|| AppConfig::data_dir().join("logs"))
        .clone()
}

pub(crate) fn log_file() -> PathBuf {
    log_dir().join(LOG_FILE)
}

pub(crate) fn open_logs_folder() -> Result<(), String> {
    let dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("failed to create logs folder {}: {e}", dir.display());
        return Err(e.to_string());
    }
    if let Err(e) = open::that(&dir) {
        tracing::warn!("failed to open logs folder {}: {e}", dir.display());
        return Err(e.to_string());
    }
    Ok(())
}

fn rotate_logs(path: &Path) -> std::io::Result<()> {
    if !path.exists() || std::fs::metadata(path)?.len() < MAX_LOG_BYTES {
        return Ok(());
    }

    for i in (1..=RETAINED_LOGS).rev() {
        let from = rotated_path(path, i);
        let to = rotated_path(path, i + 1);
        if from.exists() {
            let _ = std::fs::remove_file(&to);
            std::fs::rename(from, to)?;
        }
    }

    std::fs::rename(path, rotated_path(path, 1))?;
    Ok(())
}

fn rotated_path(path: &Path, index: u8) -> PathBuf {
    path.with_extension(format!("log.{index}"))
}
