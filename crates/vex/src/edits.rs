//! Applying language-server edits to buffers and to files.
//!
//! This is the piece rename, formatting and code actions all need, and it is worth getting exactly
//! right once rather than three times. Three rules matter:
//!
//! **One undo step.** A rename that touches nine places in a file must undo as one press of `u`, not
//! nine. That falls out of the document's single mutation funnel — every edit goes into one
//! `Transaction`, tagged `Discrete` so it never coalesces with typing on either side of it.
//!
//! **Positions are UTF-16, and the document has moved on.** Server edits are computed against a
//! version of the document, and by the time they arrive the user may have typed. Applying edits to
//! the wrong version is how an editor corrupts a file, so a stale reply is refused rather than
//! best-effort merged: the user can ask again, but they cannot un-corrupt a source tree.
//!
//! **Sorted and disjoint, or nothing.** The protocol says edits must not overlap, but a server is
//! not a proof. `Transaction::validate` is the gate, and a set that fails it is rejected whole —
//! applying the half that happens to be valid would leave a file mangled in a way no undo entry
//! describes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use vex_buffer::{Document, EditKind};
use vex_lsp::lsp_types::{TextEdit, Uri, WorkspaceEdit};
use vex_lsp::position;
use vex_text::{Edit, Text, Transaction, Version};

/// What a set of edits changed, for reporting to the user.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Edits applied to the buffer on screen.
    pub in_buffer: usize,
    /// Files changed on disk, with how many edits each took.
    pub files: Vec<(PathBuf, usize)>,
}

impl Applied {
    pub fn total(&self) -> usize {
        self.in_buffer + self.files.iter().map(|(_, count)| count).sum::<usize>()
    }

