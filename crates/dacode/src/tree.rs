//! The file tree.
//!
//! Derived from the project's flat file index rather than walking the disk again. That index is
//! already gitignore-aware and already built at startup, so the tree costs one pass over a list the
//! editor holds anyway — and it can never disagree with the file palette about what is in the
//! project, which two independent walks eventually would.
//!
//! Directories are inferred from the paths. A directory containing nothing but ignored files
//! therefore does not appear, which is the right answer: `target/` is not part of the project just
//! because it exists.

use dacode_project::ProjectFile;
use dacode_render::Rect;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The panel's geometry: which rows are on screen and where each one is.
///
/// The rule this module now obeys with the rest of them — every mouse-reachable thing is geometry
/// data plus a click handler that reads it. Drawing and hit-testing each had their own copy of the
/// walk, and the copies disagreed: drawing left a pad's worth of space at the bottom and the mouse
/// did not, so the two asked [`FileTree::visible`] for different heights. Whenever the tree was
/// longer than the panel, that is a different window of rows — and clicking one file opened
/// another.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    /// Where the "Files" heading goes.
    pub header: Rect,
    /// Each visible row and the full-width rectangle it occupies.
    pub rows: Vec<(Row, Rect)>,
    /// Which of `rows` is selected, if the selection is on screen.
    pub cursor: Option<usize>,
}

impl Panel {
    /// Which visible row a pixel is in.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        self.rows
            .iter()
            .position(|(_, rect)| y >= rect.y && y < rect.y + rect.height)
    }
}

