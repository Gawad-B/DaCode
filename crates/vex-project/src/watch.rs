//! Noticing when a file changes underneath the editor.
//!
//! Watches the *directories* holding the open files, not the files themselves. Every editor and
//! most build tools save by writing a temporary file and renaming it over the original, which
//! replaces the inode — a watch on the file follows the old one and reports nothing ever again.
//! `git checkout`, `cargo fmt` and `rustfmt` all do this.
//!
//! Events are coalesced per poll rather than delivered one by one: a single save can produce a
//! create, a chmod and two modifies, and reloading a buffer four times for one save would be both
//! slow and visible.

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

pub struct FileWatcher {
    /// Held only to keep the watcher thread alive; dropping it stops the watch.
    _watcher: RecommendedWatcher,
    events: Receiver<PathBuf>,
    /// Directories already watched, so opening ten files in one directory registers one watch.
    directories: HashSet<PathBuf>,
    /// Files worth reporting. A directory watch sees every file in it, and the editor cares about
    /// the handful it has open.
    files: HashSet<PathBuf>,
}

impl FileWatcher {
    pub fn new() -> notify::Result<Self> {
        let (sender, events) = channel();

        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            // Only changes to content or existence matter. Access events fire when *this* editor
            // reads the file, which would make it chase its own tail.
            if !matches!(
                event.kind,
                notify::EventKind::Create(_)
                    | notify::EventKind::Modify(_)
                    | notify::EventKind::Remove(_)
            ) {
                return;
            }
            for path in event.paths {
                // A closed channel means the editor is gone; nothing to do about it here.
                let _ = sender.send(path);
            }
        })?;

        Ok(Self {
            _watcher: watcher,
            events,
            directories: HashSet::new(),
            files: HashSet::new(),
        })
    }

    /// Starts watching a file, by watching the directory that holds it.
    pub fn watch(&mut self, path: &Path) -> notify::Result<()> {
        let absolute = crate::canonical(path);
        let Some(directory) = absolute.parent().map(Path::to_path_buf) else {
            return Ok(());
        };

        self.files.insert(absolute);
        if self.directories.insert(directory.clone()) {
            self._watcher
                .watch(&directory, RecursiveMode::NonRecursive)?;
        }
        Ok(())
    }

    pub fn forget(&mut self, path: &Path) {
        let absolute = crate::canonical(path);
        self.files.remove(&absolute);
    }

    pub fn watched_count(&self) -> usize {
        self.files.len()
    }

    /// Files that changed since the last call. Never blocks, and never repeats a path.
    pub fn poll(&self) -> Vec<PathBuf> {
        let mut changed: Vec<PathBuf> = Vec::new();
        while let Ok(path) = self.events.try_recv() {
            let path = crate::canonical(&path);
            if self.files.contains(&path) && !changed.contains(&path) {
                changed.push(path);
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Waits for a watch to report a path, or gives up.
    ///
    /// Filesystem notification is inherently asynchronous, and how fast it arrives is the kernel's
    /// business — so this polls to a deadline rather than sleeping a fixed amount and hoping.
    fn wait_for(watcher: &FileWatcher, path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let target = crate::canonical(path);
        while Instant::now() < deadline {
            if watcher.poll().contains(&target) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-watch-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_write_is_reported() {
        let dir = scratch("write");
        let file = dir.join("watched.rs");
        std::fs::write(&file, "before\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&file).expect("watching succeeds");

        std::fs::write(&file, "after\n").unwrap();

        assert!(
            wait_for(&watcher, &file, Duration::from_secs(5)),
            "a plain write to a watched file should be reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_save_by_rename_is_reported() {
        // The case the directory watch exists for: this is how vex itself saves, how rustfmt writes
        // and what `git checkout` does. A watch on the file's inode would miss every one of them.
        let dir = scratch("rename");
        let file = dir.join("watched.rs");
        std::fs::write(&file, "before\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&file).expect("watching succeeds");

        let temporary = dir.join("watched.rs.tmp");
        std::fs::write(&temporary, "after\n").unwrap();
        std::fs::rename(&temporary, &file).unwrap();

        assert!(
            wait_for(&watcher, &file, Duration::from_secs(5)),
            "a save-by-rename should be reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn changes_to_other_files_in_the_directory_are_ignored() {
        let dir = scratch("neighbour");
        let watched = dir.join("watched.rs");
        let other = dir.join("other.rs");
        std::fs::write(&watched, "a\n").unwrap();
        std::fs::write(&other, "b\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&watched).expect("watching succeeds");

        std::fs::write(&other, "changed\n").unwrap();

        assert!(
            !wait_for(&watcher, &other, Duration::from_millis(600)),
            "the directory is watched, but only open files are reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn watching_several_files_in_one_directory_registers_one_watch() {
        let dir = scratch("shared");
        let mut watcher = FileWatcher::new().expect("a watcher starts");

        for name in ["one.rs", "two.rs", "three.rs"] {
            let file = dir.join(name);
            std::fs::write(&file, "x\n").unwrap();
            watcher.watch(&file).unwrap();
        }

        assert_eq!(watcher.watched_count(), 3);
        assert_eq!(
            watcher.directories.len(),
            1,
            "one directory, one watch — not one per file"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_forgotten_file_stops_being_reported() {
        let dir = scratch("forget");
        let file = dir.join("watched.rs");
        std::fs::write(&file, "a\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&file).unwrap();
        watcher.forget(&file);

        std::fs::write(&file, "changed\n").unwrap();

        assert!(
            !wait_for(&watcher, &file, Duration::from_millis(600)),
            "a closed buffer's file should no longer be reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
