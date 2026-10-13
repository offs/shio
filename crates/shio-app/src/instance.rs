use crate::platform::{AssociationLaunch, create_private_dir, open_private};
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, Sender};
use transport::{Listener, Stream};

const REQUEST_MAGIC: &[u8] = b"shio-activate\0";
const MAX_REQUEST_BYTES: usize = 128 * 1024;
const MAX_SOURCES: usize = 64;
const MAX_TOKEN_BYTES: usize = 1024;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(2);

static ACTIVATIONS: Mutex<Option<Receiver<Activation>>> = Mutex::new(None);
static ACCEPTING: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Activation {
    pub(crate) launch: AssociationLaunch,
    /// The launcher's `XDG_ACTIVATION_TOKEN`, which Wayland requires to raise a window.
    pub(crate) token: Option<String>,
}

#[derive(Debug)]
pub(crate) struct InstanceGuard {
    _lock: File,
}

#[derive(Debug)]
pub(crate) enum InstanceGuardError {
    Forwarded,
    ForwardFailed(io::Error),
    Unavailable { path: PathBuf, source: io::Error },
}

impl std::fmt::Display for InstanceGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forwarded => f.write_str("forwarded the launch to the running shio"),
            Self::ForwardFailed(error) => {
                write!(f, "shio is already running, but forwarding failed: {error}")
            },
            Self::Unavailable { path, source } => write!(f, "lock {}: {source}", path.display()),
        }
    }
}

impl InstanceGuard {
    pub(crate) fn acquire(launch: &AssociationLaunch) -> Result<Self, InstanceGuardError> {
        let dir = shio_core::AppConfig::data_dir();
        let activation = Activation {
            launch: launch.clone(),
            token: std::env::var("XDG_ACTIVATION_TOKEN")
                .ok()
                .filter(|token| valid_token(token.as_bytes())),
        };
        Self::acquire_in(&dir, &transport::endpoint(&dir), &activation)
    }

    fn acquire_in(
        dir: &Path,
        endpoint: &Path,
        activation: &Activation,
    ) -> Result<Self, InstanceGuardError> {
        let path = dir.join("shio.instance.lock");
        let unavailable = |source| InstanceGuardError::Unavailable {
            path: path.clone(),
            source,
        };
        create_private_dir(dir).map_err(unavailable)?;
        let lock = open_private(
            &path,
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true),
        )
        .map_err(unavailable)?;
        match lock.try_lock() {
            Ok(()) => {
                let listener = transport::bind(endpoint).map_err(unavailable)?;
                let (tx, rx) = tokio::sync::mpsc::channel(32);
                *ACTIVATIONS.lock() = Some(rx);
                std::thread::Builder::new()
                    .name("shio-instance".into())
                    .spawn(move || listen(&listener, &tx))
                    .map_err(unavailable)?;
                Ok(Self { _lock: lock })
            },
            Err(std::fs::TryLockError::WouldBlock) => {
                forward(endpoint, activation).map_err(InstanceGuardError::ForwardFailed)?;
                Err(InstanceGuardError::Forwarded)
            },
            Err(std::fs::TryLockError::Error(source)) => Err(unavailable(source)),
        }
    }
}

pub(crate) fn take_activation_receiver() -> Option<Receiver<Activation>> {
    ACTIVATIONS.lock().take()
}

// Later launches must fail instead of queueing into an instance that will never open them.
pub(crate) fn reject_activations() {
    ACCEPTING.store(false, Ordering::Release);
}

fn listen(listener: &Listener, tx: &Sender<Activation>) {
    loop {
        let result = transport::accept(listener).and_then(|mut stream| {
            let activation = decode(&read_request(&mut stream)?)?;
            if !ACCEPTING.load(Ordering::Acquire) {
                return Err(io::Error::other("shio is shutting down"));
            }
            tx.try_send(activation).map_err(io::Error::other)?;
            stream.write_all(&[1])
        });
        if let Err(error) = result {
            tracing::warn!("ignored instance activation: {error}");
        }
    }
}

