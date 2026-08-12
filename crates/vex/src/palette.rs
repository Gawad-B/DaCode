//! The command palette: its state and its keys.
//!
//! While the palette is open it **owns the keyboard** — the design is explicit that mode keys are
//! swallowed. That is the right behavior: typing `i` into a file query should search for `i`, not
//! drop you into insert mode.
//!
//! Kept separate from the modal grammar because it is not modal editing: it is a text field with a
//! ranked list, and modelling it as another editor mode would mean the grammar had to know about
//! overlays.

use vex_project::{FileIndex, Match};

/// What the palette is searching.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PaletteKind {
    /// `ctrl-p` — files in the project.
    Files,
    /// Buffers already open, so switching back is not a re-search.
    Buffers,
}

impl PaletteKind {
    pub fn prompt(self) -> &'static str {
        match self {
            PaletteKind::Files => "open file",
            PaletteKind::Buffers => "switch buffer",
        }
    }
}

/// One row in the palette.
#[derive(Debug, Clone)]
pub struct Row {
    /// The primary text: a file name, or a buffer name.
    pub name: String,
    /// Dimmed context after the name — the directory.
    pub context: String,
    /// Match score, right-aligned in the design.
    pub score: u32,
    /// Character positions in `name` that matched, highlighted in the mode color.
    pub highlights: Vec<u32>,
    /// What choosing this row opens.
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(std::path::PathBuf),
    Buffer(usize),
}

/// How many rows the palette shows. The design's card is a fixed height, and a palette that scrolls
/// for pages stops being a way to *choose* something.
pub const VISIBLE_ROWS: usize = 9;

#[derive(Debug)]
pub struct Palette {
    pub kind: PaletteKind,
    pub query: String,
    pub rows: Vec<Row>,
    /// Index into `rows`, always valid when `rows` is non-empty.
    pub selected: usize,
    /// Total candidates considered, for the design's `4 OF 1284` counter.
    pub total: usize,
}

impl Palette {
    pub fn new(kind: PaletteKind) -> Self {
        Self {
            kind,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            total: 0,
        }
    }

