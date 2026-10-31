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
//!
//! Two questions are answered from one watch. [`FileWatcher::poll`] reports the *open* files that
//! changed, which is what decides whether a buffer reloads. [`FileWatcher::project_changed`] reports
//! that something in the project appeared or disappeared, which is what makes the file list and the
//! tree notice a file created outside the editor. Before it existed, a file added by another program
//! — or by a browser download — was invisible until the folder was opened again, because the index
//! is built by one walk at startup and nothing ever asked for a second.

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// How many directories of a project may be watched.
///
/// One inotify watch each, against a per-user limit that is 8192 on a default Linux and shared with
/// every other program on the machine. A project with more directories than this than gets the first
/// of them and a stale tree below that, which is the same failure as before this existed rather than
/// a new one — and far better than exhausting a limit that would stop *other* programs watching
/// anything.
const MAX_DIRECTORIES: usize = 2_048;

/// How long a file the editor replaced is not mistaken for one appearing.
///
/// A window rather than one event, because one rename arrives as several: inotify reports the
/// destination in a `To` event and again in the `Both` that pairs it with its source, and a drain can
/// fall between them. Long enough for both, short enough that a real deletion moments after a save
/// is the only thing it can hide — and that only until the next walk.
const REPLACEMENT_WINDOW: Duration = Duration::from_secs(2);

/// What happened, reduced to the distinction the editor acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// The file's contents changed. Only interesting for a file that is open.
    Contents,
    /// The path came into existence or left it. This is what changes the file list.
    Existence,
}

/// Called from the watcher's thread when it has queued an event. Cheap and non-blocking.
///
/// This crate sits below the binary and cannot name `dacode::wake::Waker`, so it keeps its own copy
/// of the alias — **it must stay identical to that one**, as `dacode-lsp`'s does.
type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

pub struct FileWatcher {
    /// Held only to keep the watcher thread alive; dropping it stops the watch.
    _watcher: RecommendedWatcher,
    events: Receiver<(PathBuf, Change)>,
    /// Directories already watched, so opening ten files in one directory registers one watch.
    directories: HashSet<PathBuf>,
    /// Open files that changed, held between drains.
    changed: Vec<PathBuf>,
    /// Files worth reporting. A directory watch sees every file in it, and the editor cares about
    /// the handful it has open.
    files: HashSet<PathBuf>,
    /// Directories of the project itself, watched so that a file appearing in one is noticed.
    project: HashSet<PathBuf>,
    /// An existence change arrived under a project directory since this was last asked.
    dirty: bool,
    /// Files the editor is replacing by rename, and when it said so.
    replacing: HashMap<PathBuf, Instant>,
    /// The repository's own directories — `.git`, and `.git/refs` under it — when one is watched.
    repository: Vec<PathBuf>,
    /// What has changed in the repository since this was last asked.
    repository_changed: Option<RepositoryChange>,
}

/// What moved in a repository, reduced to what the editor does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryChange {
    /// `HEAD` or a ref moved — a commit, a checkout, a fetch, a rebase. Who changed each line and
    /// what the file is diffed against may both be different now. Only the index moving (a stage
    /// in the terminal) changes neither.
    pub history: bool,
}

impl FileWatcher {
    pub fn new() -> notify::Result<Self> {
        Self::with_waker(None)
    }

