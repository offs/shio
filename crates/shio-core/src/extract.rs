use crate::error::{Result, ShioError};
use crate::filename::{
    rar_part, release_name, sanitize_filename, sevenz_volume, strip_suffix_ignore_ascii_case,
};
use crate::worker::{rename_no_clobber, sync_parent};
use std::cell::Cell;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use tokio_util::sync::CancellationToken;

const STAGING_PREFIX: &str = ".shio-extract-";
const MAX_EXTRACTED_FILES: usize = 100_000;
#[cfg(not(test))]
const MAX_EXTRACTED_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
#[cfg(test)]
const MAX_EXTRACTED_FILE_BYTES: u64 = 1024;
#[cfg(not(test))]
const MAX_EXTRACTED_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
#[cfg(test)]
const MAX_EXTRACTED_TOTAL_BYTES: u64 = 2048;
const MAX_RAR_DICTIONARY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TAR_HEADER_BYTES: usize = 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_DESTINATION_ATTEMPTS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Zip,
    SevenZ,
    Rar,
    TarGz,
    TarZst,
    Tar,
}

impl Format {
    const SUFFIXES: [(&str, Self); 8] = [
        (".tar.gz", Self::TarGz),
        (".tgz", Self::TarGz),
        (".tar.zst", Self::TarZst),
        (".tzst", Self::TarZst),
        (".tar", Self::Tar),
        (".zip", Self::Zip),
        (".7z", Self::SevenZ),
        (".rar", Self::Rar),
    ];

    pub(crate) fn of(name: &str) -> Option<Self> {
        if sevenz_volume(name).is_some() {
            return Some(Self::SevenZ);
        }
        Self::SUFFIXES
            .iter()
            .find(|(suffix, _)| strip_suffix_ignore_ascii_case(name, suffix).is_some())
            .map(|(_, format)| *format)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    Incomplete,
    PasswordRequired,
}

/// Volumes match the base name exactly, so `Photos.part1.rar` never joins `photos.part1.rar`.
pub(crate) struct Plan {
    pub(crate) first_volume: PathBuf,
    pub(crate) members: Vec<PathBuf>,
    base_name: String,
    format: Format,
    sequence_complete: bool,
}

pub(crate) fn plan(completed: &Path) -> Option<Plan> {
    let name = completed.file_name()?.to_str()?;
    let format = Format::of(name)?;
    let volume: fn(&str) -> Option<(&str, &str)> = match format {
        Format::Rar => rar_part,
        Format::SevenZ => sevenz_volume,
        _ => |_| None,
    };
    let (base, members, sequence_complete) = if let Some((base, _)) = volume(name) {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(completed.parent()?).ok()? {
            let path = entry.ok()?.path();
            let index = path
                .file_name()
                .and_then(OsStr::to_str)
                .and_then(volume)
                .filter(|(entry_base, _)| *entry_base == base)
                .and_then(|(_, digits)| digits.parse::<u32>().ok());
            if let Some(index) = index {
                found.push((index, path));
            }
        }
        found.sort_unstable_by_key(|(index, _)| *index);
        let complete = found
            .iter()
            .zip(1..)
            .all(|((index, _), expected)| *index == expected);
        (
            base,
            found.into_iter().map(|(_, path)| path).collect::<Vec<_>>(),
            complete,
        )
    } else {
        let base = Format::SUFFIXES
            .iter()
            .find_map(|(suffix, _)| strip_suffix_ignore_ascii_case(name, suffix))?;
        (base, vec![completed.to_owned()], true)
    };
    Some(Plan {
        first_volume: members.first()?.clone(),
        members,
        base_name: base.to_owned(),
        format,
        sequence_complete,
    })
}

impl Plan {
    pub(crate) fn readiness(&self) -> Readiness {
        if !self.sequence_complete {
            return Readiness::Incomplete;
        }
        let ready = match self.format {
            Format::Rar => rar_ready(self),
            Format::SevenZ => VolumeReader::open(&self.members).and_then(|reader| {
                sevenz_rust2::ArchiveReader::new(reader, sevenz_rust2::Password::empty())
                    .map(|_| true)
                    .map_err(|error| map_7z_error(error, None))
            }),
            Format::Zip => zip_ready(&self.first_volume),
            Format::Tar | Format::TarGz | Format::TarZst => Ok(true),
        };
        match ready {
            Ok(true) => Readiness::Ready,
            Err(ShioError::PasswordRequired) => Readiness::PasswordRequired,
            Ok(false) | Err(_) => Readiness::Incomplete,
        }
    }
}

fn rar_ready(plan: &Plan) -> Result<bool> {
    let archives = rar_volumes(plan, rars::ArchiveReadOptions::new())?;
    let members = rars::volume_members(&archives).map_err(|error| map_rar_error(&error))?;
    if members.iter().any(|member| member.meta.is_encrypted) {
        return Err(ShioError::PasswordRequired);
    }
    // A RAR5 volume may end exactly on a member boundary; only its end header says more follow.
    let continues = matches!(
        archives.last(),
        Some(rars::Archive::Rar50Plus(last)) if last.blocks.iter().any(
            |block| matches!(block, rars::rar50::Block::End(end) if end.has_next_volume())
        )
    );
    Ok(!continues
        && members
            .last()
            .is_none_or(|member| !member.meta.is_split_after))
}

fn zip_ready(path: &Path) -> Result<bool> {
    let mut archive = zip::ZipArchive::new(File::open(path)?).map_err(map_zip_error)?;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        if entry.encrypted() {
            return Err(ShioError::PasswordRequired);
        }
    }
    Ok(true)
}

