//! What the terminal shows: the visible rows, copied out of the grid.
//!
//! The drawing never holds the grid. It reads a [`Screen`], which is the viewport's cells and
//! nothing else — copied in one short lock, so the parser thread is never kept waiting on a frame
//! being built, and sized by the panel rather than by the scrollback. A terminal that has printed a
//! million lines costs exactly what one that has printed ten does, which is the rule every other
//! per-frame path in this editor is held to, and `latency.rs` holds this one to it too.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{self, NamedColor};

/// The size of the cell grid, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub rows: usize,
}

impl GridSize {
    /// At least two columns and one row: the grid cannot be empty, and a panel squeezed to nothing
    /// by a tiny window must not take the parser down with it.
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns: columns.max(2),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// A colour as the program asked for it. Resolved against the theme when drawn, so a theme change
/// recolours what is already on screen, as it does in every terminal with a palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// The theme's text colour.
    Foreground,
    /// The theme's background.
    Background,
    /// One of the sixteen named colours, 0–15.
    Ansi(u8),
    /// A colour given exactly: from the 256-colour cube, a 24-bit SGR, or an OSC 4 override.
    Rgb(u8, u8, u8),
}

/// The attributes the renderer can show. One weight is bundled (see "One face, one weight" in
/// CLAUDE.md), so bold is carried as a flag for the drawing to show as brightness rather than as a
/// heavier face; italic is carried and drawn upright for the same reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub inverse: bool,
    pub hidden: bool,
}

/// One cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub style: Style,
    /// The first half of a double-width character; the next cell is its spacer.
    pub wide: bool,
    /// The second half of a double-width character. Drawn as nothing.
    pub spacer: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: Color::Foreground,
            bg: Color::Background,
            style: Style::default(),
            wide: false,
            spacer: false,
        }
    }
}

/// Where the cursor is drawn, in viewport cells. `None` when the program hid it or it is scrolled
/// out of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub column: usize,
}

/// The visible part of the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub size: GridSize,
    /// `size.rows` rows of `size.columns` cells.
    pub rows: Vec<Vec<Cell>>,
    pub cursor: Option<Cursor>,
    /// How far the view is scrolled back into history, in rows. Zero is the live screen.
    pub scrolled_back: usize,
}

impl Screen {
    /// An empty screen of the given size, as a terminal shows before its shell has printed.
    pub fn blank(size: GridSize) -> Self {
        Self {
            size,
            rows: vec![vec![Cell::default(); size.columns]; size.rows],
            cursor: Some(Cursor { row: 0, column: 0 }),
            scrolled_back: 0,
        }
    }

    /// The text of one visible row, trailing blanks trimmed. For tests and for anything that wants
    /// to read the screen rather than draw it.
    pub fn row_text(&self, row: usize) -> String {
        let Some(cells) = self.rows.get(row) else {
            return String::new();
        };
        let text: String = cells
            .iter()
            .filter(|cell| !cell.spacer)
            .map(|cell| cell.c)
            .collect();
        text.trim_end().to_string()
    }

    /// Every visible row's text.
    pub fn lines(&self) -> Vec<String> {
        (0..self.rows.len()).map(|row| self.row_text(row)).collect()
    }

    /// Copies the viewport out of `term`.
    ///
    /// Walks `screen_lines × columns` cells and never the history: the rows are addressed from the
    /// display offset, which is how a scrolled-back view is read without visiting anything above
    /// it.
    pub fn capture<T: EventListener>(term: &Term<T>) -> Self {
        let grid = term.grid();
        let size = GridSize::new(grid.columns(), grid.screen_lines());
        let offset = grid.display_offset();
        let colors = term.colors();
        let resolve = |color: ansi::Color| resolve(color, colors);

        let mut rows = Vec::with_capacity(size.rows);
        for row in 0..size.rows {
            let line = Line(row as i32 - offset as i32);
            let source = &grid[line];
            let mut cells = Vec::with_capacity(size.columns);
            for column in 0..size.columns {
                let cell = &source[Column(column)];
                let flags = cell.flags;
                let (fg, dim_named) = resolve(cell.fg);
                cells.push(Cell {
                    c: if cell.c == '\t' { ' ' } else { cell.c },
                    fg,
                    bg: resolve(cell.bg).0,
                    style: Style {
                        bold: flags.contains(CellFlags::BOLD),
                        dim: flags.contains(CellFlags::DIM) || dim_named,
                        italic: flags.contains(CellFlags::ITALIC),
                        underline: flags.intersects(CellFlags::ALL_UNDERLINES),
                        strikeout: flags.contains(CellFlags::STRIKEOUT),
                        inverse: flags.contains(CellFlags::INVERSE),
                        hidden: flags.contains(CellFlags::HIDDEN),
                    },
                    wide: flags.contains(CellFlags::WIDE_CHAR),
                    spacer: flags.intersects(
                        CellFlags::WIDE_CHAR_SPACER | CellFlags::LEADING_WIDE_CHAR_SPACER,
                    ),
                });
            }
            rows.push(cells);
        }

        let content = term.renderable_content();
        let point = content.cursor.point;
        let visible_row = point.line.0 + offset as i32;
        let shown = term.mode().contains(TermMode::SHOW_CURSOR)
            && content.cursor.shape != ansi::CursorShape::Hidden;
        let cursor =
            (shown && visible_row >= 0 && (visible_row as usize) < size.rows).then(|| Cursor {
                row: visible_row as usize,
                column: point.column.0.min(size.columns - 1),
            });

        Self {
            size,
            rows,
            cursor,
            scrolled_back: offset,
        }
    }
}

