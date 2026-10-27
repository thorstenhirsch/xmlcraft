//! Files: read, write atomically, and notice outside changes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use xmlcraft_engine::{FileStamp, MAX_OPEN_BYTES};

/// A file's bytes and what it looked like when they were read.
#[derive(Debug)]
pub struct Loaded {
    pub bytes: Vec<u8>,
    pub stamp: FileStamp,
}

/// What a file looks like now.
pub fn stamp_of(path: &Path) -> io::Result<FileStamp> {
    let meta = fs::metadata(path)?;
    Ok(FileStamp { len: meta.len(), modified: meta.modified().ok() })
}

/// Read a file. One larger than [`MAX_OPEN_BYTES`] is refused before any of it is read.
pub fn read_file(path: &Path) -> io::Result<Loaded> {
    let stamp = stamp_of(path)?;
    if stamp.len > MAX_OPEN_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the file is {} bytes, more than the {MAX_OPEN_BYTES} bytes XmlCraft opens", stamp.len),
        ));
    }
    let bytes = fs::read(path)?;
    // The file may have changed between the two looks; the stamp of the bytes read is the one that matters.
    let stamp = stamp_of(path).unwrap_or(stamp);
    Ok(Loaded { bytes, stamp })
}

/// The path a write should go to: a symbolic link is followed, so that saving replaces the file it points to and keeps the link.
fn write_target(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

/// Write a file atomically: the old bytes stay whole until the new ones are complete and on disk, and a failure leaves the old file as
/// it was. Returns the stamp of the new file.
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<FileStamp> {
    craft_atomic::atomic_write(&write_target(path), bytes)?;
    stamp_of(path)
}

/// How a file compares with the stamp taken when it was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChange {
    Unchanged,
    /// Its size or modification time differs.
    Modified,
    /// It is gone (or cannot be looked at).
    Missing,
}

pub fn changed_on_disk(path: &Path, stamp: &FileStamp) -> DiskChange {
    match stamp_of(path) {
        Ok(now) if now == *stamp => DiskChange::Unchanged,
        Ok(_) => DiskChange::Modified,
        Err(_) => DiskChange::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_round_trips_and_its_stamp_follows() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.xml");
        let stamp = write_file(&path, b"<a/>").expect("writes");
        assert_eq!(stamp.len, 4);
        let loaded = read_file(&path).expect("reads");
        assert_eq!((loaded.bytes.as_slice(), loaded.stamp), (&b"<a/>"[..], stamp));
        assert_eq!(changed_on_disk(&path, &stamp), DiskChange::Unchanged);
        // Overwriting replaces the whole file.
        let second = write_file(&path, b"<a>longer</a>").expect("writes");
        assert_eq!(read_file(&path).expect("reads").bytes, b"<a>longer</a>");
        assert_eq!(changed_on_disk(&path, &stamp), DiskChange::Modified);
        assert_eq!(changed_on_disk(&path, &second), DiskChange::Unchanged);
        fs::remove_file(&path).expect("removes");
        assert_eq!(changed_on_disk(&path, &second), DiskChange::Missing);
        assert!(read_file(&path).is_err());
    }

    #[test]
    fn no_staging_file_is_left_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.xml");
        write_file(&path, b"x").expect("writes");
        write_file(&path, b"y").expect("writes");
        let names: Vec<_> = fs::read_dir(dir.path()).expect("lists").map(|e| e.expect("entry").file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("a.xml")]);
    }

    #[test]
    fn writing_to_a_missing_folder_fails_cleanly() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(write_file(&dir.path().join("no/such/dir/a.xml"), b"x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symbolic_link_keeps_the_link() {
        let dir = tempfile::tempdir().expect("temp dir");
        let real = dir.path().join("real.xml");
        let link = dir.path().join("link.xml");
        fs::write(&real, b"old").expect("writes");
        std::os::unix::fs::symlink(&real, &link).expect("links");
        write_file(&link, b"new").expect("writes");
        assert!(fs::symlink_metadata(&link).expect("stats").file_type().is_symlink(), "still a link");
        assert_eq!(fs::read(&real).expect("reads"), b"new", "the target was replaced");
    }

    #[test]
    fn a_file_over_the_limit_is_refused_without_being_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("big.xml");
        let file = fs::File::create(&path).expect("creates");
        file.set_len(MAX_OPEN_BYTES as u64 + 1).expect("sparse file");
        let error = read_file(&path).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
