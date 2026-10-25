//! The settings panel.
//!
//! A list of every option, its current value, and **where that value came from** — default, imported
//! from VSCode, or your own `init.lua`. That last column is the reason this exists rather than being
//! only a file: when a setting is not what you expect, the useful question is which of three places
//! last set it, and no text editor answers that by showing you one of them.
//!
//! Changes apply immediately and are written back to `init.lua` on request, into a clearly marked
//! block at the end of the file. Rewriting the user's own lines is not attempted: config is *code*
//! here, a `dacode.set` call may be inside a loop or a condition, and an editor that reformats your
//! program to store a checkbox has overstepped. Appending wins because `init.lua` applies in order,
//! so the panel's block is simply the last word.

use dacode_render::Rect;
use dacode_theme::{Theme, Typography, space};

/// One row of the panel, and the box it occupies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub line: Line,
    /// Its index in [`Settings::lines`], which is what `selected` counts in.
    pub index: usize,
    /// The full-width block, for the highlight and for the hit test.
    pub rect: Rect,
}

/// A section heading, and where it goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heading {
    pub label: &'static str,
    pub rect: Rect,
}

/// Where everything in the panel is.
#[derive(Debug, Clone)]
pub struct Panel {
    /// Only the rows that fit. A row that would run into the footer is not drawn *and* not
    /// clickable — the two must agree, which is the whole reason this type exists.
    pub rows: Vec<Placement>,
    /// The section headings that are on screen.
    pub headings: Vec<Heading>,
    pub footer: Rect,
    /// Whether anything is scrolled out of view, so the panel can say so.
    pub scrolls: bool,
    /// Content width inside the padding, for text.
    pub width: f32,
    pub pad: f32,
    pub line: f32,
}

/// Lays the panel out.
///
/// **One definition, read by the drawing code and by the mouse.** The panel had two: the drawing
/// walked the row list and the click handler walked it again. That was survivable while the list
/// never moved, and stopped being survivable the moment it started scrolling — a click then
/// selected whichever row *used* to be under the pointer. That is precisely the failure `layout.rs`
/// exists to prevent, reproduced in the panel next door, and it is why this returns rectangles
/// rather than a formula for them.
pub fn panel(rect: Rect, scale: f32, settings: &Settings) -> Panel {
    let selected = settings.selected;
    let pad = space::LG * scale;
    let line = 20.0 * scale;
    let width = rect.width - pad * 2.0;

    // Under the heading, and above the footer.
    let top = rect.y + pad + line * 1.7;
    let footer = Rect::new(rect.x + pad, rect.bottom() - line * 2.2, width, line);
    let bottom = footer.y - line * 0.6;

    let entries = settings.lines();
    let mut flow = 0.0;
    let mut blocks = Vec::with_capacity(entries.len());
    for entry in &entries {
        // A heading before each section, so a checklist and a report do not read as more settings.
        let heading = section(*entry).map(|label| {
            flow += line * 0.6;
            let at = (flow, label);
            flow += line * 1.4;
            at
        });
        let block = match entry {
            Line::Option(_) => line * 2.0,
            // Two lines: what DaCode looked for, and what it found.
            Line::Server(_) | Line::Formatter(_) => line * 2.0,
            _ => line * 1.4,
        };
        blocks.push((heading, flow, block));
        flow += block + 4.0 * scale;
    }

    // Scroll by as little as it takes to keep the selection in view: a list that jumps while you
    // walk down it is harder to read than one that holds still.
    let window = (bottom - top).max(line);
    let scrolls = flow > window;
    let scroll = if scrolls {
        blocks
            .get(selected)
            .map(|(_, at, block)| (at + block - window).max(0.0).min(*at))
            .unwrap_or(0.0)
            .clamp(0.0, flow - window)
    } else {
        0.0
    };

    let mut headings = Vec::new();
    let mut rows = Vec::with_capacity(entries.len());
    for (index, (entry, (at_heading, at, block))) in entries.iter().zip(&blocks).enumerate() {
        if let Some((at_heading, label)) = at_heading {
            let y = top - scroll + at_heading;
            if y >= top && y + line <= bottom {
                headings.push(Heading {
                    label,
                    rect: Rect::new(rect.x + pad, y, width, line),
                });
            }
        }

        // Half a row hanging into the footer is what this pass exists to stop.
        let y = top - scroll + at;
        if y < top || y + block > bottom {
            continue;
        }
        rows.push(Placement {
            line: *entry,
            index,
            rect: Rect::new(rect.x, y, rect.width, *block),
        });
    }

    Panel {
        rows,
        headings,
        footer,
        scrolls,
        width,
        pad,
        line,
    }
}

/// The heading a row starts, if it starts one.
fn section(entry: Line) -> Option<&'static str> {
    match entry {
        Line::Import(Importable::Settings) => Some("import from vscode"),
        Line::Server(0) => Some("language servers"),
        Line::Formatter(0) => Some("formatters"),
        _ => None,
    }
}

/// Where a value came from. The point of the panel.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Source {
    /// The built-in default — nothing has set it.
    Default,
    /// Set by `init.lua`, or by the panel writing to it.
    Config,
    /// Written by `dacode import` from a VSCode installation.
    Imported,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Default => "default",
            Source::Config => "init.lua",
            Source::Imported => "imported",
        }
    }
}

