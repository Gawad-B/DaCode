//! Writing a file the user owns, so that no failure can leave it half-written.
//!
//! `fs::write` truncates the file and then fills it. Between the two, the only copy of the user's
//! work is in memory: a crash, a power cut or a full disk at that moment leaves an empty or truncated
//! file, and the recovery copy — deleted the moment the save "succeeded" — is no longer there to fall
//! back on. That was how every save in this editor worked.
//!
//! [`write`] puts the new contents in a temporary file beside the original, flushes it to the disk,
//! and renames it over the original. A rename within one directory is atomic, so at every instant
//! the path names either the whole old file or the whole new one. The temporary takes the original's
//! permissions and, on Unix, its owner first, because a rename replaces the inode — and with it the
//! executable bit on a script, or a group-writable mode a team relies on.
//!
//! Two kinds of file are written in place instead, because replacing their inode would change what
//! they *are*: one with more than one hard link (a rename would silently split it from the others),
//! and one whose owner cannot be kept (a file you may write but do not own — the rename would make it
//! yours). In-place writes are still flushed, so they are durable, just not atomic. That is the same
//! trade every editor that saves atomically makes.
//!
//! A symbolic link is followed, and the file it names is the one replaced; the link itself is left
//! as it was. Replacing the link with a regular file was what the cross-file rename path used to do.
//!
//! Not preserved on Windows: alternate data streams and an ACL that differs from the directory's. A
//! file relying on either is rare enough in a source tree that atomicity is worth more.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

/// How many symbolic links are followed before giving up. The kernel's own limit, past which it
/// reports a loop.
const MAX_LINKS: usize = 40;

/// Replaces the file at `path` with `contents`, durably and — wherever it can be — atomically.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let target = follow_links(path)?;
    let existing = fs::metadata(&target).ok();

    match strategy(existing.is_some(), existing.as_ref().map_or(1, link_count)) {
        Strategy::InPlace => in_place(&target, contents),
        Strategy::Replace => match replace(&target, contents, existing.as_ref()) {
            Err(error) if falls_back(&error, existing.is_some()) => in_place(&target, contents),
            result => result,
        },
    }
}

/// How a file is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    /// A temporary beside it, renamed over it.
    Replace,
    /// Truncated and rewritten where it is.
    InPlace,
}

/// The choice, from the facts that decide it, so it can be tested without a filesystem that has
/// hard links in it.
fn strategy(exists: bool, links: u64) -> Strategy {
    if exists && links > 1 {
        Strategy::InPlace
    } else {
        Strategy::Replace
    }
}

/// Whether a failed replacement should be retried in place.
///
/// Only for a file that already exists, and only when the failure was a refusal: a directory the
/// user cannot create files in, an owner they cannot give the replacement, or — on Windows — a file
/// another program holds open without letting it be renamed over. Each of those still allows writing
/// the file itself, which is what `fs::write` always did. Anything else, a full disk above all, is
/// reported rather than retried, because retrying in place is exactly the write that destroys the
/// original when it fails halfway.
fn falls_back(error: &io::Error, exists: bool) -> bool {
    exists && error.kind() == io::ErrorKind::PermissionDenied
}

