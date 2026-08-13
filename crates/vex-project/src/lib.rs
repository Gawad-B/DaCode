//! `vex-project` — the project layer: which files exist, which one you mean, and where a string
//! appears.
//!
//! Three services, all of them off the UI thread and all bounded:
//!
//! - [`files`] — gitignore-aware discovery plus fuzzy ranking. A file list containing `target/` and
//!   `node_modules/` is worse than no list, so ripgrep's `ignore` walker does the filtering.
//! - [`search`] — project-wide search on ripgrep's own engine.
//!
//! Both follow the same rule as the syntax worker: the UI thread starts work and polls for results,
//! never waits, and a superseded result is discarded rather than applied.

pub mod files;
pub mod search;
pub mod watch;

pub use files::{FileIndex, FileWalk, Match, ProjectFile, WalkResult};
pub use search::{ProjectSearch, SearchHit, SearchQuery, search_file};
pub use watch::FileWatcher;

/// Finds the project root by walking up for a version-control or manifest marker.
///
/// `.git` wins over a manifest, and this matters: in a Cargo workspace the first `Cargo.toml` found
/// walking up is a *member* manifest, so preferring it would index one crate and call it the
/// project. A manifest is only used when there is no repository above it at all.
///
/// Falls back to the starting directory rather than `$HOME`: opening a stray file should not index a
/// whole home directory.
/// Removes Windows' extended-length prefix from a canonicalized path.
///
/// `canonicalize` returns `\\?\C:\x` on Windows — correct for the filesystem API and wrong for
/// everything else. It reaches the statusline, gets compared against paths that do not have it, and
/// gets turned into URIs. A Windows CI run caught it as a test comparing a canonicalized root against
/// `temp_dir()`, but the visible symptom would have been a project root reading `\\?\C:\Users\...` in
/// the editor's own chrome.
///
/// Takes the text rather than a `Path` so both platforms' behaviour is testable from either.
pub fn strip_extended_prefix(path: &str) -> &str {
    // `\\?\UNC\server\share` is a network path whose real form starts with two slashes, so it keeps one
    // of them; `\\?\C:\x` simply loses the prefix.
    if let Some(rest) = path.strip_prefix(r"\\?\UNC") {
        return rest;
    }
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

/// Canonicalizes a path and removes the prefix Windows adds while doing it.
pub fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    match path.canonicalize() {
        Ok(resolved) => {
            let text = resolved.to_string_lossy();
            std::path::PathBuf::from(strip_extended_prefix(&text))
        }
        Err(_) => path.to_path_buf(),
    }
}

pub fn find_root(from: &std::path::Path) -> std::path::PathBuf {
    const MANIFESTS: [&str; 4] = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

    // Relative paths cannot be walked upward, so resolve first.
    let resolved = match from.canonicalize() {
        Ok(_) => canonical(from),
        Err(_) => std::env::current_dir().unwrap_or_default().join(from),
    };
    let start = if resolved.is_file() {
        resolved.parent().unwrap_or(&resolved).to_path_buf()
    } else {
        resolved
    };

    let mut nearest_manifest: Option<std::path::PathBuf> = None;
    let mut current: Option<&std::path::Path> = Some(&start);

    while let Some(directory) = current {
        // A repository root is the project root, full stop.
        if directory.join(".git").exists() || directory.join(".hg").exists() {
            return directory.to_path_buf();
        }
        if nearest_manifest.is_none()
            && MANIFESTS
                .iter()
                .any(|marker| directory.join(marker).exists())
        {
            nearest_manifest = Some(directory.to_path_buf());
        }
        current = directory.parent();
    }

    nearest_manifest.unwrap_or(start)
}