/// What the panel can change. Deliberately the options that are *safe* to toggle blind: appearance,
/// widths, and switches. A theme name or a keybinding needs a file, and pretending otherwise would
/// mean building a colour picker and a key capturer into a panel that has neither.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Field {
    /// Vim's modal grammar. First in the list because it changes what every other key does.
    VimKeys,
    FontSize,
    TabWidth,
    SoftWrap,
    FormatOnSave,
    Theme,
    LineNumbers,
    StickyScroll,
    InlayHints,
    Autosave,
    Transparency,
}

impl Field {
    pub const ALL: [Field; 11] = [
        Field::VimKeys,
        Field::FontSize,
        Field::TabWidth,
        Field::SoftWrap,
        Field::FormatOnSave,
        Field::Theme,
        Field::LineNumbers,
        Field::StickyScroll,
        Field::InlayHints,
        Field::Autosave,
        Field::Transparency,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::VimKeys => "vim_keys",
            Field::FontSize => "font_size",
            Field::TabWidth => "tab_width",
            Field::SoftWrap => "soft_wrap",
            Field::FormatOnSave => "format_on_save",
            Field::Theme => "theme",
            Field::LineNumbers => "line_numbers",
            Field::StickyScroll => "sticky_scroll",
            Field::InlayHints => "inlay_hints",
            Field::Autosave => "autosave",
            Field::Transparency => "transparency",
        }
    }

    /// What the panel calls the row.
    ///
    /// Separate from [`Field::name`], which is the key written to `init.lua` and read back, and so
    /// cannot change. This is the label, and with the modal grammar off it stops saying "vim".
    pub fn title(self, modal: bool) -> &'static str {
        match (self, modal) {
            (Field::VimKeys, false) => "modal editing",
            _ => self.name(),
        }
    }

    pub fn help(self, modal: bool) -> &'static str {
        match self {
            // The one row that must talk about Vim, since it is the switch. With the switch already
            // off, though, describing what "off" means is describing where the reader already is —
            // so it says what turning it *on* would do.
            Field::VimKeys if modal => "vim's modal editing: normal, insert and visual",
            Field::VimKeys => "off: typing always inserts. on: vim's modal editing",
            Field::FontSize => "size of the code face, in pixels",
            Field::TabWidth => "columns a tab stands for",
            Field::SoftWrap => "wrap long lines instead of scrolling sideways",
            Field::FormatOnSave => "reformat through the language server after each save",
            Field::Theme => "one of the six that ship with DaCode",
            Field::LineNumbers if modal => "absolute, relative (vim's), or none at all",
            Field::LineNumbers => "absolute, relative, or none at all",
            Field::StickyScroll => "pin enclosing block heads above the viewport",
            Field::InlayHints => "show inferred types and parameter names inline",
            Field::Autosave => "write the file a moment after you stop typing",
            Field::Transparency => "how much of the desktop shows through the window",
        }
    }
}

/// What the panel can import from VSCode, and whether each is ticked.
///
/// A checklist rather than one "import everything" action: an import that silently replaces your theme
/// because you wanted your keybindings is an import nobody runs twice.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Importable {
    Settings,
    Theme,
    Keybindings,
    Snippets,
}

impl Importable {
    pub const ALL: [Importable; 4] = [
        Importable::Settings,
        Importable::Theme,
        Importable::Keybindings,
        Importable::Snippets,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Importable::Settings => "settings.json",
            Importable::Theme => "color theme",
            Importable::Keybindings => "keybindings.json",
            Importable::Snippets => "snippets",
        }
    }

    pub fn item(self) -> dacode_import::ImportItem {
        match self {
            Importable::Settings => dacode_import::ImportItem::Settings,
            Importable::Theme => dacode_import::ImportItem::Theme,
            Importable::Keybindings => dacode_import::ImportItem::Keybindings,
            Importable::Snippets => dacode_import::ImportItem::Snippets,
        }
    }
}

/// Every row in the panel: the options, then the import checklist, then the action that runs it,
/// then what DaCode found when it looked for language servers.
///
/// One list rather than several panes, so `j` and `k` walk the whole thing and there is no second
/// focus to keep track of.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Line {
    Option(Field),
    Import(Importable),
    RunImport,
    /// Load a colour scheme from a file, in any format the importer reads.
    LoadTheme,
    /// An index into [`Settings::servers`].
    Server(usize),
    /// An index into [`Settings::formatters`].
    Formatter(usize),
}

/// What DaCode found when it looked for a language server.
///
/// **A report, not a setting.** The panel cannot install a program, and a row that looks like a
/// control and is not would be worse than plain text — so these rows do not cycle, and the only
/// thing pressing one does is put the whole line in the info box, which is how a path too long for
/// a 292-pixel sidebar becomes readable.
///
/// It exists because the failure it describes is otherwise invisible. "Completion does not work"
/// has three causes that look identical from the outside: DaCode knows no server for the language, it
/// knows one and cannot find it, or `init.lua` named one that is not where it says. Without this
/// section the only evidence is a statusline cell a few words wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    /// "Python", not "py".
    pub language: String,
    pub program: String,
    /// Where it is, or that it is not there and what would install it.
    pub status: String,
    pub found: bool,
    /// Whether `init.lua` asked for this one.
    pub configured: bool,
}