fn replace(target: &Path, contents: &[u8], existing: Option<&Metadata>) -> io::Result<()> {
    let directory = parent_of(target);
    let (mut file, mut temporary) = create_temporary(directory, target)?;

    file.write_all(contents)?;
    if let Some(original) = existing {
        file.set_permissions(original.permissions())?;
        keep_owner(&file, original)?;
    }
    file.sync_all()?;
    drop(file);

    fs::rename(&temporary.path, target)?;
    temporary.kept = true;

    // The rename is durable only once the directory entry is. Opening a directory as a file is how
    // that is done on Unix; Windows refuses to, and its rename is already written through, so the
    // failure there is the expected one and is ignored.
    if let Ok(directory) = File::open(directory) {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn in_place(target: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(target)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// A temporary file that removes itself unless it was renamed into place.
///
/// So a failure at any step — a full disk while writing, a refused rename — leaves the directory as
/// it was, rather than littering it with half-written copies of the user's file.
struct Temporary {
    path: PathBuf,
    kept: bool,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Creates the temporary in the target's own directory, because a rename across filesystems is not
/// atomic — it is not even a rename.
///
/// Hidden, so the file tree and the project watcher pass over it, and named after the target and
/// this process so that one left behind by a power cut says where it came from. `create_new` rather
/// than `create`, so it can never truncate something that happens to have the same name.
fn create_temporary(directory: &Path, target: &Path) -> io::Result<(File, Temporary)> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());

    loop {
        let path = directory.join(format!(
            ".{name}.vex-{}-{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, Temporary { path, kept: false })),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// The file a path finally names, after every symbolic link on the way.
///
/// Walked by hand rather than through `canonical`, because that answers only for a file that
/// exists — and a link to a file not yet created is one a save should create, not replace.
fn follow_links(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&current)?;
                current = if link.is_absolute() {
                    link
                } else {
                    parent_of(&current).join(link)
                };
            }
            _ => return Ok(current),
        }
    }
    Err(io::Error::other(format!(
        "{} is a loop of symbolic links",
        path.display()
    )))
}

fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

// Two functions with a `#[cfg]` each, against this project's rule of runtime selection: the Unix
// half calls methods that do not exist on Windows, so it cannot be compiled there at all. Windows
// has neither a link count std will report nor an owner a rename would change.

#[cfg(unix)]
fn link_count(meta: &Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(meta)
}

#[cfg(not(unix))]
fn link_count(_: &Metadata) -> u64 {
    1
}

/// Gives the replacement the original's owner and group, or refuses — which sends the save back to
/// writing in place, rather than quietly handing someone else's file to the user.
#[cfg(unix)]
fn keep_owner(file: &File, original: &Metadata) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let created = file.metadata()?;
    if created.uid() == original.uid() && created.gid() == original.gid() {
        return Ok(());
    }
    std::os::unix::fs::fchown(file, Some(original.uid()), Some(original.gid()))
}

#[cfg(not(unix))]
fn keep_owner(_: &File, _: &Metadata) -> io::Result<()> {
    Ok(())
}

/// What a file on disk looked like when the editor last read or wrote it.
///
/// The modification time *and* the length, because a time alone can repeat: two writes inside one
/// tick of a coarse clock — some network filesystems, FAT — would look like none. Both together can
/// still miss a same-length rewrite inside one tick, which is the honest limit of asking the
/// filesystem rather than reading the file back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let meta = fs::metadata(path).ok()?;
        Some(Stamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

/// Whether the file changed since `recorded`, so that saving would overwrite someone else's work.
///
/// Nothing recorded is not a conflict — there is nothing to compare with, and refusing would make a
/// file the editor never read unsaveable. Nor is a file that has gone: saving puts it back, which is
/// what someone saving a deleted file is asking for.
pub fn changed_since(recorded: Option<Stamp>, now: Option<Stamp>) -> bool {
    matches!((recorded, now), (Some(recorded), Some(now)) if recorded != now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-save-{}-{label}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn a_save_replaces_the_contents_and_leaves_nothing_behind() {
        let dir = scratch("replace");
        let file = dir.join("a.rs");
        fs::write(&file, "before\n").unwrap();

        write(&file, b"after\n").unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "after\n");
        assert!(
            leftovers(&dir).is_empty(),
            "a successful save must not leave its temporary beside the file"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_new_file_is_created() {
        let dir = scratch("new");
        let file = dir.join("new.rs");

        write(&file, b"fn main() {}\n").unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "fn main() {}\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_bare_file_name_is_written_beside_the_process() {
        // `Path::parent` of a bare name is the empty path, and a temporary joined onto that, or a
        // directory sync opened on it, would be asking about nothing.
        assert_eq!(parent_of(Path::new("a.rs")), Path::new("."));
        assert_eq!(parent_of(Path::new("src/a.rs")), Path::new("src"));
    }

    #[test]
    fn a_failed_write_cleans_up_its_temporary() {
        let dir = scratch("cleanup");
        let (file, temporary) = create_temporary(&dir, &dir.join("a.rs")).unwrap();
        drop(file);
        let path = temporary.path.clone();
        assert!(path.exists());

        drop(temporary);

        assert!(
            !path.exists(),
            "a temporary that was never renamed into place must remove itself"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_temporaries_for_one_file_never_collide() {
        // `create_new` plus a counter: a second save started while a first is mid-write must get its
        // own file rather than truncating the first one's.
        let dir = scratch("collide");
        let target = dir.join("a.rs");
        let (_one, first) = create_temporary(&dir, &target).unwrap();
        let (_two, second) = create_temporary(&dir, &target).unwrap();
        assert_ne!(first.path, second.path);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_a_shared_file_is_written_in_place() {
        assert_eq!(strategy(false, 1), Strategy::Replace, "a new file");
        assert_eq!(strategy(true, 1), Strategy::Replace, "an ordinary file");
        // A rename would give one name a new inode and leave every other link on the old one,
        // silently splitting what the user thinks is one file into two.
        assert_eq!(strategy(true, 2), Strategy::InPlace, "a hard-linked file");
    }

    #[test]
    fn only_a_refusal_on_an_existing_file_is_retried_in_place() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        let full = io::Error::other("no space left on device");
        assert!(falls_back(&denied, true));
        assert!(
            !falls_back(&denied, false),
            "with no file there, in place is the same refused directory"
        );
        assert!(
            !falls_back(&full, true),
            "a full disk retried in place is the write that destroys the original"
        );
    }

    #[test]
    fn unsaved_and_vanished_files_are_not_conflicts() {
        let dir = scratch("stamp");
        let file = dir.join("a.rs");
        fs::write(&file, "one\n").unwrap();
        let before = Stamp::of(&file);
        assert!(before.is_some());

        assert!(
            !changed_since(None, before),
            "nothing was recorded to differ from"
        );
        assert!(
            !changed_since(before, None),
            "saving puts a deleted file back"
        );
        assert!(!changed_since(before, Stamp::of(&file)), "nothing happened");

        fs::write(&file, "one and more\n").unwrap();
        assert!(
            changed_since(before, Stamp::of(&file)),
            "another program rewrote it"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_save_keeps_the_files_permissions() {
        use std::os::unix::fs::PermissionsExt;
        // A rename replaces the inode, and with it the mode. Saving a script must not make it
        // unexecutable, and saving a file kept private must not make it world-readable.
        let dir = scratch("mode");
        let file = dir.join("run.sh");
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o740)).unwrap();

        write(&file, b"#!/bin/sh\necho hi\n").unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o740);
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_save_through_a_symbolic_link_keeps_the_link() {
        let dir = scratch("symlink");
        let real = dir.join("real.rs");
        let link = dir.join("link.rs");
        fs::write(&real, "before\n").unwrap();
        std::os::unix::fs::symlink("real.rs", &link).unwrap();

        write(&link, b"after\n").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must still be a link, not a copy that has stopped following the original"
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "after\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_save_through_a_dangling_link_creates_what_it_names() {
        let dir = scratch("dangling");
        let link = dir.join("link.rs");
        std::os::unix::fs::symlink("missing.rs", &link).unwrap();

        write(&link, b"new\n").unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(dir.join("missing.rs")).unwrap(), "new\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_loop_of_links_is_an_error_rather_than_a_hang() {
        let dir = scratch("loop");
        std::os::unix::fs::symlink("b", dir.join("a")).unwrap();
        std::os::unix::fs::symlink("a", dir.join("b")).unwrap();

        assert!(write(&dir.join("a"), b"x").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_linked_file_stays_one_file() {
        let dir = scratch("hardlink");
        let one = dir.join("one.rs");
        let two = dir.join("two.rs");
        fs::write(&one, "before\n").unwrap();
        fs::hard_link(&one, &two).unwrap();

        write(&one, b"after\n").unwrap();

        assert_eq!(
            fs::read_to_string(&two).unwrap(),
            "after\n",
            "the other name must see the save, as it did when every save was in place"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
