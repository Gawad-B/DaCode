//! Crash recovery for unsaved work.
//!
//! The rule from PLAN.md §4: *a daily driver must not lose unsaved work.* Everything else in this
//! project can be rebuilt from the repository; a buffer that only existed in memory cannot.
//!
//! The mechanism is deliberately dull. Dirty buffers are written to a recovery directory on a timer,
//! so a crash or a kill loses at most the last couple of seconds. On a successful save the recovery
//! file is deleted, so its presence at startup *means* something: the last session ended without
//! saving. There is no clever journaling and no attempt to reconstruct edit history — a plain copy
//! of the text plus where it came from is what a person actually needs after a crash.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often a dirty buffer is persisted. Two seconds bounds the loss at roughly a sentence, and a
/// write of a few kilobytes at that rate is invisible.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// Where recovery files live.
///
/// `$XDG_STATE_HOME` (or `~/.local/state`) rather than the config directory: this is machine state
/// nobody edits by hand, and it must not end up in a dotfiles repository.
pub fn recovery_dir() -> PathBuf {
    crate::paths::state_dir().join("recovery")
}

/// A stable file name for a buffer's recovery copy.
///
/// Derived from the full path so two files with the same name in different directories do not
/// collide, and so reopening a file finds its own recovery copy rather than someone else's.
fn recovery_name(path: &Path) -> String {
    let text = path.to_string_lossy();
    // FNV-1a: a few lines, no dependency, and collision resistance far beyond what naming needs.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }

    let stem = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "buffer".into());
    format!("{hash:016x}-{stem}")
}

/// Tracks when each buffer was last persisted.
///
/// The directory is a field rather than a global lookup: that keeps the tests from having to mutate
/// process-wide environment variables, which they cannot do safely in parallel.
#[derive(Debug)]
pub struct Recovery {
    directory: PathBuf,
    last_written: Option<Instant>,
}

impl Default for Recovery {
    fn default() -> Self {
        Self::new()
    }
}

impl Recovery {
    pub fn new() -> Self {
        Self::in_dir(recovery_dir())
    }

    pub fn in_dir(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            last_written: None,
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Persists the text if the interval has elapsed. Returns true if a write happened.
    ///
    /// `force` skips the timer, for the moments where losing anything is unacceptable: quitting,
    /// switching buffers, or a panic on the way out.
    pub fn maybe_write(&mut self, path: &Path, text: &str, force: bool) -> bool {
        let due = force
            || self
                .last_written
                .map(|last| last.elapsed() >= INTERVAL)
                .unwrap_or(true);
        if !due {
            return false;
        }

        if write_in(&self.directory, path, text).is_ok() {
            self.last_written = Some(Instant::now());
            return true;
        }
        false
    }

    /// Whether any file has unsaved work waiting in this directory.
    ///
    /// Not used by the editor, which asks per path — this is for tests and for anything that wants
    /// to know whether a crash left anything behind at all.
    pub fn is_empty(&self) -> bool {
        std::fs::read_dir(&self.directory)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true)
    }

    /// Called after a successful save: the on-disk file is now authoritative.
    pub fn discard(&mut self, path: &Path) {
        self.last_written = None;
        let _ = std::fs::remove_file(recovery_path_in(&self.directory, path));
    }

    /// Looks for unsaved work for `path` in this recovery directory.
    pub fn find(&self, path: &Path) -> Option<Found> {
        find_in(&self.directory, path)
    }

    pub fn path_for(&self, path: &Path) -> PathBuf {
        recovery_path_in(&self.directory, path)
    }

    pub fn reset(&mut self) {
        self.last_written = None;
    }
}

pub fn recovery_path_in(directory: &Path, path: &Path) -> PathBuf {
    directory.join(recovery_name(path))
}

/// Writes a recovery copy, atomically.
///
/// Written to a temporary file and renamed, because a crash *during* the recovery write would
/// otherwise replace a good copy with a truncated one — losing the very thing this exists to save.
pub fn write_in(directory: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;

    let final_path = directory.join(recovery_name(path));
    let temporary = final_path.with_extension("tmp");

    {
        let mut file = std::fs::File::create(&temporary)?;
        // The original path is recorded in the file, so a recovery copy found later is traceable
        // without depending on the hashed name.
        writeln!(file, "vex-recovery: {}", path.display())?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }

    std::fs::rename(&temporary, &final_path)
}

/// A recovery copy that is newer than the file it came from.
#[derive(Debug, Clone)]
pub struct Found {
    pub recovery: PathBuf,
    pub text: String,
}

