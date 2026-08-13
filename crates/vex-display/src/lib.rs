//! `vex-display` — the map from buffer coordinates to what is on screen.
//!
//! Soft wrap is a v1 feature, which means one buffer line can occupy many display rows and
//! [`Point`] and [`DisplayPoint`] are genuinely different spaces. This crate owns that mapping.
//!
//! # Why the viewport is anchored, not indexed
//!
//! The obvious design — "give me display row 40,000" — requires knowing how many rows every
//! preceding line wraps into, which is O(document) per query, or an incrementally-maintained
//! sum tree. A million-line file would either stall or need a much larger machine before the
//! editor does anything useful.
//!
//! So the viewport is instead an **anchor**: a buffer line plus a row offset within that line's
//! wrapped rows. Iterating a screenful walks forward from the anchor, which is O(visible rows)
//! regardless of document size — bounded work, per the 60fps contract. The cost is that there
//! is no cheap absolute display-row number; scrollbars use buffer lines as a proxy, which is
//! what the user's mental model uses anyway.

use std::ops::Range;
use vex_text::{CharOffset, DisplayPoint, Point, Text};

/// How lines that exceed the viewport width are handled.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// One buffer line per display row; long lines scroll horizontally.
    None,
    /// Break at the last word boundary that fits, falling back to mid-word for unbreakable runs.
    #[default]
    Word,
}

/// Where the top of the viewport sits.
///
/// `sub_row` is the wrapped row *within* `line`, so scrolling stays stable while a long line is
/// partially visible — and remains valid when text far above changes, since it never refers to
/// an absolute row.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct ScrollAnchor {
    pub line: usize,
    pub sub_row: usize,
}

impl ScrollAnchor {
    pub fn at_line(line: usize) -> Self {
        Self { line, sub_row: 0 }
    }
}

/// One display row: a slice of one buffer line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    /// How many lines this row hides beneath it, when it is the head of a closed fold.
    pub folded: usize,
    /// Index of this row within its buffer line. 0 for the first.
    pub sub_row: usize,
    /// Char range of the text on this row, excluding the line break.
    pub range: Range<CharOffset>,
    /// Columns of leading indentation to repeat on continuation rows.
    pub indent: usize,
}

impl Row {
    pub fn is_first(&self) -> bool {
        self.sub_row == 0
    }

    pub fn len(&self) -> usize {
        self.range.end.get() - self.range.start.get()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Wrap geometry and viewport arithmetic.
///
/// Widths are in **columns**, not pixels: correct for the monospaced code face, and the seam
/// where measured advances will be substituted once proportional fallback glyphs matter. Kept
/// explicit so that substitution is a change of one function, not of every caller.
/// Which line ranges are folded away.
///
/// Folds are computed from *indentation* rather than from the syntax tree. Indentation is available
/// synchronously, works in every language the editor supports and several it does not, and matches
/// what the code looks like — which is what someone folding a block is actually pointing at. A
/// tree-sitter fold query would be more precise about a few constructs and would arrive one frame
/// late, from a worker, for a document version that may already have moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Folds {
    /// Sorted, non-overlapping, and never empty ranges. The first line of each stays visible: a fold
    /// you cannot see is a fold you cannot open.
    closed: Vec<Range<usize>>,
}

impl Folds {
    pub fn is_empty(&self) -> bool {
        self.closed.is_empty()
    }

    pub fn clear(&mut self) {
        self.closed.clear();
    }

    /// Whether a line is hidden inside a closed fold.
    pub fn is_hidden(&self, line: usize) -> bool {
        self.closed
            .iter()
            .any(|fold| line > fold.start && line <= fold.end)
    }

    /// The fold headed by this line, if it is closed.
    pub fn closed_at(&self, line: usize) -> Option<Range<usize>> {
        self.closed.iter().find(|fold| fold.start == line).cloned()
    }

    /// Closes a range. Ranges that would nest inside it are absorbed.
    pub fn close(&mut self, range: Range<usize>) {
        if range.end <= range.start {
            return;
        }
        self.closed
            .retain(|fold| !(fold.start >= range.start && fold.end <= range.end));
        self.closed.push(range);
        self.closed.sort_by_key(|fold| fold.start);
    }

    pub fn open(&mut self, line: usize) -> bool {
        let before = self.closed.len();
        self.closed.retain(|fold| fold.start != line);
        self.closed.len() != before
    }