/// A colour from the grid, and whether it was one of the "dim" named colours.
fn resolve(
    color: ansi::Color,
    overrides: &alacritty_terminal::term::color::Colors,
) -> (Color, bool) {
    match color {
        ansi::Color::Spec(rgb) => (Color::Rgb(rgb.r, rgb.g, rgb.b), false),
        ansi::Color::Indexed(index) => match overrides[index as usize] {
            Some(rgb) => (Color::Rgb(rgb.r, rgb.g, rgb.b), false),
            None if index < 16 => (Color::Ansi(index), false),
            None => {
                let (r, g, b) = xterm_256(index);
                (Color::Rgb(r, g, b), false)
            }
        },
        ansi::Color::Named(named) => {
            let index = named as usize;
            if index < 16 {
                return match overrides[index] {
                    Some(rgb) => (Color::Rgb(rgb.r, rgb.g, rgb.b), false),
                    None => (Color::Ansi(index as u8), false),
                };
            }
            match named {
                NamedColor::Background => (Color::Background, false),
                NamedColor::DimForeground => (Color::Foreground, true),
                NamedColor::DimBlack
                | NamedColor::DimRed
                | NamedColor::DimGreen
                | NamedColor::DimYellow
                | NamedColor::DimBlue
                | NamedColor::DimMagenta
                | NamedColor::DimCyan
                | NamedColor::DimWhite => (
                    Color::Ansi((index - NamedColor::DimBlack as usize) as u8),
                    true,
                ),
                _ => (Color::Foreground, false),
            }
        }
    }
}

/// The xterm 256-colour palette above the sixteen named ones: a 6×6×6 cube, then 24 greys.
pub fn xterm_256(index: u8) -> (u8, u8, u8) {
    match index {
        16..=231 => {
            let n = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        232..=255 => {
            let grey = 8 + (index - 232) * 10;
            (grey, grey, grey)
        }
        _ => (0, 0, 0),
    }
}

/// A terminal with no process behind it, fed bytes directly.
///
/// For the screenshot harness, which has to draw the same transcript every time, and for tests that
/// want the parser without a shell. The live [`crate::Terminal`] runs the same `Term` and the same
/// [`Screen::capture`]; only where the bytes come from differs.
pub struct Emulator {
    term: Term<alacritty_terminal::event::VoidListener>,
    parser: ansi::Processor,
}

impl Emulator {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let config = alacritty_terminal::term::Config {
            scrolling_history: scrollback,
            ..Default::default()
        };
        Self {
            term: Term::new(config, &size, alacritty_terminal::event::VoidListener),
            parser: ansi::Processor::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn screen(&self) -> Screen {
        Screen::capture(&self.term)
    }

    /// Scrolls the view back by `rows` (or forward, if negative), as the wheel does.
    pub fn scroll(&mut self, rows: i32) {
        self.term
            .scroll_display(alacritty_terminal::grid::Scroll::Delta(rows));
    }

    /// How many rows of history are held above the screen.
    pub fn history(&self) -> usize {
        self.term.grid().history_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_attributes_survive_the_parser() {
        let mut emulator = Emulator::new(GridSize::new(20, 3), 100);
        emulator.feed(b"\x1b[1;31mred\x1b[0m \x1b[38;5;196mx\x1b[38;2;1;2;3my\x1b[7mz");
        let screen = emulator.screen();
        assert_eq!(screen.row_text(0), "red xyz");
        let row = &screen.rows[0];
        assert_eq!(row[0].fg, Color::Ansi(1));
        assert!(row[0].style.bold);
        assert_eq!(row[3].fg, Color::Foreground, "reset by SGR 0");
        assert_eq!(
            row[4].fg,
            Color::Rgb(255, 0, 0),
            "index 196 is the cube's red"
        );
        assert_eq!(row[5].fg, Color::Rgb(1, 2, 3), "truecolor passes through");
        assert!(row[6].style.inverse);
    }

    #[test]
    fn the_cursor_is_where_the_program_left_it() {
        let mut emulator = Emulator::new(GridSize::new(10, 4), 0);
        emulator.feed(b"ab\r\ncd");
        assert_eq!(emulator.screen().cursor, Some(Cursor { row: 1, column: 2 }));
        emulator.feed(b"\x1b[?25l");
        assert_eq!(emulator.screen().cursor, None, "hidden on request");
    }

    #[test]
    fn scrolling_back_shows_history_and_hides_the_cursor() {
        let mut emulator = Emulator::new(GridSize::new(10, 2), 100);
        for n in 0..10 {
            emulator.feed(format!("line {n}\r\n").as_bytes());
        }
        assert!(emulator.history() >= 8);
        emulator.scroll(3);
        let screen = emulator.screen();
        assert_eq!(screen.scrolled_back, 3);
        assert_eq!(screen.row_text(0), "line 6");
        assert_eq!(screen.cursor, None, "the cursor is below the view");
    }

    #[test]
    fn the_256_colour_cube_is_xterms() {
        assert_eq!(xterm_256(16), (0, 0, 0));
        assert_eq!(xterm_256(231), (255, 255, 255));
        assert_eq!(xterm_256(232), (8, 8, 8));
        assert_eq!(xterm_256(255), (238, 238, 238));
    }

    #[test]
    fn a_grid_is_never_empty() {
        assert_eq!(
            GridSize::new(0, 0),
            GridSize {
                columns: 2,
                rows: 1
            }
        );
    }
}
