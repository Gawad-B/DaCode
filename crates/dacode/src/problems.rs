//! The problems in the file on screen.
//!
//! A language server publishes diagnostics for the whole workspace as it indexes, and all of them
//! are kept, so switching to a file shows its problems at once rather than after the server answers
//! again. What the panel, the activity badge and the statusline show is only the **current** file's:
//! a list of every file's problems read as a wall of errors that had nothing to do with the file
//! being worked on, and it carried on showing them after the folder they came from was closed.
//!
//! **Staleness works differently here.** The active buffer's diagnostics are version-tagged and a
//! reply computed against a superseded version is refused, because they are drawn *on* text whose
//! offsets must still be valid. A diagnostic held for a file that is not open has no such constraint:
//! it is a line and a message, and the worst a slightly old one costs is a jump to a line that has
//! since moved.
//!
//! What is bounded, and where:
//!
//! - `MAX_PER_FILE` stops one file with a broken macro from producing an unreadable list, and the
//!   panel says how many it left out.
//! - `MAX_TOTAL` bounds what is *held* across every file, by forgetting whole files other than the
//!   current one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// At most this many problems from any one file.
const MAX_PER_FILE: usize = 200;

/// At most this many in the whole project.
const MAX_TOTAL: usize = 5_000;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn from_lsp(severity: Option<dacode_lsp::lsp_types::DiagnosticSeverity>) -> Self {
        use dacode_lsp::lsp_types::DiagnosticSeverity as S;
        match severity {
            Some(S::ERROR) => Severity::Error,
            Some(S::INFORMATION) => Severity::Information,
            Some(S::HINT) => Severity::Hint,
            // Unset means warning, per the protocol's own default.
            _ => Severity::Warning,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Information => "info",
            Severity::Hint => "hint",
        }
    }
}

/// One problem, flattened out of the protocol into what the panel needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    /// Zero-based, as everything internal is. The panel adds one.
    pub line: usize,
    pub column: usize,
    pub severity: Severity,
    pub message: String,
}

/// Every file's problems, keyed by canonical path.
///
/// Keyed by `PathBuf` rather than by the URI string: the client already decodes the URI, and two
/// spellings of one path — `\\?\` prefixes, separator direction, case — are exactly what
/// `dacode_project::canonical` exists to reconcile. Keying by URI text would reintroduce all of it.
#[derive(Debug, Default)]
pub struct Store {
    files: BTreeMap<PathBuf, Vec<Problem>>,
    /// How many each file had beyond `MAX_PER_FILE`, so the panel can say so.
    dropped: BTreeMap<PathBuf, usize>,
    /// The file on screen, canonical: the one whose problems are shown.
    current: Option<PathBuf>,
}

impl Store {
    /// Replaces one file's problems. A server republishes a file whole, never incrementally.
    pub fn publish(
        &mut self,
        path: &Path,
        diagnostics: &[dacode_lsp::lsp_types::Diagnostic],
        text: Option<&dacode_text::Text>,
    ) {
        let key = dacode_project::canonical(path);

        if diagnostics.is_empty() {
            self.files.remove(&key);
            self.dropped.remove(&key);
            return;
        }

        let mut items: Vec<Problem> = diagnostics
            .iter()
            .take(MAX_PER_FILE)
            .map(|diagnostic| {
                // The protocol's line and character are what the panel shows, and for a file that
                // is not open they are the only thing available — there is no rope to convert
                // against. For one that is open the numbers agree anyway, since both are what the
                // server computed.
                let _ = text;
                Problem {
                    path: key.clone(),
                    line: diagnostic.range.start.line as usize,
                    column: diagnostic.range.start.character as usize,
                    severity: Severity::from_lsp(diagnostic.severity),
                    message: diagnostic.message.lines().next().unwrap_or("").to_string(),
                }
            })
            .collect();

        let over = diagnostics.len().saturating_sub(items.len());
        if over > 0 {
            self.dropped.insert(key.clone(), over);
        } else {
            self.dropped.remove(&key);
        }
        items.sort_by_key(|problem| (problem.line, problem.column, problem.severity));
        self.files.insert(key, items);

        // The total cap forgets whole files, never the one on screen: a held file is only a head
        // start for when it is opened, and the server republishes it then anyway.
        while self.total() > MAX_TOTAL {
            let Some(last) = self
                .files
                .keys()
                .rev()
                .find(|path| Some(*path) != self.current.as_ref())
                .cloned()
            else {
                break;
            };
            self.files.remove(&last);
            self.dropped.remove(&last);
        }
    }

    /// Makes `path` the file whose problems are shown. Called when the buffer on screen changes;
    /// canonicalised once here rather than on every frame that draws the counts.
    pub fn set_current(&mut self, path: Option<&Path>) {
        self.current = path.map(dacode_project::canonical);
    }