/// Reads one request within a total deadline so a stalled client can't block later launches.
fn read_request(stream: &mut Stream) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + FORWARD_TIMEOUT;
    let mut request = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(&mut chunk)? {
            0 => return Ok(request),
            read if request.len() + read > MAX_REQUEST_BYTES => {
                return Err(io::Error::other("activation is too large"));
            },
            read => request.extend_from_slice(&chunk[..read]),
        }
    }
}

fn forward(endpoint: &Path, activation: &Activation) -> io::Result<()> {
    let request = encode(activation)?;
    let deadline = Instant::now() + FORWARD_TIMEOUT;
    loop {
        match forward_once(endpoint, &request) {
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            result => return result,
        }
    }
}

fn forward_once(endpoint: &Path, request: &[u8]) -> io::Result<()> {
    let mut stream = transport::connect(endpoint)?;
    stream.set_read_timeout(Some(FORWARD_TIMEOUT))?;
    stream.set_write_timeout(Some(FORWARD_TIMEOUT))?;
    stream.write_all(request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut ack = [0];
    stream.read_exact(&mut ack)?;
    if ack == [1] {
        Ok(())
    } else {
        Err(io::Error::other("the running shio rejected the launch"))
    }
}

#[cfg(unix)]
mod transport {
    use crate::platform::remove_if_exists;
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    pub(super) type Listener = UnixListener;
    pub(super) type Stream = UnixStream;

    // Only this user can connect: the runtime dir is user-only and the data dir is 0700. The
    // runtime socket is named by a hash of the data dir to fit the 108-byte path limit.
    pub(super) fn endpoint(data_dir: &Path) -> PathBuf {
        match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
            Some(runtime) if runtime.is_absolute() => {
                runtime.join(format!("shio-{:016x}.sock", fnv1a(data_dir.as_os_str())))
            },
            _ => local_endpoint(data_dir),
        }
    }

    pub(super) fn local_endpoint(dir: &Path) -> PathBuf {
        dir.join("shio.sock")
    }

    fn fnv1a(value: &std::ffi::OsStr) -> u64 {
        std::os::unix::ffi::OsStrExt::as_bytes(value)
            .iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            })
    }

    pub(super) fn bind(socket: &Path) -> io::Result<UnixListener> {
        remove_if_exists(socket)?;
        UnixListener::bind(socket)
    }

    pub(super) fn accept(listener: &UnixListener) -> io::Result<UnixStream> {
        listener.accept().map(|(stream, _)| stream)
    }

    pub(super) fn connect(socket: &Path) -> io::Result<UnixStream> {
        UnixStream::connect(socket)
    }
}

#[cfg(windows)]
mod transport {
    use super::FORWARD_TIMEOUT;
    use crate::platform::remove_if_exists;
    use std::io::{self, Read, Write};
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};

    pub(super) type Stream = TcpStream;

    pub(super) struct Listener {
        tcp: TcpListener,
        token: String,
    }

    // Loopback is reachable by every local user, so the client proves it read the
    // owner-only record by sending its token first.
    pub(super) fn endpoint(data_dir: &Path) -> PathBuf {
        local_endpoint(data_dir)
    }

    pub(super) fn local_endpoint(dir: &Path) -> PathBuf {
        dir.join("shio.instance")
    }

    pub(super) fn bind(record: &Path) -> io::Result<Listener> {
        remove_if_exists(record)?;
        let tcp = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let token = uuid::Uuid::new_v4().to_string();
        let temporary = record.with_file_name(format!("shio.instance.{token}.tmp"));
        let mut file = std::fs::File::create_new(&temporary)?;
        write!(file, "{}\n{token}", tcp.local_addr()?.port())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, record)?;
        Ok(Listener { tcp, token })
    }

    pub(super) fn accept(listener: &Listener) -> io::Result<TcpStream> {
        let (mut stream, _) = listener.tcp.accept()?;
        stream.set_read_timeout(Some(FORWARD_TIMEOUT))?;
        let mut token = vec![0; listener.token.len()];
        stream.read_exact(&mut token)?;
        if token == listener.token.as_bytes() {
            Ok(stream)
        } else {
            Err(io::Error::other("activation token did not match"))
        }
    }

    pub(super) fn connect(record: &Path) -> io::Result<TcpStream> {
        let record = std::fs::read_to_string(record)?;
        let (port, token) = record
            .split_once('\n')
            .and_then(|(port, token)| Some((port.parse().ok()?, token)))
            .ok_or_else(|| io::Error::other("invalid instance record"))?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mut stream = TcpStream::connect_timeout(&address.into(), FORWARD_TIMEOUT)?;
        stream.write_all(token.as_bytes())?;
        Ok(stream)
    }
}

