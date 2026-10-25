//! Finding the files in a project, and ranking them against a query.
//!
//! Discovery honors `.gitignore` via ripgrep's `ignore` walker, because a file list that includes
//! `target/` and `node_modules/` is useless — those directories dwarf the actual project and would
//! bury every real result.
//!
//! The walk runs off the UI thread and is capped, so opening the editor in `/` degrades to a
//! truncated list rather than a hang.

use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// Upper bound on indexed files. A project this size is already beyond what a palette can usefully
/// present, and the cap is what keeps memory bounded on a mistaken directory choice.
pub const MAX_FILES: usize = 200_000;

/// One file in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Path relative to the project root — what the palette shows and what the matcher scores.
    pub relative: String,
    pub absolute: PathBuf,
}

impl ProjectFile {
    /// The file name alone, for display and for weighting matches on it.
    pub fn name(&self) -> &str {
        self.relative.rsplit('/').next().unwrap_or(&self.relative)
    }

    /// The directory portion, shown dimmed after the name.
    pub fn directory(&self) -> &str {
        match self.relative.rfind('/') {
            Some(index) => &self.relative[..index],
            None => "",
        }
    }
}

/// A ranked match, with the character positions that matched so they can be highlighted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub file: ProjectFile,
    pub score: u32,
    /// Indices into `relative` that the query matched. The design highlights exactly these.
    pub positions: Vec<u32>,
}

/// The project's file list, plus the matcher state.
pub struct FileIndex {
    root: PathBuf,
    files: Vec<ProjectFile>,
    matcher: Matcher,
    /// Set when the walk hit [`MAX_FILES`], so the UI can say the list is partial rather than
    /// implying it is complete.
    truncated: bool,
}

impl FileIndex {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            files: Vec::new(),
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            truncated: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn files(&self) -> &[ProjectFile] {
        &self.files
    }

    pub fn set_files(&mut self, files: Vec<ProjectFile>, truncated: bool) {
        self.files = files;
        self.truncated = truncated;
    }

    /// Ranks files against a query, best first.
    ///
    /// An empty query returns the files in index order rather than nothing: opening the palette
    /// should show something immediately, before anything is typed.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<Match> {
        if query.is_empty() {
            return self
                .files
                .iter()
                .take(limit)
                .map(|file| Match {
                    file: file.clone(),
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        }

        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buffer = Vec::new();
        let mut matches: Vec<Match> = Vec::new();

        for file in &self.files {
            buffer.clear();
            let haystack = Utf32Str::new(&file.relative, &mut buffer);

            let mut positions = Vec::new();
            if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut positions) {
                positions.sort_unstable();
                positions.dedup();
                matches.push(Match {
                    file: file.clone(),
                    score,
                    positions,
                });
            }
        }

        // Highest score first; ties broken by the shorter path, which is nearly always the one
        // meant — `src/lib.rs` before `vendor/x/src/lib.rs`.
        matches.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.file.relative.len().cmp(&b.file.relative.len()))
                .then_with(|| a.file.relative.cmp(&b.file.relative))
        });
        matches.truncate(limit);
        matches
    }
}

/// A walk running on a background thread.
pub struct FileWalk {
    results: Receiver<WalkResult>,
    cancelled: Arc<AtomicBool>,
}

pub struct WalkResult {
    pub files: Vec<ProjectFile>,
    pub truncated: bool,
}

impl FileWalk {
    /// Starts walking `root`, honoring ignore files. Never blocks the caller.
    pub fn start(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let (sender, results) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();

        std::thread::Builder::new()
            .name("dacode-file-walk".into())
            .spawn(move || {
                let mut files = Vec::new();
                let mut truncated = false;

                let walker = WalkBuilder::new(&root)
                    // Respect .gitignore, .ignore and global excludes; skip hidden files. This is
                    // what makes the list the project rather than the filesystem.
                    .hidden(true)
                    .git_ignore(true)
                    .git_global(true)
                    .parents(true)
                    // Honor .gitignore even outside a git repository: an extracted tarball or a
                    // vendored subtree still means what its ignore file says. The `ignore` crate
                    // requires a .git directory by default, which would silently include
                    // `target/` in exactly those cases.
                    .require_git(false)
                    .build();

                for entry in walker {
                    if flag.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(entry) = entry else { continue };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        continue;
                    }
                    if files.len() >= MAX_FILES {
                        truncated = true;
                        break;
                    }

                    let absolute = entry.into_path();
                    let relative = absolute
                        .strip_prefix(&root)
                        .unwrap_or(&absolute)
                        .to_string_lossy()
                        .replace('\\', "/");

                    files.push(ProjectFile { relative, absolute });
                }

                files.sort_by(|a, b| a.relative.cmp(&b.relative));
                let _ = sender.send(WalkResult { files, truncated });
            })
            .expect("spawning the file walk");

        Self { results, cancelled }
    }

    /// Collects the result if the walk has finished. Never blocks.
    pub fn poll(&self) -> Option<WalkResult> {
        self.results.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for FileWalk {
    fn drop(&mut self) {
        // A walk of a huge tree must not keep the process alive after the editor closes.
        self.cancel();
    }
}
