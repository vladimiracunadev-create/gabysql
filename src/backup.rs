//! Verified backup / restore / verify primitives.
//!
//! These operate on a closed-DB invariant: the source must not be open
//! by another `gabysql` process (the exclusive file lock from ADR-0013
//! enforces this). They go page-by-page, validate every CRC32 trailer
//! on read, and after writing the destination they re-open it and walk
//! it once more to confirm end-to-end integrity. The intent is a backup
//! command that **either produces a verified-good `.db` file or fails
//! loudly** — never a silent partial copy.

use crate::storage::Pager;
use crate::{DbError, DbResult};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Summary returned by [`backup`] and [`restore`]. Useful both for
/// human-readable CLI output and for callers that want to log a
/// structured result.
#[derive(Debug, Clone)]
pub struct BackupReport {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub pages: u32,
    pub bytes: u64,
}

/// Summary returned by [`verify`].
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub path: PathBuf,
    pub pages: u32,
    pub bytes: u64,
}

/// Stream every page of `src` into `dst`, validating each CRC32 page
/// trailer on read and re-validating end-to-end by re-opening `dst`
/// after the copy. The source must be closed by every other process
/// (otherwise the exclusive lock from `Pager::open` will reject it).
///
/// Refuses to overwrite an existing `dst` unless `force` is true.
pub fn backup(src: &Path, dst: &Path, force: bool) -> DbResult<BackupReport> {
    if src == dst {
        return Err(DbError::new(
            "backup source and destination are the same path",
        ));
    }
    if !force && dst.exists() {
        return Err(DbError::new(format!(
            "refusing to overwrite existing file: {}. Pass --force to overwrite.",
            dst.display()
        )));
    }

    // Phase 1: open src, replay any pending WAL, stream page-by-page
    // into a fresh dst file. `page_data` runs `verify_page_checksum`
    // internally on every page read, so a single corrupt page in src
    // aborts the backup before we publish a bad copy.
    let (header_clone, page_count, page_size) = {
        let mut src_pager = Pager::open(src)?;
        let header = src_pager.header();
        let page_size = header.page_size as usize;
        let page_count = header.page_count;

        let mut dst_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(dst)?;

        for page_no in 0..page_count {
            let data = src_pager.page_data(page_no)?;
            if data.len() != page_size {
                return Err(DbError::new(format!(
                    "page {} has unexpected size {} (expected {})",
                    page_no,
                    data.len(),
                    page_size
                )));
            }
            dst_file.write_all(&data)?;
        }
        dst_file.flush()?;
        dst_file.sync_all()?;
        drop(dst_file);
        src_pager.close()?;
        (header, page_count, page_size)
    };

    // Phase 2: end-to-end verification of the destination. Open it
    // fresh (this also acquires the file lock and runs header decode
    // which checks magic + version), then walk every page so each
    // CRC32 trailer is validated post-write.
    let mut dst_pager = Pager::open(dst)?;
    let dst_header = dst_pager.header();
    if dst_header != header_clone {
        dst_pager.close()?;
        return Err(DbError::new(
            "backup verification failed: destination header does not match source",
        ));
    }
    for page_no in 0..page_count {
        let _ = dst_pager.page_data(page_no)?;
    }
    dst_pager.close()?;

    Ok(BackupReport {
        src: src.to_path_buf(),
        dst: dst.to_path_buf(),
        pages: page_count,
        bytes: page_count as u64 * page_size as u64,
    })
}

/// Restore is currently the same physical operation as backup — both
/// produce a verified copy at the destination path. Kept as a separate
/// entry point so the CLI verb matches user intent ("restore from
/// `backups/demo.db.bak` to `demo.db`"), and so a future version can
/// add restore-specific behavior (atomic rename, pre-flight space
/// check, etc.) without renaming the public API.
pub fn restore(src: &Path, dst: &Path, force: bool) -> DbResult<BackupReport> {
    backup(src, dst, force)
}

/// Walk every page of `path` and validate its CRC32 trailer. The
/// `Pager::open` call itself already replays any pending WAL and
/// validates the header page; the loop below covers the rest.
pub fn verify(path: &Path) -> DbResult<VerifyReport> {
    let mut pager = Pager::open(path)?;
    let header = pager.header();
    let page_count = header.page_count;
    let page_size = header.page_size as usize;
    for page_no in 0..page_count {
        let _ = pager.page_data(page_no)?;
    }
    pager.close()?;
    Ok(VerifyReport {
        path: path.to_path_buf(),
        pages: page_count,
        bytes: page_count as u64 * page_size as u64,
    })
}