    /// A one-line summary, in the terms the user thinks in.
    pub fn summary(&self, what: &str) -> String {
        let edits = self.total();
        let plural = |n: usize, noun: &str| {
            if n == 1 {
                format!("{n} {noun}")
            } else {
                format!("{n} {noun}s")
            }
        };

        if self.files.is_empty() {
            format!("{what}: {}", plural(edits, "change"))
        } else {
            format!(
                "{what}: {} in {}",
                plural(edits, "change"),
                plural(self.files.len() + usize::from(self.in_buffer > 0), "file")
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The document changed after the request was sent.
    Stale { expected: Version, found: Version },
    /// The server sent edits that overlap, or that fall outside the document.
    Invalid(String),
    /// A file the edit names could not be read or written.
    Io { path: PathBuf, message: String },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Stale { expected, found } => write!(
                f,
                "the buffer changed since the request (v{} → v{}); nothing was applied",
                expected.0, found.0
            ),
            EditError::Invalid(why) => write!(f, "the server sent edits that cannot apply: {why}"),
            EditError::Io { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
        }
    }
}

/// Converts server edits into document edits, sorted ascending.
///
/// Sorted rather than applied in reverse: a `Transaction` applies a whole set at once and accounts
/// for the shifts itself, which is the reason the reverse-order dance every other editor does is
/// absent here.
fn to_document_edits(text: &Text, edits: &[TextEdit]) -> Vec<Edit> {
    let mut converted: Vec<Edit> = edits
        .iter()
        .map(|edit| {
            let range = position::range_from_lsp(text, edit.range);
            Edit::replace(range, edit.new_text.clone())
        })
        .filter(|edit| !(edit.range.is_empty() && edit.text.is_empty()))
        .collect();

    converted.sort_by_key(|edit| (edit.range.start, edit.range.end));
    converted
}

/// Applies edits to the open document as one undoable change.
///
/// `against` is the version the server computed them for; `None` means the caller has already
/// established freshness (formatting on save, where nothing can have typed in between).
pub fn apply_to_document(
    document: &mut Document,
    edits: &[TextEdit],
    against: Option<Version>,
) -> Result<usize, EditError> {
    if let Some(expected) = against {
        if document.version() != expected {
            return Err(EditError::Stale {
                expected,
                found: document.version(),
            });
        }
    }

    let converted = to_document_edits(document.text(), edits);
    if converted.is_empty() {
        return Ok(0);
    }

    let count = converted.len();
    let transaction = Transaction::new(converted);
    // Validated before applying, so a bad set changes nothing at all.
    transaction
        .validate(document.text())
        .map_err(|err| EditError::Invalid(err.to_string()))?;

    document
        .apply(transaction, EditKind::Discrete)
        .map_err(|err| EditError::Invalid(err.to_string()))?;
    Ok(count)
}

/// Applies edits to a file that is not open, in place.
///
/// Written whole through a temporary file in the same directory and renamed over the original, so an
/// interrupted rename cannot leave a half-written source file. The same reasoning as the crash
/// recovery writer.
fn apply_to_file(path: &Path, edits: &[TextEdit]) -> Result<usize, EditError> {
    let io = |message: String| EditError::Io {
        path: path.to_path_buf(),
        message,
    };

    let contents = std::fs::read_to_string(path).map_err(|err| io(err.to_string()))?;
    let mut text = Text::from_str(&contents);

    let converted = to_document_edits(&text, edits);
    if converted.is_empty() {
        return Ok(0);
    }

    let count = converted.len();
    let transaction = Transaction::new(converted);
    transaction
        .validate(&text)
        .map_err(|err| EditError::Invalid(format!("{}: {err}", path.display())))?;
    transaction
        .apply(&mut text)
        .map_err(|err| EditError::Invalid(format!("{}: {err}", path.display())))?;

    let temporary = path.with_extension(format!(
        "{}.vex-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&temporary, text.to_string()).map_err(|err| io(err.to_string()))?;
    std::fs::rename(&temporary, path).map_err(|err| io(err.to_string()))?;

    Ok(count)
}

/// Applies a workspace edit: the open buffer through its undo history, other files on disk.
///
/// Files that are not open are written immediately and are **not** undoable, which is a real
/// limitation and the reason the caller reports how many files were touched. Making a cross-file
/// rename undoable needs a workspace-level history, and inventing half of one here would be worse
/// than saying plainly what happened.
pub fn apply_workspace_edit(
    document: &mut Document,
    open_path: Option<&Path>,
    edit: &WorkspaceEdit,
    against: Option<Version>,
) -> Result<Applied, EditError> {
    let mut by_file: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();

    // Two shapes carry the same information: `changes` is a map, `document_changes` a list that can
    // also create and delete files. Only text edits are honored; a server that wants to rename files
    // during a symbol rename is refused rather than half-obeyed.
    if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            if let Some(path) = path_for(uri) {
                by_file.entry(path).or_default().extend(edits.clone());
            }
        }
    }
    if let Some(document_changes) = &edit.document_changes {
        use vex_lsp::lsp_types::DocumentChanges;
        match document_changes {
            DocumentChanges::Edits(edits) => {
                for change in edits {
                    if let Some(path) = path_for(&change.text_document.uri) {
                        let entry = by_file.entry(path).or_default();
                        for edit in &change.edits {
                            entry.push(text_edit_of(edit));
                        }
                    }
                }
            }
            DocumentChanges::Operations(operations) => {
                use vex_lsp::lsp_types::DocumentChangeOperation;
                for operation in operations {
                    match operation {
                        DocumentChangeOperation::Edit(change) => {
                            if let Some(path) = path_for(&change.text_document.uri) {
                                let entry = by_file.entry(path).or_default();
                                for edit in &change.edits {
                                    entry.push(text_edit_of(edit));
                                }
                            }
                        }
                        DocumentChangeOperation::Op(_) => {
                            return Err(EditError::Invalid(
                                "the server asked to create, delete or rename files, which vex does \
                                 not do as part of an edit"
                                    .into(),
                            ));
                        }
                    }
                }
            }
        }
    }

    let open = open_path.map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));

