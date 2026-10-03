//! Unpacking `.tar.xz` and `.tar.gz` release archives into a staging directory.
//!
//! Release archives hold one top-level directory (`LLVM-19.1.0-Linux-X64/`,
//! `xpack-gcc-13.3.0-2/`), which is dropped so that its content becomes the install prefix. Nothing may land outside the destination:
//! absolute paths, `..` components and symlinks pointing out of the tree are refused.

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufReader, IsTerminal, Read};
use std::path::{Component, Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use tar::EntryType;

/// Unpacks the `.tar.xz` or `.tar.gz` at `archive`, told apart by name, into `dest`, without
/// its top-level directory. The error is a reason for the user.
pub fn unpack_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let name = archive.to_string_lossy();
    let gzip = name.ends_with(".tar.gz");
    if !gzip && !name.ends_with(".tar.xz") {
        return Err(format!(
            "{} is neither a .tar.xz nor a .tar.gz archive",
            archive.display()
        ));
    }
    let file =
        fs::File::open(archive).map_err(|e| format!("cannot read {}: {e}", archive.display()))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let progress = progress_bar(size);
    let reader = Counting {
        inner: BufReader::with_capacity(1 << 20, file),
        progress: progress.clone(),
    };
    let result = if gzip {
        unpack(flate2::read::GzDecoder::new(reader), dest)
    } else {
        unpack(liblzma::read::XzDecoder::new(reader), dest)
    };
    match &result {
        Ok(()) => progress.finish_and_clear(),
        Err(_) => progress.abandon(),
    }
    result
}

/// Unpacks the tar stream `reader` into `dest`, dropping its single top-level directory.
fn unpack(reader: impl Read, dest: &Path) -> Result<(), String> {
    let io = |what: &str, path: &Path| {
        let message = format!("cannot {what} {}", path.display());
        move |e: io::Error| format!("{message}: {e}")
    };
    fs::create_dir_all(dest).map_err(io("create", dest))?;
    let mut archive = tar::Archive::new(reader);
    let mut top: Option<OsString> = None;
    let entries = archive
        .entries()
        .map_err(|e| format!("cannot read the archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("cannot read the archive: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("cannot read the archive: {e}"))?
            .into_owned();
        let Some(rel) = strip_top(&path, &mut top)? else {
            continue;
        };
        let target = dest.join(&rel);
        let entry_type = entry.header().entry_type();
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io("create", parent))?;
        }
        match entry_type {
            EntryType::Directory => fs::create_dir_all(&target).map_err(io("create", &target))?,
            EntryType::Regular | EntryType::Continuous => {
                entry.unpack(&target).map_err(io("write", &target))?;
            }
            EntryType::Symlink => {
                let link = link_name(&entry, &path)?;
                if !stays_inside(&rel, &link) {
                    return Err(format!(
                        "the archive's symlink {} points outside it, to {}",
                        path.display(),
                        link.display()
                    ));
                }
                replace(&target)?;
                std::os::unix::fs::symlink(&link, &target).map_err(io("write", &target))?;
            }
            EntryType::Link => {
                let link = link_name(&entry, &path)?;
                let source = strip_top(&link, &mut top)?.ok_or_else(|| {
                    format!("the archive's hard link {} has no target", path.display())
                })?;
                replace(&target)?;
                fs::hard_link(dest.join(source), &target).map_err(io("write", &target))?;
            }
            EntryType::XGlobalHeader | EntryType::XHeader => {}
            other => {
                return Err(format!(
                    "the archive holds {}, of unsupported type {other:?}",
                    path.display()
                ));
            }
        }
    }
    if top.is_none() {
        return Err("the archive is empty".into());
    }
    Ok(())
}