    /// The first line at or after `line` that is not hidden.
    pub fn next_visible(&self, line: usize) -> usize {
        let mut line = line;
        // Folds are sorted and disjoint, so one pass over them settles it; the loop repeats only
        // when skipping one fold lands inside the next.
        loop {
            match self
                .closed
                .iter()
                .find(|fold| line > fold.start && line <= fold.end)
            {
                Some(fold) => line = fold.end + 1,
                None => return line,
            }
        }
    }
}

/// The line range a fold at `line` would cover, from indentation.
///
/// The block is the run of following lines indented deeper than `line`, with blank lines carried
/// along rather than ending it — a blank line inside a function body does not end the function.
pub fn fold_range(text: &Text, line: usize, tab_width: usize) -> Option<Range<usize>> {
    let indent_of = |line: usize| -> Option<usize> {
        let range = text.line_range(line);
        let mut columns = 0;
        for c in text
            .rope()
            .slice(range.start.get()..range.end.get())
            .chars()
        {
            match c {
                ' ' => columns += 1,
                '\t' => columns += tab_width - (columns % tab_width),
                // A blank line has no indentation to speak of, so it reports none.
                '\n' | '\r' => return None,
                _ => return Some(columns),
            }
        }
        None
    };

    let head = indent_of(line)?;
    let last = text.last_line();
    let mut end = line;
    let mut candidate = line;

    while candidate < last {
        candidate += 1;
        match indent_of(candidate) {
            // Deeper: part of the block.
            Some(indent) if indent > head => end = candidate,
            // Blank: might be inside the block, so it is only taken if something deeper follows.
            None => continue,
            // Same or shallower: the block is over.
            Some(_) => break,
        }
    }

    if end == line {
        return None;
    }

    // A closing brace sits at the *head's* indentation, so pure indent folding leaves it dangling
    // below the fold — the block hides and `}` stays behind on a line of its own. Vim does exactly
    // that, and it reads as a bug in every brace language. If the line after the block is nothing but
    // closing delimiters at the head's indent, it belongs to the fold.
    if end < last {
        let after = end + 1;
        if indent_of(after) == Some(head) {
            let range = text.line_range(after);
            let content: String = text
                .rope()
                .slice(range.start.get()..range.end.get())
                .chars()
                .collect();
            let trimmed = content.trim();
            if !trimmed.is_empty()
                && trimmed
                    .chars()
                    .all(|c| matches!(c, '}' | ')' | ']' | ',' | ';'))
            {
                end = after;
            }
        }
    }

    Some(line..end)
}

#[derive(Debug, Clone)]
pub struct DisplayMap {
    pub mode: WrapMode,
    /// Lines folded away. Part of the display map because folding *is* a mapping from buffer lines
    /// to display rows, which is what this type is.
    pub folds: Folds,
    /// Usable width in columns.
    pub columns: usize,
    /// Tab stop width, used when measuring a line's columns.
    pub tab_width: usize,
    /// Keep continuation rows aligned under their line's indentation.
    pub preserve_indent: bool,
}

impl Default for DisplayMap {
    fn default() -> Self {
        Self {
            mode: WrapMode::Word,
            folds: Folds::default(),
            columns: 100,
            tab_width: 4,
            preserve_indent: true,
        }
    }
}

impl DisplayMap {
    pub fn new(mode: WrapMode, columns: usize) -> Self {
        Self {
            mode,
            columns: columns.max(1),
            ..Default::default()
        }
    }

