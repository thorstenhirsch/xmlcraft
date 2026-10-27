//! Files: read, write atomically, and notice outside changes.

use xmlcraft_engine::FileStamp;

/// A file's bytes and what it looked like when they were read.
#[derive(Debug)]
pub struct Loaded {
    pub bytes: Vec<u8>,
    pub stamp: FileStamp,
}

/// How a file compares with the stamp taken when it was read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskChange {
    Unchanged,
    /// Its size, modification time or identity differs.
    Modified,
    /// It is gone (or cannot be looked at).
    Missing,
}

pub use imp::{changed_on_disk, read_file, stamp_of, write_file};

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::fs;
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};

    use xmlcraft_engine::{FileStamp, MAX_OPEN_BYTES};

    use super::{DiskChange, Loaded};

    fn stamp_from(meta: &fs::Metadata) -> FileStamp {
        #[cfg(unix)]
        let (changed, file_id) = {
            use std::os::unix::fs::MetadataExt;
            (Some((meta.ctime(), meta.ctime_nsec())), Some((meta.dev(), meta.ino())))
        };
        #[cfg(not(unix))]
        let (changed, file_id) = (None, None);
        FileStamp { len: meta.len(), modified: meta.modified().ok(), changed, file_id }
    }

    /// What a file looks like now.
    pub fn stamp_of(path: &Path) -> io::Result<FileStamp> {
        Ok(stamp_from(&fs::metadata(path)?))
    }

    fn too_large(len: u64) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, format!("the file is {len} bytes, more than the {MAX_OPEN_BYTES} bytes XmlCraft opens"))
    }

    /// How many times a file that changes while it is read is read again.
    const READ_ATTEMPTS: usize = 3;

    /// Read a file. Only a regular file is read (not a pipe or a device, which could block or never end), one larger than
    /// [`MAX_OPEN_BYTES`] is refused (also when it grows while being read), and the stamp returned describes the very file that was read,
    /// from the open handle: if the file changed while it was read, it is read again rather than returning bytes and a stamp that do not
    /// belong together.
    pub fn read_file(path: &Path) -> io::Result<Loaded> {
        for _ in 0..READ_ATTEMPTS {
            // Looking before opening keeps a pipe from blocking the open; the handle is checked again below.
            if !fs::metadata(path)?.is_file() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
            }
            let mut file = fs::File::open(path)?;
            let meta = file.metadata()?;
            if !meta.is_file() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
            }
            let before = stamp_from(&meta);
            if before.len > MAX_OPEN_BYTES as u64 {
                return Err(too_large(before.len));
            }
            let mut bytes = Vec::with_capacity(usize::try_from(before.len).unwrap_or(0));
            // One byte more than allowed, so a file that outgrew its size is noticed without reading all of it.
            (&mut file).take(MAX_OPEN_BYTES as u64 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > MAX_OPEN_BYTES {
                return Err(too_large(bytes.len() as u64));
            }
            let after = stamp_from(&file.metadata()?);
            if before == after && after.len == bytes.len() as u64 {
                return Ok(Loaded { bytes, stamp: after });
            }
        }
        Err(io::Error::other("the file keeps changing while it is read"))
    }

    /// How many symbolic links are followed to find where a write goes.
    const MAX_LINK_HOPS: usize = 40;

    /// The path a write should go to: symbolic links are followed (also a dangling one, whose target is then created), so that saving
    /// replaces the file a link points to and keeps the link.
    fn write_target(path: &Path) -> io::Result<PathBuf> {
        let mut current = path.to_path_buf();
        for _ in 0..MAX_LINK_HOPS {
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    let link = fs::read_link(&current)?;
                    current = match current.parent() {
                        Some(parent) if link.is_relative() => parent.join(link),
                        _ => link,
                    };
                }
                Ok(_) => return Ok(current),
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(current), // a new file
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::InvalidInput, "too many levels of symbolic links"))
    }

    /// Write a file atomically: the old bytes stay whole until the new ones are complete and on disk, and a failure leaves the old file as
    /// it was. Returns the stamp of the new file.
    pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<FileStamp> {
        craft_atomic::atomic_write(&write_target(path)?, bytes)?;
        stamp_of(path)
    }

    pub fn changed_on_disk(path: &Path, stamp: &FileStamp) -> DiskChange {
        match stamp_of(path) {
            Ok(now) if now == *stamp => DiskChange::Unchanged,
            Ok(_) => DiskChange::Modified,
            Err(_) => DiskChange::Missing,
        }
    }
}

