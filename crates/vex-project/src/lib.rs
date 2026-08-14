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

/// Resolves a project-relative path, refusing anything that escapes the root.
///
/// **Lexically, without asking the filesystem**, which is the whole point. The version this
/// replaced joined the name onto the root, canonicalized the *parent*, and compared with
/// `starts_with`. Two things defeated that. [`canonical`] returns its input unchanged when
/// `canonicalize` fails, and `canonicalize` fails with `NotFound` the moment any component of the
/// path does not exist yet — which is the normal case for a file you are about to create.
/// `Path::starts_with` compares components, and components normalise `.` but never `..`. So
/// `../evil.txt` was caught, and `x/../../../../evil.txt` with no `x` on disk was not: the parent
/// came back verbatim, `..` chain intact, and matched the root on its first two components.
/// `create_dir_all` then walked the chain happily and `write` put the file wherever it landed.
///
/// Taking the root as a parameter rather than reading it from `self` is what makes the rules
/// testable, and this one had no test at all. It is also why the Windows-only accident that hid it
/// no longer matters: Win32 collapses `..` before the syscall, so the machine this editor is
/// developed on could not reproduce its own bug.
///
/// Returns the *folded* path, so what the caller writes to is what was checked. Symlinks are not
/// visible to lexical folding; callers that can afford a filesystem round trip should follow up
/// with [`canonical`] where the parent genuinely exists.
pub fn within(root: &std::path::Path, relative: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;

    let relative = relative.trim();
    if relative.is_empty() {
        return None;
    }

    let mut folded = root.to_path_buf();
    // How deep below the root we are. A `..` at zero is an escape, and popping is only safe while
    // this is positive — otherwise it would eat the root's own components.
    let mut depth = 0usize;

    for component in std::path::Path::new(relative).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                folded.push(part);
                depth += 1;
            }
            Component::ParentDir => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                folded.pop();
            }
            // An absolute or drive-qualified name replaces the root outright under `join`, so
            // `/etc/passwd` and `C:\Windows\x` never even reached the old check.
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    // `.`, or `a/..`, names the root itself — a directory, and not a file anything here may create,
    // rename or delete.
    (depth > 0).then_some(folded)
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