/// Removes staging directories a crash left in `dir`; call before any extraction into `dir`.
pub(crate) fn remove_stale_staging(dir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        entries => entries?,
    };
    for entry in entries {
        let entry = entry?;
        let is_staging = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(STAGING_PREFIX));
        if is_staging && entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

pub(crate) fn extract(
    plan: &Plan,
    password: Option<&str>,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    ensure_not_cancelled(cancel)?;
    let parent = plan
        .first_volume
        .parent()
        .ok_or_else(|| invalid("archive has no parent directory"))?;
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)?;
    let mut output = Output {
        root: staging.path(),
        cancel,
        entries: 0,
        file_bytes: 0,
        total_bytes: 0,
    };
    let open = || File::open(&plan.first_volume);
    match plan.format {
        Format::Zip => extract_zip(&plan.first_volume, password, &mut output),
        Format::SevenZ => extract_7z(plan, password, &mut output),
        Format::Rar => extract_rar(plan, password, parent, &mut output),
        Format::TarGz => unpack_tar(flate2::read::MultiGzDecoder::new(open()?), &mut output),
        Format::TarZst => unpack_tar(zstd::Decoder::new(open()?)?, &mut output),
        Format::Tar => unpack_tar(open()?, &mut output),
    }?;
    sync_tree(staging.path(), cancel)?;

    let name = release_name(&plan.base_name);
    for attempt in 1..=MAX_DESTINATION_ATTEMPTS {
        let candidate = if attempt == 1 {
            name.clone()
        } else {
            sanitize_filename(&format!("{name} ({attempt})"))
        };
        let destination = parent.join(candidate);
        match rename_no_clobber(staging.path(), &destination) {
            Ok(()) => {
                sync_parent(&destination)?;
                return Ok(destination);
            },
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid(&format!("no free extract destination for {name}")))
}

struct Output<'a> {
    root: &'a Path,
    cancel: &'a CancellationToken,
    entries: usize,
    file_bytes: u64,
    total_bytes: u64,
}