    // The open buffer first: if its version is stale, nothing at all should have been written.
    let mut applied = Applied::default();
    if let Some(open) = &open {
        if let Some(edits) = by_file.remove(open) {
            applied.in_buffer = apply_to_document(document, &edits, against)?;
        }
    }

    for (path, edits) in by_file {
        let count = apply_to_file(&path, &edits)?;
        if count > 0 {
            applied.files.push((path, count));
        }
    }

    Ok(applied)
}

/// The path a URI names, canonicalized so the open buffer is recognized however it was spelled.
fn path_for(uri: &Uri) -> Option<PathBuf> {
    let path = vex_lsp::position::uri_to_path(uri.as_str())?;
    Some(path.canonicalize().unwrap_or(path))
}

/// Unwraps the annotated form of a text edit. Annotations describe changes for a user to approve
/// individually, which vex does not offer, so the edit itself is what matters.
fn text_edit_of(
    edit: &vex_lsp::lsp_types::OneOf<TextEdit, vex_lsp::lsp_types::AnnotatedTextEdit>,
) -> TextEdit {
    match edit {
        vex_lsp::lsp_types::OneOf::Left(edit) => edit.clone(),
        vex_lsp::lsp_types::OneOf::Right(annotated) => annotated.text_edit.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vex_lsp::lsp_types::{Position, Range};

    /// A URI for a path, through the same conversion the client uses — so the tests exercise the
    /// round trip rather than a second implementation of it.
    fn uri(path: &Path) -> Uri {
        use std::str::FromStr;
        Uri::from_str(&position::path_to_uri(path)).expect("a valid file URI")
    }

    fn edit(line: u32, from: u32, to: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: Position {
                    line,
                    character: from,
                },
                end: Position {
                    line,
                    character: to,
                },
            },
            new_text: text.to_string(),
        }
    }

    #[test]
    fn several_edits_apply_as_one_undo_step() {
        // The point of the whole module: a rename touching three places is one `u`.
        let mut document = Document::from_str("let value = value + value;\n");
        let version = document.version();

        let count = apply_to_document(
            &mut document,
            &[
                edit(0, 4, 9, "total"),
                edit(0, 12, 17, "total"),
                edit(0, 20, 25, "total"),
            ],
            Some(version),
        )
        .expect("applies");

        assert_eq!(count, 3);
        assert_eq!(document.text().to_string(), "let total = total + total;\n");

        assert!(document.undo());
        assert_eq!(
            document.text().to_string(),
            "let value = value + value;\n",
            "one undo restores everything the rename touched"
        );
    }

    #[test]
    fn edits_arriving_out_of_order_still_apply_correctly() {
        // Servers are not required to sort, and applying front-to-back without accounting for shifts
        // is the classic way a rename eats a character.
        let mut document = Document::from_str("aaa bbb ccc\n");
        let version = document.version();

        apply_to_document(
            &mut document,
            &[edit(0, 8, 11, "three"), edit(0, 0, 3, "one")],
            Some(version),
        )
        .expect("applies");

        assert_eq!(document.text().to_string(), "one bbb three\n");
    }

    #[test]
    fn a_stale_reply_is_refused_whole() {
        let mut document = Document::from_str("let value = 1;\n");
        let version = document.version();

        // The user types while the server is thinking.
        document.insert("x");
        let after_typing = document.text().to_string();

        let result = apply_to_document(&mut document, &[edit(0, 4, 9, "total")], Some(version));

        assert!(matches!(result, Err(EditError::Stale { .. })));
        assert_eq!(
            document.text().to_string(),
            after_typing,
            "a stale reply must not touch the buffer at all"
        );
    }

    #[test]
    fn overlapping_edits_are_rejected_without_applying_any() {
        let mut document = Document::from_str("let value = 1;\n");
        let version = document.version();

        let result = apply_to_document(
            &mut document,
            &[edit(0, 4, 9, "total"), edit(0, 6, 12, "other")],
            Some(version),
        );

        assert!(matches!(result, Err(EditError::Invalid(_))), "{result:?}");
        assert_eq!(
            document.text().to_string(),
            "let value = 1;\n",
            "nothing is applied when part of the set is bad"
        );
        assert_eq!(document.history().depth(), 0, "and no undo entry");
    }

    #[test]
    fn utf16_positions_are_honored() {
        // An emoji is two UTF-16 code units and one char. A server counting the former and an editor
        // counting the latter is the bug this conversion exists to prevent.
        let mut document = Document::from_str("let 🌍 = value;\n");
        let version = document.version();

        // `value` starts at UTF-16 character 9, not char 8.
        apply_to_document(&mut document, &[edit(0, 9, 14, "world")], Some(version))
            .expect("applies");

        assert_eq!(document.text().to_string(), "let 🌍 = world;\n");
    }

    #[test]
    fn an_empty_edit_set_changes_nothing() {
        let mut document = Document::from_str("unchanged\n");
        let version = document.version();

        assert_eq!(
            apply_to_document(&mut document, &[], Some(version)).unwrap(),
            0
        );
        assert_eq!(document.version(), version, "no version bump either");
        assert!(!document.is_dirty());
    }

    #[test]
    fn a_no_op_edit_does_not_dirty_the_buffer() {
        // Formatters routinely return an edit that replaces a range with what it already contains.
        let mut document = Document::from_str("fine\n");
        let version = document.version();

        let count = apply_to_document(&mut document, &[edit(0, 0, 0, "")], Some(version)).unwrap();

        assert_eq!(count, 0);
        assert!(!document.is_dirty(), "an empty replacement is not a change");
    }

    // `WorkspaceEdit::changes` is keyed by `Uri`, whose interior mutability trips the lint. The
    // type is the protocol's, not ours, and the keys are never mutated after insertion.
    #[allow(clippy::mutable_key_type)]
    #[test]
    fn a_workspace_edit_writes_other_files_and_reports_them() {
        let dir = std::env::temp_dir().join(format!("vex-edits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.rs");
        std::fs::write(&other, "use value;\n").unwrap();

        let open = dir.join("open.rs");
        std::fs::write(&open, "let value = 1;\n").unwrap();
        let mut document = Document::from_str("let value = 1;\n");
        let version = document.version();

        let mut changes = std::collections::HashMap::new();
        changes.insert(uri(&open), vec![edit(0, 4, 9, "total")]);
        changes.insert(uri(&other), vec![edit(0, 4, 9, "total")]);

        let applied = apply_workspace_edit(
            &mut document,
            Some(&open),
            &WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            },
            Some(version),
        )
        .expect("applies");

        assert_eq!(
            applied.in_buffer, 1,
            "the open file went through the buffer"
        );
        assert_eq!(applied.files.len(), 1, "and the other one through the disk");
        assert_eq!(applied.total(), 2);
        assert_eq!(document.text().to_string(), "let total = 1;\n");
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "use total;\n");
        assert_eq!(applied.summary("rename"), "rename: 2 changes in 2 files");

        // The temporary file used for the atomic write must not be left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("vex-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left a temporary file behind");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[allow(clippy::mutable_key_type)]
    #[test]
    fn a_stale_workspace_edit_leaves_other_files_untouched() {
        // The open buffer is checked first precisely so that a stale reply cannot half-rename a
        // source tree.
        let dir = std::env::temp_dir().join(format!("vex-edits-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.rs");
        std::fs::write(&other, "use value;\n").unwrap();
        let open = dir.join("open.rs");

        let mut document = Document::from_str("let value = 1;\n");
        let version = document.version();
        document.insert("x");

        let mut changes = std::collections::HashMap::new();
        changes.insert(uri(&open), vec![edit(0, 4, 9, "total")]);
        changes.insert(uri(&other), vec![edit(0, 4, 9, "total")]);

        let result = apply_workspace_edit(
            &mut document,
            Some(&open),
            &WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            },
            Some(version),
        );

        assert!(matches!(result, Err(EditError::Stale { .. })));
        assert_eq!(
            std::fs::read_to_string(&other).unwrap(),
            "use value;\n",
            "the other file was never written"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_creation_and_deletion_are_refused() {
        use vex_lsp::lsp_types::{
            CreateFile, DocumentChangeOperation, DocumentChanges, ResourceOp,
        };

        let mut document = Document::from_str("");
        let result = apply_workspace_edit(
            &mut document,
            None,
            &WorkspaceEdit {
                document_changes: Some(DocumentChanges::Operations(vec![
                    DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                        uri: uri(Path::new("/tmp/new.rs")),
                        options: None,
                        annotation_id: None,
                    })),
                ])),
                ..Default::default()
            },
            None,
        );

        assert!(matches!(result, Err(EditError::Invalid(_))));
    }

    #[test]
    fn the_summary_reads_like_a_sentence() {
        let one = Applied {
            in_buffer: 1,
            files: Vec::new(),
        };
        assert_eq!(one.summary("rename"), "rename: 1 change");

        let many = Applied {
            in_buffer: 4,
            files: vec![(PathBuf::from("a.rs"), 2)],
        };
        assert_eq!(many.summary("rename"), "rename: 6 changes in 2 files");
    }
}

