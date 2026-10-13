use crate::platform::{create_private_dir, open_private};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::util::SubscriberInitExt as _;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const RETAINED_LOGS: u8 = 3;

pub(crate) fn init_logging() {
    let filter = std::env::var("SHIO_LOG").unwrap_or_else(|_| {
        let level = if cfg!(debug_assertions) {
            "debug"
        } else {
            "info"
        };
        format!("shio={level},shio_core={level},warn")
    });
    let writer = match LogFile::open(&log_dir()) {
        Ok(log) => BoxMakeWriter::new(std::sync::Mutex::new(log)),
        Err(error) => {
            eprintln!("logging to stderr: {error}");
            BoxMakeWriter::new(io::stderr)
        },
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .finish();
    if subscriber.try_init().is_err() {
        eprintln!("failed to install tracing subscriber");
    }
}

fn log_dir() -> PathBuf {
    shio_core::AppConfig::data_dir().join("logs")
}

pub(crate) fn open_logs_folder() -> io::Result<()> {
    let dir = log_dir();
    create_private_dir(&dir)?;
    open::that(&dir)
}

struct LogFile {
    path: PathBuf,
    file: File,
    len: u64,
}

impl LogFile {
    fn open(dir: &Path) -> io::Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join("shio.log");
        let file = open_append(&path)?;
        let len = file.metadata()?.len();
        Ok(Self { path, file, len })
    }

    fn rotate(&mut self) -> io::Result<()> {
        for index in (1..RETAINED_LOGS).rev() {
            match std::fs::rename(rotated(&self.path, index), rotated(&self.path, index + 1)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {},
            }
        }
        std::fs::rename(&self.path, rotated(&self.path, 1))?;
        self.file = open_append(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len >= MAX_LOG_BYTES
            && let Err(error) = self.rotate()
        {
            eprintln!("log rotation failed: {error}");
        }
        let written = self.file.write(buf)?;
        self.len += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn rotated(path: &Path, index: u8) -> PathBuf {
    path.with_extension(format!("log.{index}"))
}

fn open_append(path: &Path) -> io::Result<File> {
    open_private(path, OpenOptions::new().create(true).append(true))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn logs_are_owner_only_and_rotate_while_running() {
        let dir = std::env::temp_dir().join(format!("shio-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join("shio.log");
        std::fs::write(rotated(&path, 1), b"old").unwrap();

        let mut log = LogFile::open(&dir).unwrap();
        log.write_all(&vec![b'x'; MAX_LOG_BYTES as usize]).unwrap();
        log.write_all(b"fresh").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        assert_eq!(std::fs::read(rotated(&path, 2)).unwrap(), b"old");
        assert_eq!(
            [mode(&dir), mode(&path), mode(&rotated(&path, 1))],
            [0o700, 0o600, 0o600]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn log_open_refuses_symbolic_links() {
        let dir = std::env::temp_dir().join(format!("shio-log-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("target");
        std::fs::write(&target, b"private").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("shio.log")).unwrap();

        assert!(LogFile::open(&dir).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"private");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