    fn current_problems(&self) -> &[Problem] {
        self.current
            .as_ref()
            .and_then(|path| self.files.get(path))
            .map_or(&[], Vec::as_slice)
    }

    /// Forgets a file — it was closed by the server, or the project changed.
    pub fn forget(&mut self, path: &Path) {
        let key = dacode_project::canonical(path);
        self.files.remove(&key);
        self.dropped.remove(&key);
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.dropped.clear();
    }

    pub fn total(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }

    /// How many of the current file's problems are not shown.
    pub fn dropped(&self) -> usize {
        self.current
            .as_ref()
            .and_then(|path| self.dropped.get(path))
            .copied()
            .unwrap_or(0)
    }

    /// Errors and warnings in the current file.
    pub fn counts(&self) -> (usize, usize) {
        let mut errors = 0;
        let mut warnings = 0;
        for problem in self.current_problems() {
            match problem.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                _ => {}
            }
        }
        (errors, warnings)
    }

    /// The current file's problems, by line.
    pub fn ordered(&self) -> &[Problem] {
        self.current_problems()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dacode_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

    fn diagnostic(line: u32, severity: DiagnosticSeverity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position { line, character: 0 },
                end: Position { line, character: 4 },
            },
            severity: Some(severity),
            message: message.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn problems_from_files_that_are_not_open_are_kept_but_not_shown() {
        // Kept, so opening the file shows them at once; not shown, because a list of every file's
        // problems buried the file being worked on under ones it had nothing to do with.
        let mut store = Store::default();
        store.publish(
            Path::new("/project/src/other.rs"),
            &[diagnostic(4, DiagnosticSeverity::ERROR, "mismatched types")],
            None,
        );

        assert_eq!(store.total(), 1);
        assert_eq!(store.counts(), (0, 0));
        assert!(store.ordered().is_empty());

        store.set_current(Some(Path::new("/project/src/other.rs")));
        assert_eq!(store.counts(), (1, 0));
        assert_eq!(store.ordered()[0].message, "mismatched types");
    }

    #[test]
    fn republishing_a_file_replaces_it_rather_than_appending() {
        // A server sends a file whole every time. Appending would grow without bound and show
        // problems that had already been fixed.
        let mut store = Store::default();
        let path = Path::new("/project/a.rs");

        store.publish(
            path,
            &[diagnostic(1, DiagnosticSeverity::ERROR, "one")],
            None,
        );
        store.publish(
            path,
            &[
                diagnostic(1, DiagnosticSeverity::ERROR, "one"),
                diagnostic(9, DiagnosticSeverity::WARNING, "two"),
            ],
            None,
        );
        assert_eq!(store.total(), 2);

        // And an empty publish means "this file is clean now".
        store.publish(path, &[], None);
        assert_eq!(store.total(), 0);
    }

    #[test]
    fn only_the_file_you_are_in_is_listed() {
        // Switching files switches the list: the problems of the file left behind are not the ones
        // anyone is looking at.
        let mut store = Store::default();
        store.publish(
            Path::new("/project/zzz.rs"),
            &[diagnostic(1, DiagnosticSeverity::ERROR, "in zzz")],
            None,
        );
        store.publish(
            Path::new("/project/aaa.rs"),
            &[diagnostic(1, DiagnosticSeverity::WARNING, "in aaa")],
            None,
        );

        store.set_current(Some(Path::new("/project/zzz.rs")));
        let messages: Vec<&str> = store.ordered().iter().map(|p| p.message.as_str()).collect();
        assert_eq!(messages, ["in zzz"]);

        store.set_current(Some(Path::new("/project/aaa.rs")));
        assert_eq!(store.counts(), (0, 1));

        // With no file open, nothing.
        store.set_current(None);
        assert!(store.ordered().is_empty());
    }

    #[test]
    fn one_broken_file_cannot_hide_every_other() {
        // A single macro expansion can produce thousands. Without the per-file cap it would fill
        // the budget and the panel would show one file's problems and nothing else.
        let mut store = Store::default();
        let flood: Vec<Diagnostic> = (0..MAX_PER_FILE + 50)
            .map(|line| diagnostic(line as u32, DiagnosticSeverity::ERROR, "flood"))
            .collect();

        store.set_current(Some(Path::new("/project/broken.rs")));
        store.publish(Path::new("/project/broken.rs"), &flood, None);
        assert_eq!(store.total(), MAX_PER_FILE);
        assert_eq!(
            store.dropped(),
            50,
            "and it must say how many it dropped — a panel that truncates silently is a panel \
             that has lied about being complete"
        );
    }
}
