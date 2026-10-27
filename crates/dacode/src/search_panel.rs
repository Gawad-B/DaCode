//! The search view in the sidebar: find across the project, and replace.
//!
//! What `ctrl-shift-f` opens, as it does in VSCode — a box for the query with the three toggles that
//! decide what matches (case, whole word, regex), a box for the replacement, and the hits grouped by
//! file underneath. `ctrl-f` stays the search in this file; this is the other one.
//!
//! The search streams from the same worker the results panel uses, restarted on every keystroke —
//! the old one is cancelled when it is dropped, so a query being typed costs one walk at a time.
//!
//! **Replace all writes files**, on a worker of its own, through [`crate::save::write`]. It is not
//! undoable, any more than a cross-file rename is, and it asks to be pressed twice for that reason.
//! A file open with unsaved changes is left alone and counted: rewriting it on disk under a dirty
//! buffer would set up a save that throws the replacement away. An open file with nothing unsaved is
//! rewritten like any other, and the watcher reloads it.

use crate::results::{MAX_HITS, Results};
use dacode_project::{SearchHit, SearchQuery};
use dacode_render::Rect;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// Which box the keys type into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Query,
    Replace,
}

/// One of the switches beside the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Case,
    Word,
    Regex,
}

impl Toggle {
    pub const ALL: [Toggle; 3] = [Toggle::Case, Toggle::Word, Toggle::Regex];

    /// What the button shows: the glyphs every editor uses for these, so they read without a label.
    pub fn glyph(self) -> &'static str {
        match self {
            Toggle::Case => "Aa",
            Toggle::Word => "ab",
            Toggle::Regex => ".*",
        }
    }
}

/// A line of the results: a file's heading, or one of its hits by index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    File { path: PathBuf, count: usize },
    Hit(usize),
}

/// What a finished replace-all did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub replacements: usize,
    /// Open with unsaved changes, so not touched.
    pub skipped: usize,
    /// Files that matched and could not be written, each with why.
    pub failed: Vec<String>,
}

impl Report {
    /// The sentence the status line shows.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "replaced {} in {} file{}",
            self.replacements,
            self.files,
            if self.files == 1 { "" } else { "s" }
        );
        if self.skipped > 0 {
            out.push_str(&format!(
                " — {} with unsaved changes left alone",
                self.skipped
            ));
        }
        if let Some(first) = self.failed.first() {
            out.push_str(&format!(" — {} failed: {first}", self.failed.len()));
        }
        out
    }
}

/// Where everything in the panel is, as data: drawing and the mouse both read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub heading: Rect,
    pub query: Rect,
    pub toggles: [(Toggle, Rect); 3],
    pub replace: Rect,
    pub replace_all: Rect,
    pub summary: Rect,
    /// The visible rows, `(index into the row list, rectangle)`.
    pub rows: Vec<(usize, Rect)>,
    /// How many rows there is room for.
    pub capacity: usize,
}

impl Geometry {
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows
            .iter()
            .find(|(_, rect)| crate::layout::contains(*rect, x, y))
            .map(|(index, _)| *index)
    }
}

/// The height of a result row, at scale 1.
pub const ROW: f32 = 22.0;

pub struct SearchPanel {
    pub query: String,
    pub replace: String,
    pub field: Field,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    results: Option<Results>,
    /// The results flattened into headings and hits, rebuilt when hits arrive rather than per frame.
    rows: Vec<Row>,
    /// The selected hit, by index into the hits.
    pub selected: usize,
    /// The first visible row.
    first: usize,
    /// Replace all was pressed once and is waiting for the second press.
    pub confirming: bool,
    replacing: Option<Receiver<Report>>,
}

impl SearchPanel {
    pub fn new(query: &str) -> Self {
        Self {
            query: query.to_string(),
            replace: String::new(),
            field: Field::Query,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            results: None,
            rows: Vec::new(),
            selected: 0,
            first: 0,
            confirming: false,
            replacing: None,
        }
    }

    /// The query as the toggles set it. With "match case" off the search ignores case outright, as
    /// VSCode's does, rather than guessing from the pattern.
    pub fn search_query(&self) -> SearchQuery {
        SearchQuery {
            pattern: self.query.clone(),
            literal: !self.regex,
            case_sensitive: Some(self.case_sensitive),
            whole_word: self.whole_word,
            ..Default::default()
        }
    }