impl Output<'_> {
    fn record_entry(&mut self) -> Result<()> {
        ensure_not_cancelled(self.cancel)?;
        self.entries += 1;
        if self.entries > MAX_EXTRACTED_FILES {
            return Err(invalid("archive contains too many entries"));
        }
        Ok(())
    }

    fn directory(&mut self, name: &Path) -> Result<()> {
        self.record_entry()?;
        if name
            .components()
            .all(|component| component == Component::CurDir)
        {
            return Ok(());
        }
        Ok(std::fs::create_dir_all(self.join(name)?)?)
    }

    fn file(&mut self, name: &Path, size: Option<u64>, mode: Option<u32>) -> Result<File> {
        self.record_entry()?;
        if size.is_some_and(|size| size > MAX_EXTRACTED_FILE_BYTES) {
            return Err(invalid("archive entry is too large"));
        }
        let path = self.join(name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(
            &mut options,
            mode.map_or(0o666, |mode| (mode & 0o755) | 0o600),
        );
        #[cfg(not(unix))]
        let _ = mode;
        self.file_bytes = 0;
        options.open(&path).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                invalid("archive contains duplicate entries")
            } else {
                error.into()
            }
        })
    }

    fn write(&mut self, file: &mut File, bytes: &[u8]) -> Result<()> {
        ensure_not_cancelled(self.cancel)?;
        self.file_bytes += bytes.len() as u64;
        self.total_bytes += bytes.len() as u64;
        if self.file_bytes > MAX_EXTRACTED_FILE_BYTES {
            return Err(invalid("archive entry is too large"));
        }
        if self.total_bytes > MAX_EXTRACTED_TOTAL_BYTES {
            return Err(invalid("archive expanded size is too large"));
        }
        Ok(file.write_all(bytes)?)
    }

    fn copy(&mut self, reader: &mut dyn Read, file: &mut File, buffer: &mut [u8]) -> Result<()> {
        loop {
            match reader.read(buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => self.write(file, &buffer[..read])?,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                Err(error) => return Err(invalid(&error)),
            }
        }
    }

    fn join(&self, name: &Path) -> Result<PathBuf> {
        // Tarballs made with `tar -C dir .` name their entries `./…`.
        let relative = name.strip_prefix(".").unwrap_or(name);
        crate::path::validate_relative_path(relative)
            .map_err(|_| invalid("archive entry escapes destination"))?;
        Ok(self.root.join(relative))
    }
}

fn extract_zip(archive: &Path, password: Option<&str>, output: &mut Output<'_>) -> Result<()> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(map_zip_error)?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    for index in 0..zip.len() {
        let encrypted = zip.by_index_raw(index).map_err(map_zip_error)?.encrypted();
        let mut entry = match (encrypted, password) {
            (false, _) => zip.by_index(index),
            (true, Some(password)) => zip.by_index_decrypt(index, password.as_bytes()),
            (true, None) => return Err(ShioError::PasswordRequired),
        }
        .map_err(map_zip_error)?;
        let name = entry
            .enclosed_name()
            .ok_or_else(|| invalid("archive entry escapes destination"))?;
        if entry.is_symlink() {
            return Err(invalid("archive contains unsupported zip entry"));
        }
        if entry.is_dir() {
            output.directory(&name)?;
            continue;
        }
        let mut file = output.file(&name, Some(entry.size()), entry.unix_mode())?;
        output.copy(&mut entry, &mut file, &mut buffer)?;
    }
    Ok(())
}

fn map_zip_error(error: zip::result::ZipError) -> ShioError {
    match error {
        zip::result::ZipError::InvalidPassword => ShioError::PasswordRequired,
        other => invalid(&other),
    }
}

fn extract_7z(plan: &Plan, password: Option<&str>, output: &mut Output<'_>) -> Result<()> {
    let reader = VolumeReader::open(&plan.members)?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut failure = None;
    let mut stash = |error: ShioError| {
        failure = Some(error);
        sevenz_rust2::Error::Other("extraction stopped".into())
    };
    let result = sevenz_rust2::decompress_with_extract_fn_and_password(
        reader,
        output.root,
        password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::new),
        |entry, reader, _| {
            if entry.is_anti_item() {
                return Ok(true);
            }
            let name = Path::new(entry.name());
            if entry.is_directory() {
                output.directory(name).map_err(&mut stash)?;
                return Ok(true);
            }
            // 7-Zip keeps a Unix mode in the high half of the attributes when bit 15 is set.
            let mode = (entry.has_windows_attributes && entry.windows_attributes & 0x8000 != 0)
                .then_some(entry.windows_attributes >> 16);
            let mut file = output
                .file(name, Some(entry.size()), mode)
                .map_err(&mut stash)?;
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    return Ok(true);
                }
                output
                    .write(&mut file, &buffer[..read])
                    .map_err(&mut stash)?;
            }
        },
    );
    if let Some(error) = failure {
        return Err(error);
    }
    result.map_err(|error| map_7z_error(error, password))
}