/// `path` without its first component, which must be the same for every entry (`top`).
/// `None` for the top-level directory itself. Absolute paths and `..` are refused.
fn strip_top(path: &Path, top: &mut Option<OsString>) -> Result<Option<PathBuf>, String> {
    let unsafe_path = || format!("the archive holds an unsafe path: {}", path.display());
    let mut components = path.components().filter(|c| *c != Component::CurDir);
    let first = match components.next() {
        Some(Component::Normal(first)) => first,
        None => return Ok(None),
        Some(_) => return Err(unsafe_path()),
    };
    match top {
        Some(top) if top != first => {
            return Err(format!(
                "the archive has more than one top-level directory: {} and {}",
                top.to_string_lossy(),
                first.to_string_lossy()
            ));
        }
        Some(_) => {}
        None => *top = Some(first.to_owned()),
    }
    let mut rel = PathBuf::new();
    for component in components {
        match component {
            Component::Normal(name) => rel.push(name),
            Component::CurDir => {}
            _ => return Err(unsafe_path()),
        }
    }
    Ok((!rel.as_os_str().is_empty()).then_some(rel))
}

/// Whether a symlink at `rel` (relative to the root) pointing at `link` stays inside the root.
fn stays_inside(rel: &Path, link: &Path) -> bool {
    let mut depth = rel.components().count().saturating_sub(1);
    for component in link.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return false,
        }
    }
    true
}

fn link_name<R: Read>(entry: &tar::Entry<'_, R>, path: &Path) -> Result<PathBuf, String> {
    entry
        .link_name()
        .ok()
        .flatten()
        .map(|link| link.into_owned())
        .ok_or_else(|| format!("the archive's link {} has no target", path.display()))
}