/// On the web there are no files to open by path: every function says so.
#[cfg(target_arch = "wasm32")]
mod imp {
    use std::io;
    use std::path::Path;

    use xmlcraft_engine::FileStamp;

    use super::{DiskChange, Loaded};

    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    pub fn stamp_of(_path: &Path) -> io::Result<FileStamp> {
        unsupported()
    }

    pub fn read_file(_path: &Path) -> io::Result<Loaded> {
        unsupported()
    }

    pub fn write_file(_path: &Path, _bytes: &[u8]) -> io::Result<FileStamp> {
        unsupported()
    }

    pub fn changed_on_disk(_path: &Path, _stamp: &FileStamp) -> DiskChange {
        DiskChange::Missing
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::fs;
    use std::io;

    use xmlcraft_engine::MAX_OPEN_BYTES;

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
    fn only_regular_files_are_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = read_file(dir.path()).expect_err("a folder");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_refused_instead_of_blocking() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fifo = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo").arg(&fifo).status().expect("mkfifo runs");
        assert!(status.success());
        assert_eq!(read_file(&fifo).expect_err("a pipe").kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn a_write_of_the_same_size_that_restores_the_time_is_still_noticed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.xml");
        fs::write(&path, b"aaaa").expect("writes");
        let loaded = read_file(&path).expect("reads");
        assert!(loaded.stamp.changed.is_some() && loaded.stamp.file_id.is_some());
        // The inode change time has a coarse clock on some systems: let it tick.
        std::thread::sleep(std::time::Duration::from_millis(30));
        let file = fs::OpenOptions::new().write(true).open(&path).expect("opens");
        io::Write::write_all(&mut &file, b"bbbb").expect("writes");
        file.set_modified(loaded.stamp.modified.expect("has a time")).expect("puts the time back");
        drop(file);
        let now = stamp_of(&path).expect("stats");
        assert_eq!((now.len, now.modified), (loaded.stamp.len, loaded.stamp.modified), "size and modification time look the same");
        assert_eq!(changed_on_disk(&path, &loaded.stamp), DiskChange::Modified);
        assert_eq!(now.file_id, loaded.stamp.file_id, "it is the same file");
    }

    #[cfg(unix)]
    #[test]
    fn a_replacement_file_is_noticed_and_two_paths_to_one_file_share_an_identity() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("a.xml");
        let link = dir.path().join("link.xml");
        fs::write(&path, b"one").expect("writes");
        std::os::unix::fs::symlink(&path, &link).expect("links");
        let through_link = read_file(&link).expect("reads");
        let direct = read_file(&path).expect("reads");
        assert_eq!(through_link.stamp.file_id, direct.stamp.file_id);
        // Replacing the file by renaming another over it: different inode, same size.
        let other = dir.path().join("other.xml");
        fs::write(&other, b"two").expect("writes");
        fs::rename(&other, &path).expect("replaces");
        assert_eq!(changed_on_disk(&path, &direct.stamp), DiskChange::Modified);
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_dangling_link_creates_the_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("target.xml");
        let link = dir.path().join("link.xml");
        let second = dir.path().join("second.xml");
        std::os::unix::fs::symlink("target.xml", &link).expect("links"); // relative, and the target does not exist
        std::os::unix::fs::symlink("link.xml", &second).expect("links"); // a chain
        write_file(&second, b"new").expect("writes");
        assert!(
            fs::symlink_metadata(&link).expect("stats").file_type().is_symlink()
                && fs::symlink_metadata(&second).expect("stats").file_type().is_symlink()
        );
        assert_eq!(fs::read(&target).expect("reads"), b"new");
        // A loop of links is an error, not a hang.
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::os::unix::fs::symlink(&b, &a).expect("links");
        std::os::unix::fs::symlink(&a, &b).expect("links");
        assert!(write_file(&a, b"x").is_err());
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