fn map_7z_error(error: sevenz_rust2::Error, password: Option<&str>) -> ShioError {
    match error {
        sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
            ShioError::PasswordRequired
        },
        sevenz_rust2::Error::ChecksumVerificationFailed if password.is_some() => {
            ShioError::PasswordRequired
        },
        other => invalid(&other),
    }
}

fn rar_volumes(plan: &Plan, options: rars::ArchiveReadOptions<'_>) -> Result<Vec<rars::Archive>> {
    plan.members
        .iter()
        .map(|path| rars::ArchiveReader::read_path_with_options(path, options))
        .collect::<rars::Result<_>>()
        .map_err(|error| map_rar_error(&error))
}

/// rars creates no output itself: it skips RAR5 links, and service headers (NTFS streams) never
/// reach `open`, so every extracted path goes through `Output`.
fn extract_rar(
    plan: &Plan,
    password: Option<&str>,
    parent: &Path,
    output: &mut Output<'_>,
) -> Result<()> {
    let scratch_dir = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)?;
    // rars admits a member only with room for twice its declared size (raw + filtered output).
    let scratch = rars::Rar50Scratch::new(scratch_dir.path(), MAX_EXTRACTED_FILE_BYTES * 2);
    let options = rars::ArchiveReadOptions::with_optional_password(password.map(str::as_bytes))
        .with_max_member_output_bytes(MAX_EXTRACTED_FILE_BYTES)
        .with_max_total_output_bytes(MAX_EXTRACTED_TOTAL_BYTES)
        .with_rar50_dictionary_size_limit(MAX_RAR_DICTIONARY_BYTES)
        .with_rar50_scratch(&scratch);
    let archives = rar_volumes(plan, options)?;
    let mut failure = None;
    let mut stash = |error: ShioError| {
        failure = Some(error);
        rars::Error::WriterFailure("extraction stopped")
    };
    let result = rars::extract_volumes_to_with_options(&archives, options, |meta| {
        let name = rars::entry_relative_path(&meta.name)
            .map_err(|_| stash(invalid("archive entry escapes destination")))?;
        if meta.is_directory {
            output.directory(&name).map_err(&mut stash)?;
            return Ok(Box::new(io::sink()));
        }
        let mode = u32::try_from(meta.file_attr)
            .ok()
            .filter(|_| meta.attr_source == rars::AttrSource::Unix);
        let file = output.file(&name, None, mode).map_err(&mut stash)?;
        Ok(Box::new(CancellableWriter {
            file,
            cancel: output.cancel.clone(),
        }))
    });
    if let Some(error) = failure {
        return Err(error);
    }
    ensure_not_cancelled(output.cancel)?;
    result.map_err(|error| map_rar_error(&error))
}

fn map_rar_error(error: &rars::Error) -> ShioError {
    match error.kind() {
        rars::ErrorKind::PasswordRequired | rars::ErrorKind::BadPassword => {
            ShioError::PasswordRequired
        },
        _ => invalid(error),
    }
}

/// rars owns the writer, so cancellation travels inside it; rars enforces the byte limits.
struct CancellableWriter {
    file: File,
    cancel: CancellationToken,
}

impl Write for CancellableWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("extraction cancelled"));
        }
        self.file.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn unpack_tar(reader: impl Read, output: &mut Output<'_>) -> Result<()> {
    let header_budget = Cell::new(MAX_TAR_HEADER_BYTES);
    let mut archive = tar::Archive::new(HeaderLimit {
        reader,
        remaining: &header_budget,
    });
    let mut entries = archive.entries().map_err(|error| invalid(&error))?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    loop {
        header_budget.set(MAX_TAR_HEADER_BYTES);
        let Some(entry) = entries.next() else {
            return Ok(());
        };
        header_budget.set(usize::MAX);
        let mut entry = entry.map_err(|error| invalid(&error))?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            output.record_entry()?;
            continue;
        }
        let name = entry.path().map_err(|error| invalid(&error))?.into_owned();
        if kind.is_dir() {
            output.directory(&name)?;
        } else if kind.is_file() {
            let mode = entry.header().mode().ok();
            let mut file = output.file(&name, entry.header().size().ok(), mode)?;
            output.copy(&mut entry, &mut file, &mut buffer)?;
        } else {
            return Err(invalid("archive contains unsupported tar entry"));
        }
    }
}

