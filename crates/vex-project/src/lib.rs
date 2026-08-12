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

pub use files::{FileIndex, FileWalk, Match, ProjectFile, WalkResult};
pub use search::{ProjectSearch, SearchHit, SearchQuery, search_file};

/// Finds the project root by walking up for a version-control or manifest marker.
///
/// `.git` wins over a manifest, and this matters: in a Cargo workspace the first `Cargo.toml` found
/// walking up is a *member* manifest, so preferring it would index one crate and call it the
/// project. A manifest is only used when there is no repository above it at all.
///
/// Falls back to the starting directory rather than `$HOME`: opening a stray file should not index a
/// whole home directory.
pub fn find_root(from: &std::path::Path) -> std::path::PathBuf {
    const MANIFESTS: [&str; 4] = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];

    // Relative paths cannot be walked upward, so resolve first.
    let resolved = from
        .canonicalize()
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(from));
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
