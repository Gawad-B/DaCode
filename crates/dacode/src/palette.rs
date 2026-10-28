//! The command palette: its state and its keys.
//!
//! While the palette is open it **owns the keyboard** — the design is explicit that mode keys are
//! swallowed. That is the right behavior: typing `i` into a file query should search for `i`, not
//! drop you into insert mode.
//!
//! Kept separate from the modal grammar because it is not modal editing: it is a text field with a
//! ranked list, and modelling it as another editor mode would mean the grammar had to know about
//! overlays.

use dacode_project::{FileIndex, Match};
use dacode_render::Rect;

/// What the palette is searching.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PaletteKind {
    /// `ctrl-p` — files in the project.
    Files,
    /// Buffers already open, so switching back is not a re-search.
    Buffers,
    /// What the language server offers to do about the code under the cursor.
    ///
    /// The same card rather than a menu of its own: it is a list you filter and choose from, which
    /// is exactly what the palette already is, and a second overlay with its own keys would be one
    /// more thing to learn for no gain.
    CodeActions,
    /// `ctrl-shift-p` — every command the editor has, with the key that runs it beside it.
    ///
    /// The reason this exists: there were seventy-odd commands and the only way to reach one was to
    /// type its name blind into the command line, which requires already knowing the name. A list
    /// you can search is how anybody finds out what an editor can do, and showing each command's key
    /// beside it is how the palette stops being needed — you use it twice and learn the shortcut.
    Commands,
    /// `ctrl-shift-o` — every name in this file, as an outline you can search.
    ///
    /// Fetched once and filtered here, like the code actions: it is one file, the server answered
    /// it in full, and asking again per keystroke would be paying a round trip for a `contains`.
    Symbols,
    /// `ctrl-shift-t` — every name in the project.
    ///
    /// The one palette whose list the *server* filters: a project's symbol table is not something
    /// the editor holds, and asking for all of it up front is the request no server wants. So each
    /// keystroke sends the query and the reply replaces the list — with the query it was for, so
    /// an answer that arrives after you have typed on is discarded rather than flickering back.
    ProjectSymbols,
}