/// Replaces every match of `pattern` with `replacement`, across the files a search found.
///
/// Driven by the search results the user is already looking at, rather than by a fresh sweep. That
/// is the honest design: you replace in what you can see, having read it, and the count in the panel
/// is the count that changes. A replace that silently touched files never listed would be a
/// different and much more dangerous operation.
///
/// The open buffer goes through its undo history; other files are written atomically and are not
/// undoable — the same limitation a cross-file rename has, and reported the same way.
pub fn replace_in_hits(
    document: &mut Document,
    open_path: Option<&Path>,
    hits: &[vex_project::SearchHit],
    pattern: &str,
    replacement: &str,
) -> Result<Applied, EditError> {
    if pattern.is_empty() {
        return Err(EditError::Invalid("nothing to replace".into()));
    }

    // Grouped per file, and *rebuilt* from each file's text rather than trusting the byte ranges in
    // the hits: those were computed when the search ran, and a file edited since would be corrupted
    // by applying them blindly.
    let mut by_file: BTreeMap<PathBuf, ()> = BTreeMap::new();
    for hit in hits {
        by_file.insert(
            hit.path.canonicalize().unwrap_or_else(|_| hit.path.clone()),
            (),
        );
    }

    let open = open_path.map(|path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()));
    let mut applied = Applied::default();

    for (path, ()) in by_file {
        if Some(&path) == open.as_ref() {
            let text = document.text().to_string();
            let count = text.matches(pattern).count();
            if count == 0 {
                continue;
            }
            let edits = replacement_edits(&text, pattern, replacement);
            applied.in_buffer = apply_to_document(document, &edits, None)?;
            continue;
        }

        let contents = std::fs::read_to_string(&path).map_err(|err| EditError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        if !contents.contains(pattern) {
            continue;
        }

        let edits = replacement_edits(&contents, pattern, replacement);
        let count = apply_to_file(&path, &edits)?;
        if count > 0 {
            applied.files.push((path, count));
        }
    }

    Ok(applied)
}