/// tar-rs reads GNU long names and PAX headers into memory whole, so the bytes it may consume
/// between two entries are bounded.
struct HeaderLimit<'a, R> {
    reader: R,
    remaining: &'a Cell<usize>,
}

impl<R: Read> Read for HeaderLimit<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.reader.read(buffer)?;
        let remaining = self
            .remaining
            .get()
            .checked_sub(read)
            .ok_or_else(|| io::Error::other("archive entry header is too large"))?;
        self.remaining.set(remaining);
        Ok(read)
    }
}

fn invalid(reason: &(impl std::fmt::Display + ?Sized)) -> ShioError {
    ShioError::Extract(reason.to_string())
}

fn sync_tree(dir: &Path, cancel: &CancellationToken) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        ensure_not_cancelled(cancel)?;
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path(), cancel)?;
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .open(entry.path())?
                .sync_all()?;
        }
    }
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn ensure_not_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(ShioError::Cancelled);
    }
    Ok(())
}

struct VolumeReader {
    volumes: Vec<(File, u64, u64)>,
    position: u64,
    len: u64,
}

impl VolumeReader {
    fn open(paths: &[PathBuf]) -> Result<Self> {
        let mut volumes = Vec::with_capacity(paths.len());
        let mut len = 0_u64;
        for path in paths {
            let file = File::open(path)?;
            let end = len
                .checked_add(file.metadata()?.len())
                .ok_or_else(|| invalid("archive set is too large"))?;
            volumes.push((file, len, end));
            len = end;
        }
        Ok(Self {
            volumes,
            position: 0,
            len,
        })
    }
}