    pub fn selection(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// Re-ranks against the file index.
    pub fn refresh_files(&mut self, index: &mut FileIndex) {
        let matches = index.search(&self.query, VISIBLE_ROWS * 4);
        self.total = index.len();
        self.rows = matches.into_iter().map(row_from_match).collect();
        // The selection is clamped rather than reset: typing a character that narrows the list
        // should not throw you back to the top if your choice is still there.
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// Re-ranks the open buffers. Substring matching is enough here: there are tens of buffers, not
    /// thousands of files, and the ranking machinery would add nothing.
    pub fn refresh_buffers(&mut self, names: &[(String, String)]) {
        let query = self.query.to_lowercase();
        self.total = names.len();
        self.rows = names
            .iter()
            .enumerate()
            .filter(|(_, (name, path))| {
                query.is_empty()
                    || name.to_lowercase().contains(&query)
                    || path.to_lowercase().contains(&query)
            })
            .map(|(index, (name, path))| Row {
                name: name.clone(),
                context: path.clone(),
                score: 0,
                highlights: highlight_substring(name, &query),
                target: Target::Buffer(index),
            })
            .take(VISIBLE_ROWS * 4)
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
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

    /// The window of rows to draw, and the selection's index within it.
    ///
    /// Scrolls only when the selection would leave the window, so the list stays still while you
    /// move within it.
    pub fn visible(&self) -> (&[Row], usize) {
        if self.rows.len() <= VISIBLE_ROWS {
            return (&self.rows, self.selected);
        }
        let start = self
            .selected
            .saturating_sub(VISIBLE_ROWS - 1)
            .min(self.rows.len() - VISIBLE_ROWS);
        (
            &self.rows[start..start + VISIBLE_ROWS],
            self.selected - start,
        )
    }
}

fn row_from_match(matched: Match) -> Row {
    // Match positions are against the whole relative path; the design highlights within the name,
    // so they are rebased and out-of-range ones dropped.
    let name_start = matched.file.relative.len() - matched.file.name().len();
    let highlights = matched
        .positions
        .iter()
        .filter(|position| **position as usize >= name_start)
        .map(|position| *position - name_start as u32)
        .collect();

    Row {
        name: matched.file.name().to_string(),
        context: matched.file.directory().to_string(),
        score: matched.score,
        highlights,
        target: Target::File(matched.file.absolute),
    }
}

/// Positions of a substring match, for the buffer palette.
fn highlight_substring(name: &str, query: &str) -> Vec<u32> {
    if query.is_empty() {
        return Vec::new();
    }
    let lowered = name.to_lowercase();
    match lowered.find(query) {
        Some(byte_index) => {
            let char_start = lowered[..byte_index].chars().count() as u32;
            (0..query.chars().count() as u32)
                .map(|offset| char_start + offset)
                .collect()
        }
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use vex_project::ProjectFile;

    fn index(paths: &[&str]) -> FileIndex {
        let mut index = FileIndex::new("/project");
        index.set_files(
            paths
                .iter()
                .map(|relative| ProjectFile {
                    relative: (*relative).to_string(),
                    absolute: PathBuf::from("/project").join(relative),
                })
                .collect(),
            false,
        );
        index
    }

    #[test]
    fn typing_narrows_the_list() {
        let mut index = index(&["src/main.rs", "src/lib.rs", "docs/readme.md"]);
        let mut palette = Palette::new(PaletteKind::Files);

        palette.refresh_files(&mut index);
        assert_eq!(palette.rows.len(), 3, "an empty query shows everything");
        assert_eq!(palette.total, 3);

        palette.query.push_str("lib");
        palette.refresh_files(&mut index);
        assert_eq!(palette.rows.len(), 1);
        assert_eq!(palette.rows[0].name, "lib.rs");
    }

    #[test]
    fn rows_separate_the_name_from_its_directory() {
        let mut index = index(&["crates/vex-text/src/anchor.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.query.push_str("anchor");
        palette.refresh_files(&mut index);

        assert_eq!(palette.rows[0].name, "anchor.rs");
        assert_eq!(palette.rows[0].context, "crates/vex-text/src");
    }

    #[test]
    fn highlights_are_rebased_onto_the_name() {
        // The matcher scores the whole path; the design highlights inside the name, so a position
        // in the directory part must not paint a random character of the name.
        let mut index = index(&["crates/thing/src/anchor.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.query.push_str("anchor");
        palette.refresh_files(&mut index);

        let row = &palette.rows[0];
        let chars: Vec<char> = row.name.chars().collect();
        for position in &row.highlights {
            assert!(
                (*position as usize) < chars.len(),
                "highlight {position} is outside {:?}",
                row.name
            );
        }
        let highlighted: String = row.highlights.iter().map(|p| chars[*p as usize]).collect();
        assert_eq!(highlighted.to_lowercase(), "anchor");
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut index = index(&["a.rs", "b.rs", "c.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.refresh_files(&mut index);

        palette.move_selection(1);
        assert_eq!(palette.selected, 1);
        palette.move_selection(10);
        assert_eq!(palette.selected, 2, "clamps at the end");
        palette.move_selection(-10);
        assert_eq!(palette.selected, 0, "clamps at the start");
    }

    #[test]
    fn selection_survives_narrowing_when_it_can() {
        let mut index = index(&["alpha.rs", "beta.rs", "gamma.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.refresh_files(&mut index);
        palette.move_selection(2);

        palette.query.push('a');
        palette.refresh_files(&mut index);
        assert!(
            palette.selected < palette.rows.len(),
            "selection must stay valid after the list changes"
        );
    }

    #[test]
    fn an_empty_list_has_no_selection() {
        let mut index = index(&["a.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.query.push_str("zzzqqq");
        palette.refresh_files(&mut index);

        assert!(palette.rows.is_empty());
        assert!(palette.selection().is_none(), "must not index into nothing");
        palette.move_selection(1); // must not panic
    }

    #[test]
    fn the_visible_window_follows_the_selection() {
        let paths: Vec<String> = (0..40).map(|i| format!("f{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut index = index(&refs);

        let mut palette = Palette::new(PaletteKind::Files);
        palette.refresh_files(&mut index);

        let (rows, cursor) = palette.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert_eq!(cursor, 0);

        // Moving beyond the window scrolls it, keeping the selection inside.
        palette.move_selection(VISIBLE_ROWS as isize + 3);
        let (rows, cursor) = palette.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert!(cursor < VISIBLE_ROWS, "selection must stay in the window");
        assert_eq!(
            rows[cursor].name, palette.rows[palette.selected].name,
            "the highlighted row must be the selected one"
        );
    }

    #[test]
    fn the_buffer_palette_matches_names_and_paths() {
        let buffers = vec![
            ("anchor.rs".to_string(), "crates/vex-text/src".to_string()),
            ("main.rs".to_string(), "crates/vex/src".to_string()),
        ];

        let mut palette = Palette::new(PaletteKind::Buffers);
        palette.refresh_buffers(&buffers);
        assert_eq!(palette.rows.len(), 2);

        palette.query.push_str("anch");
        palette.refresh_buffers(&buffers);
        assert_eq!(palette.rows.len(), 1);
        assert_eq!(palette.rows[0].target, Target::Buffer(0));
        assert_eq!(palette.rows[0].highlights, vec![0, 1, 2, 3]);

        // Matching on the directory works too, since that is how you distinguish two `main.rs`.
        palette.query.clear();
        palette.query.push_str("vex-text");
        palette.refresh_buffers(&buffers);
        assert_eq!(palette.rows.len(), 1);
    }
}
