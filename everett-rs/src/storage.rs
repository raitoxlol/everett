use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use fs2::FileExt;

use crate::error::{EverettError, Result};

fn bad(message: impl Into<String>) -> EverettError {
    EverettError::new(2, message)
}

fn io_error(error: std::io::Error) -> EverettError {
    bad(format!("peer storage: {error}"))
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    private_dir_io(path).map_err(io_error)
}

/// Persist each new directory's link in its parent, not just its contents.
/// Directory fsync is supported on both Linux and macOS; no Linux-only flags.
pub(crate) fn private_dir_io(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if !path.is_dir() {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| std::io::Error::other("missing directory parent"))?;
        if !parent.is_dir() {
            private_dir_io(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => (),
            Err(e) => return Err(e),
        }
    }
    let mode = fs::metadata(path)?.permissions().mode() & 0o700;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    File::open(path)?.sync_all()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| bad("missing parent"))?)
            .map_err(io_error)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(io_error)?;
    temp.write_all(bytes).map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist(path).map_err(|e| io_error(e.error))?;
    Ok(())
}

pub(crate) fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(io_error)?;
    file.lock_exclusive().map_err(io_error)?;
    Ok(file)
}
