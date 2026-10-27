//! Safe reads of cached receipts. A bad file is skipped instead of failing the caller.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::de::DeserializeOwned;

const FILE_LIMIT: u64 = 256 * 1024;

pub(super) fn read_record<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.len() > FILE_LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(FILE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > FILE_LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