    /// Like [`new`](Self::new), but `waker` is called from the watcher's thread after each event it
    /// queues. An editor asleep until input would otherwise not look until the next keypress, and a
    /// file changed by `git checkout` would sit unnoticed behind a window nobody touched.
    pub fn with_waker(waker: Option<Waker>) -> notify::Result<Self> {
        let (sender, events) = channel();

        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            // Only changes to content or existence matter. Access events fire when *this* editor
            // reads the file, which would make it chase its own tail.
            let change = match event.kind {
                notify::EventKind::Create(_) | notify::EventKind::Remove(_) => Change::Existence,
                // A rename is delivered as a modify, and it is an existence change at both ends.
                notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => Change::Existence,
                notify::EventKind::Modify(_) => Change::Contents,
                _ => return,
            };
            for path in event.paths {
                // A closed channel means the editor is gone; nothing to do about it here.
                let _ = sender.send((path, change));
                if let Some(wake) = &waker {
                    wake();
                }
            }
        })?;

        Ok(Self {
            _watcher: watcher,
            events,
            directories: HashSet::new(),
            changed: Vec::new(),
            files: HashSet::new(),
            project: HashSet::new(),
            dirty: false,
            replacing: HashMap::new(),
            repository: Vec::new(),
            repository_changed: None,
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

    /// Watches the directories the project's files live in, so a new file in one is noticed.
    ///
    /// Takes the directories rather than the root, and takes them non-recursively, because the list
    /// they come from has already been filtered by `.gitignore` — a recursive watch on the root
    /// would register one on every directory under `target/` too, which is thousands of watches
    /// against a shared per-user limit and a storm of events every time anything is built.
    ///
    /// Registering a directory that is already watched is free, so this may be called after every
    /// walk. Returns whether every directory offered could be watched.
    pub fn watch_project(&mut self, directories: impl IntoIterator<Item = PathBuf>) -> bool {
        let mut all = true;
        for directory in directories {
            let directory = crate::canonical(&directory);
            if self.project.len() >= MAX_DIRECTORIES && !self.project.contains(&directory) {
                all = false;
                continue;
            }
            self.project.insert(directory.clone());
            if self.directories.insert(directory.clone())
                && self
                    ._watcher
                    .watch(&directory, RecursiveMode::NonRecursive)
                    .is_err()
            {
                all = false;
            }
        }
        all
    }

    /// Watches a repository's own files, so a commit, checkout or stage made outside the editor is
    /// noticed: `git_dir` non-recursively (`HEAD`, the index, `packed-refs`, `ORIG_HEAD`) and
    /// `common_dir/refs` recursively, where a branch's ref is a file of its own. They are two
    /// directories in a linked worktree, whose `HEAD` and index are its own and whose refs are
    /// shared.
    ///
    /// Not recursive on `.git` itself: `objects/` takes a write per object, and a fetch writes
    /// thousands. Replaces the previous repository's watch, since a project has one. Returns
    /// whether every directory could be watched.
    pub fn watch_repository(&mut self, git_dir: &Path, common_dir: &Path) -> bool {
        let wanted = vec![
            crate::canonical(git_dir),
            crate::canonical(common_dir),
            crate::canonical(&common_dir.join("refs")),
        ];
        if wanted == self.repository {
            return true;
        }
        for old in std::mem::take(&mut self.repository) {
            let _ = self._watcher.unwatch(&old);
        }
        let mut all = true;
        for (index, directory) in wanted.iter().enumerate() {
            if self.repository.contains(directory) {
                continue;
            }
            let mode = if index == 2 {
                RecursiveMode::Recursive
            } else {
                RecursiveMode::NonRecursive
            };
            if self._watcher.watch(directory, mode).is_err() {
                all = false;
            }
            self.repository.push(directory.clone());
        }
        // Kept in the order given, so the next call with the same directories is recognised.
        self.repository = wanted;
        all
    }

    /// What changed in the watched repository since this was last asked, if anything.
    pub fn repository_changed(&mut self) -> Option<RepositoryChange> {
        self.drain();
        self.repository_changed.take()
    }

    /// Says that `path` is about to be replaced by a rename, which is how the editor saves.
    ///
    /// A rename reports its destination as appearing, and a file appearing in the project is what
    /// starts a fresh walk — so without this, every save reindexed the project, and with autosave
    /// that meant after every pause in typing. The file list cannot change when a file that was
    /// already there is replaced. Its own contents change is still reported by [`Self::poll`].
    pub fn replacing(&mut self, path: &Path) {
        self.replacing
            .insert(crate::canonical(path), Instant::now());
    }

    /// Whether a file appeared or disappeared in the project since this was last asked.
    ///
    /// Deliberately a single flag rather than a list of paths: the answer is used to start another
    /// walk, and one walk covers a hundred new files as cheaply as one. `git checkout` of a large
    /// branch is the case that makes the difference.
    pub fn project_changed(&mut self) -> bool {
        self.drain();
        std::mem::take(&mut self.dirty)
    }

    pub fn watched_count(&self) -> usize {
        self.files.len()
    }

    /// Files that changed since the last call. Never blocks, and never repeats a path.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        self.drain();
        std::mem::take(&mut self.changed)
    }

    /// Empties the channel, sorting what arrives into the two questions this answers.
    ///
    /// One drain for both, because two independent ones would race: whichever ran first would take
    /// the events the other needed. This is why `poll` takes `&mut self`.
    fn drain(&mut self) {
        self.replacing
            .retain(|_, at| at.elapsed() < REPLACEMENT_WINDOW);

        while let Ok((path, change)) = self.events.try_recv() {
            let path = crate::canonical(&path);

            if let Some(change) = self.repository_change(&path) {
                let history = self.repository_changed.is_some_and(|seen| seen.history);
                self.repository_changed = Some(RepositoryChange {
                    history: history || change.history,
                });
                continue;
            }

            if self.files.contains(&path) && !self.changed.contains(&path) {
                self.changed.push(path.clone());
            }

            if change == Change::Existence
                && !self.dirty
                && !self.replacing.contains_key(&path)
                && self.in_project(&path)
            {
                self.dirty = true;
            }
        }
    }

    /// What a path inside the watched repository means, or `None` for a path outside it.
    ///
    /// The index and its lock are a stage; anything else there — `HEAD`, a ref, `packed-refs`,
    /// `FETCH_HEAD` — is history moving. Lock files of refs are history too, which over-reports by
    /// one event per ref update and is harmless: the editor waits for the repository to settle
    /// before asking anything.
    fn repository_change(&self, path: &Path) -> Option<RepositoryChange> {
        if self.repository.is_empty() {
            return None;
        }
        let inside = self
            .repository
            .iter()
            .enumerate()
            .any(|(index, directory)| {
                if index == 2 {
                    path.starts_with(directory)
                } else {
                    path.parent() == Some(directory.as_path())
                }
            });
        if !inside {
            return None;
        }
        let name = path.file_name()?.to_string_lossy();
        // Written by every `git` that reads the repository, this one's included, and meaning nothing.
        if name == "FETCH_HEAD.lock" || name.ends_with(".tmp") {
            return None;
        }
        Some(RepositoryChange {
            history: name != "index" && name != "index.lock",
        })
    }

    /// Whether a path that appeared or vanished should make the file list be rebuilt.
    ///
    /// A watched directory reports everything in it, including the editor droppings of every other
    /// program: vim's `4913`, a `.swp`, a partial download. Hidden paths are refused because the
    /// walk that builds the list refuses them too — anything else would rebuild the index for files
    /// it would then decline to show.
    fn in_project(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        if !self.project.contains(parent) {
            return false;
        }
        !path
            .file_name()
            .map(|name| name.to_string_lossy().starts_with('.'))
            .unwrap_or(true)
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
    fn wait_for(watcher: &mut FileWatcher, path: &Path, timeout: Duration) -> bool {
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
        let dir = std::env::temp_dir().join(format!("dacode-watch-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn wait_for_repository(watcher: &mut FileWatcher) -> Option<RepositoryChange> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(change) = watcher.repository_changed() {
                return Some(change);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn a_commit_or_a_stage_made_outside_the_editor_is_noticed_and_told_apart() {
        // A commit moves a branch's ref, deep under `refs/`; a stage only rewrites the index. The
        // first changes who wrote each line and what the gutter diffs against, the second neither —
        // so the editor re-blames for one and only refreshes the list for the other.
        let dir = scratch("repository");
        let git = dir.join(".git");
        std::fs::create_dir_all(git.join("refs/heads/feature")).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("index"), "one").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        assert!(watcher.watch_repository(&git, &git));
        // The setup's own writes drained first. FSEvents can deliver the creation of `HEAD` and of
        // `refs/heads/feature` after the watch has begun, and the watcher ORs `history` across
        // unread changes — so on a Mac the index write below was read as a commit, by the
        // setup's events and not by its own.
        let settle = Instant::now() + Duration::from_millis(800);
        while Instant::now() < settle {
            watcher.repository_changed();
            std::thread::sleep(Duration::from_millis(20));
        }

        std::fs::write(git.join("index"), "two").unwrap();
        let staged = wait_for_repository(&mut watcher).expect("the index write is seen");
        assert!(!staged.history);

        std::fs::write(git.join("refs/heads/feature/x"), "abc\n").unwrap();
        let committed = wait_for_repository(&mut watcher).expect("a nested ref is seen");
        assert!(committed.history);

        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert!(
            wait_for_repository(&mut watcher)
                .expect("HEAD is seen")
                .history
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_change_on_disk_wakes_whoever_is_waiting() {
        // The editor sleeps until something wakes it. A file changed by `git checkout` behind an
        // untouched window must not wait for a keypress to be noticed.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = scratch("wake");
        let file = dir.join("watched.rs");
        std::fs::write(&file, "before\n").unwrap();

        let woken = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let mut watcher = FileWatcher::with_waker(Some(std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })))
        .expect("a watcher starts");
        watcher.watch(&file).expect("watching succeeds");

        std::fs::write(&file, "after\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while woken.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            woken.load(Ordering::SeqCst) > 0,
            "the waker was never called"
        );

        std::fs::remove_dir_all(&dir).ok();
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
            wait_for(&mut watcher, &file, Duration::from_secs(5)),
            "a plain write to a watched file should be reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_save_by_rename_is_reported() {
        // The case the directory watch exists for: this is how DaCode itself saves, how rustfmt writes
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
            wait_for(&mut watcher, &file, Duration::from_secs(5)),
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
            !wait_for(&mut watcher, &other, Duration::from_millis(600)),
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
            !wait_for(&mut watcher, &file, Duration::from_millis(600)),
            "a closed buffer's file should no longer be reported"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dacode-project-watch-{}-{label}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Polls to a deadline, because when the kernel delivers a notification is the kernel's business.
    fn became_dirty(watcher: &mut FileWatcher, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if watcher.project_changed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The reported bug: a file created by anything other than DaCode was invisible until the folder
    /// was opened again, because the index is built by one walk and nothing asked for a second.
    #[test]
    fn a_file_created_by_another_program_is_noticed() {
        let dir = scratch("created");
        let mut watcher = FileWatcher::new().expect("a watcher starts");
        assert!(watcher.watch_project([dir.clone()]));

        std::fs::write(dir.join("downloaded.rs"), "fn main() {}\n").unwrap();

        assert!(
            became_dirty(&mut watcher, Duration::from_secs(5)),
            "a file appearing in a watched project directory should ask for a new walk"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_deleted_file_is_noticed_too() {
        let dir = scratch("deleted");
        let file = dir.join("gone.rs");
        std::fs::write(&file, "x\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        assert!(watcher.watch_project([dir.clone()]));
        assert!(!watcher.project_changed(), "nothing has happened yet");

        std::fs::remove_file(&file).unwrap();

        assert!(became_dirty(&mut watcher, Duration::from_secs(5)));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Writing to a file that already exists is not a change to the *list* of files.
    ///
    /// Without this the editor would rebuild its index on every keystroke that reaches disk — its
    /// own recovery writes included, which land every few seconds.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "FSEvents keeps a file's created flag on later events for a newly made file, so a \
                  write to it reads as a creation and costs one background re-walk; nothing in the \
                  event tells the two apart"
    )]
    fn writing_to_an_existing_file_does_not_ask_for_a_walk() {
        let dir = scratch("written");
        let file = dir.join("edited.rs");
        std::fs::write(&file, "before\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        assert!(watcher.watch_project([dir.clone()]));
        // Let the creation settle before measuring what a write does.
        became_dirty(&mut watcher, Duration::from_millis(300));

        use std::io::Write;
        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        handle.write_all(b"after\n").unwrap();
        handle.flush().unwrap();
        drop(handle);

        assert!(
            !became_dirty(&mut watcher, Duration::from_millis(600)),
            "the set of files did not change, so nothing should be re-walked"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The droppings of other editors are refused, because the walk that builds the list refuses
    /// them: rebuilding the index for a file that would never be shown is pure waste.
    #[test]
    fn a_hidden_file_is_not_worth_a_walk() {
        let dir = scratch("hidden");
        let mut watcher = FileWatcher::new().expect("a watcher starts");
        assert!(watcher.watch_project([dir.clone()]));

        std::fs::write(dir.join(".edited.rs.swp"), "vim was here\n").unwrap();

        assert!(
            !became_dirty(&mut watcher, Duration::from_millis(600)),
            "a hidden file is not in the project list, so it cannot change it"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The editor saves by rename, and a rename reports its destination as appearing. Replacing a
    /// file that was already there cannot change the file list, so it must not start a walk —
    /// otherwise autosave reindexed the project every time typing paused.
    #[test]
    fn a_file_the_editor_replaces_does_not_ask_for_a_walk() {
        let dir = scratch("replaced");
        let file = dir.join("open.rs");
        std::fs::write(&file, "before\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&file).expect("watching the open file");
        watcher.watch_project([dir.clone()]);

        watcher.replacing(&file);
        let temporary = dir.join(".open.rs.tmp");
        std::fs::write(&temporary, "after\n").unwrap();
        std::fs::rename(&temporary, &file).unwrap();

        // Waiting for the file's own report is what proves the rename's events have arrived, so
        // the assertion below is about them rather than about how fast the kernel is.
        let target = crate::canonical(&file);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reported = false;
        while Instant::now() < deadline && !reported {
            reported = watcher.poll().contains(&target);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(reported, "the replaced file's change is still reported");
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !watcher.project_changed(),
            "replacing a file that was already there is not a change to the file list"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Both questions are answered from one channel, so asking one must not consume the other's
    /// events — the failure that made this a single drain rather than two.
    #[test]
    fn asking_about_the_project_does_not_swallow_an_open_files_change() {
        let dir = scratch("both");
        let file = dir.join("open.rs");
        std::fs::write(&file, "before\n").unwrap();

        let mut watcher = FileWatcher::new().expect("a watcher starts");
        watcher.watch(&file).expect("watching the open file");
        watcher.watch_project([dir.clone()]);

        std::fs::write(dir.join("new.rs"), "appeared\n").unwrap();
        std::fs::write(&file, "after\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut saw_project, mut saw_file) = (false, false);
        while Instant::now() < deadline && !(saw_project && saw_file) {
            saw_project |= watcher.project_changed();
            saw_file |= watcher.poll().contains(&crate::canonical(&file));
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(saw_project, "the new file should ask for a walk");
        assert!(saw_file, "the open file's change should still be reported");
        std::fs::remove_dir_all(&dir).ok();
    }
}
