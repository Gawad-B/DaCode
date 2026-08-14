//! Every problem in the project, not just in the file on screen.
//!
//! A language server publishes diagnostics for the whole workspace as it indexes, and vex threw away
//! everything that was not the active buffer. So the question people actually ask — "did my change
//! break anything?" — could not be answered inside an editor that also has no terminal to ask it
//! from. This keeps them.
//!
//! **Staleness works differently here.** The active buffer's diagnostics are version-tagged and a
//! reply computed against a superseded version is refused, because they are drawn *on* text whose
//! offsets must still be valid. A diagnostic for a file that is not open has no such constraint: it
//! is a path, a line and a message, and the worst that a slightly old one costs is a jump to a line
//! that has since moved. Refusing them would mean refusing nearly all of them, since the version a
//! server quotes is the version of *that* file and the editor is holding a different one.
//!
//! What is bounded, and where:
//!
//! - `MAX_PER_FILE` stops one file with a broken macro from consuming the whole budget and hiding
//!   every other file's problems.
//! - `MAX_TOTAL` bounds what is *held*, which is what bounds the sort — the only part of this that
//!   is more than linear.
//!
//! Both are reported rather than applied silently: a panel that truncates without saying so reads
//! as a panel that has told you everything.

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
    pub fn from_lsp(severity: Option<vex_lsp::lsp_types::DiagnosticSeverity>) -> Self {
        use vex_lsp::lsp_types::DiagnosticSeverity as S;
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
/// `vex_project::canonical` exists to reconcile. Keying by URI text would reintroduce all of it.
#[derive(Debug, Default)]
pub struct Store {
    files: BTreeMap<PathBuf, Vec<Problem>>,
    /// How many were dropped by the caps, so the panel can say so.
    dropped: usize,
}

impl Store {
    /// Replaces one file's problems. A server republishes a file whole, never incrementally.
    pub fn publish(
        &mut self,
        path: &Path,
        diagnostics: &[vex_lsp::lsp_types::Diagnostic],
        text: Option<&vex_text::Text>,
    ) {
        let key = vex_project::canonical(path);

        if diagnostics.is_empty() {
            self.files.remove(&key);
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

        self.dropped += diagnostics.len().saturating_sub(items.len());
        items.sort_by_key(|problem| (problem.line, problem.column, problem.severity));
        self.files.insert(key, items);

        // The total cap is applied by dropping whole files from the end, so what remains is always
        // a complete answer for the files it covers rather than a truncated one for all of them.
        while self.total() > MAX_TOTAL {
            let Some(last) = self.files.keys().next_back().cloned() else {
                break;
            };
            if let Some(removed) = self.files.remove(&last) {
                self.dropped += removed.len();
            }
        }
    }

    /// Forgets a file — it was closed by the server, or the project changed.
    pub fn forget(&mut self, path: &Path) {
        self.files.remove(&vex_project::canonical(path));
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.dropped = 0;
    }

    pub fn total(&self) -> usize {
        self.files.values().map(Vec::len).sum()
    }

    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Errors and warnings across the whole project.
    pub fn counts(&self) -> (usize, usize) {
        let mut errors = 0;
        let mut warnings = 0;
        for problem in self.files.values().flatten() {
            match problem.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                _ => {}
            }
        }
        (errors, warnings)
    }

    /// Every problem, ordered for reading.
    ///
    /// **The file you are in comes first**, then files containing an error, then the rest by path.
    /// Within a file, by line. The panel is read to decide where to go next, and going somewhere
    /// means opening a *file* — so grouping by file is what makes it scannable, and severity is
    /// carried by colour and by the tie-break rather than by the top-level order. A list sorted by
    /// severity across the project interleaves twelve files and is unreadable.
    pub fn ordered(&self, current: Option<&Path>) -> Vec<Problem> {
        let current = current.map(vex_project::canonical);

        let mut keys: Vec<&PathBuf> = self.files.keys().collect();
        keys.sort_by_key(|path| {
            let is_current = current.as_ref() == Some(*path);
            let has_error = self.files[*path]
                .iter()
                .any(|problem| problem.severity == Severity::Error);
            // false sorts before true, so the negations put "current" and "has an error" first.
            (!is_current, !has_error, (*path).clone())
        });

        keys.into_iter()
            .flat_map(|path| self.files[path].iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

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
    fn problems_from_files_that_are_not_open_are_kept() {
        // The whole point: a server reports the project as it indexes, and vex discarded everything
        // that was not the active buffer — so "did my change break anything?" had no answer inside
        // the editor.
        let mut store = Store::default();
        store.publish(
            Path::new("/project/src/other.rs"),
            &[diagnostic(4, DiagnosticSeverity::ERROR, "mismatched types")],
            None,
        );

        assert_eq!(store.total(), 1);
        assert_eq!(store.counts(), (1, 0));
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
    fn the_file_you_are_in_is_listed_first() {
        // The panel is read to decide where to go next, and the answer is most often "the thing I
        // just broke".
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

        let ordered = store.ordered(Some(Path::new("/project/zzz.rs")));
        assert_eq!(ordered[0].message, "in zzz");

        // With no file open, a file with an error outranks one with only a warning.
        let ordered = store.ordered(None);
        assert_eq!(ordered[0].message, "in zzz");
    }

    #[test]
    fn one_broken_file_cannot_hide_every_other() {
        // A single macro expansion can produce thousands. Without the per-file cap it would fill
        // the budget and the panel would show one file's problems and nothing else.
        let mut store = Store::default();
        let flood: Vec<Diagnostic> = (0..MAX_PER_FILE + 50)
            .map(|line| diagnostic(line as u32, DiagnosticSeverity::ERROR, "flood"))
            .collect();

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