/// Looks for every language server DaCode knows about or was told about.
///
/// Costs a `PATH` walk per program — nine of them, each a bounded number of `is_file` calls against
/// a fixed table. Bounded, but not free, which is why it runs once when the panel opens rather than
/// while drawing. If it ever shows up in a frame, the fix is the same shape as every other producer
/// here: a worker and a version tag.
pub fn detect(
    root: &std::path::Path,
    home: Option<&std::path::Path>,
    configured: &[dacode_lsp::Configured],
) -> Vec<ServerRow> {
    let mut rows = Vec::new();

    // Configured entries get a row each, and go first. Someone who named a server wants to know
    // about *that* one, and folding it into a per-language "what won" row would hide a mistyped
    // path behind a working fallback — which is precisely the case this section exists for.
    for entry in configured {
        let found = dacode_lsp::resolve_command(&entry.spec.program, root, home);
        rows.push(ServerRow {
            language: entry.spec.language.clone(),
            program: entry.spec.program.clone(),
            status: match &found {
                Some(path) => path.display().to_string(),
                None => "not found — check the path in init.lua".into(),
            },
            found: found.is_some(),
            configured: true,
        });
    }

    for language in &dacode_lsp::LANGUAGES {
        let found = language
            .programs()
            .into_iter()
            .find_map(|program| dacode_lsp::resolve(program).map(|path| (program, path)));
        rows.push(match found {
            Some((program, path)) => ServerRow {
                language: language.name.to_string(),
                program: program.to_string(),
                status: path.display().to_string(),
                found: true,
                configured: false,
            },
            // Names every program it looked for, not just the first: "no Python server" is not
            // actionable when the reader has pylsp installed and DaCode wanted pyright.
            None => ServerRow {
                language: language.name.to_string(),
                program: language.programs().join(", "),
                status: language.install().to_string(),
                found: false,
                configured: false,
            },
        });
    }

    rows
}