    /// Splits one buffer line into display rows. Always yields at least one row, so an empty
    /// line still occupies a screen row and can hold a cursor.
    pub fn wrap_line(&self, text: &Text, line: usize) -> Vec<Row> {
        let range = text.line_range(line);
        let start = range.start;
        let length = range.end.get() - start.get();

        if self.mode == WrapMode::None || length == 0 {
            return vec![Row {
                folded: 0,
                line,
                sub_row: 0,
                range,
                indent: 0,
            }];
        }

        let content = text.slice_to_string(range.clone());
        let chars: Vec<char> = content.chars().collect();

        // Cap the inset so a deeply indented line still has room for text on its
        // continuation rows.
        let indent = if self.preserve_indent {
            chars
                .iter()
                .take_while(|c| **c == ' ' || **c == '\t')
                .count()
                .min(self.columns.saturating_sub(8))
        } else {
            0
        };

        let mut rows = Vec::new();
        let mut row_start = 0usize; // index into `chars`
        let mut sub_row = 0usize;

        while row_start < chars.len() {
            // Continuation rows are inset by the line's indentation, so less text fits.
            let budget = if sub_row == 0 {
                self.columns
            } else {
                self.columns.saturating_sub(indent).max(8)
            };

            let remaining = chars.len() - row_start;
            if self.columns_of(&chars[row_start..]) <= budget {
                rows.push(self.row(line, sub_row, start, row_start, chars.len(), indent));
                break;
            }

            // How many chars fit in the budget, accounting for tab stops.
            let mut fit = 0usize;
            let mut used = 0usize;
            while fit < remaining {
                let width = self.char_columns(chars[row_start + fit], used);
                if used + width > budget {
                    break;
                }
                used += width;
                fit += 1;
            }
            fit = fit.max(1); // never make zero progress

            // Prefer the last word boundary inside the fitted span.
            let break_at = if self.mode == WrapMode::Word {
                chars[row_start..row_start + fit]
                    .iter()
                    .rposition(|c| c.is_whitespace())
                    .map(|i| i + 1)
                    .filter(|i| *i > 0)
                    .unwrap_or(fit) // an unbreakable run (a long URL, a minified line) breaks mid-word
            } else {
                fit
            };

            let end = row_start + break_at;
            rows.push(self.row(line, sub_row, start, row_start, end, indent));
            row_start = end;
            sub_row += 1;
        }

        if rows.is_empty() {
            rows.push(Row {
                folded: 0,
                line,
                sub_row: 0,
                range,
                indent: 0,
            });
        }
        rows
    }

    fn row(
        &self,
        line: usize,
        sub_row: usize,
        line_start: CharOffset,
        from: usize,
        to: usize,
        indent: usize,
    ) -> Row {
        Row {
            folded: 0,
            line,
            sub_row,
            range: CharOffset(line_start.get() + from)..CharOffset(line_start.get() + to),
            indent: if sub_row == 0 { 0 } else { indent },
        }
    }

    /// Display columns a character occupies, given how many columns precede it on the row.
    fn char_columns(&self, c: char, at_column: usize) -> usize {
        if c == '\t' {
            self.tab_width - (at_column % self.tab_width)
        } else {
            1
        }
    }

    fn columns_of(&self, chars: &[char]) -> usize {
        let mut columns = 0;
        for c in chars {
            columns += self.char_columns(*c, columns);
        }
        columns
    }

    pub fn rows_in_line(&self, text: &Text, line: usize) -> usize {
        self.wrap_line(text, line).len()
    }

    /// Walks display rows forward from an anchor. Bounded by `count`, never by document size.
    pub fn rows_from(&self, text: &Text, anchor: ScrollAnchor, count: usize) -> Vec<Row> {
        let mut out = Vec::with_capacity(count);
        let mut line = anchor.line.min(text.last_line());
        let mut skip = anchor.sub_row;

        // An anchor can land inside a fold — the fold was closed while scrolled into it — so the
        // first line is nudged out rather than drawing hidden text.
        line = self.folds.next_visible(line);

        while out.len() < count {
            // A closed fold contributes one row: its first line, marked with what it hides.
            if let Some(fold) = self.folds.closed_at(line) {
                let mut row = self.wrap_line(text, line).into_iter().next();
                if let Some(row) = row.take() {
                    out.push(Row {
                        folded: fold.end - fold.start,
                        ..row
                    });
                }
                if fold.end >= text.last_line() {
                    break;
                }
                line = fold.end + 1;
                skip = 0;
                continue;
            }

            let rows = self.wrap_line(text, line);
            for row in rows.into_iter().skip(skip) {
                if out.len() == count {
                    break;
                }
                out.push(row);
            }
            skip = 0;
            if line >= text.last_line() {
                break;
            }
            line += 1;
        }

        out
    }

    /// Where a buffer position lands on screen, relative to the anchor. `None` if it is outside
    /// the visible rows — which is precisely the test for "should we scroll?".
    pub fn display_point(
        &self,
        text: &Text,
        anchor: ScrollAnchor,
        height: usize,
        offset: CharOffset,
    ) -> Option<DisplayPoint> {
        let rows = self.rows_from(text, anchor, height);
        for (index, row) in rows.iter().enumerate() {
            // `<=` on the end so a caret at end-of-row belongs to that row.
            if offset >= row.range.start && offset <= row.range.end {
                let column = self.columns_of(
                    &text
                        .slice_to_string(row.range.start..offset)
                        .chars()
                        .collect::<Vec<_>>(),
                );
                return Some(DisplayPoint::new(index, column + row.indent));
            }
        }
        None
    }