    /// Starts the search again from the query and toggles as they are now.
    pub fn restart(&mut self, root: &Path) {
        self.results =
            (!self.query.is_empty()).then(|| Results::start_with(root, self.search_query()));
        self.rows.clear();
        self.selected = 0;
        self.first = 0;
        self.confirming = false;
    }

    pub fn is_on(&self, toggle: Toggle) -> bool {
        match toggle {
            Toggle::Case => self.case_sensitive,
            Toggle::Word => self.whole_word,
            Toggle::Regex => self.regex,
        }
    }

    pub fn flip(&mut self, toggle: Toggle, root: &Path) {
        match toggle {
            Toggle::Case => self.case_sensitive = !self.case_sensitive,
            Toggle::Word => self.whole_word = !self.whole_word,
            Toggle::Regex => self.regex = !self.regex,
        }
        self.restart(root);
    }

    /// Types into the focused box. Only the query restarts the search: the replacement changes what
    /// would be written, not what matches.
    pub fn type_text(&mut self, text: &str, root: &Path) {
        self.confirming = false;
        match self.field {
            Field::Query => {
                self.query.push_str(text);
                self.restart(root);
            }
            Field::Replace => self.replace.push_str(text),
        }
    }

    pub fn backspace(&mut self, root: &Path) {
        self.confirming = false;
        match self.field {
            Field::Query => {
                if self.query.pop().is_some() {
                    self.restart(root);
                }
            }
            Field::Replace => {
                self.replace.pop();
            }
        }
    }

    pub fn other_field(&mut self) {
        self.field = match self.field {
            Field::Query => Field::Replace,
            Field::Replace => Field::Query,
        };
    }

    /// Collects hits that have arrived. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(results) = &mut self.results else {
            return false;
        };
        let changed = results.poll();
        if changed {
            self.rows = group(&results.hits);
        }
        changed
    }

    /// Whether a search is still streaming, so the loop keeps polling it.
    pub fn searching(&self) -> bool {
        self.results
            .as_ref()
            .is_some_and(|results| !results.finished)
    }

    pub fn hits(&self) -> &[SearchHit] {
        self.results.as_ref().map_or(&[], |results| &results.hits)
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selection(&self) -> Option<&SearchHit> {
        self.hits().get(self.selected)
    }

    /// Moves the selection by `delta` hits and keeps it inside a window of `capacity` rows.
    pub fn move_selection(&mut self, delta: isize, capacity: usize) {
        let count = self.hits().len();
        if count == 0 {
            return;
        }
        self.selected = self.selected.saturating_add_signed(delta).min(count - 1);
        self.reveal(capacity);
    }

    /// Selects the hit on row `row`, if it is one.
    pub fn select_row(&mut self, row: usize) -> bool {
        match self.rows.get(row) {
            Some(Row::Hit(index)) => {
                self.selected = *index;
                true
            }
            _ => false,
        }
    }

    fn reveal(&mut self, capacity: usize) {
        let Some(row) = self
            .rows
            .iter()
            .position(|row| *row == Row::Hit(self.selected))
        else {
            return;
        };
        let capacity = capacity.max(1);
        if row < self.first {
            // One row more, so a file's first hit brings its heading with it.
            self.first = row.saturating_sub(1);
        } else if row >= self.first + capacity {
            self.first = row + 1 - capacity;
        }
    }

    /// Scrolls the list by whole rows, as the wheel does.
    pub fn scroll(&mut self, rows: isize, capacity: usize) {
        let last = self.rows.len().saturating_sub(capacity.max(1));
        self.first = self.first.saturating_add_signed(rows).min(last);
    }

    /// "12 results in 3 files", or what the search is doing instead.
    pub fn summary(&self) -> String {
        let Some(results) = &self.results else {
            return "type to search the project".to_string();
        };
        let hits = results.hits.len();
        let files = self
            .rows
            .iter()
            .filter(|row| matches!(row, Row::File { .. }))
            .count();
        if hits == 0 {
            return if results.finished {
                "no results".to_string()
            } else {
                "searching…".to_string()
            };
        }
        // A capped list says it is one, so the count is not read as the whole answer.
        let more = if results.truncated {
            format!(" (first {MAX_HITS})")
        } else {
            String::new()
        };
        format!(
            "{hits} result{} in {files} file{}{more}",
            if hits == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" },
        )
    }

    /// Lays the panel out in `rect`.
    pub fn geometry(&self, rect: Rect, scale: f32) -> Geometry {
        let pad = 12.0 * scale;
        let width = (rect.width - pad * 2.0).max(0.0);
        let left = rect.x + pad;
        let field = 28.0 * scale;
        let gap = 6.0 * scale;
        let button = 24.0 * scale;

        let heading = Rect::new(left, rect.y + 10.0 * scale, width, 22.0 * scale);
        let query = Rect::new(left, heading.bottom() + gap, width, field);
        // The toggles sit inside the query box, at its right, as in VSCode.
        let inset = (field - button) / 2.0;
        let mut x = query.right() - inset;
        let mut toggles = Toggle::ALL.map(|toggle| (toggle, Rect::new(0.0, 0.0, 0.0, 0.0)));
        for slot in toggles.iter_mut().rev() {
            x -= button;
            slot.1 = Rect::new(x, query.y + inset, button, button);
            x -= 2.0 * scale;
        }
        let replace_all = Rect::new(query.right() - field, query.bottom() + gap, field, field);
        let replace = Rect::new(
            left,
            query.bottom() + gap,
            (replace_all.x - gap - left).max(0.0),
            field,
        );
        let summary = Rect::new(left, replace.bottom() + gap, width, 22.0 * scale);

        let row = ROW * scale;
        let top = summary.bottom() + 4.0 * scale;
        let capacity = (((rect.bottom() - pad) - top) / row).floor().max(0.0) as usize;
        let rows = (self.first..self.rows.len().min(self.first + capacity))
            .enumerate()
            .map(|(slot, index)| {
                (
                    index,
                    Rect::new(rect.x, top + slot as f32 * row, rect.width, row),
                )
            })
            .collect();

        Geometry {
            heading,
            query,
            toggles,
            replace,
            replace_all,
            summary,
            rows,
            capacity,
        }
    }

    /// Starts replacing every match in the project with the replacement, on a worker. `skip` is the
    /// canonical paths of files open with unsaved changes. Returns false if there is nothing to do.
    pub fn start_replace(
        &mut self,
        root: &Path,
        skip: Vec<PathBuf>,
        waker: Option<crate::wake::Waker>,
    ) -> bool {
        if self.query.is_empty() || self.replacing.is_some() {
            return false;
        }
        self.confirming = false;
        let (send, answer) = channel();
        let query = self.search_query();
        let replacement = self.replace.clone();
        let root = root.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("dacode-replace".into())
            .spawn(move || {
                let report = replace_in_project(&root, &query, &replacement, &skip);
                let _ = send.send(report);
                if let Some(wake) = waker {
                    wake();
                }
            });
        if spawned.is_err() {
            return false;
        }
        self.replacing = Some(answer);
        true
    }

    pub fn replacing(&self) -> bool {
        self.replacing.is_some()
    }

    /// The finished replace-all's report, once. Never blocks.
    pub fn poll_replace(&mut self) -> Option<Report> {
        let report = self.replacing.as_ref()?.try_recv().ok()?;
        self.replacing = None;
        Some(report)
    }
}

