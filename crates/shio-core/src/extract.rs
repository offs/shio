#![allow(clippy::case_sensitive_file_extension_comparisons)]

use crate::error::{Result, ShioError};
use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

const MAX_EXTRACTED_FILES: usize = 100_000;
#[cfg(not(test))]
const MAX_EXTRACTED_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
#[cfg(test)]
const MAX_EXTRACTED_FILE_BYTES: u64 = 1024;
#[cfg(not(test))]
const MAX_EXTRACTED_TOTAL_BYTES: u64 = 64 * 1024 * 1024 * 1024;
#[cfg(test)]
const MAX_EXTRACTED_TOTAL_BYTES: u64 = 2048;

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
    #[must_use]
    pub(crate) fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        if name.ends_with(".zip") {
            Some(Self::Zip)
        } else if name.ends_with(".7z") {
            Some(Self::SevenZ)
        } else if name.ends_with(".rar") {
            Some(Self::Rar)
        } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if name.ends_with(".tar.zst") || name.ends_with(".tzst") {
            Some(Self::TarZst)
        } else if name.ends_with(".tar") {
            Some(Self::Tar)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Plan {
    pub(crate) first_volume: PathBuf,
    pub(crate) base_name: String,
    pub(crate) members: Vec<PathBuf>,
}

#[must_use]
pub(crate) fn plan(completed: &Path) -> Option<Plan> {
    let info = SetInfo::from(completed)?;
    let dir = completed.parent()?.to_path_buf();
    let entries = scan(&dir, &info)?;
    let first_volume = info.first_volume(&dir, &entries);
    if !first_volume.exists() {
        return None;
    }
    Some(Plan {
        first_volume,
        base_name: info.base,
        members: entries.paths,
    })
}

#[must_use]
pub(crate) fn is_complete(plan: &Plan) -> bool {
    let Some(format) = Format::of(&plan.first_volume) else {
        return false;
    };
    match format {
        Format::Rar => rar_is_complete(&plan.first_volume).unwrap_or(false),
        Format::SevenZ => sevenz_is_complete(&plan.first_volume).unwrap_or(false),
        Format::Zip => zip_is_complete(&plan.first_volume).unwrap_or(false),
        Format::Tar | Format::TarGz | Format::TarZst => true,
    }
}

fn rar_is_complete(first_volume: &Path) -> Result<bool> {
    let archive = unrar::Archive::new(first_volume).as_first_part();
    let opened = archive
        .open_for_listing()
        .map_err(|e| ShioError::Extract(e.to_string()))?;
    for entry in opened {
        if entry.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn sevenz_is_complete(first_volume: &Path) -> Result<bool> {
    let file = File::open(first_volume)?;
    match sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty()) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn zip_is_complete(first_volume: &Path) -> Result<bool> {
    let file = File::open(first_volume)?;
    match zip::ZipArchive::new(file) {
        Ok(mut archive) => {
            for i in 0..archive.len() {
                if archive.by_index(i).is_err() {
                    return Ok(false);
                }
            }
            Ok(true)
        },
        Err(_) => Ok(false),
    }
}

#[derive(Debug, Clone)]
struct SetInfo {
    base: String,
    kind: SetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetKind {
    RarPart,
    SevenZ,
    ZipSplit,
    Standalone(Format),
}

impl SetInfo {
    fn from(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();

        if let Some(base) = strip_rar_part(&name) {
            return Some(Self {
                base,
                kind: SetKind::RarPart,
            });
        }
        if let Some(base) = strip_sevenz_volume(&name) {
            return Some(Self {
                base,
                kind: SetKind::SevenZ,
            });
        }
        if let Some(base) = strip_zip_split(&name) {
            return Some(Self {
                base,
                kind: SetKind::ZipSplit,
            });
        }

        let format = Format::of(path)?;
        let base = standalone_base(&name, format);
        Some(Self {
            base,
            kind: SetKind::Standalone(format),
        })
    }

    fn index_of(&self, path: &Path) -> Option<u32> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        match self.kind {
            SetKind::RarPart => {
                let stem = name.strip_suffix(".rar")?;
                stem.strip_prefix(&self.base)?
                    .strip_prefix(".part")?
                    .parse()
                    .ok()
            },
            SetKind::SevenZ => {
                let tail = name.strip_prefix(&self.base)?.strip_prefix(".7z.")?;
                tail.parse().ok()
            },
            SetKind::ZipSplit => {
                if name == format!("{}.zip", self.base) {
                    return Some(0);
                }
                let tail = name.strip_prefix(&self.base)?.strip_prefix(".z")?;
                tail.parse().ok()
            },
            SetKind::Standalone(_) => Some(0),
        }
    }

    fn first_volume(&self, dir: &Path, entries: &Entries) -> PathBuf {
        match self.kind {
            SetKind::RarPart => {
                let p1 = dir.join(format!("{}.part01.rar", self.base));
                if p1.exists() {
                    return p1;
                }
                let p1b = dir.join(format!("{}.part1.rar", self.base));
                if p1b.exists() {
                    return p1b;
                }
                entries.paths.first().cloned().unwrap_or(p1)
            },
            SetKind::SevenZ => dir.join(format!("{}.7z.001", self.base)),
            SetKind::ZipSplit => dir.join(format!("{}.zip", self.base)),
            SetKind::Standalone(_) => entries
                .paths
                .first()
                .cloned()
                .unwrap_or_else(|| dir.join(&self.base)),
        }
    }
}

#[derive(Debug)]
struct Entries {
    paths: Vec<PathBuf>,
}

fn scan(dir: &Path, info: &SetInfo) -> Option<Entries> {
    let read = std::fs::read_dir(dir).ok()?;
    let mut found: Vec<(u32, PathBuf)> = Vec::new();
    for entry in read {
        let Ok(entry) = entry else {
            return None;
        };
        let path = entry.path();
        let Some(index) = info.index_of(&path) else {
            continue;
        };
        found.push((index, path));
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by_key(|(i, _)| *i);
    let paths: Vec<PathBuf> = found.into_iter().map(|(_, p)| p).collect();
    Some(Entries { paths })
}

fn strip_rar_part(lower: &str) -> Option<String> {
    let stem = lower.strip_suffix(".rar")?;
    let idx = stem.rfind(".part")?;
    let after = &stem[idx + 5..];
    if after.is_empty() || !after.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(stem[..idx].to_string())
}

fn strip_sevenz_volume(lower: &str) -> Option<String> {
    let (base_with_ext, ext) = lower.rsplit_once('.')?;
    if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let base = base_with_ext.strip_suffix(".7z")?;
    Some(base.to_string())
}

fn strip_zip_split(lower: &str) -> Option<String> {
    let (base, ext) = lower.rsplit_once('.')?;
    if !ext.starts_with('z') || ext.len() < 2 || ext == "zip" {
        return None;
    }
    if !ext[1..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(base.to_string())
}

fn standalone_base(lower: &str, format: Format) -> String {
    let suffix_len = match format {
        Format::TarGz if lower.ends_with(".tar.gz") => 7,
        Format::TarZst if lower.ends_with(".tar.zst") => 8,
        Format::TarZst => 5,
        Format::TarGz | Format::Tar | Format::Zip | Format::Rar => 4,
        Format::SevenZ => 3,
    };
    lower[..lower.len() - suffix_len].to_string()
}

pub(crate) fn extract(archive: &Path, dest: &Path, password: Option<&str>) -> Result<PathBuf> {
    let format = Format::of(archive).ok_or_else(|| {
        ShioError::Extract(format!("unknown archive format: {}", archive.display()))
    })?;
    std::fs::create_dir_all(dest)?;
    match format {
        Format::Zip => extract_zip(archive, dest, password),
        Format::SevenZ => extract_7z(archive, dest, password),
        Format::Rar => extract_rar(archive, dest, password),
        Format::TarGz => extract_tar_gz(archive, dest),
        Format::TarZst => extract_tar_zst(archive, dest),
        Format::Tar => extract_tar(archive, dest),
    }?;
    Ok(dest.to_path_buf())
}

fn extract_zip(archive: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
    use std::io::copy;
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(map_zip_err)?;
    let mut guard = ExtractGuard::new();
    for i in 0..zip.len() {
        let encrypted = zip.by_index_raw(i).is_ok_and(|f| f.encrypted());
        let mut entry = if encrypted {
            let Some(pw) = password else {
                return Err(ShioError::PasswordRequired);
            };
            zip.by_index_decrypt(i, pw.as_bytes())
                .map_err(map_zip_err)?
        } else {
            zip.by_index(i).map_err(map_zip_err)?
        };
        if entry.is_symlink() {
            return Err(ShioError::Extract(
                "archive contains unsupported zip entry".into(),
            ));
        }
        let out_path = safe_join(dest, Path::new(entry.name()))?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if !entry.is_file() {
            return Err(ShioError::Extract(
                "archive contains unsupported zip entry".into(),
            ));
        }
        guard.record_file(entry.size())?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&out_path)?;
        copy(&mut entry, &mut out)?;
    }
    Ok(())
}

fn map_zip_err(e: zip::result::ZipError) -> ShioError {
    match e {
        zip::result::ZipError::InvalidPassword => ShioError::PasswordRequired,
        other => ShioError::Extract(other.to_string()),
    }
}

fn extract_7z(archive: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
    use std::io::copy;

    let file = File::open(archive)?;
    let pw = password.map_or_else(sevenz_rust2::Password::empty, sevenz_rust2::Password::new);
    let mut guard = ExtractGuard::new();
    sevenz_rust2::decompress_with_extract_fn_and_password(file, dest, pw, |entry, reader, _| {
        let out_path = safe_join(dest, Path::new(entry.name())).map_err(|e| sevenz_io_error(&e))?;
        if entry.is_directory() {
            std::fs::create_dir_all(out_path).map_err(sevenz_rust2::Error::from)?;
            return Ok(true);
        }
        guard
            .record_file(entry.size())
            .map_err(|e| sevenz_io_error(&e))?;
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(sevenz_rust2::Error::from)?;
        }
        let mut out = File::create(out_path).map_err(sevenz_rust2::Error::from)?;
        copy(reader, &mut out).map_err(sevenz_rust2::Error::from)?;
        Ok(true)
    })
    .map_err(map_7z_err)
}

fn map_7z_err(e: sevenz_rust2::Error) -> ShioError {
    match e {
        sevenz_rust2::Error::PasswordRequired => ShioError::PasswordRequired,
        other => ShioError::Extract(other.to_string()),
    }
}

fn sevenz_io_error(e: &ShioError) -> sevenz_rust2::Error {
    sevenz_rust2::Error::from(io::Error::other(e.to_string()))
}

fn extract_rar(archive: &Path, dest: &Path, password: Option<&str>) -> Result<()> {
    let path = archive.to_path_buf();
    let mut open = match password {
        Some(pw) => unrar::Archive::with_password(&path, pw).open_for_processing(),
        None => unrar::Archive::new(&path).open_for_processing(),
    }
    .map_err(|e| map_rar_err(&e))?;
    let mut guard = ExtractGuard::new();
    while let Some(header) = open.read_header().map_err(|e| map_rar_err(&e))? {
        let out_path = safe_join(dest, &header.entry().filename)?;
        open = if header.entry().is_file() {
            guard.record_file(header.entry().unpacked_size)?;
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            header.extract_to(&out_path).map_err(|e| map_rar_err(&e))?
        } else {
            header.skip().map_err(|e| map_rar_err(&e))?
        };
    }
    Ok(())
}

fn map_rar_err(e: &unrar::error::UnrarError) -> ShioError {
    use unrar::error::Code;
    match e.code {
        Code::MissingPassword | Code::BadPassword => ShioError::PasswordRequired,
        _ => ShioError::Extract(e.to_string()),
    }
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let reader = flate2::read::GzDecoder::new(file);
    unpack_tar(reader, dest)
}

fn extract_tar_zst(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let reader = zstd::Decoder::new(file).map_err(ShioError::Io)?;
    unpack_tar(reader, dest)
}

fn extract_tar(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    unpack_tar(file, dest)
}

fn unpack_tar<R: io::Read>(reader: R, dest: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_permissions(false);
    let dest = dest.to_path_buf();
    let mut guard = ExtractGuard::new();

    for entry in tar
        .entries()
        .map_err(|e| ShioError::Extract(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| ShioError::Extract(e.to_string()))?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(ShioError::Extract(
                "archive contains unsupported tar entry".into(),
            ));
        }

        let size = entry.header().size().unwrap_or(u64::MAX);
        if kind.is_file() {
            guard.record_file(size)?;
        }

        let path = entry
            .path()
            .map_err(|e| ShioError::Extract(e.to_string()))?;
        let out_path = safe_join(&dest, path.as_ref())?;
        if kind.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&out_path)?;
        io::copy(&mut entry, &mut out)?;
        out.flush()?;
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ExtractGuard {
    files: usize,
    total_bytes: u64,
}

impl ExtractGuard {
    const fn new() -> Self {
        Self {
            files: 0,
            total_bytes: 0,
        }
    }

    fn record_file(&mut self, size: u64) -> Result<()> {
        self.files += 1;
        if self.files > MAX_EXTRACTED_FILES {
            return Err(ShioError::Extract("archive contains too many files".into()));
        }
        if size > MAX_EXTRACTED_FILE_BYTES {
            return Err(ShioError::Extract("archive entry is too large".into()));
        }
        self.total_bytes = self
            .total_bytes
            .checked_add(size)
            .ok_or_else(|| ShioError::Extract("archive expanded size is too large".into()))?;
        if self.total_bytes > MAX_EXTRACTED_TOTAL_BYTES {
            return Err(ShioError::Extract(
                "archive expanded size is too large".into(),
            ));
        }
        Ok(())
    }
}

fn safe_join(dest: &Path, member: &Path) -> Result<PathBuf> {
    crate::path::validate_relative_path(member)
        .map_err(|_| ShioError::Extract("archive entry escapes destination".into()))?;
    Ok(dest.join(member))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};
    use tempfile::tempdir;

    #[test]
    fn format_detection() {
        assert_eq!(Format::of(Path::new("x.zip")), Some(Format::Zip));
        assert_eq!(Format::of(Path::new("x.7z")), Some(Format::SevenZ));
        assert_eq!(Format::of(Path::new("x.rar")), Some(Format::Rar));
        assert_eq!(Format::of(Path::new("x.tar.gz")), Some(Format::TarGz));
        assert_eq!(Format::of(Path::new("x.tgz")), Some(Format::TarGz));
        assert_eq!(Format::of(Path::new("x.tar.zst")), Some(Format::TarZst));
        assert_eq!(Format::of(Path::new("x.tar")), Some(Format::Tar));
        assert_eq!(Format::of(Path::new("x.mp4")), None);
    }

    #[test]
    fn plan_standalone_zip_returns_zip() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("foo.zip");
        std::fs::write(&archive, b"pk").unwrap();
        let p = plan(&archive).unwrap();
        assert_eq!(p.base_name, "foo");
        assert_eq!(p.first_volume, archive);
    }

    #[test]
    fn plan_rar_part_identifies_base() {
        let tmp = tempdir().unwrap();
        let part01 = tmp.path().join("game.part01.rar");
        let part02 = tmp.path().join("game.part02.rar");
        std::fs::write(&part01, b"a").unwrap();
        std::fs::write(&part02, b"b").unwrap();
        let p = plan(&part02).unwrap();
        assert_eq!(p.base_name, "game");
        assert_eq!(p.first_volume, part01);
        assert_eq!(p.members.len(), 2);
    }

    #[test]
    fn plan_7z_set() {
        let tmp = tempdir().unwrap();
        let p1 = tmp.path().join("game.7z.001");
        let p2 = tmp.path().join("game.7z.002");
        std::fs::write(&p1, b"a").unwrap();
        std::fs::write(&p2, b"b").unwrap();
        let p = plan(&p2).unwrap();
        assert_eq!(p.base_name, "game");
        assert_eq!(p.first_volume, p1);
    }

    #[test]
    fn is_complete_rejects_fake_rar() {
        let tmp = tempdir().unwrap();
        let fake = tmp.path().join("fake.rar");
        std::fs::write(&fake, b"not a real rar").unwrap();
        let p = plan(&fake).unwrap();
        assert!(!is_complete(&p));
    }

    #[test]
    fn extract_zip_roundtrip() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("test.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("hello.txt", opts).unwrap();
        zw.write_all(b"hello world").unwrap();
        zw.finish().unwrap();

        let dest = tmp.path().join("out");
        extract(&archive, &dest, None).unwrap();
        let content = std::fs::read_to_string(dest.join("hello.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn extract_7z_roundtrip() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("test.7z");
        write_7z_archive(&archive, "nested/hello.txt", b"hello world");

        let dest = tmp.path().join("out");
        extract(&archive, &dest, None).unwrap();
        let content = std::fs::read_to_string(dest.join("nested").join("hello.txt")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn extract_7z_rejects_path_traversal_entry() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("traversal.7z");
        write_7z_archive(&archive, "../evil.txt", b"evil");

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn extract_zip_rejects_path_traversal_entry() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("traversal.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("../evil.txt", opts).unwrap();
        zw.write_all(b"evil").unwrap();
        zw.finish().unwrap();

        let dest = tmp.path().join("out");
        let result = extract(&archive, &dest, None);

        assert!(result.is_err());
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn extract_zip_rejects_oversized_single_file() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("large.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("large.bin", opts).unwrap();
        zw.write_all(&vec![0; MAX_EXTRACTED_FILE_BYTES as usize + 1])
            .unwrap();
        zw.finish().unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
    }

    #[test]
    fn extract_zip_rejects_oversized_total_output() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("large-total.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("a.bin", opts).unwrap();
        zw.write_all(&vec![0; MAX_EXTRACTED_FILE_BYTES as usize])
            .unwrap();
        zw.start_file("b.bin", opts).unwrap();
        zw.write_all(&vec![
            0;
            MAX_EXTRACTED_TOTAL_BYTES as usize
                - MAX_EXTRACTED_FILE_BYTES as usize
                + 1
        ])
        .unwrap();
        zw.finish().unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
    }

    #[test]
    fn extract_zip_rejects_too_many_files() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("many.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 0..=MAX_EXTRACTED_FILES {
            zw.start_file(format!("{i}.txt"), opts).unwrap();
            zw.write_all(b"x").unwrap();
        }
        zw.finish().unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
    }

    #[test]
    fn is_complete_accepts_real_zip() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("real.zip");
        let file = File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("hello.txt", opts).unwrap();
        zw.write_all(b"hi").unwrap();
        zw.finish().unwrap();

        let p = plan(&archive).unwrap();
        assert!(is_complete(&p));
    }

    fn write_7z_archive(archive: &Path, entry_name: &str, content: &[u8]) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(archive).unwrap();
        writer.set_content_methods(vec![sevenz_rust2::EncoderMethod::COPY.into()]);
        let entry = sevenz_rust2::ArchiveEntry::new_file(entry_name);
        writer
            .push_archive_entry(entry, Some(Cursor::new(content)))
            .unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn extract_tar_gz_roundtrip() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("test.tar.gz");

        let gz = flate2::write::GzEncoder::new(
            File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gz);
        let data = b"hi";
        let mut header = tar::Header::new_gnu();
        header.set_path("hello.txt").unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, &data[..]).unwrap();
        let gz = tar.into_inner().unwrap();
        gz.finish().unwrap();

        let dest = tmp.path().join("out");
        extract(&archive, &dest, None).unwrap();
        let content = std::fs::read_to_string(dest.join("hello.txt")).unwrap();
        assert_eq!(content, "hi");
    }

    #[test]
    fn extract_tar_rejects_path_traversal() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("bad.tar");
        write_raw_tar_file(&archive, "../escape.txt", b"escape");

        let dest = tmp.path().join("out");
        let result = extract(&archive, &dest, None);

        assert!(result.is_err());
        assert!(!tmp.path().join("escape.txt").exists());
    }

    fn write_raw_tar_file(path: &Path, name: &str, data: &[u8]) {
        let mut file = File::create(path).unwrap();
        let mut header = [0_u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], data.len() as u64);
        write_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u32 = header.iter().map(|b| u32::from(*b)).sum();
        write_checksum(&mut header[148..156], checksum);
        file.write_all(&header).unwrap();
        file.write_all(data).unwrap();
        let padding = (512 - data.len() % 512) % 512;
        file.write_all(&vec![0; padding]).unwrap();
        file.write_all(&[0; 1024]).unwrap();
    }

    fn write_octal(dst: &mut [u8], value: u64) {
        let text = format!("{value:0width$o}\0", width = dst.len() - 1);
        dst.copy_from_slice(text.as_bytes());
    }

    fn write_checksum(dst: &mut [u8], value: u32) {
        let text = format!("{value:06o}\0 ");
        dst.copy_from_slice(text.as_bytes());
    }

    #[test]
    fn extract_tar_rejects_symlinks() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("link.tar");
        let file = File::create(&archive).unwrap();
        let mut tar = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_path("link").unwrap();
        header.set_link_name("target").unwrap();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        tar.append(&header, std::io::empty()).unwrap();
        tar.finish().unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
    }

    #[test]
    fn extract_tar_rejects_oversized_single_file() {
        let tmp = tempdir().unwrap();
        let archive = tmp.path().join("large.tar");
        let file = File::create(&archive).unwrap();
        let mut tar = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_path("large.bin").unwrap();
        header.set_size(MAX_EXTRACTED_FILE_BYTES + 1);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(
            &header,
            std::io::repeat(0).take(MAX_EXTRACTED_FILE_BYTES + 1),
        )
        .unwrap();
        tar.finish().unwrap();

        let result = extract(&archive, &tmp.path().join("out"), None);

        assert!(result.is_err());
    }
}