impl PaletteKind {
    pub fn prompt(self) -> &'static str {
        match self {
            PaletteKind::CodeActions => "code action",
            PaletteKind::Files => "open file",
            PaletteKind::Buffers => "switch buffer",
            PaletteKind::Commands => "run a command",
            PaletteKind::Symbols => "go to symbol",
            PaletteKind::ProjectSymbols => "go to symbol in project",
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
    /// The group this row is listed under, when the list is shown in groups. A heading is drawn
    /// above the first row of each, and only where it says something the rows do not — see
    /// [`Palette::lines`].
    pub heading: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    File(std::path::PathBuf),
    Buffer(usize),
    /// The index of a code action in the list the server sent.
    CodeAction(usize),
    /// A command, by the name config binds it under.
    Command(&'static str),
    /// The index of a symbol in the list the server sent.
    Symbol(usize),
}

/// How many rows the palette shows. The design's card is a fixed height, and a palette that scrolls
/// for pages stops being a way to *choose* something.
pub const VISIBLE_ROWS: usize = 9;

/// How many lines the card holds at most, headings included. A few more than [`VISIBLE_ROWS`] so a
/// grouped list still shows about as many choices as an ungrouped one.
pub const LINES: usize = 12;

/// A line of the palette's list: a group's heading, or a row by its index in [`Palette::rows`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Line {
    Heading(&'static str),
    Item(usize),
}

/// Where the palette is, as data: drawing and the mouse both read this, so a click lands on the row
/// that is drawn under it. Only the lines on screen have rectangles.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// The card. The shadow drawn around it is outside this and takes no clicks.
    pub card: Rect,
    /// The query field across the top.
    pub input: Rect,
    /// One rectangle per visible row, top to bottom.
    pub rows: Vec<Rect>,
    /// Which of [`Palette::rows`] each of `rows` is.
    pub items: Vec<usize>,
    /// The visible group headings, and where each goes.
    pub headings: Vec<(Rect, &'static str)>,
    /// The list's area under the input: where "no matches" goes when there are no rows.
    pub list: Rect,
}

/// The rectangle the palette hangs from: the title bar's command centre, when it has room for one.
///
/// One function, read by drawing and by the mouse, so the two cannot hang the card from different
/// places.
pub fn anchor(bounds: Rect, scale: f32, typography: &dacode_theme::Typography) -> Option<Rect> {
    crate::layout::title_bar(crate::layout::title_strip(bounds, scale), scale, typography).search
}

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
    /// Fills the palette with the server's offers.
    pub fn refresh_code_actions(&mut self, actions: &[(String, String)]) {
        let query = self.query.to_lowercase();
        self.total = actions.len();
        self.rows = actions
            .iter()
            .enumerate()
            .filter(|(_, (title, _))| query.is_empty() || title.to_lowercase().contains(&query))
            .map(|(index, (title, kind))| Row {
                name: title.clone(),
                context: kind.clone(),
                score: 0,
                highlights: highlight_substring(title, &query),
                target: Target::CodeAction(index),
                heading: None,
            })
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// Fills the palette with symbols.
    ///
    /// `filter` is false for the project-wide list, whose ranking is the server's and whose rows
    /// are already the answer to the query — filtering them again here would drop the fuzzy matches
    /// that are the whole reason a server does the searching.
    pub fn refresh_symbols(&mut self, symbols: &[dacode_lsp::Symbol], filter: bool) {
        let query = self.query.to_lowercase();
        self.total = symbols.len();
        self.rows = symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| {
                !filter || query.is_empty() || symbol.name.to_lowercase().contains(&query)
            })
            .map(|(index, symbol)| Row {
                // Indented into the shape of the file, which is what turns a list of names into an
                // outline. Two spaces, capped: past three levels the indent costs more room than
                // the structure it shows is worth.
                name: format!("{}{}", "  ".repeat(symbol.depth.min(3)), symbol.name),
                context: match (symbol.kind, symbol.container.as_str()) {
                    ("", container) => container.to_string(),
                    (kind, "") => kind.to_string(),
                    (kind, container) => format!("{kind}  {container}"),
                },
                score: 0,
                highlights: highlight_substring(&symbol.name, &query)
                    .into_iter()
                    // The name is drawn after its indent, so a highlight computed against the bare
                    // name would land that many characters to the left of the letter it means.
                    .map(|at| at + (symbol.depth.min(3) * 2) as u32)
                    .collect(),
                target: Target::Symbol(index),
                heading: None,
            })
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

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
                heading: None,
            })
            .take(VISIBLE_ROWS * 4)
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// Fills the palette with every command, ranked against the query.
    ///
    /// Matched on the description as well as the name, because "comment" is what someone types and
    /// `toggle_comment` is what the command is called — a palette that only matched names would
    /// require knowing the vocabulary it exists to teach. The key is the row's context, which is the
    /// dimmed column the design already draws for a file's directory.
    pub fn refresh_commands(&mut self, modal: bool) {
        let query = self.query.trim().to_lowercase();
        // Only what this grammar has. The palette listed the whole table regardless, so someone who
        // had turned Vim off could still find "start selecting", "repeat the last change" and
        // "replay a recorded macro" in the one surface whose entire job is to say what the editor
        // they chose can do.
        let commands: Vec<_> = dacode_input::commands::available(modal).collect();
        self.total = commands.len();

        let mut scored: Vec<(u32, Row)> = commands
            .iter()
            .filter_map(|command| {
                let score = score_command(command, &query)?;
                Some((
                    score,
                    Row {
                        name: command.what.to_string(),
                        context: command.shown(modal),
                        score,
                        highlights: highlight_substring(command.what, &query),
                        target: Target::Command(command.name),
                        // Grouped only while nothing is typed: the list is then the whole table,
                        // browsed, and "files", "editing", "the language server" say where you are
                        // in it. Once a query ranks it, neighbours come from different groups and
                        // a heading over each would be noise.
                        heading: query.is_empty().then(|| command.group.title()),
                    },
                ))
            })
            .collect();

        // Stable within a score, so an empty query shows the table in its own order — gathered by
        // group, so each group's heading is drawn once rather than every time the table's order
        // wanders back into it.
        if query.is_empty() {
            scored.sort_by_key(|(_, row)| match row.target {
                Target::Command(name) => dacode_input::commands::by_name(name).map(|c| c.group),
                _ => None,
            });
        } else {
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        self.rows = scored.into_iter().map(|(_, row)| row).collect();
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
}

impl Palette {
    /// The list as it is drawn: each row, with its group's heading above the first row of a group.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::with_capacity(self.rows.len() + 8);
        let mut last = None;
        for (index, row) in self.rows.iter().enumerate() {
            if let Some(heading) = row.heading.filter(|heading| last != Some(*heading)) {
                lines.push(Line::Heading(heading));
                last = Some(heading);
            }
            lines.push(Line::Item(index));
        }
        lines
    }

    /// The lines on screen, given room for `capacity` of them: a window that holds the selection.
    /// It keeps no scroll position of its own, so the list moves only when the selection would
    /// leave it.
    ///
    /// **Bounded by the window, not the list**, because drawing and every pointer move ask for it.
    /// An ungrouped list — files, buffers, a file's outline, a project's symbols — is windowed over
    /// its rows by arithmetic and never walked; an outline of a large file runs to thousands of
    /// rows. Only a grouped list has to be laid out in full to know where its headings fall, and
    /// the only grouped list is the command table, browsed with nothing typed: a fixed size,
    /// whatever document is open.
    pub fn visible_lines(&self, capacity: usize) -> Vec<Line> {
        let capacity = capacity.max(1);
        let grouped = self.rows.first().is_some_and(|row| row.heading.is_some());
        if !grouped {
            let count = capacity.min(self.rows.len());
            let start = self
                .selected
                .saturating_sub(count.saturating_sub(1))
                .min(self.rows.len() - count);
            return (start..start + count).map(Line::Item).collect();
        }
        let lines = self.lines();
        if lines.len() <= capacity {
            return lines;
        }
        let at = lines
            .iter()
            .position(|line| *line == Line::Item(self.selected))
            .unwrap_or(0);
        let start = at.saturating_sub(capacity - 1).min(lines.len() - capacity);
        lines[start..start + capacity].to_vec()
    }

    /// Where the card, its input and its visible rows are. See [`Geometry`].
    ///
    /// Hangs `6 * scale` below `anchor` — the command centre — and centred on it, as VSCode's and
    /// Zed's palettes drop from theirs; with no command centre on the bar it is centred on the
    /// window, just under the title bar. Kept inside `bounds` either way, and it gives up lines
    /// rather than running off the bottom of a short window.
    pub fn geometry(&self, bounds: Rect, anchor: Option<Rect>, scale: f32) -> Geometry {
        let px = |v: f32| v * scale;
        let (margin, pad, input_height, gap) = (px(16.0), px(6.0), px(36.0), px(6.0));
        let (row_height, heading_height) = (px(28.0), px(24.0));
        let height_of = |line: &Line| match line {
            Line::Heading(_) => heading_height,
            Line::Item(_) => row_height,
        };

        let width = px(600.0).min(bounds.width - margin * 2.0).max(0.0);
        let (centre, top) = match anchor {
            Some(anchor) => (anchor.x + anchor.width / 2.0, anchor.bottom() + px(6.0)),
            None => (
                bounds.x + bounds.width / 2.0,
                crate::layout::title_strip(bounds, scale).bottom() + px(6.0),
            ),
        };
        let x = (centre - width / 2.0)
            .min(bounds.right() - margin - width)
            .max(bounds.x + margin);

        // As many lines as fit above the window's foot, and never fewer than one: a palette with no
        // room for a single row is not one anybody could use, and a window that short has worse
        // problems than the palette's.
        let room = bounds.bottom() - px(8.0) - top - pad * 2.0 - input_height - gap;
        let mut capacity = LINES;
        let lines = loop {
            let lines = self.visible_lines(capacity);
            let height: f32 = lines.iter().map(height_of).sum();
            if height <= room || capacity == 1 {
                break lines;
            }
            capacity -= 1;
        };
        let list_height = match lines.iter().map(height_of).sum::<f32>() {
            // An empty list still has a row: "no matches" needs somewhere to go.
            0.0 => row_height,
            height => height,
        };

        let card = Rect::new(x, top, width, pad + input_height + gap + list_height + pad);
        let input = Rect::new(
            card.x + pad,
            card.y + pad,
            card.width - pad * 2.0,
            input_height,
        );
        let list = Rect::new(input.x, input.bottom() + gap, input.width, list_height);

        let (mut rows, mut items, mut headings) = (Vec::new(), Vec::new(), Vec::new());
        let mut y = list.y;
        for line in &lines {
            let rect = Rect::new(list.x, y, list.width, height_of(line));
            match *line {
                Line::Heading(title) => headings.push((rect, title)),
                Line::Item(index) => {
                    rows.push(rect);
                    items.push(index);
                }
            }
            y = rect.bottom();
        }

        Geometry {
            card,
            input,
            rows,
            items,
            headings,
            list,
        }
    }

    /// Which row a point is over, by its index in [`Palette::rows`].
    pub fn row_at(geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .rows
            .iter()
            .position(|row| crate::layout::contains(*row, x, y))
            .and_then(|at| geometry.items.get(at).copied())
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
        heading: None,
    }
}

/// How well a command answers a query, or `None` if it does not.
///
/// Ranked so that what someone typed the *start* of comes first: typing "com" should offer "comment
/// or uncomment these lines" above "collapse to one cursor", and both above anything that merely
/// contains the letters somewhere. Every word of the description is a candidate for that
/// prefix match, so "uncomment" finds it too.
fn score_command(command: &dacode_input::commands::Command, query: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }

    let what = command.what.to_lowercase();
    let name = command.name.to_lowercase();

    if what.starts_with(query) || name.starts_with(query) {
        return Some(100);
    }
    if what.split_whitespace().any(|word| word.starts_with(query))
        || name.split('_').any(|word| word.starts_with(query))
    {
        return Some(80);
    }
    if what.contains(query) || name.contains(query) {
        return Some(50);
    }
    // Last resort: the letters in order, anywhere — how a fuzzy finder behaves, and what rescues a
    // query like "tgcm" for "toggle comment".
    subsequence(&what, query)
        .or_else(|| subsequence(&name, query))
        .map(|_| 10)
}

/// Whether `query`'s characters appear in `haystack` in order.
fn subsequence(haystack: &str, query: &str) -> Option<()> {
    let mut chars = haystack.chars();
    for wanted in query.chars() {
        chars.by_ref().find(|c| *c == wanted)?;
    }
    Some(())
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
    use dacode_project::ProjectFile;
    use std::path::PathBuf;

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
        let mut index = index(&["crates/dacode-text/src/anchor.rs"]);
        let mut palette = Palette::new(PaletteKind::Files);
        palette.query.push_str("anchor");
        palette.refresh_files(&mut index);

        assert_eq!(palette.rows[0].name, "anchor.rs");
        assert_eq!(palette.rows[0].context, "crates/dacode-text/src");
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

        let first = palette.visible_lines(VISIBLE_ROWS);
        assert_eq!(first.len(), VISIBLE_ROWS);
        assert_eq!(first[0], Line::Item(0));

        // Moving beyond the window scrolls it, keeping the selection inside.
        palette.move_selection(VISIBLE_ROWS as isize + 3);
        let lines = palette.visible_lines(VISIBLE_ROWS);
        assert_eq!(lines.len(), VISIBLE_ROWS);
        assert!(
            lines.contains(&Line::Item(palette.selected)),
            "selection must stay in the window"
        );
    }

    #[test]
    fn the_buffer_palette_matches_names_and_paths() {
        let buffers = vec![
            (
                "anchor.rs".to_string(),
                "crates/dacode-text/src".to_string(),
            ),
            ("main.rs".to_string(), "crates/dacode/src".to_string()),
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
        palette.query.push_str("dacode-text");
        palette.refresh_buffers(&buffers);
        assert_eq!(palette.rows.len(), 1);
    }

    fn symbol(name: &str, kind: &'static str, container: &str, depth: usize) -> dacode_lsp::Symbol {
        dacode_lsp::Symbol {
            name: name.into(),
            kind,
            container: container.into(),
            path: std::path::PathBuf::from("/p/src/anchor.rs"),
            position: dacode_lsp::lsp_types::Position {
                line: 0,
                character: 0,
            },
            depth,
        }
    }

    #[test]
    fn an_outline_is_indented_into_the_shape_of_the_file() {
        let symbols = vec![
            symbol("Anchor", "struct", "", 0),
            symbol("offset", "field", "Anchor", 1),
            symbol("new", "method", "Anchor", 1),
        ];

        let mut palette = Palette::new(PaletteKind::Symbols);
        palette.refresh_symbols(&symbols, true);
        assert_eq!(palette.rows[0].name, "Anchor");
        assert_eq!(
            palette.rows[1].name, "  offset",
            "nesting is what turns a list of names into an outline"
        );
        assert_eq!(palette.rows[1].context, "field  Anchor");

        palette.query.push_str("ne");
        palette.refresh_symbols(&symbols, true);
        assert_eq!(palette.rows.len(), 1);
        assert_eq!(palette.rows[0].target, Target::Symbol(2));
        assert_eq!(
            palette.rows[0].highlights,
            vec![2, 3],
            "the name is drawn after its indent, so the highlight has to move with it"
        );
    }

    #[test]
    fn the_project_wide_list_is_not_filtered_again_here() {
        // Its rows are already the answer to the query, ranked by the server — and a server matches
        // fuzzily, so filtering by `contains` on top would throw away exactly the results that
        // asking a server for them was supposed to buy.
        let symbols = vec![symbol("resolve_command", "fn", "dacode_lsp", 0)];

        let mut palette = Palette::new(PaletteKind::ProjectSymbols);
        palette.query.push_str("rescmd");
        palette.refresh_symbols(&symbols, false);
        assert_eq!(palette.rows.len(), 1);

        // The same query against a file outline, which DaCode does filter, keeps nothing.
        let mut outline = Palette::new(PaletteKind::Symbols);
        outline.query.push_str("rescmd");
        outline.refresh_symbols(&symbols, true);
        assert!(outline.rows.is_empty());
    }

    /// The sizes every geometry test sweeps: the narrowest window the title bar is laid out for,
    /// up to a large monitor, at a short and a tall height each.
    const SIZES: [(f32, f32); 8] = [
        (640.0, 480.0),
        (800.0, 600.0),
        (1024.0, 768.0),
        (1280.0, 720.0),
        (1600.0, 1000.0),
        (1920.0, 1080.0),
        (2560.0, 1440.0),
        (2560.0, 600.0),
    ];

    fn many_files(count: usize) -> Palette {
        let paths: Vec<String> = (0..count).map(|i| format!("src/f{i:02}.rs")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut palette = Palette::new(PaletteKind::Files);
        palette.refresh_files(&mut index(&refs));
        palette
    }

    fn inside(outer: Rect, inner: Rect) -> bool {
        inner.x >= outer.x - 0.01
            && inner.y >= outer.y - 0.01
            && inner.right() <= outer.right() + 0.01
            && inner.bottom() <= outer.bottom() + 0.01
    }

    #[test]
    fn the_palette_hangs_from_the_command_centre() {
        // Opened from the search box in the title bar, it drops from that box — the place the eye
        // already is — rather than appearing somewhere else on the window.
        let bounds = Rect::new(0.0, 0.0, 1600.0, 1000.0);
        let anchor = Rect::new(560.0, 10.0, 420.0, 28.0);
        let geometry = many_files(5).geometry(bounds, Some(anchor), 1.0);

        assert_eq!(geometry.card.y, anchor.bottom() + 6.0);
        let centre = |rect: Rect| rect.x + rect.width / 2.0;
        assert!((centre(geometry.card) - centre(anchor)).abs() < 0.5);
    }

    #[test]
    fn without_a_command_centre_the_palette_centres_on_the_window() {
        // At a narrow width the bar has no room for the search box, and a card hung from nothing
        // would sit at the left edge.
        let bounds = Rect::new(0.0, 0.0, 640.0, 480.0);
        let geometry = many_files(5).geometry(bounds, None, 1.0);
        let centre = geometry.card.x + geometry.card.width / 2.0;
        assert!((centre - 320.0).abs() < 0.5, "centred at {centre}");
        assert!(
            geometry.card.y >= crate::layout::title_strip(bounds, 1.0).bottom(),
            "under the title bar, not over it"
        );
    }

    #[test]
    fn every_row_stays_inside_the_card_at_every_size() {
        let typography = dacode_theme::Typography::default();
        for scale in [1.0, 2.0] {
            for (width, height) in SIZES {
                let bounds = Rect::new(0.0, 0.0, width * scale, height * scale);
                let anchor = anchor(bounds, scale, &typography);
                for palette in [many_files(40), many_files(2), many_files(0), {
                    let mut commands = Palette::new(PaletteKind::Commands);
                    commands.refresh_commands(false);
                    commands
                }] {
                    let geometry = palette.geometry(bounds, anchor, scale);
                    let at = format!("{width}x{height} at {scale}x");
                    assert!(inside(bounds, geometry.card), "card off the window at {at}");
                    assert!(inside(geometry.card, geometry.input), "input at {at}");
                    assert!(inside(geometry.card, geometry.list), "list at {at}");
                    assert_eq!(geometry.rows.len(), geometry.items.len());
                    if !palette.rows.is_empty() {
                        assert!(!geometry.rows.is_empty(), "no rows at all at {at}");
                    }
                    let mut boxes: Vec<Rect> = geometry.rows.clone();
                    boxes.extend(geometry.headings.iter().map(|(rect, _)| *rect));
                    for rect in &boxes {
                        assert!(inside(geometry.list, *rect), "{rect:?} outside at {at}");
                        assert!(rect.y >= geometry.input.bottom(), "under the input at {at}");
                    }
                    boxes.sort_by(|a, b| a.y.total_cmp(&b.y));
                    for pair in boxes.windows(2) {
                        assert!(pair[0].bottom() <= pair[1].y + 0.01, "overlap at {at}");
                    }
                }
            }
        }
    }

    #[test]
    fn only_the_rows_on_screen_have_rectangles_and_the_selection_is_one_of_them() {
        let mut palette = many_files(40);
        let bounds = Rect::new(0.0, 0.0, 1600.0, 1000.0);
        palette.move_selection(25);
        let geometry = palette.geometry(bounds, None, 1.0);
        assert!(geometry.rows.len() <= LINES);
        assert!(
            geometry.items.contains(&palette.selected),
            "the selected row is on screen"
        );
        // Contiguous: a window, not a scatter.
        for pair in geometry.items.windows(2) {
            assert_eq!(pair[1], pair[0] + 1);
        }
    }

    #[test]
    fn a_short_window_shows_fewer_rows_rather_than_running_off_it() {
        let palette = many_files(40);
        let tall = palette.geometry(Rect::new(0.0, 0.0, 1600.0, 1000.0), None, 1.0);
        let short = palette.geometry(Rect::new(0.0, 0.0, 1600.0, 300.0), None, 1.0);
        assert!(short.rows.len() < tall.rows.len());
        assert!(short.card.bottom() <= 300.0);
    }

    #[test]
    fn the_command_list_is_grouped_until_something_is_typed() {
        // Browsing, the headings say where in the table you are; searching, rows from different
        // groups are neighbours by score, and a heading over every other row would say nothing.
        let mut palette = Palette::new(PaletteKind::Commands);
        palette.refresh_commands(false);
        let headings: Vec<&str> = palette
            .lines()
            .into_iter()
            .filter_map(|line| match line {
                Line::Heading(title) => Some(title),
                Line::Item(_) => None,
            })
            .collect();
        assert!(headings.contains(&"files"), "{headings:?}");
        let unique: std::collections::HashSet<&str> = headings.iter().copied().collect();
        assert_eq!(
            unique.len(),
            headings.len(),
            "each group is headed once: {headings:?}"
        );

        palette.query.push_str("comment");
        palette.refresh_commands(false);
        assert!(
            palette
                .lines()
                .iter()
                .all(|line| matches!(line, Line::Item(_)))
        );
    }

    #[test]
    fn a_file_list_has_no_headings() {
        // One kind of thing, in one ranking: a heading would be a caption saying "files" over a
        // list everyone can see is files.
        assert!(
            many_files(5)
                .lines()
                .iter()
                .all(|line| matches!(line, Line::Item(_)))
        );
    }
}