/// Lays the tree out in `rect`. Read by `chrome::draw_tree` and by `App::click_sidebar`.
pub fn panel(rect: Rect, scale: f32, tree: &FileTree) -> Panel {
    let pad = dacode_theme::space::LG * scale;
    let line = 22.0 * scale;
    let top = rect.y + pad;
    let header = Rect::new(rect.x + pad, top, rect.width - pad * 2.0, line);
    let first = top + line * 1.6;

    // The pad at the bottom is what the two copies disagreed about. It stays: a row flush against
    // the edge of the panel reads as a row that has been cut off.
    let height = (((rect.bottom() - pad) - first) / line).floor().max(1.0) as usize;
    let (visible, cursor) = tree.visible(height);

    Panel {
        header,
        rows: visible
            .iter()
            .enumerate()
            .map(|(index, row)| {
                (
                    row.clone(),
                    Rect::new(rect.x, first + index as f32 * line, rect.width, line),
                )
            })
            .collect(),
        cursor: (cursor < visible.len()).then_some(cursor),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// How deep, for indentation. The root's children are at zero.
    pub depth: usize,
    /// The last path segment — what is drawn.
    pub name: String,
    /// The path relative to the project root, which is also the identity used for expansion.
    pub relative: String,
    pub is_directory: bool,
    pub expanded: bool,
}

pub struct FileTree {
    /// Every path in the project, directories included, in display order.
    all: Vec<Row>,
    /// Which directories are open. Everything starts closed except the root's own children.
    expanded: BTreeSet<String>,
    /// Rows currently visible, recomputed when expansion changes.
    rows: Vec<Row>,
    pub selected: usize,
    root: PathBuf,
}

impl FileTree {
    pub fn new(root: PathBuf, files: &[ProjectFile]) -> Self {
        let mut tree = Self {
            all: build(files),
            expanded: BTreeSet::new(),
            rows: Vec::new(),
            selected: 0,
            root,
        };
        tree.rebuild();
        tree
    }

    /// Replaces the file list, keeping which directories were open.
    ///
    /// Called when the background walk finishes, which happens *after* the tree can first be opened
    /// on a large project — losing the user's expansion state at that moment would be maddening.
    pub fn refresh(&mut self, files: &[ProjectFile]) {
        let selected = self.selected_path();
        self.all = build(files);
        self.rebuild();
        if let Some(path) = selected {
            self.select_path(&path);
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    fn selected_path(&self) -> Option<String> {
        self.selected_row().map(|row| row.relative.clone())
    }

    fn select_path(&mut self, relative: &str) {
        if let Some(index) = self.rows.iter().position(|row| row.relative == relative) {
            self.selected = index;
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = match delta {
            d if d < 0 => self.selected.saturating_sub(d.unsigned_abs()),
            d => (self.selected + d as usize).min(last),
        };
    }

    /// Opens or closes the selected directory. Returns the file to open, if a file was selected.
    pub fn activate(&mut self) -> Option<PathBuf> {
        let row = self.rows.get(self.selected)?.clone();
        if !row.is_directory {
            return Some(self.root.join(&row.relative));
        }

        if self.expanded.contains(&row.relative) {
            self.expanded.remove(&row.relative);
        } else {
            self.expanded.insert(row.relative.clone());
        }
        self.rebuild();
        self.select_path(&row.relative);
        None
    }

    /// Opens every directory on the way to a file, and selects it.
    ///
    /// What makes the tree usable alongside the file palette: open something with `ctrl-p`, and the
    /// tree is already showing where it lives rather than sitting collapsed at the root.
    pub fn reveal(&mut self, relative: &str) {
        let mut prefix = String::new();
        for segment in relative.split('/').filter(|s| !s.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            if prefix != relative {
                self.expanded.insert(prefix.clone());
            }
        }
        self.rebuild();
        self.select_path(relative);
    }

    /// The visible window of rows, and the selection's index within it.
    pub fn visible(&self, height: usize) -> (&[Row], usize) {
        if self.rows.len() <= height || height == 0 {
            return (&self.rows, self.selected);
        }
        // Keep the selection in view with a little context, the same rule the results panel uses.
        let start = self
            .selected
            .saturating_sub(height / 2)
            .min(self.rows.len() - height);
        (&self.rows[start..start + height], self.selected - start)
    }

    /// Recomputes the visible rows from the expansion set.
    fn rebuild(&mut self) {
        let expanded = &self.expanded;
        self.rows = self
            .all
            .iter()
            // A row is visible when every directory above it is open. Top-level rows have no
            // ancestors, so they always are.
            .filter(|row| {
                let mut prefix = String::new();
                let mut segments = row.relative.split('/').peekable();
                while let Some(segment) = segments.next() {
                    if segments.peek().is_none() {
                        break;
                    }
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(segment);
                    if !expanded.contains(&prefix) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .map(|mut row| {
                row.expanded = self.expanded.contains(&row.relative);
                row
            })
            .collect();

        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }
}

/// Every file and the directories implied by their paths, sorted directories-first per level.
fn build(files: &[ProjectFile]) -> Vec<Row> {
    let mut paths: BTreeSet<(String, bool)> = BTreeSet::new();

    for file in files {
        let mut prefix = String::new();
        let segments: Vec<&str> = file.relative.split('/').collect();
        for (index, segment) in segments.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            paths.insert((prefix.clone(), index + 1 < segments.len()));
        }
    }

    let mut rows: Vec<Row> = paths
        .into_iter()
        .map(|(relative, is_directory)| Row {
            depth: relative.matches('/').count(),
            name: relative.rsplit('/').next().unwrap_or(&relative).to_string(),
            relative,
            is_directory,
            expanded: false,
        })
        .collect();

    // Directories before files within the same parent, which is how every file tree is read. The
    // sort is stable on the path, so ordering within a group stays alphabetical.
    rows.sort_by(|a, b| {
        let a_parent = a.relative.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let b_parent = b.relative.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        a_parent
            .cmp(b_parent)
            .then(b.is_directory.cmp(&a.is_directory))
            .then(a.relative.cmp(&b.relative))
    });

    // Re-thread the rows so children follow their parent rather than being grouped by parent name.
    let mut ordered = Vec::with_capacity(rows.len());
    thread(&rows, "", &mut ordered);
    ordered
}

/// Depth-first ordering: each directory immediately followed by its contents.
fn thread(rows: &[Row], parent: &str, out: &mut Vec<Row>) {
    for row in rows
        .iter()
        .filter(|row| row.relative.rsplit_once('/').map(|(p, _)| p).unwrap_or("") == parent)
    {
        out.push(row.clone());
        if row.is_directory {
            thread(rows, &row.relative, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<ProjectFile> {
        paths
            .iter()
            .map(|path| ProjectFile {
                relative: path.to_string(),
                absolute: PathBuf::from("/project").join(path),
            })
            .collect()
    }

    fn tree(paths: &[&str]) -> FileTree {
        FileTree::new(PathBuf::from("/project"), &files(paths))
    }

    fn names(tree: &FileTree) -> Vec<String> {
        tree.rows().iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn only_the_top_level_shows_until_a_directory_is_opened() {
        let tree = tree(&["src/main.rs", "src/lib.rs", "README.md"]);

        assert_eq!(
            names(&tree),
            vec!["src", "README.md"],
            "directories first, and nothing inside them yet"
        );
    }

    #[test]
    fn opening_a_directory_shows_its_contents_beneath_it() {
        let mut tree = tree(&["src/main.rs", "src/lib.rs", "README.md"]);
        assert_eq!(
            tree.activate(),
            None,
            "a directory opens rather than opening a file"
        );

        assert_eq!(names(&tree), vec!["src", "lib.rs", "main.rs", "README.md"]);
        assert_eq!(
            tree.rows()[1].depth,
            1,
            "contents are indented one level deeper"
        );
    }

    #[test]
    fn activating_a_file_returns_its_path() {
        let mut tree = tree(&["README.md"]);
        assert_eq!(
            tree.activate(),
            Some(PathBuf::from("/project/README.md")),
            "an absolute path, so the caller does not have to know the root"
        );
    }

    #[test]
    fn a_directory_closes_again_and_the_selection_stays_on_it() {
        let mut tree = tree(&["src/main.rs", "README.md"]);
        tree.activate();
        assert_eq!(tree.rows().len(), 3);

        tree.activate();
        assert_eq!(tree.rows().len(), 2, "closed again");
        assert_eq!(
            tree.selected_row().map(|row| row.name.as_str()),
            Some("src"),
            "the selection stays on the directory that was toggled"
        );
    }

    #[test]
    fn nested_directories_stay_hidden_until_each_is_opened() {
        let mut tree = tree(&["a/b/c/deep.rs", "top.rs"]);

        assert_eq!(names(&tree), vec!["a", "top.rs"]);
        tree.activate();
        assert_eq!(
            names(&tree),
            vec!["a", "b", "top.rs"],
            "one level at a time"
        );
    }

    #[test]
    fn reveal_opens_every_directory_on_the_way_to_a_file() {
        // The tree has to be usable *with* the file palette: opening something with ctrl-p and
        // finding the tree still collapsed at the root would make it useless.
        let mut tree = tree(&["a/b/c/deep.rs", "top.rs"]);
        tree.reveal("a/b/c/deep.rs");

        assert_eq!(names(&tree), vec!["a", "b", "c", "deep.rs", "top.rs"]);
        assert_eq!(
            tree.selected_row().map(|row| row.relative.as_str()),
            Some("a/b/c/deep.rs"),
            "and the file itself is selected"
        );
    }

    #[test]
    fn the_selection_cannot_leave_the_list() {
        let mut tree = tree(&["one.rs", "two.rs"]);

        tree.move_selection(-5);
        assert_eq!(tree.selected, 0);
        tree.move_selection(50);
        assert_eq!(tree.selected, 1, "clamped to the last row");
    }

    #[test]
    fn refreshing_keeps_the_directories_that_were_open() {
        // The background walk finishes after the tree can first be opened, so a refresh that
        // collapsed everything would undo whatever the user had just done.
        let mut tree = tree(&["src/main.rs", "README.md"]);
        tree.activate();
        assert_eq!(tree.rows().len(), 3);

        tree.refresh(&files(&["src/main.rs", "src/extra.rs", "README.md"]));

        assert_eq!(
            names(&tree),
            vec!["src", "extra.rs", "main.rs", "README.md"],
            "still open, and now showing the new file"
        );
    }

    #[test]
    fn an_empty_project_is_an_empty_tree_rather_than_a_panic() {
        let mut tree = tree(&[]);
        assert!(tree.is_empty());
        assert_eq!(tree.activate(), None);
        assert_eq!(tree.selected_row(), None);
        tree.move_selection(1);
    }

    #[test]
    fn the_window_follows_the_selection() {
        let paths: Vec<String> = (0..40).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut tree = tree(&refs);

        tree.move_selection(30);
        let (rows, cursor) = tree.visible(10);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[cursor].name, tree.selected_row().unwrap().name);
    }

    #[test]
    fn the_panel_a_click_reads_is_the_panel_that_was_drawn() {
        // The rule the rest of the module list obeys: one walk, two readers. Drawing and
        // hit-testing each had their own, and the two had already drifted a pad apart — drawing
        // left space at the bottom of the panel and the mouse did not, so they asked `visible` for
        // different heights. Once the tree is longer than the panel that is a different *window* of
        // rows, and clicking one file opens another.
        let paths: Vec<String> = (0..40).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut tree = tree(&refs);
        tree.move_selection(30);

        let rect = Rect::new(0.0, 0.0, 240.0, 400.0);
        let panel = panel(rect, 1.0, &tree);

        assert!(!panel.rows.is_empty());
        for (index, (_, box_)) in panel.rows.iter().enumerate() {
            // The top of a row, its middle, and the pixel before the next one all resolve to it.
            assert_eq!(panel.row_at(box_.y), Some(index));
            assert_eq!(panel.row_at(box_.y + box_.height / 2.0), Some(index));
            assert_eq!(panel.row_at(box_.y + box_.height - 0.01), Some(index));
        }

        // Nothing above the first row or below the last claims one.
        assert_eq!(panel.row_at(panel.header.y), None);
        let bottom = panel.rows.last().expect("rows").1;
        assert_eq!(panel.row_at(bottom.y + bottom.height + 1.0), None);
    }

    #[test]
    fn the_rows_stay_inside_the_panel_they_are_drawn_in() {
        // The bottom pad is what the two copies disagreed about, so it is what this pins down: a
        // row flush against the edge reads as one that has been cut off.
        let paths: Vec<String> = (0..80).map(|i| format!("file{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let tree = tree(&refs);

        let rect = Rect::new(0.0, 0.0, 240.0, 300.0);
        let panel = panel(rect, 1.0, &tree);
        let last = panel.rows.last().expect("rows").1;
        assert!(
            last.y + last.height <= rect.bottom(),
            "a row was laid out past the bottom of the panel"
        );
    }
}