/// Looks for unsaved work for `path`.
///
/// Returns nothing when the saved file is at least as new as the recovery copy: that means the last
/// session ended cleanly, and offering to restore stale text would be worse than silence.
pub fn find_in(directory: &Path, path: &Path) -> Option<Found> {
    let recovery = recovery_path_in(directory, path);
    let contents = std::fs::read_to_string(&recovery).ok()?;

    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    };
    if modified(path) >= modified(&recovery) {
        // Stale: the file was saved after this copy was written.
        let _ = std::fs::remove_file(&recovery);
        return None;
    }

    // Strip the provenance header written above.
    let text = match contents.split_once('\n') {
        Some((header, rest)) if header.starts_with("vex-recovery:") => rest.to_string(),
        _ => contents,
    };

    Some(Found { recovery, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_dirty_files_each_keep_their_own_copy() {
        // The defect this covers: only the *active* document was ever written, so editing three
        // files and crashing gave one of them back. Each path has its own recovery file, and one
        // must not overwrite another — the flattened naming scheme is what guarantees that, and two
        // files with the same basename in different directories are how it would fail.
        let sandbox = Sandbox::new("several");
        let one = sandbox.file("src/main.rs", "fn main() {}\n");
        let two = sandbox.file("tests/main.rs", "fn test() {}\n");

        // `write_newer`, not `write_in`: `find_in` only offers a copy that is newer than the saved
        // file, and on a filesystem with coarse timestamps two writes in the same millisecond are
        // not ordered. This passed on Windows and failed on Linux for exactly that reason.
        sandbox.write_newer(&one, "fn main() { edited }\n");
        sandbox.write_newer(&two, "fn test() { also }\n");

        assert_ne!(
            recovery_path_in(&sandbox.recovery, &one),
            recovery_path_in(&sandbox.recovery, &two),
            "two files called main.rs must not share a recovery file"
        );
        assert_eq!(
            find_in(&sandbox.recovery, &one).map(|found| found.text),
            Some("fn main() { edited }\n".to_string())
        );
        assert_eq!(
            find_in(&sandbox.recovery, &two).map(|found| found.text),
            Some("fn test() { also }\n".to_string())
        );
        std::fs::remove_dir_all(&sandbox.root).ok();
    }

    /// A disposable project directory plus its own recovery directory.
    struct Sandbox {
        root: PathBuf,
        recovery: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("vex-recovery-{}-{name}", std::process::id()));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            let recovery = root.join("state");
            Self { root, recovery }
        }

        fn file(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, contents).unwrap();
            path
        }

        /// Writes a recovery copy that is definitely newer than the saved file.
        fn write_newer(&self, path: &Path, text: &str) {
            std::thread::sleep(Duration::from_millis(20));
            write_in(&self.recovery, path, text).unwrap();
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn a_recovery_copy_is_written_and_found() {
        let sandbox = Sandbox::new("roundtrip");
        let path = sandbox.file("notes.txt", "saved contents");
        sandbox.write_newer(&path, "unsaved contents");

        let found = find_in(&sandbox.recovery, &path).expect("recovery copy is found");
        assert_eq!(found.text, "unsaved contents");
    }

    #[test]
    fn the_provenance_header_is_stripped_but_recorded() {
        let sandbox = Sandbox::new("header");
        let path = sandbox.file("a.rs", "x");
        sandbox.write_newer(&path, "fn main() {}\n");

        let raw = std::fs::read_to_string(recovery_path_in(&sandbox.recovery, &path)).unwrap();
        assert!(
            raw.starts_with("vex-recovery: "),
            "the original path must be recorded: {raw:?}"
        );
        assert!(raw.contains("a.rs"));

        assert_eq!(
            find_in(&sandbox.recovery, &path).unwrap().text,
            "fn main() {}\n",
            "but the header must not come back as buffer text"
        );
    }

    #[test]
    fn a_copy_older_than_the_saved_file_is_discarded() {
        // The session ended cleanly; restoring stale text would be worse than silence.
        let sandbox = Sandbox::new("stale");
        let path = sandbox.file("b.rs", "old");

        write_in(&sandbox.recovery, &path, "older unsaved").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "newer saved").unwrap();

        assert!(
            find_in(&sandbox.recovery, &path).is_none(),
            "a stale copy must not be offered"
        );
        assert!(
            !recovery_path_in(&sandbox.recovery, &path).exists(),
            "and it should be cleaned up rather than left to confuse later"
        );
    }

    #[test]
    fn saving_discards_the_copy() {
        let sandbox = Sandbox::new("discard");
        let path = sandbox.file("c.rs", "x");

        let mut recovery = Recovery::in_dir(&sandbox.recovery);
        assert!(recovery.maybe_write(&path, "in progress", true));
        assert!(recovery.path_for(&path).exists());

        recovery.discard(&path);
        assert!(!recovery.path_for(&path).exists());
    }

    #[test]
    fn writes_are_throttled_unless_forced() {
        let sandbox = Sandbox::new("throttle");
        let path = sandbox.file("d.rs", "x");
        let mut recovery = Recovery::in_dir(&sandbox.recovery);

        assert!(
            recovery.maybe_write(&path, "first", false),
            "the first write happens"
        );
        assert!(
            !recovery.maybe_write(&path, "second", false),
            "a second write within the interval is skipped"
        );
        assert!(
            recovery.maybe_write(&path, "third", true),
            "forcing overrides the timer"
        );
    }

    #[test]
    fn an_atomic_write_leaves_no_temporary_behind() {
        // A crash during the recovery write must not replace a good copy with a truncated one.
        let sandbox = Sandbox::new("atomic");
        let path = sandbox.file("e.rs", "x");
        write_in(&sandbox.recovery, &path, "contents").unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(&sandbox.recovery)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a .tmp file was left behind");
    }

    #[test]
    fn two_files_with_the_same_name_do_not_collide() {
        let sandbox = Sandbox::new("collide");
        let first = sandbox.file("one/main.rs", "a");
        let second = sandbox.file("two/main.rs", "b");

        assert_ne!(
            recovery_path_in(&sandbox.recovery, &first),
            recovery_path_in(&sandbox.recovery, &second),
            "the name is derived from the full path, not just the file name"
        );
    }

    #[test]
    fn no_recovery_file_means_nothing_to_offer() {
        let sandbox = Sandbox::new("absent");
        let path = sandbox.file("f.rs", "x");
        assert!(find_in(&sandbox.recovery, &path).is_none());
    }
}