/// Removes a file or symlink at `path` so a link can take its place.
fn replace(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() => {
            fs::remove_file(path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Advances `progress` by the compressed bytes read.
struct Counting<R> {
    inner: R,
    progress: ProgressBar,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.progress.inc(n as u64);
        Ok(n)
    }
}

fn progress_bar(size: u64) -> ProgressBar {
    eprintln!("Extracting");
    if !io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }
    ProgressBar::new(size).with_style(
        ProgressStyle::with_template("  [{bar:30}] {percent}%, {eta} left")
            .expect("valid template")
            .progress_chars("=> "),
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn header(path: &str, entry_type: EntryType, size: u64, mode: u32) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        // `set_path` refuses `..`; write the raw name so unsafe archives can be built.
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(entry_type);
        header.set_size(size);
        header.set_mode(mode);
        header.set_cksum();
        header
    }

    enum Item<'a> {
        Dir(&'a str),
        File(&'a str, &'a str, u32),
        Symlink(&'a str, &'a str),
        HardLink(&'a str, &'a str),
    }

    fn tar(items: &[Item<'_>]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for item in items {
            match *item {
                Item::Dir(path) => {
                    let h = header(path, EntryType::Directory, 0, 0o755);
                    builder.append(&h, io::empty()).unwrap();
                }
                Item::File(path, content, mode) => {
                    let h = header(path, EntryType::Regular, content.len() as u64, mode);
                    builder.append(&h, content.as_bytes()).unwrap();
                }
                Item::Symlink(path, target) | Item::HardLink(path, target) => {
                    let kind = if matches!(item, Item::Symlink(..)) {
                        EntryType::Symlink
                    } else {
                        EntryType::Link
                    };
                    let mut h = header(path, kind, 0, 0o777);
                    h.set_link_name(target).unwrap();
                    h.set_cksum();
                    builder.append(&h, io::empty()).unwrap();
                }
            }
        }
        builder.into_inner().unwrap()
    }

    fn unpack_items(items: &[Item<'_>]) -> (tempfile::TempDir, Result<(), String>) {
        let dir = tempfile::tempdir().unwrap();
        let result = unpack(tar(items).as_slice(), &dir.path().join("out"));
        (dir, result)
    }

    #[test]
    fn strips_the_top_directory_and_keeps_modes_and_links() {
        let (dir, result) = unpack_items(&[
            Item::Dir("LLVM-19.1.0-Linux-X64/"),
            Item::Dir("LLVM-19.1.0-Linux-X64/bin/"),
            Item::File("LLVM-19.1.0-Linux-X64/bin/clang-19", "#!/bin/sh\n", 0o755),
            Item::Symlink("LLVM-19.1.0-Linux-X64/bin/clang", "clang-19"),
            Item::Symlink("LLVM-19.1.0-Linux-X64/bin/clang++", "./clang"),
            Item::HardLink(
                "LLVM-19.1.0-Linux-X64/bin/ld.lld",
                "LLVM-19.1.0-Linux-X64/bin/clang-19",
            ),
            Item::File(
                "./LLVM-19.1.0-Linux-X64/lib/clang/19/include/stddef.h",
                "",
                0o644,
            ),
            Item::Symlink("LLVM-19.1.0-Linux-X64/lib/up", "../bin"),
        ]);
        result.unwrap();
        let out = dir.path().join("out");
        let mode = |p: &str| fs::metadata(out.join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("bin/clang-19"), 0o755);
        assert_eq!(mode("lib/clang/19/include/stddef.h"), 0o644);
        assert_eq!(
            fs::read_link(out.join("bin/clang")).unwrap(),
            Path::new("clang-19")
        );
        assert_eq!(
            fs::read_to_string(out.join("bin/clang++")).unwrap(),
            "#!/bin/sh\n"
        );
        assert_eq!(
            fs::read_to_string(out.join("bin/ld.lld")).unwrap(),
            "#!/bin/sh\n"
        );
        assert!(out.join("lib/up/clang-19").is_file());
        assert!(!out.join("LLVM-19.1.0-Linux-X64").exists());
    }

    #[test]
    fn refuses_paths_that_escape() {
        for (items, reason) in [
            (vec![Item::File("top/../../evil", "", 0o644)], "unsafe path"),
            (vec![Item::File("/etc/evil", "", 0o644)], "unsafe path"),
            (
                vec![Item::Symlink("top/lib/x", "../../..")],
                "points outside",
            ),
            (vec![Item::Symlink("top/x", "/etc")], "points outside"),
            (
                vec![Item::Dir("a/"), Item::File("b/evil", "", 0o644)],
                "more than one top-level directory",
            ),
            (vec![], "empty"),
        ] {
            let (dir, result) = unpack_items(&items);
            let err = result.unwrap_err();
            assert!(err.contains(reason), "expected {reason:?} in {err}");
            assert!(!dir.path().join("evil").exists());
        }
    }

    #[test]
    fn symlinks_inside_the_tree_are_allowed() {
        assert!(stays_inside(Path::new("bin/clang"), Path::new("clang-19")));
        assert!(stays_inside(Path::new("lib/a/b"), Path::new("../../bin/x")));
        assert!(!stays_inside(Path::new("lib/a"), Path::new("../../x")));
        assert!(!stays_inside(Path::new("a"), Path::new("/a")));
    }

    #[test]
    fn unpacks_a_tar_xz_file() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.xz");
        let mut encoder = liblzma::write::XzEncoder::new(fs::File::create(&archive).unwrap(), 1);
        io::Write::write_all(
            &mut encoder,
            &tar(&[Item::File("top/bin/clang", "x", 0o755)]),
        )
        .unwrap();
        encoder.finish().unwrap();
        unpack_archive(&archive, &dir.path().join("out")).unwrap();
        assert!(dir.path().join("out/bin/clang").is_file());

        fs::write(&archive, "not xz").unwrap();
        let err = unpack_archive(&archive, &dir.path().join("bad")).unwrap_err();
        assert!(err.contains("cannot read the archive"), "{err}");
    }

    #[test]
    fn unpacks_a_tar_gz_file() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.tar.gz");
        let mut encoder = flate2::write::GzEncoder::new(
            fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        io::Write::write_all(&mut encoder, &tar(&[Item::File("top/bin/gcc", "x", 0o755)])).unwrap();
        encoder.finish().unwrap();
        unpack_archive(&archive, &dir.path().join("out")).unwrap();
        assert!(dir.path().join("out/bin/gcc").is_file());

        fs::write(&archive, "not gzip").unwrap();
        let err = unpack_archive(&archive, &dir.path().join("bad")).unwrap_err();
        assert!(err.contains("cannot read the archive"), "{err}");
    }

    #[test]
    fn refuses_other_archive_formats() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, "PK").unwrap();
        let err = unpack_archive(&archive, &dir.path().join("out")).unwrap_err();
        assert!(err.contains("neither a .tar.xz nor a .tar.gz"), "{err}");
        assert!(!dir.path().join("out").exists());
    }
}
