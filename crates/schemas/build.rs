//! Packs everything under the repository's `schemas/` folder (except the manifest, which is for `cargo xtask schemas`) into one blob
//! that the crate embeds: an index of the files followed by each file deflated.
//!
//! Layout (all integers little-endian): `XSB1`, the number of files (u32); per file the length of its path (u16), the path, the length
//! of the file (u32), the length of its deflated bytes (u32) and where they start in the data that follows the index (u32); then the data.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

/// Files that are about the bundle rather than in it.
const SKIPPED: &[&str] = &["manifest.toml"];
/// A bundled file cannot be larger than this.
const MAX_FILE: usize = 64 * 1024 * 1024;

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), Box<dyn Error>> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = format!("{prefix}{name}");
        if entry.file_type()?.is_dir() {
            collect(&entry.path(), &format!("{path}/"), out)?;
        } else if !SKIPPED.contains(&path.as_str()) {
            out.push((path, entry.path()));
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?).join("../../schemas");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed=build.rs");
    let mut files = Vec::new();
    if root.is_dir() {
        collect(&root, "", &mut files)?;
    }
    // The index is searched by bisection, so it is in the order of the paths themselves (not of the folders).
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut index = Vec::new();
    let mut data = Vec::new();
    index.extend_from_slice(b"XSB1");
    index.extend_from_slice(&u32::try_from(files.len())?.to_le_bytes());
    for (path, file) in &files {
        let bytes = fs::read(file)?;
        if bytes.len() > MAX_FILE {
            return Err(format!("{path} is larger than {MAX_FILE} bytes").into());
        }
        let deflated = miniz_oxide::deflate::compress_to_vec(&bytes, 9);
        index.extend_from_slice(&u16::try_from(path.len())?.to_le_bytes());
        index.extend_from_slice(path.as_bytes());
        index.extend_from_slice(&u32::try_from(bytes.len())?.to_le_bytes());
        index.extend_from_slice(&u32::try_from(deflated.len())?.to_le_bytes());
        index.extend_from_slice(&u32::try_from(data.len())?.to_le_bytes());
        data.extend_from_slice(&deflated);
    }
    index.extend_from_slice(&data);
    fs::write(PathBuf::from(std::env::var("OUT_DIR")?).join("bundle.bin"), index)?;
    Ok(())
}