fn encode(activation: &Activation) -> io::Result<Vec<u8>> {
    let launch = &activation.launch;
    if launch.magnets.len() + launch.torrent_files.len() > MAX_SOURCES {
        return Err(io::Error::other(format!(
            "open {MAX_SOURCES} sources or fewer at once"
        )));
    }
    let mut bytes = REQUEST_MAGIC.to_vec();
    push_count(&mut bytes, launch.magnets.len());
    for magnet in &launch.magnets {
        push_bytes(&mut bytes, magnet.as_bytes());
    }
    push_count(&mut bytes, launch.torrent_files.len());
    for path in &launch.torrent_files {
        push_bytes(&mut bytes, &path_bytes(path));
    }
    push_bytes(
        &mut bytes,
        activation.token.as_deref().unwrap_or_default().as_bytes(),
    );
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(io::Error::other("activation is too large"));
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> io::Result<Activation> {
    let invalid =
        |reason: &str| io::Error::new(io::ErrorKind::InvalidData, format!("activation {reason}"));
    let mut cursor = bytes
        .strip_prefix(REQUEST_MAGIC)
        .ok_or_else(|| invalid("has an invalid header"))?;
    let magnet_count = take_count(&mut cursor)
        .filter(|&n| n <= MAX_SOURCES)
        .ok_or_else(|| invalid("has too many sources"))?;
    let mut magnets = Vec::with_capacity(magnet_count);
    for _ in 0..magnet_count {
        let magnet = take_bytes(&mut cursor).ok_or_else(|| invalid("is truncated"))?;
        let magnet = std::str::from_utf8(magnet).map_err(|_| invalid("magnet is not UTF-8"))?;
        magnets.push(magnet.to_owned());
    }
    let file_count = take_count(&mut cursor)
        .filter(|&n| n <= MAX_SOURCES - magnet_count)
        .ok_or_else(|| invalid("has too many sources"))?;
    let mut torrent_files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        let path = take_bytes(&mut cursor).ok_or_else(|| invalid("is truncated"))?;
        torrent_files.push(path_from_bytes(path).ok_or_else(|| invalid("path is malformed"))?);
    }
    let token = match take_bytes(&mut cursor).ok_or_else(|| invalid("is truncated"))? {
        [] => None,
        token if valid_token(token) => Some(String::from_utf8_lossy(token).into_owned()),
        _ => return Err(invalid("token is malformed")),
    };
    if !cursor.is_empty() {
        return Err(invalid("has trailing data"));
    }
    Ok(Activation {
        launch: AssociationLaunch {
            magnets,
            torrent_files,
        },
        token,
    })
}

fn valid_token(token: &[u8]) -> bool {
    (1..=MAX_TOKEN_BYTES).contains(&token.len()) && token.iter().all(u8::is_ascii_graphic)
}

fn push_count(bytes: &mut Vec<u8>, count: usize) {
    // Bounded by MAX_REQUEST_BYTES before the request is sent.
    bytes.extend_from_slice(&u32::try_from(count).unwrap_or(u32::MAX).to_le_bytes());
}

fn push_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    push_count(bytes, value.len());
    bytes.extend_from_slice(value);
}

fn take_count(cursor: &mut &[u8]) -> Option<usize> {
    let (count, rest) = cursor.split_first_chunk::<4>()?;
    *cursor = rest;
    usize::try_from(u32::from_le_bytes(*count)).ok()
}