/// Every occurrence of `pattern` in `text`, as LSP-shaped edits.
///
/// Built in line/character terms because that is what the edit pipeline already validates and
/// applies; going through it means replace inherits the same overlap checking and the same single
/// undo entry rather than growing a second path with its own bugs.
fn replacement_edits(text: &str, pattern: &str, replacement: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();

    for (line_number, line) in text.lines().enumerate() {
        let mut from = 0;
        while let Some(found) = line[from..].find(pattern) {
            let start = from + found;
            let end = start + pattern.len();

            // UTF-16 code units, which is what LSP positions count — a match after an emoji is at a
            // different character than its byte offset suggests.
            let to_utf16 = |byte: usize| line[..byte].encode_utf16().count() as u32;

            edits.push(TextEdit {
                range: vex_lsp::lsp_types::Range {
                    start: vex_lsp::lsp_types::Position {
                        line: line_number as u32,
                        character: to_utf16(start),
                    },
                    end: vex_lsp::lsp_types::Position {
                        line: line_number as u32,
                        character: to_utf16(end),
                    },
                },
                new_text: replacement.to_string(),
            });
            from = end;
        }
    }

    edits
}

#[cfg(test)]
mod replace_tests {
    use super::*;
    use vex_project::SearchHit;

    fn hit(path: &Path) -> SearchHit {
        SearchHit {
            path: path.to_path_buf(),
            line_number: 1,
            line: String::new(),
            range: 0..0,
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-replace-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_occurrence_in_the_open_buffer_changes_as_one_undo_step() {
        let dir = scratch("buffer");
        let open = dir.join("open.rs");
        std::fs::write(&open, "").unwrap();

        let mut document = Document::from_str("let a = old; let b = old;\nold\n");
        let applied =
            replace_in_hits(&mut document, Some(&open), &[hit(&open)], "old", "new").unwrap();

        assert_eq!(applied.in_buffer, 3);
        assert_eq!(
            document.text().to_string(),
            "let a = new; let b = new;\nnew\n"
        );

        assert!(document.undo());
        assert_eq!(
            document.text().to_string(),
            "let a = old; let b = old;\nold\n",
            "one undo, not three"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_that_are_not_open_are_rewritten_on_disk() {
        let dir = scratch("files");
        let one = dir.join("one.rs");
        let two = dir.join("two.rs");
        std::fs::write(&one, "old and old\n").unwrap();
        std::fs::write(&two, "nothing here\n").unwrap();

        let mut document = Document::from_str("");
        let applied =
            replace_in_hits(&mut document, None, &[hit(&one), hit(&two)], "old", "new").unwrap();

        assert_eq!(std::fs::read_to_string(&one).unwrap(), "new and new\n");
        assert_eq!(
            std::fs::read_to_string(&two).unwrap(),
            "nothing here\n",
            "a file with no match is left alone entirely"
        );
        assert_eq!(applied.files.len(), 1, "and is not counted as changed");
        assert_eq!(applied.total(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn matches_are_found_by_rereading_the_file_not_by_trusting_stale_offsets() {
        // The ranges in a SearchHit were computed when the search ran. If the file changed since,
        // applying them blindly corrupts it — so they are deliberately not used.
        let dir = scratch("stale");
        let file = dir.join("moved.rs");
        std::fs::write(&file, "old\n").unwrap();

        let mut stale = hit(&file);
        stale.range = 900..950;
        stale.line_number = 42;

        let mut document = Document::from_str("");
        let applied = replace_in_hits(&mut document, None, &[stale], "old", "new").unwrap();

        assert_eq!(applied.total(), 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn utf16_positions_survive_a_replace_after_an_emoji() {
        let dir = scratch("utf16");
        let file = dir.join("emoji.rs");
        std::fs::write(&file, "// 🌍 old\n").unwrap();

        let mut document = Document::from_str("");
        replace_in_hits(&mut document, None, &[hit(&file)], "old", "new").unwrap();

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "// 🌍 new\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_pattern_is_refused() {
        let mut document = Document::from_str("text");
        assert!(matches!(
            replace_in_hits(&mut document, None, &[], "", "x"),
            Err(EditError::Invalid(_))
        ));
    }

    #[test]
    fn replacing_with_a_longer_string_does_not_shift_later_matches() {
        // Every edit is computed against the original text and applied as one transaction, so a
        // replacement longer than what it replaces cannot disturb the ranges after it.
        let mut document = Document::from_str("a a a\n");
        let dir = scratch("shift");
        let open = dir.join("open.rs");
        std::fs::write(&open, "").unwrap();

        replace_in_hits(&mut document, Some(&open), &[hit(&open)], "a", "xxxx").unwrap();
        assert_eq!(document.text().to_string(), "xxxx xxxx xxxx\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