/// The same, for external formatters.
///
/// A separate section rather than a column on the server rows: they are different programs, a
/// language can have one without the other, and "format does nothing" is a different question from
/// "completion does nothing" even when the answer is the same shape.
pub fn detect_formatters(
    root: &std::path::Path,
    home: Option<&std::path::Path>,
    configured: &[crate::format::Configured],
) -> Vec<ServerRow> {
    let mut rows = Vec::new();

    for entry in configured {
        let found = crate::format::resolve(&entry.formatter, root, home);
        rows.push(ServerRow {
            language: entry.formatter.language.clone(),
            program: entry.formatter.program.clone(),
            status: match &found {
                Some(path) => path.display().to_string(),
                None => "not found — check the path in init.lua".into(),
            },
            found: found.is_some(),
            configured: true,
        });
    }

    // One row per language DaCode knows a formatter for, keyed off a file name it would match — the
    // table is private, and asking it the same question the editor asks keeps the report honest
    // rather than a second description of the same list.
    for probe in [
        "x.rs", "x.ts", "x.py", "x.cpp", "x.go", "x.json", "x.css", "x.md",
    ] {
        let candidates = crate::format::formatters_for(probe, &[]);
        let Some(first) = candidates.first() else {
            continue;
        };
        let found = candidates.iter().find_map(|formatter| {
            crate::format::resolve(formatter, root, home).map(|at| (formatter, at))
        });
        rows.push(match found {
            Some((formatter, path)) => ServerRow {
                language: formatter.language.clone(),
                program: formatter.program.clone(),
                status: path.display().to_string(),
                found: true,
                configured: false,
            },
            None => ServerRow {
                language: first.language.clone(),
                program: candidates
                    .iter()
                    .map(|formatter| formatter.program.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                status: first.install.clone(),
                found: false,
                configured: false,
            },
        });
    }

    rows
}

/// The panel's state: which row is selected, and whether anything is unsaved.
pub struct Settings {
    pub selected: usize,
    /// Fields changed since the panel opened, so the footer can say there is something to write.
    pub dirty: bool,
    /// Which import items are ticked. Nothing, until asked — the same default the CLI has.
    pub ticked: Vec<Importable>,
    /// What the last import said, shown in place of the hint.
    pub import_report: Option<String>,
    /// What DaCode found when it looked for language servers, gathered when the panel opened.
    ///
    /// Gathered once rather than per frame: finding a program means walking `PATH` and asking the
    /// filesystem about every candidate, which is exactly the kind of unbounded work that must
    /// never happen while drawing.
    pub servers: Vec<ServerRow>,
    /// The same, for external formatters.
    pub formatters: Vec<ServerRow>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

impl Settings {
    pub fn new() -> Self {
        Self {
            selected: 0,
            dirty: false,
            ticked: Vec::new(),
            import_report: None,
            servers: Vec::new(),
            formatters: Vec::new(),
        }
    }

    /// Every line, in display order.
    ///
    /// A method rather than an associated function because the last section's length depends on how
    /// many servers were found — and a layout that disagrees with the list it is laying out is the
    /// defect this module already has one test for.
    pub fn lines(&self) -> Vec<Line> {
        Field::ALL
            .iter()
            .map(|field| Line::Option(*field))
            .chain(Importable::ALL.iter().map(|item| Line::Import(*item)))
            .chain([Line::RunImport, Line::LoadTheme])
            .chain((0..self.servers.len()).map(Line::Server))
            .chain((0..self.formatters.len()).map(Line::Formatter))
            .collect()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = self.lines().len() - 1;
        self.selected = match delta {
            d if d < 0 => self.selected.saturating_sub(d.unsigned_abs()),
            d => (self.selected + d as usize).min(last),
        };
    }

    /// The selected line.
    pub fn line(&self) -> Line {
        let lines = self.lines();
        lines[self.selected.min(lines.len() - 1)]
    }

    /// The selected option, when one is selected.
    pub fn field(&self) -> Option<Field> {
        match self.line() {
            Line::Option(field) => Some(field),
            _ => None,
        }
    }

    pub fn is_ticked(&self, item: Importable) -> bool {
        self.ticked.contains(&item)
    }

    pub fn tick(&mut self, item: Importable) {
        if let Some(at) = self.ticked.iter().position(|ticked| *ticked == item) {
            self.ticked.remove(at);
        } else {
            self.ticked.push(item);
        }
    }

    /// The checklist to hand the importer.
    pub fn checklist(&self) -> dacode_import::Checklist {
        self.ticked
            .iter()
            .fold(dacode_import::Checklist::nothing(), |list, item| {
                list.with(item.item())
            })
    }
}

/// The values the panel shows and edits.
///
/// A plain struct rather than a reference into the editor, so the panel can be tested and so writing
/// it back is one function over one value.
#[derive(Debug, Clone, PartialEq)]
pub struct Values {
    pub modal: bool,
    pub font_size: f32,
    pub tab_width: usize,
    pub soft_wrap: bool,
    pub format_on_save: bool,
    /// The built-in theme's name, or `None` when a theme file is loaded — which the panel cannot
    /// cycle through, because a file's colours are not one of a known set.
    pub theme: Option<&'static str>,
    pub sticky_scroll: bool,
    pub inlay_hints: bool,
    pub autosave: bool,
    pub line_numbers: crate::config::LineNumbers,
    /// Percent, in steps of [`Values::TRANSPARENCY_STEP`].
    pub transparency: u8,
}

impl Values {
    /// The same ceiling `dacode-lua` clamps a configured value to.
    pub const MAX_TRANSPARENCY: u8 = 90;
    pub const TRANSPARENCY_STEP: u8 = 10;

    /// What a tab may stand for. The same bounds `dacode-lua` clamps a configured `tab_width` to —
    /// stated once here because the panel and the config file must agree about what is reachable,
    /// and the font size next to it is the case where they did not.
    pub const MIN_TAB_WIDTH: usize = 1;
    pub const MAX_TAB_WIDTH: usize = 16;

    pub fn get(&self, field: Field) -> String {
        match field {
            Field::VimKeys => on_off(self.modal),
            Field::FontSize => format!("{:.1}", self.font_size),
            Field::TabWidth => self.tab_width.to_string(),
            Field::SoftWrap => on_off(self.soft_wrap),
            Field::FormatOnSave => on_off(self.format_on_save),
            Field::Theme => self.theme.unwrap_or("from a file").to_string(),
            Field::LineNumbers => self.line_numbers.name().to_string(),
            Field::StickyScroll => on_off(self.sticky_scroll),
            Field::InlayHints => on_off(self.inlay_hints),
            Field::Autosave => on_off(self.autosave),
            Field::Transparency => match self.transparency {
                0 => "off".into(),
                percent => format!("{percent}%"),
            },
        }
    }

    /// What a *click* on a row does: the same as `l`, except that a number at its limit wraps.
    ///
    /// The keyboard clamps, deliberately — holding `l` should stop at the maximum rather than
    /// silently starting again at the minimum. But a click has no other direction to go in, so a
    /// mouse user who clicked the font size up to its ceiling would be stuck there with no way back.
    /// Wrapping is the only honest single-direction control, and it is why this is not `adjust(+1)`.
    ///
    /// The limit has to be the one the editor actually enforces. It was written as 32 while
    /// [`Typography::with_code_size`] clamps at 18 and [`Values`] is read back from the live
    /// typography — so the value could never reach 32, this branch never ran, and the mouse-only
    /// user it exists for was stuck at 18 with the panel offering no way down. A wrap point above
    /// the real ceiling is the same as no wrap at all.
    pub fn cycle(&mut self, field: Field) {
        let at_limit = match field {
            Field::FontSize => self.font_size >= Typography::MAX_CODE_SIZE,
            Field::TabWidth => self.tab_width >= Self::MAX_TAB_WIDTH,
            Field::Transparency => self.transparency >= Self::MAX_TRANSPARENCY,
            _ => false,
        };
        if at_limit {
            match field {
                Field::FontSize => self.font_size = Typography::MIN_CODE_SIZE,
                Field::TabWidth => self.tab_width = Self::MIN_TAB_WIDTH,
                Field::Transparency => self.transparency = 0,
                _ => {}
            }
            return;
        }
        self.adjust(field, 1);
    }

    /// Changes a field. `delta` is +1 or -1; a toggle ignores its sign.
    pub fn adjust(&mut self, field: Field, delta: i32) {
        match field {
            // The bounds are the editor's own, taken from the types that enforce them rather than
            // written out again here. They were written out again here, as 8 and 32, while
            // `Typography` clamped at 11 and 18 — so the panel showed a number the editor had
            // already refused, which is exactly the lying it was trying to avoid.
            Field::VimKeys => self.modal = !self.modal,
            Field::FontSize => {
                self.font_size = (self.font_size + delta as f32)
                    .clamp(Typography::MIN_CODE_SIZE, Typography::MAX_CODE_SIZE);
            }
            Field::TabWidth => {
                self.tab_width = (self.tab_width as i32 + delta)
                    .clamp(Self::MIN_TAB_WIDTH as i32, Self::MAX_TAB_WIDTH as i32)
                    as usize;
            }
            Field::SoftWrap => self.soft_wrap = !self.soft_wrap,
            Field::FormatOnSave => self.format_on_save = !self.format_on_save,
            Field::Theme => {
                // Cycles through the built-ins in the order they are listed, both ways.
                let names: Vec<&'static str> =
                    Theme::BUILT_IN.iter().map(|(name, _)| *name).collect();
                let at = self
                    .theme
                    .and_then(|current| names.iter().position(|name| *name == current))
                    .unwrap_or(0);
                let next = if delta < 0 {
                    (at + names.len() - 1) % names.len()
                } else {
                    (at + 1) % names.len()
                };
                self.theme = Some(names[next]);
            }
            Field::LineNumbers => {
                let all = crate::config::LineNumbers::ALL;
                let at = all
                    .iter()
                    .position(|value| *value == self.line_numbers)
                    .unwrap_or(0);
                let next = if delta < 0 {
                    (at + all.len() - 1) % all.len()
                } else {
                    (at + 1) % all.len()
                };
                self.line_numbers = all[next];
            }
            Field::StickyScroll => self.sticky_scroll = !self.sticky_scroll,
            Field::InlayHints => self.inlay_hints = !self.inlay_hints,
            Field::Autosave => self.autosave = !self.autosave,
            Field::Transparency => {
                let step = Self::TRANSPARENCY_STEP as i32;
                self.transparency = (self.transparency as i32 + delta.signum() * step)
                    .clamp(0, Self::MAX_TRANSPARENCY as i32)
                    as u8;
            }
        }
    }
}

fn on_off(value: bool) -> String {
    if value { "on".into() } else { "off".into() }
}

/// The marker the panel's block is written under.
const MARKER: &str =
    "-- dacode: written by the settings panel (ctrl-,). Edit freely — rewritten on save.";

/// Rewrites `init.lua` so the panel's block reflects `values`.
///
/// Everything the user wrote is preserved verbatim; only the block under [`MARKER`] is replaced, and
/// it is appended if absent. `init.lua` applies in order, so the panel's block being last is what
/// makes it authoritative — and moving a setting out of it back into your own code is as simple as
/// deleting the line, because your line runs first only until the block runs after it.
pub fn write_back(existing: &str, values: &Values) -> String {
    // A theme loaded from a file has no name the panel could write, and writing "dark" for it would
    // throw that theme away the next time anyone changed the font size — a setting the user did not
    // touch, silently replacing one they did. Nothing written means nothing changed.
    let theme = match values.theme {
        Some(name) => format!("  theme = \"{name}\",\n"),
        None => String::new(),
    };

    let block = format!(
        "{MARKER}\ndacode.set {{\n  \
         vim_keys = {},\n  \
         font_size = {:.1},\n  \
         tab_width = {},\n  \
         soft_wrap = {},\n  \
         format_on_save = {},\n\
         {theme}  \
         line_numbers = \"{}\",\n  \
         sticky_scroll = {},\n  \
         inlay_hints = {},\n  \
         autosave = {},\n  \
         transparency = {},\n}}\n",
        values.modal,
        values.font_size,
        values.tab_width,
        values.soft_wrap,
        values.format_on_save,
        values.line_numbers.name(),
        values.sticky_scroll,
        values.inlay_hints,
        values.autosave,
        values.transparency,
    );

    match existing.find(MARKER) {
        // Replace the old block. Anything the user added *after* it is kept, which matters because
        // people do add lines below whatever a tool wrote.
        Some(at) => {
            let before = &existing[..at];
            let rest = &existing[at..];
            let end = rest
                .find("\n}\n")
                .map(|i| at + i + 3)
                .unwrap_or(existing.len());
            format!("{before}{block}{}", &existing[end..])
        }
        None => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&block);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouse_can_bring_the_font_size_back_down() {
        // The trap: `cycle` wrapped at 32 and `adjust` clamped to 32, while
        // `Typography::with_code_size` clamps at 18 and `Values` is read back from the live
        // typography. So the panel's value could never exceed 18, the wrap never fired, and someone
        // who had clicked the row up to 18 — the only direction a click goes — had no way back.
        //
        // Driven through `cycle`, which is what a click calls, rather than through a value set by
        // hand. The previous tests set the field directly and passed the whole time this was broken.
        let mut values = Values {
            font_size: Typography::MIN_CODE_SIZE,
            tab_width: 4,
            soft_wrap: true,
            format_on_save: false,
            modal: true,
            theme: Some("dark"),
            line_numbers: crate::config::LineNumbers::Absolute,
            transparency: 0,
            sticky_scroll: true,
            inlay_hints: false,
            autosave: false,
        };

        // Enough clicks to pass the ceiling several times over, so a wrap that never fires shows up
        // as a value stuck at the top rather than as a lucky landing.
        let mut seen = Vec::new();
        for _ in 0..40 {
            values.cycle(Field::FontSize);
            seen.push(values.font_size);
        }

        assert!(
            seen.contains(&Typography::MIN_CODE_SIZE),
            "clicking the font size never brings it back to {}: it reached {:?} and stayed",
            Typography::MIN_CODE_SIZE,
            seen.last()
        );

        // And every value a click can produce must be one the editor will actually keep. A panel
        // offering a number that `with_code_size` then clamps away is the same defect wearing the
        // other face: the row would show 24 and the text would be drawn at 18.
        for size in seen {
            assert_eq!(
                Typography::with_code_size(size).code_size,
                size,
                "the panel offers {size}, which the typography clamps away"
            );
        }
    }

    #[test]
    fn a_click_selects_the_row_it_looks_like_it_selects_at_every_height() {
        // The regression this exists for: the drawing code was taught to scroll and the click
        // handler was not, so in any window short enough to scroll the panel a click selected
        // whichever row *used* to be under the pointer. One function now answers both, and this is
        // the test that says so — at every height, and at every scroll position the panel can be in.
        let mut settings = Settings::new();
        settings.servers = vec![
            ServerRow {
                language: "Python".into(),
                program: ".venv/bin/pylsp".into(),
                status: "not found — check the path in init.lua".into(),
                found: false,
                configured: true,
            },
            ServerRow {
                language: "Rust".into(),
                program: "rust-analyzer".into(),
                status: "/home/x/.cargo/bin/rust-analyzer".into(),
                found: true,
                configured: false,
            },
        ];

        for height in [300.0_f32, 420.0, 560.0, 700.0, 860.0, 1000.0] {
            for selected in 0..settings.lines().len() {
                settings.selected = selected;
                let rect = Rect::new(1300.0, 120.0, 292.0, height);
                let panel = panel(rect, 1.0, &settings);

                for row in &panel.rows {
                    for probe in [row.rect.y + 1.0, row.rect.y + row.rect.height / 2.0] {
                        let hit = panel
                            .rows
                            .iter()
                            .find(|other| probe >= other.rect.y && probe < other.rect.bottom());
                        assert_eq!(
                            hit.map(|hit| hit.index),
                            Some(row.index),
                            "at height {height} with row {selected} selected, a click inside row \
                             {} landed elsewhere",
                            row.index
                        );
                    }
                }

                // Nothing may overlap the footer, which is what the original defect looked like.
                for row in &panel.rows {
                    assert!(
                        row.rect.bottom() <= panel.footer.y,
                        "a row runs into the footer at height {height}"
                    );
                }

                // The selection is always reachable: a list that scrolls away from the thing you
                // are changing is worse than one that does not scroll.
                assert!(
                    panel.rows.iter().any(|row| row.index == selected),
                    "row {selected} is not visible at height {height}"
                );
            }
        }
    }

    #[test]
    fn a_named_server_is_reported_separately_from_the_one_dacode_would_fall_back_to() {
        // The failure this section exists to expose: a mistyped path in init.lua, hidden behind a
        // working fallback. Reporting only the winner would say "Python: found" and be useless.
        let dir = std::env::temp_dir().join(format!("dacode-detect-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".venv/bin")).unwrap();
        std::fs::write(dir.join(".venv/bin/pylsp"), "").unwrap();

        let good = dacode_lsp::configure("python", ".venv/bin/pylsp", &[], &[]).unwrap();
        let bad = dacode_lsp::configure("python", ".venv/bin/typo", &[], &[]).unwrap();
        let rows = detect(&dir, None, &[good, bad]);

        assert!(rows[0].configured && rows[0].found, "{:?}", rows[0]);
        assert!(rows[1].configured && !rows[1].found, "{:?}", rows[1]);
        assert!(
            rows[1].status.contains("init.lua"),
            "a wrong path is the user's to fix, and the message says where: {}",
            rows[1].status
        );
        assert_eq!(
            rows.len(),
            2 + dacode_lsp::LANGUAGES.len(),
            "every language dacode knows is still reported, found or not"
        );
        assert!(
            rows.iter().all(|row| !row.status.is_empty()),
            "a row with nothing to say is a row that reads as broken"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tall_panel_does_not_scroll_at_all() {
        let settings = Settings::new();
        let panel = panel(Rect::new(0.0, 0.0, 292.0, 1400.0), 1.0, &settings);
        assert!(!panel.scrolls);
        assert_eq!(panel.rows.len(), settings.lines().len(), "all of it fits");
        assert!(
            panel
                .headings
                .iter()
                .any(|h| h.label == "import from vscode"),
            "including the import heading"
        );
    }

    #[test]
    fn the_language_server_section_appears_only_when_there_is_something_to_report() {
        // A heading over nothing is worse than no heading: it reads as a section that failed to
        // load rather than one that had nothing to say.
        let mut settings = Settings::new();
        let empty = panel(Rect::new(0.0, 0.0, 292.0, 1400.0), 1.0, &settings);
        assert!(!empty.headings.iter().any(|h| h.label == "language servers"));

        settings.servers.push(ServerRow {
            language: "Rust".into(),
            program: "rust-analyzer".into(),
            status: "/usr/bin/rust-analyzer".into(),
            found: true,
            configured: false,
        });
        let filled = panel(Rect::new(0.0, 0.0, 292.0, 1400.0), 1.0, &settings);
        assert!(
            filled
                .headings
                .iter()
                .any(|h| h.label == "language servers")
        );
        assert_eq!(*settings.lines().last().unwrap(), Line::Server(0));
    }

    fn values() -> Values {
        Values {
            modal: true,
            font_size: 13.5,
            tab_width: 4,
            soft_wrap: true,
            format_on_save: false,
            theme: Some("dark"),
            line_numbers: crate::config::LineNumbers::Relative,
            transparency: 0,
            sticky_scroll: true,
            inlay_hints: false,
            autosave: false,
        }
    }

    #[test]
    fn toggles_flip_and_numbers_step() {
        let mut values = values();

        values.adjust(Field::SoftWrap, 1);
        assert!(!values.soft_wrap);
        values.adjust(Field::SoftWrap, -1);
        assert!(values.soft_wrap, "a toggle ignores the sign");

        values.adjust(Field::TabWidth, 1);
        assert_eq!(values.tab_width, 5);
        values.adjust(Field::FontSize, -1);
        assert_eq!(values.get(Field::FontSize), "12.5");
    }

    #[test]
    fn numbers_are_bounded_at_both_ends() {
        // A panel that accepts 400 and then shows 99 is lying about what it did.
        let mut values = values();
        for _ in 0..100 {
            values.adjust(Field::FontSize, 1);
            values.adjust(Field::TabWidth, 1);
        }
        assert_eq!(values.font_size, Typography::MAX_CODE_SIZE);
        assert_eq!(values.tab_width, Values::MAX_TAB_WIDTH);

        for _ in 0..100 {
            values.adjust(Field::FontSize, -1);
            values.adjust(Field::TabWidth, -1);
        }
        assert_eq!(values.font_size, Typography::MIN_CODE_SIZE);
        assert_eq!(
            values.tab_width,
            Values::MIN_TAB_WIDTH,
            "never zero, which would divide by it"
        );
    }

    #[test]
    fn the_theme_cycles_through_every_built_in_both_ways() {
        let mut values = values();
        let mut seen = vec![values.get(Field::Theme)];

        for _ in 1..Theme::BUILT_IN.len() {
            values.adjust(Field::Theme, 1);
            seen.push(values.get(Field::Theme));
        }
        assert_eq!(seen.len(), Theme::BUILT_IN.len());
        assert!(seen.contains(&"rose-pine".to_string()));
        assert!(seen.contains(&"amoled".to_string()));

        // Wherever it started, one more step comes back to it, and one back from there is the last
        // one seen — the wrap, in both directions, without the test knowing the list's order.
        values.adjust(Field::Theme, 1);
        assert_eq!(values.get(Field::Theme), seen[0], "and wraps round");
        values.adjust(Field::Theme, -1);
        assert_eq!(
            &values.get(Field::Theme),
            seen.last().unwrap(),
            "backwards too"
        );
    }

    #[test]
    fn a_theme_loaded_from_a_file_says_so_rather_than_lying() {
        // The panel cannot cycle a file's colours, and showing "dark" when the theme is something
        // imported would be worse than admitting it.
        let mut values = values();
        values.theme = None;
        assert_eq!(values.get(Field::Theme), "from a file");
    }

    #[test]
    fn the_selection_cannot_leave_the_list() {
        let mut settings = Settings::new();
        settings.move_selection(-3);
        assert_eq!(settings.selected, 0);
        settings.move_selection(99);
        assert_eq!(settings.selected, settings.lines().len() - 1);
    }

    #[test]
    fn writing_to_an_empty_config_produces_a_readable_block() {
        let written = write_back("", &values());
        assert!(written.starts_with(MARKER));
        assert!(written.contains("tab_width = 4"));
        assert!(written.contains("theme = \"dark\""));
        assert!(written.ends_with("}\n"));
    }

    #[test]
    fn a_file_theme_is_left_alone_by_the_panel_rather_than_overwritten_with_a_built_in() {
        // The panel has no name for a theme that came from a file, and it used to write "dark" in
        // that case — so changing the font size threw away an imported theme, a setting the user did
        // not touch replacing one they did.
        let mut values = values();
        values.theme = None;
        let written = write_back("dacode.set { theme = \"one-dark-pro\" }\n", &values);

        assert!(!written.contains("theme = \"dark\""), "{written}");
        assert!(
            written.contains("theme = \"one-dark-pro\""),
            "the user's own line still chooses the theme: {written}"
        );
        assert!(
            written.contains("tab_width = 4"),
            "the rest is still written"
        );
    }

    #[test]
    fn the_users_own_lines_are_preserved() {
        // The whole reason for a marked block: config is code, and rewriting someone's program to
        // store a checkbox would be overstepping.
        let existing =
            "-- my config\ndacode.set { tab_width = 8 }\ndacode.map(\"normal\", \"x\", \"save\")\n";
        let written = write_back(existing, &values());

        assert!(written.contains("-- my config"));
        assert!(written.contains("dacode.map(\"normal\", \"x\", \"save\")"));
        assert!(
            written.find("-- my config") < written.find(MARKER),
            "the panel's block goes last, so it is the last word"
        );
    }

    #[test]
    fn writing_twice_replaces_the_block_rather_than_stacking_them() {
        let once = write_back("dacode.map(\"normal\", \"x\", \"save\")\n", &values());

        let mut changed = values();
        changed.adjust(Field::TabWidth, -2);
        let twice = write_back(&once, &changed);

        assert_eq!(twice.matches(MARKER).count(), 1, "one block, not two");
        assert!(twice.contains("tab_width = 2"));
        assert!(!twice.contains("tab_width = 4"));
        assert!(twice.contains("dacode.map"), "and the user's line survived");
    }

    #[test]
    fn lines_added_after_the_block_are_kept() {
        // People do add things below whatever a tool wrote, and losing them would be unforgivable.
        let once = write_back("", &values());
        let edited = format!("{once}\ndacode.map(\"normal\", \"z\", \"quit\")\n");
        let again = write_back(&edited, &values());

        assert!(
            again.contains("dacode.map(\"normal\", \"z\", \"quit\")"),
            "what came after the block is still there: {again}"
        );
    }

    #[test]
    fn what_is_written_is_valid_lua_that_reads_back_the_same() {
        // The panel writes a program. If it cannot be parsed, the next launch loses every setting —
        // so it is round-tripped through the real config reader rather than eyeballed.
        let mut original = values();
        original.adjust(Field::Theme, 1);
        original.adjust(Field::SoftWrap, 1);
        original.adjust(Field::TabWidth, 4);

        let source = write_back("", &original);
        let config = dacode_lua::eval(&source, "settings.lua");

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.tab_width, Some(original.tab_width));
        assert_eq!(config.soft_wrap, Some(original.soft_wrap));
        assert_eq!(config.theme_name.as_deref(), Some("light"));
        assert_eq!(
            config.font_size.map(|size| format!("{size:.1}")),
            Some(original.get(Field::FontSize))
        );
    }

    #[test]
    fn the_panel_is_one_list_of_options_then_imports_then_the_action() {
        let lines = Settings::new().lines();
        // Options, then the import checklist, then the two actions.
        assert_eq!(lines.len(), Field::ALL.len() + Importable::ALL.len() + 2);
        assert_eq!(
            lines[0],
            Line::Option(Field::VimKeys),
            "the modal switch comes first: it changes what every other key does"
        );
        assert_eq!(lines[Field::ALL.len()], Line::Import(Importable::Settings));
        assert_eq!(*lines.last().unwrap(), Line::LoadTheme);
        assert!(lines.contains(&Line::RunImport));
    }

    #[test]
    fn the_selection_walks_the_whole_list_and_stops() {
        let mut settings = Settings::new();
        settings.move_selection(99);
        assert_eq!(settings.line(), Line::LoadTheme);
        assert_eq!(settings.field(), None, "the action row is not an option");

        settings.move_selection(-99);
        assert_eq!(settings.field(), Some(Field::VimKeys));
    }

    #[test]
    fn nothing_is_ticked_until_asked() {
        // The rule the import CLI already follows: an import that replaces your theme because you
        // wanted your keybindings is an import nobody runs twice.
        let mut settings = Settings::new();
        assert!(settings.checklist().is_empty());

        settings.tick(Importable::Theme);
        assert!(settings.is_ticked(Importable::Theme));
        assert!(!settings.is_ticked(Importable::Settings));
        assert!(settings.checklist().wants(dacode_import::ImportItem::Theme));
        assert!(
            !settings
                .checklist()
                .wants(dacode_import::ImportItem::Settings)
        );

        settings.tick(Importable::Theme);
        assert!(
            !settings.is_ticked(Importable::Theme),
            "ticking again unticks"
        );
    }

    #[test]
    fn the_modal_switch_is_a_toggle_and_reaches_the_config_file() {
        let mut values = values();
        assert!(values.modal, "modal editing is the default");

        values.adjust(Field::VimKeys, 1);
        assert!(!values.modal);
        assert_eq!(values.get(Field::VimKeys), "off");

        // And it survives the round trip through the file, or turning Vim off would not stick.
        let source = write_back("", &values);
        let config = dacode_lua::eval(&source, "settings.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.modal, Some(false));
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;

    fn values() -> Values {
        Values {
            modal: true,
            font_size: 13.5,
            tab_width: 4,
            soft_wrap: true,
            format_on_save: false,
            theme: Some("dark"),
            line_numbers: crate::config::LineNumbers::Relative,
            transparency: 0,
            sticky_scroll: true,
            inlay_hints: false,
            autosave: false,
        }
    }

    #[test]
    fn a_click_on_a_switch_flips_it() {
        // The panel is reachable from the toolbar by someone who has been told no keys at all. If a
        // click only highlighted the row, the panel would be unusable to exactly the people the
        // toolbar exists for.
        let mut values = values();
        values.cycle(Field::Autosave);
        assert!(values.autosave);
        values.cycle(Field::Autosave);
        assert!(!values.autosave);
    }

    #[test]
    fn a_click_on_a_number_at_its_limit_wraps_rather_than_sticking() {
        // A click has only one direction. Clamping at the top would leave a mouse user stuck at the
        // largest font with no way back — a control that has trapped the person using it.
        //
        // The limits come from the types that enforce them. Written out as 32 and 8, this test
        // passed for as long as the panel and `Typography` disagreed — the wrap it asserts was
        // unreachable in the running editor, because the value is read back from a typography that
        // had already clamped it to 18.
        let mut values = values();
        values.font_size = Typography::MAX_CODE_SIZE;
        values.cycle(Field::FontSize);
        assert_eq!(values.font_size, Typography::MIN_CODE_SIZE);

        values.tab_width = Values::MAX_TAB_WIDTH;
        values.cycle(Field::TabWidth);
        assert_eq!(values.tab_width, Values::MIN_TAB_WIDTH);
    }

    #[test]
    fn the_keyboard_still_clamps() {
        // Holding `l` should stop at the largest size, not silently start again at the smallest.
        // The two paths differ on purpose, which is why `cycle` is not `adjust(+1)`.
        let mut values = values();
        for _ in 0..100 {
            values.adjust(Field::FontSize, 1);
        }
        assert_eq!(values.font_size, Typography::MAX_CODE_SIZE);
    }

    #[test]
    fn line_numbers_cycle_through_all_three() {
        let mut values = values();
        let seen: Vec<&str> = (0..3)
            .map(|_| {
                values.cycle(Field::LineNumbers);
                values.line_numbers.name()
            })
            .collect();
        assert_eq!(seen, vec!["off", "absolute", "relative"]);
    }
}