fn take_bytes<'a>(cursor: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = take_count(cursor)?;
    let (value, rest) = cursor.split_at_checked(len)?;
    *cursor = rest;
    Some(value)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()).to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    (!bytes.is_empty() && !bytes.contains(&0))
        .then(|| PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let (units, []) = bytes.as_chunks::<2>() else {
        return None;
    };
    let wide: Vec<u16> = units.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
    (!wide.is_empty() && !wide.contains(&0))
        .then(|| PathBuf::from(std::ffi::OsString::from_wide(&wide)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activation(magnets: &[&str], token: Option<&str>) -> Activation {
        Activation {
            launch: AssociationLaunch {
                magnets: magnets.iter().map(|&magnet| magnet.to_owned()).collect(),
                torrent_files: vec![std::env::temp_dir().join("example.torrent")],
            },
            token: token.map(str::to_owned),
        }
    }

    #[test]
    fn second_launch_is_forwarded_and_lock_releases_on_drop() {
        // macOS caps socket paths at 104 bytes and its temp dir is already long.
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().simple().to_string());
        let endpoint = transport::local_endpoint(&dir);
        let idle = activation(&[], None);
        let launch = activation(&["magnet:?xt=urn:btih:test"], Some("launcher/42_TIME7"));

        let first = InstanceGuard::acquire_in(&dir, &endpoint, &idle).unwrap();
        let mut activations = take_activation_receiver().unwrap();
        assert!(matches!(
            InstanceGuard::acquire_in(&dir, &endpoint, &launch),
            Err(InstanceGuardError::Forwarded)
        ));
        assert_eq!(activations.blocking_recv(), Some(launch));

        drop(first);
        drop(InstanceGuard::acquire_in(&dir, &endpoint, &idle).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn data_dir_is_private_and_lock_refuses_symbolic_links() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shio-instance-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = dir.join("target");
        std::fs::write(&target, b"keep me").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("shio.instance.lock")).unwrap();

        assert!(matches!(
            InstanceGuard::acquire_in(
                &dir,
                &transport::local_endpoint(&dir),
                &activation(&[], None)
            ),
            Err(InstanceGuardError::Unavailable { .. })
        ));
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn decoding_rejects_hostile_requests() {
        let valid = encode(&activation(&["magnet:?a"], None)).unwrap();
        assert!(decode(&valid).is_ok());

        let mut too_many = REQUEST_MAGIC.to_vec();
        push_count(&mut too_many, MAX_SOURCES);
        too_many.extend(std::iter::repeat_n([0, 0, 0, 0], MAX_SOURCES).flatten());
        push_count(&mut too_many, 1);
        let mut huge_count = REQUEST_MAGIC.to_vec();
        push_count(&mut huge_count, u32::MAX as usize);
        let mut empty_path = REQUEST_MAGIC.to_vec();
        push_count(&mut empty_path, 0);
        push_count(&mut empty_path, 1);
        push_bytes(&mut empty_path, &[]);
        #[cfg(unix)]
        let bad_path = b"a\0.torrent".as_slice();
        #[cfg(windows)]
        let bad_path = b"abc".as_slice();
        let mut malformed_path = REQUEST_MAGIC.to_vec();
        push_count(&mut malformed_path, 0);
        push_count(&mut malformed_path, 1);
        push_bytes(&mut malformed_path, bad_path);
        let mut malformed_token = REQUEST_MAGIC.to_vec();
        push_count(&mut malformed_token, 0);
        push_count(&mut malformed_token, 0);
        push_bytes(&mut malformed_token, b"token\nwith a newline");

        for request in [
            &b"shio\0"[..],
            &valid[..valid.len() - 1],
            &[valid.as_slice(), b"x"].concat(),
            &too_many,
            &huge_count,
            &empty_path,
            &malformed_path,
            &malformed_token,
        ] {
            assert!(decode(request).is_err());
        }
    }
}