impl Read for VolumeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let position = self.position;
        let Some((file, start, end)) = self
            .volumes
            .iter_mut()
            .find(|(_, start, end)| (*start..*end).contains(&position))
        else {
            return Ok(0);
        };
        file.seek(SeekFrom::Start(position - *start))?;
        let read = file.take(*end - position).read(buffer)?;
        if read == 0 && !buffer.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for VolumeReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position = next
            .filter(|next| *next <= self.len)
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;
    use tar::EntryType;
    use tempfile::tempdir;

    pub(crate) fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap();
    }

    fn write_7z(path: &Path, name: &str, content: &[u8], password: Option<&str>) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(path).unwrap();
        let mut methods = Vec::new();
        if let Some(password) = password {
            methods.push(
                sevenz_rust2::encoder_options::AesEncoderOptions::new(password.into()).into(),
            );
        }
        methods.push(sevenz_rust2::EncoderMethod::COPY.into());
        writer.set_content_methods(methods);
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(Cursor::new(content)),
            )
            .unwrap();
        writer.finish().unwrap();
    }

    fn rar_builder(entries: &[(&str, &[u8])]) -> rars::Builder {
        let mut builder = rars::Builder::new(rars::ArchiveVersion::Rar50).store(true);
        for (name, content) in entries {
            builder
                .add_bytes(
                    name.as_bytes().to_vec(),
                    content.to_vec(),
                    None,
                    Some(0o755),
                )
                .unwrap();
        }
        builder
    }

    fn tar_entry(name: &str, kind: EntryType, data: &[u8]) -> Vec<u8> {
        let mut header = tar::Header::new_ustar();
        header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(kind);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(data);
        bytes.resize(bytes.len().next_multiple_of(512), 0);
        bytes
    }

    fn extract_path(archive: &Path, password: Option<&str>) -> Result<PathBuf> {
        let plan = plan(archive).expect("archive plan");
        extract(&plan, password, &CancellationToken::new())
    }

    #[test]
    fn format_detection_ignores_suffix_case() {
        for (name, format) in [
            ("x.ZIP", Some(Format::Zip)),
            ("x.7z.001", Some(Format::SevenZ)),
            ("x.part01.RAR", Some(Format::Rar)),
            ("x.TGZ", Some(Format::TarGz)),
            ("x.tar.zst", Some(Format::TarZst)),
            ("x.zip.001", None),
            ("x.tar.bz2", None),
            ("x.gz", None),
        ] {
            assert_eq!(Format::of(name), format, "{name}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn plans_match_volumes_case_sensitively() {
        let tmp = tempdir().unwrap();
        for name in [
            "photos.zip",
            "Photos.zip",
            "game.part01.rar",
            "game.PART02.rar",
            "Game.part03.rar",
        ] {
            std::fs::write(tmp.path().join(name), b"x").unwrap();
        }

        let zip = plan(&tmp.path().join("photos.zip")).unwrap();
        assert_eq!(zip.members, [tmp.path().join("photos.zip")]);

        let rar = plan(&tmp.path().join("game.PART02.rar")).unwrap();
        assert_eq!(rar.first_volume, tmp.path().join("game.part01.rar"));
        assert_eq!(rar.members.len(), 2);
        assert_eq!(rar.readiness(), Readiness::Incomplete);
    }

    #[test]
    fn split_7z_volumes_extract_and_gaps_are_incomplete() {
        let tmp = tempdir().unwrap();
        let whole = tmp.path().join("whole.7z");
        write_7z(&whole, "nested/hello.txt", b"hello from volumes", None);
        let bytes = std::fs::read(&whole).unwrap();
        let (first, second) = bytes.split_at(bytes.len() / 2);
        std::fs::write(tmp.path().join("game.7z.001"), first).unwrap();
        std::fs::write(tmp.path().join("game.7z.002"), second).unwrap();

        let set = plan(&tmp.path().join("game.7z.002")).unwrap();
        assert_eq!(set.readiness(), Readiness::Ready);
        let out = extract(&set, None, &CancellationToken::new()).unwrap();
        assert_eq!(out, tmp.path().join("game"));
        assert_eq!(
            std::fs::read(out.join("nested/hello.txt")).unwrap(),
            b"hello from volumes"
        );

        std::fs::rename(
            tmp.path().join("game.7z.002"),
            tmp.path().join("game.7z.003"),
        )
        .unwrap();
        let gap = plan(&tmp.path().join("game.7z.003")).unwrap();
        assert_eq!(gap.readiness(), Readiness::Incomplete);
    }

    #[test]
    fn rar_sets_extract_through_rars_and_need_every_volume() {
        let tmp = tempdir().unwrap();
        let payload = vec![7_u8; 900];
        let volumes = rar_builder(&[("dir/run.sh", &payload), ("notes.txt", b"notes")])
            .volume_size(Some(600))
            .build_volumes(None)
            .unwrap();
        assert!(volumes.len() > 1);
        let volume_path = |number: usize| {
            tmp.path().join(format!(
                "Some_Game_--_uploader.example_--_.part{number}.rar"
            ))
        };
        for (index, volume) in volumes.iter().enumerate() {
            std::fs::write(volume_path(index + 1), volume).unwrap();
        }

        let set = plan(&volume_path(2)).unwrap();
        assert_eq!(set.readiness(), Readiness::Ready);
        let out = extract(&set, None, &CancellationToken::new()).unwrap();
        assert_eq!(out.file_name(), Some("Some Game".as_ref()));

        assert_eq!(std::fs::read(out.join("dir/run.sh")).unwrap(), payload);
        assert_eq!(std::fs::read(out.join("notes.txt")).unwrap(), b"notes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(out.join("dir/run.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o100, 0);
        }

        std::fs::remove_file(volume_path(volumes.len())).unwrap();
        assert_eq!(
            plan(&volume_path(1)).unwrap().readiness(),
            Readiness::Incomplete
        );
    }

    #[test]
    fn rar_links_are_never_created() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("links.rar");
        let mut builder = rar_builder(&[("file.txt", b"file")]);
        builder
            .add_unix_symlink(b"link".to_vec(), b"/etc/passwd".to_vec(), false, None, None)
            .unwrap();
        std::fs::write(&archive, builder.to_bytes().unwrap()).unwrap();

        let out = extract_path(&archive, None).unwrap();

        assert_eq!(std::fs::read(out.join("file.txt")).unwrap(), b"file");
        assert!(std::fs::symlink_metadata(out.join("link")).is_err());
    }

    #[test]
    fn encrypted_archives_ask_for_a_password() {
        let tmp = tempdir().unwrap();
        let rar = tmp.path().join("secret.rar");
        let bytes = rar_builder(&[("secret.txt", b"secret")])
            .password(Some(b"right".to_vec()))
            .to_bytes()
            .unwrap();
        std::fs::write(&rar, bytes).unwrap();
        let sevenz = tmp.path().join("secret.7z");
        write_7z(&sevenz, "secret.txt", b"secret", Some("right"));
        let zip = tmp.path().join("secret.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .with_aes_encryption(zip::AesMode::Aes256, "right");
        writer.start_file("secret.txt", options).unwrap();
        writer.write_all(b"secret").unwrap();
        writer.finish().unwrap();

        for archive in [&rar, &sevenz, &zip] {
            assert_eq!(
                plan(archive).unwrap().readiness(),
                Readiness::PasswordRequired,
                "{archive:?}"
            );
            assert!(
                matches!(
                    extract_path(archive, Some("wrong")),
                    Err(ShioError::PasswordRequired)
                ),
                "{archive:?}"
            );
            let out = extract_path(archive, Some("right")).unwrap();
            assert_eq!(std::fs::read(out.join("secret.txt")).unwrap(), b"secret");
        }
    }

    #[test]
    fn existing_destinations_are_never_merged_or_replaced() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("test.zip");
        write_zip(&archive, &[("hello.txt", b"hello")]);
        let existing = tmp.path().join("test");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("keep.txt"), b"keep").unwrap();
        std::fs::create_dir(tmp.path().join("test (2)")).unwrap();

        let out = extract_path(&archive, None).unwrap();

        assert_eq!(out, tmp.path().join("test (3)"));
        assert_eq!(std::fs::read(out.join("hello.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(existing.join("keep.txt")).unwrap(), b"keep");
        assert!(!existing.join("hello.txt").exists());
        assert_eq!(
            std::fs::read_dir(tmp.path().join("test (2)"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn cancelled_extraction_publishes_nothing() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("test.zip");
        write_zip(&archive, &[("hello.txt", b"hello")]);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = extract(&plan(&archive).unwrap(), None, &cancel);

        assert!(matches!(result, Err(ShioError::Cancelled)));
        assert!(!tmp.path().join("test").exists());
    }

    #[test]
    fn stale_staging_is_removed_but_symlinks_are_not_followed() {
        let tmp = tempdir().unwrap();
        let stale = tmp.path().join(format!("{STAGING_PREFIX}abc"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("partial.bin"), b"x").unwrap();
        std::fs::write(tmp.path().join("keep.zip"), b"x").unwrap();
        #[cfg(unix)]
        {
            let outside = tempdir().unwrap();
            std::fs::write(outside.path().join("precious"), b"x").unwrap();
            std::os::unix::fs::symlink(
                outside.path(),
                tmp.path().join(format!("{STAGING_PREFIX}link")),
            )
            .unwrap();
            remove_stale_staging(tmp.path()).unwrap();
            assert!(outside.path().join("precious").exists());
        }
        remove_stale_staging(tmp.path()).unwrap();

        assert!(!stale.exists());
        assert!(tmp.path().join("keep.zip").exists());
    }

    #[test]
    fn hostile_entries_are_rejected_without_leaving_output() {
        let tmp = tempdir().unwrap();
        let mut archives = Vec::new();

        let zip = tmp.path().join("traversal.zip");
        write_zip(&zip, &[("../evil.txt", b"evil")]);
        archives.push(zip);
        let zip_link = tmp.path().join("link.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip_link).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer.add_symlink("link", "/etc/passwd", options).unwrap();
        writer.finish().unwrap();
        archives.push(zip_link);

        let sevenz = tmp.path().join("traversal.7z");
        write_7z(&sevenz, "../evil.txt", b"evil", None);
        archives.push(sevenz);

        let rar = tmp.path().join("device.rar");
        let bytes = rar_builder(&[("CON.txt", b"evil")]).to_bytes().unwrap();
        std::fs::write(&rar, bytes).unwrap();
        archives.push(rar);

        let mut duplicate = tar_entry("a.txt", EntryType::Regular, b"first");
        duplicate.extend(tar_entry("a.txt", EntryType::Regular, b"second"));
        for (index, bytes) in [
            tar_entry("../evil.txt", EntryType::Regular, b"evil"),
            tar_entry("/evil.txt", EntryType::Regular, b"evil"),
            tar_entry("a\\..\\..\\evil.txt", EntryType::Regular, b"evil"),
            tar_entry("link", EntryType::Symlink, b""),
            tar_entry("hard", EntryType::Link, b""),
            duplicate,
        ]
        .into_iter()
        .enumerate()
        {
            let tar = tmp.path().join(format!("hostile{index}.tar"));
            std::fs::write(&tar, bytes).unwrap();
            archives.push(tar);
        }

        for archive in &archives {
            assert!(extract_path(archive, None).is_err(), "{archive:?}");
        }
        assert!(!tmp.path().join("evil.txt").exists());
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            archives.len()
        );
    }

    #[test]
    fn tarballs_from_common_tools_extract() {
        let tmp = tempdir().unwrap();
        let mut tarball = tar_entry("pax_global_header", EntryType::XGlobalHeader, b"");
        tarball.extend(tar_entry("./", EntryType::Directory, b""));
        tarball.extend(tar_entry("./bin/", EntryType::Directory, b""));
        tarball.extend(tar_entry("./bin/tool", EntryType::Regular, b"#!/bin/sh"));
        tarball.extend(tar_entry("./second", EntryType::Regular, b"second member"));
        tarball.extend([0; 1024]);
        let split = tarball.len() - 2048;
        let archive = tmp.path().join("release.tar.gz");
        let mut gz = Vec::new();
        for part in [&tarball[..split], &tarball[split..]] {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(part).unwrap();
            gz.extend(encoder.finish().unwrap());
        }
        std::fs::write(&archive, gz).unwrap();

        let out = extract_path(&archive, None).unwrap();

        assert_eq!(out, tmp.path().join("release"));
        assert_eq!(std::fs::read(out.join("bin/tool")).unwrap(), b"#!/bin/sh");
        assert_eq!(std::fs::read(out.join("second")).unwrap(), b"second member");
    }

    #[test]
    fn size_and_count_limits_stop_bombs() {
        let tmp = tempdir().unwrap();
        let file_limit = usize::try_from(MAX_EXTRACTED_FILE_BYTES).unwrap();
        let oversized = vec![0; file_limit + 1];
        let part = vec![0; file_limit - 24];
        let names: Vec<String> = (0..=MAX_EXTRACTED_FILES)
            .map(|n| format!("{n}.txt"))
            .collect();
        let many: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), &b"x"[..]))
            .collect();
        let cases: [&[(&str, &[u8])]; 3] = [
            &[("large.bin", &oversized[..])],
            &[
                ("a.bin", &part[..]),
                ("b.bin", &part[..]),
                ("c.bin", &part[..]),
            ],
            &many,
        ];
        for (index, entries) in cases.into_iter().enumerate() {
            let archive = tmp.path().join(format!("bomb{index}.zip"));
            write_zip(&archive, entries);
            assert!(extract_path(&archive, None).is_err(), "case {index}");
        }

        let tar = tmp.path().join("bomb.tar");
        std::fs::write(&tar, tar_entry("large.bin", EntryType::Regular, &oversized)).unwrap();
        assert!(extract_path(&tar, None).is_err());

        let value = "x".repeat(MAX_TAR_HEADER_BYTES);
        let record = format!("{} comment={value}\n", value.len() + 17);
        let mut pax = tar_entry("PaxHeader", EntryType::XHeader, record.as_bytes());
        pax.extend(tar_entry("small.txt", EntryType::Regular, b"x"));
        let tar = tmp.path().join("header-bomb.tar");
        std::fs::write(&tar, pax).unwrap();
        assert!(extract_path(&tar, None).is_err());

        let rar = tmp.path().join("bomb.rar");
        let bomb = rar_builder(&[("large.bin", &oversized[..])])
            .to_bytes()
            .unwrap();
        std::fs::write(&rar, bomb).unwrap();
        assert!(extract_path(&rar, None).is_err());
    }

    #[test]
    fn corrupt_archives_are_not_ready() {
        let tmp = tempdir().unwrap();
        for name in ["fake.rar", "fake.zip", "fake.7z"] {
            let path = tmp.path().join(name);
            std::fs::write(&path, b"not an archive").unwrap();
            assert_eq!(
                plan(&path).unwrap().readiness(),
                Readiness::Incomplete,
                "{name}"
            );
        }
    }
}