    /// Scrolls the least amount that brings `offset` into view, keeping `margin` rows of context.
    pub fn scroll_to_reveal(
        &self,
        text: &Text,
        anchor: ScrollAnchor,
        height: usize,
        offset: CharOffset,
        margin: usize,
    ) -> ScrollAnchor {
        if height == 0 {
            return anchor;
        }

        let target_line = text.char_to_line(offset);
        let target_sub_row = self
            .wrap_line(text, target_line)
            .iter()
            .position(|row| offset >= row.range.start && offset <= row.range.end)
            .unwrap_or(0);
        let target = ScrollAnchor {
            line: target_line,
            sub_row: target_sub_row,
        };

        // Above the viewport: put the target `margin` rows below the top.
        if target.line < anchor.line
            || (target.line == anchor.line && target.sub_row < anchor.sub_row)
        {
            return self.step_back(text, target, margin);
        }

        // Below: check whether it appears within the visible rows.
        let rows = self.rows_from(text, anchor, height);
        let visible = rows
            .iter()
            .position(|row| offset >= row.range.start && offset <= row.range.end);

        match visible {
            Some(index) if index + margin < height => anchor,
            // Visible but inside the bottom margin, or not visible: scroll so the target sits
            // `margin` rows above the bottom.
            _ => self.step_back(text, target, height.saturating_sub(margin + 1)),
        }
    }

    /// Moves an anchor back by `count` display rows.
    fn step_back(&self, text: &Text, from: ScrollAnchor, count: usize) -> ScrollAnchor {
        let mut line = from.line;
        let mut sub_row = from.sub_row;
        let mut remaining = count;

        while remaining > 0 {
            if sub_row > 0 {
                let step = sub_row.min(remaining);
                sub_row -= step;
                remaining -= step;
                continue;
            }
            if line == 0 {
                break;
            }
            line -= 1;
            sub_row = self.rows_in_line(text, line).saturating_sub(1);
            remaining -= 1;
        }

        ScrollAnchor { line, sub_row }
    }

    /// Vertical motion that follows *display* rows, so moving down a wrapped line lands on the
    /// next visual row rather than skipping the rest of the paragraph.
    pub fn move_vertically(
        &self,
        text: &Text,
        offset: CharOffset,
        delta: isize,
        goal_column: Option<usize>,
    ) -> CharOffset {
        let line = text.char_to_line(offset);
        let rows = self.wrap_line(text, line);
        let index = rows
            .iter()
            .position(|row| offset >= row.range.start && offset <= row.range.end)
            .unwrap_or(0);

        let column = goal_column.unwrap_or_else(|| {
            self.columns_of(
                &text
                    .slice_to_string(rows[index].range.start..offset)
                    .chars()
                    .collect::<Vec<_>>(),
            )
        });

        let target = index as isize + delta;
        let row = if target >= 0 && (target as usize) < rows.len() {
            // Still inside this buffer line's wrapped rows.
            rows[target as usize].clone()
        } else if target < 0 {
            if line == 0 {
                return CharOffset::ZERO;
            }
            let previous = self.wrap_line(text, line - 1);
            previous[previous.len() - 1].clone()
        } else {
            if line >= text.last_line() {
                return text.len_chars();
            }
            self.wrap_line(text, line + 1)[0].clone()
        };

        self.offset_at_column(text, &row, column)
    }

    /// The offset on `row` closest to `column`, clamped to the row's end.
    pub fn offset_at_column(&self, text: &Text, row: &Row, column: usize) -> CharOffset {
        let content = text.slice_to_string(row.range.clone());
        let mut used = 0usize;
        let mut offset = row.range.start.get();

        for c in content.chars() {
            if used >= column {
                break;
            }
            used += self.char_columns(c, used);
            offset += 1;
        }

        CharOffset(offset)
    }

    /// Maps a click to a position: which row, then which column within it.
    pub fn offset_at_display_point(
        &self,
        text: &Text,
        anchor: ScrollAnchor,
        height: usize,
        point: DisplayPoint,
    ) -> CharOffset {
        let rows = self.rows_from(text, anchor, height.max(1));
        let Some(row) = rows.get(point.row).or_else(|| rows.last()) else {
            return CharOffset::ZERO;
        };
        self.offset_at_column(text, row, point.column.saturating_sub(row.indent))
    }
}

/// Converts a [`Point`] to the offset it names, for callers holding logical positions.
pub fn point_offset(text: &Text, point: Point) -> CharOffset {
    text.point_to_char(point)
}