/// Groups hits under a heading for each file, in the order they arrived — which is the walk's, so
/// each file's hits are already together.
fn group(hits: &[SearchHit]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(hits.len() + hits.len() / 4);
    let mut heading: Option<usize> = None;
    for (index, hit) in hits.iter().enumerate() {
        let same = heading
            .is_some_and(|at| matches!(&rows[at], Row::File { path, .. } if *path == hit.path));
        if !same {
            heading = Some(rows.len());
            rows.push(Row::File {
                path: hit.path.clone(),
                count: 0,
            });
        }
        if let Some(Row::File { count, .. }) = heading.map(|at| &mut rows[at]) {
            *count += 1;
        }
        rows.push(Row::Hit(index));
    }
    rows
}

/// The worker's half of replace-all: every file the search would look in, rewritten where it
/// matches.
fn replace_in_project(
    root: &Path,
    query: &SearchQuery,
    replacement: &str,
    skip: &[PathBuf],
) -> Report {
    let mut report = Report::default();
    for path in dacode_project::project_files(root) {
        if !skip.is_empty() && skip.contains(&dacode_project::canonical(&path)) {
            // Counted only if it would have changed: a dirty buffer with no match is not news.
            if std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| dacode_project::replace_all(&text, query, replacement))
                .is_some()
            {
                report.skipped += 1;
            }
            continue;
        }
        let too_big = std::fs::metadata(&path)
            .map(|meta| meta.len() > crate::open::MAX_BYTES)
            .unwrap_or(true);
        if too_big {
            continue;
        }
        // Not text, or not UTF-8: the search skipped it, so the replacement does too.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.contains('\0') {
            continue;
        }
        let Some((replaced, count)) = dacode_project::replace_all(&text, query, replacement) else {
            continue;
        };
        match crate::save::write(&path, replaced.as_bytes()) {
            Ok(()) => {
                report.files += 1;
                report.replacements += count;
            }
            Err(error) => report.failed.push(format!(
                "{}: {error}",
                path.strip_prefix(root).unwrap_or(&path).display()
            )),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dacode-search-panel-{}-{label}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dacode_project::canonical(&dir)
    }

    fn settle(panel: &mut SearchPanel) {
        let start = std::time::Instant::now();
        while panel.searching() && start.elapsed() < std::time::Duration::from_secs(10) {
            panel.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panel.poll();
    }

    #[test]
    fn hits_are_grouped_under_their_file() {
        // A flat list of `path:line` across twelve files is unreadable; a heading per file with its
        // count is what makes the panel scannable.
        let dir = scratch("group");
        std::fs::write(dir.join("a.txt"), "needle\nhay\nneedle\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle\n").unwrap();
        let mut panel = SearchPanel::new("needle");
        panel.restart(&dir);
        settle(&mut panel);

        assert_eq!(panel.hits().len(), 3);
        let mut headings: Vec<(String, usize)> = panel
            .rows()
            .iter()
            .filter_map(|row| match row {
                Row::File { path, count } => Some((
                    path.file_name().unwrap().to_string_lossy().to_string(),
                    *count,
                )),
                Row::Hit(_) => None,
            })
            .collect();
        headings.sort();
        assert_eq!(
            headings,
            [("a.txt".to_string(), 2), ("b.txt".to_string(), 1)]
        );
        assert_eq!(panel.summary(), "3 results in 2 files");
    }

    #[test]
    fn the_toggles_change_what_matches() {
        let dir = scratch("toggles");
        std::fs::write(dir.join("a.txt"), "Cat cat concat\n").unwrap();
        let mut panel = SearchPanel::new("cat");
        panel.restart(&dir);
        settle(&mut panel);
        assert_eq!(
            panel.hits()[0].range,
            0..3,
            "case is ignored until asked for"
        );

        panel.flip(Toggle::Case, &dir);
        panel.flip(Toggle::Word, &dir);
        settle(&mut panel);
        assert_eq!(panel.hits()[0].range, 4..7, "the lowercase whole word");

        panel.query = "C.t".into();
        panel.flip(Toggle::Regex, &dir);
        settle(&mut panel);
        assert_eq!(panel.hits()[0].range, 0..3);
    }

    #[test]
    fn replace_all_rewrites_every_matching_file_and_leaves_skipped_ones() {
        // The skipped one stands for a buffer with unsaved changes: rewriting it underneath would
        // be undone by that buffer's next save.
        let dir = scratch("replace");
        std::fs::write(dir.join("a.txt"), "old old\n").unwrap();
        std::fs::write(dir.join("b.txt"), "old\n").unwrap();
        std::fs::write(dir.join("c.txt"), "nothing here\n").unwrap();
        let mut panel = SearchPanel::new("old");
        panel.replace = "new".into();
        assert!(panel.start_replace(&dir, vec![dir.join("b.txt")], None));

        let start = std::time::Instant::now();
        let report = loop {
            if let Some(report) = panel.poll_replace() {
                break report;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "new new\n"
        );
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "old\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("c.txt")).unwrap(),
            "nothing here\n"
        );
        assert_eq!(
            report,
            Report {
                files: 1,
                replacements: 2,
                skipped: 1,
                failed: Vec::new(),
            }
        );
        assert!(!panel.replacing());
    }

    #[test]
    fn the_selection_stays_inside_the_visible_rows() {
        let dir = scratch("window");
        let text: String = (0..40).map(|_| "x\n").collect();
        std::fs::write(dir.join("a.txt"), text).unwrap();
        let mut panel = SearchPanel::new("x");
        panel.restart(&dir);
        settle(&mut panel);

        let rect = Rect::new(0.0, 0.0, 300.0, 400.0);
        let capacity = panel.geometry(rect, 1.0).capacity;
        assert!(capacity > 0 && capacity < 40);
        panel.move_selection(30, capacity);
        let geometry = panel.geometry(rect, 1.0);
        assert!(
            geometry
                .rows
                .iter()
                .any(|(row, _)| panel.rows()[*row] == Row::Hit(panel.selected)),
            "the selected hit is drawn"
        );
    }
}
