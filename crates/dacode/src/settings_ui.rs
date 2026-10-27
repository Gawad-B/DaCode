//! The settings tab: its model, its geometry, and the block it writes into `init.lua`.
//!
//! JetBrains' category tree on the left, VSCode's rows on the right — a setting's name, a sentence
//! or two under it when the name does not say everything, and its control — with one search box
//! over all of it. The drawing (`chrome::draw_settings_page`) and the mouse both read
//! [`Page::geometry_at`], so a click cannot land somewhere other than where it looks.
//!
//! Changes apply immediately and are written back to `init.lua` a moment after the last one, into a
//! clearly marked block at the end of the file. Rewriting the user's own lines is not attempted:
//! config is *code* here, a `dacode.set` call may be inside a loop or a condition, and an editor that
//! reformats your program to store a checkbox has overstepped. Appending wins because `init.lua`
//! applies in order, so the page's block is simply the last word.

use dacode_render::Rect;
use dacode_theme::{Theme, Typography, space};

/// Where a value came from: the built-in default, `init.lua`, or an import from VSCode.
///
/// Kept by `config::load` for each setting it reads. The page no longer prints it beside every value
/// — readers took "init.lua" and "imported" for section names, and it explained nothing — but where
/// a value came from is still worth knowing to the code that loads it.
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
    /// How much the interface moves: full, reduced or off.
    Animations,
}

/// The index a field's glow is keyed by (`motion::Key::Glow`): its place in [`Field::ALL`], a set
/// that does not grow, as every key with an index must be.
pub fn glow_index(field: Field) -> u32 {
    Field::ALL
        .iter()
        .position(|candidate| *candidate == field)
        .unwrap_or(0) as u32
}

impl Field {
    pub const ALL: [Field; 12] = [
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
        Field::Animations,
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
            Field::Animations => "animations",
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

/// A row of the Import and Languages pages: an import item to tick, the actions that import and
/// load, and what DaCode found when it looked for language servers and formatters.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Line {
    Import(Importable),
    RunImport,
    /// Load a colour scheme from a file, in any format the importer reads.
    LoadTheme,
    /// An index into [`Page::servers`].
    Server(usize),
    /// An index into [`Page::formatters`].
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
    pub animations: crate::motion::Animations,
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
            // The display name: the row shows "Charcoal coral", and the block still writes the slug.
            Field::Theme => self
                .theme
                .map_or("from a file", |slug| {
                    Theme::display_name_of(slug).unwrap_or(slug)
                })
                .to_string(),
            Field::LineNumbers => self.line_numbers.name().to_string(),
            Field::StickyScroll => on_off(self.sticky_scroll),
            Field::InlayHints => on_off(self.inlay_hints),
            Field::Autosave => on_off(self.autosave),
            Field::Transparency => match self.transparency {
                0 => "off".into(),
                percent => format!("{percent}%"),
            },
            Field::Animations => self.animations.name().to_string(),
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
                // Cycles through the built-ins both ways, in the order the dropdown lists them, so the
                // arrows on the row walk the same list a click opens.
                let names = Theme::grouped();
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
            Field::Animations => {
                let all = crate::motion::Animations::ALL;
                let at = all
                    .iter()
                    .position(|value| *value == self.animations)
                    .unwrap_or(0);
                let next = if delta < 0 {
                    (at + all.len() - 1) % all.len()
                } else {
                    (at + 1) % all.len()
                };
                self.animations = all[next];
            }
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

/// The marker the settings block is written under. Its words predate the tab and are kept: an
/// `init.lua` already carrying this line has to be found by it, or the next save appends a second
/// block.
const MARKER: &str =
    "-- dacode: written by the settings panel (ctrl-,). Edit freely — rewritten on save.";

/// The start of the marker vex wrote, before the editor was called DaCode.
const LEGACY_MARKER: &str = "-- vex: written by the settings panel";

/// Rewrites `init.lua` so the settings block holds every one of `values`. What the first-run wizard
/// writes, where every answer is a choice the user has just made. The settings tab writes only what
/// was changed on it — see [`write_touched`].
pub fn write_back(existing: &str, values: &Values) -> String {
    write_touched(existing, values, &Field::ALL, &[])
}

/// One field's line in the block, or `None` for a theme loaded from a file: it has no name the
/// block could write, and writing a built-in in its place would throw that theme away.
fn block_line(field: Field, values: &Values) -> Option<String> {
    let value = match field {
        Field::VimKeys => values.modal.to_string(),
        Field::FontSize => format!("{:.1}", values.font_size),
        Field::TabWidth => values.tab_width.to_string(),
        Field::SoftWrap => values.soft_wrap.to_string(),
        Field::FormatOnSave => values.format_on_save.to_string(),
        Field::Theme => format!("\"{}\"", values.theme?),
        Field::LineNumbers => format!("\"{}\"", values.line_numbers.name()),
        Field::StickyScroll => values.sticky_scroll.to_string(),
        Field::InlayHints => values.inlay_hints.to_string(),
        Field::Autosave => values.autosave.to_string(),
        Field::Transparency => values.transparency.to_string(),
        Field::Animations => format!("\"{}\"", values.animations.name()),
    };
    Some(format!("  {} = {value},", field.name()))
}

/// Rewrites `init.lua` so the settings block carries `touched` at their values in `values`, and
/// everything else the block already said, unchanged — except `removed`, whose lines are taken out:
/// a setting put back to its default is no longer set, rather than set to today's default.
///
/// **Only what was changed is written.** Writing every live value pinned whatever the session
/// happened to hold — a ctrl-+ zoom for a screen share, a theme toggled by a key, values imported
/// from VSCode — the first time any setting was touched, and since `init.lua` runs last it then
/// overrode a later import too. A line already in the block keeps its text, so an edit made there
/// by hand survives; lines that are not `name = value,` are kept as they are.
///
/// Everything outside the block is preserved verbatim; the block is appended if absent.
/// `init.lua` applies in order, so the block being last is what makes it authoritative.
pub fn write_touched(
    existing: &str,
    values: &Values,
    touched: &[Field],
    removed: &[Field],
) -> String {
    // The block's marker as vex wrote it, too: an init.lua carried over from before the rename has
    // that one, and looking only for the new marker appended a second block on the first save —
    // two `set` calls, the older one silently overridden by the newer.
    let found = existing
        .find(MARKER)
        .or_else(|| existing.find(LEGACY_MARKER))
        .map(|at| {
            let rest = &existing[at..];
            let end = rest
                .find("\n}\n")
                .map(|i| at + i + 3)
                .unwrap_or(existing.len());
            // The lines between `set {` and the closing brace.
            let block = &existing[at..end];
            let body = block
                .find("set {\n")
                .map(|open| &block[open + "set {\n".len()..])
                .map(|body| body.strip_suffix("}\n").unwrap_or(body))
                .unwrap_or("");
            (
                at,
                end,
                body.lines().map(str::to_string).collect::<Vec<_>>(),
            )
        });

    let mut lines = found
        .as_ref()
        .map(|(_, _, lines)| lines.clone())
        .unwrap_or_default();
    let key_of = |line: &str| {
        line.trim()
            .split_once('=')
            .map(|(key, _)| key.trim().to_string())
    };
    lines.retain(|line| {
        key_of(line).is_none_or(|key| !removed.iter().any(|field| field.name() == key))
    });
    // A new line goes in at the top of the body, not after its last line: every line written here
    // ends in its own comma, while a hand-edited last line need not — Lua allows a table's last
    // field without one — and appending after `tab_width = 4` broke the whole file.
    let mut inserted = 0;
    for field in touched {
        let Some(line) = block_line(*field, values) else {
            continue;
        };
        match lines
            .iter()
            .position(|existing| key_of(existing).as_deref() == Some(field.name()))
        {
            Some(at) => lines[at] = line,
            None => {
                lines.insert(inserted, line);
                inserted += 1;
            }
        }
    }

    let mut block = format!("{MARKER}\ndacode.set {{\n");
    for line in &lines {
        block.push_str(line);
        block.push('\n');
    }
    block.push_str("}\n");

    match found {
        // Anything the user added *after* the block is kept, which matters because people do add
        // lines below whatever a tool wrote.
        Some((at, end, _)) => format!("{}{block}{}", &existing[..at], &existing[end..]),
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

// ---- the settings page --------------------------------------------------------------------------
//
// JetBrains' category tree on the left, VSCode's rows on the right — a setting's name, a sentence
// or two under it when the name does not say everything, and its control — and one search box over
// all of it. This half is the model and the geometry; the drawing and the mouse both read
// `Page::geometry_at`, so a click lands on the row that is drawn under the pointer.
//
// What the sidebar panel this replaced printed beside every value — "default", "init.lua", "imported" — is gone. It
// answered where a value came from, which readers took for section names, and it explained
// nothing. What a reader wants from a row is what the setting does and whether they have changed
// it; the explanation says the first, and a ● against the default says the second.

use dacode_input::commands::{self, Command, Group};
use dacode_theme::Role;
use std::cell::{Ref, RefCell};

/// A page of the settings tab, as the tree lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Category {
    #[default]
    Appearance,
    Editor,
    Files,
    Keys,
    Languages,
    Import,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Appearance,
        Category::Editor,
        Category::Files,
        Category::Keys,
        Category::Languages,
        Category::Import,
    ];

    /// The tree's name for the page.
    ///
    /// One word each, deliberately. Below [`SIDE_TREE_MIN`] the tree folds into a strip of three
    /// tiles, where "Import from VSCode" would be elided to nothing useful; the section headings
    /// inside each page ("From VSCode", "Saving") say the rest.
    pub fn title(self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Editor => "Editor",
            Category::Files => "Files",
            Category::Keys => "Keys",
            Category::Languages => "Languages",
            Category::Import => "Import",
        }
    }

    /// The page after this one, wrapping: what a click on the folded tree's one button does, the
    /// same single direction every choice on the page cycles in.
    pub fn next(self) -> Category {
        let at = Category::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Category::ALL[(at + 1) % Category::ALL.len()]
    }
}

/// A heading on a page, and the settings it groups — VSCode's "Editor › Wrapping".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub fields: &'static [Field],
}

// One section for the theme and the type, not one each: a heading "Theme" over a single row also
// called "Theme" was the kind of sub-header that says nothing, which is what was asked to go.
const APPEARANCE: &[Section] = &[
    Section {
        title: "Colours and type",
        fields: &[Field::Theme, Field::FontSize],
    },
    Section {
        title: "Window",
        fields: &[Field::Transparency, Field::Animations],
    },
];

const EDITOR: &[Section] = &[
    Section {
        title: "Editing",
        fields: &[Field::VimKeys, Field::TabWidth],
    },
    Section {
        title: "Wrapping",
        fields: &[Field::SoftWrap],
    },
    Section {
        title: "Display",
        fields: &[Field::LineNumbers, Field::StickyScroll, Field::InlayHints],
    },
];

const FILES: &[Section] = &[Section {
    title: "Saving",
    fields: &[Field::Autosave, Field::FormatOnSave],
}];

/// The settings on a page, under their headings, in order.
///
/// Empty for Keys, Languages and Import: those pages list commands, detected programs and import
/// actions rather than [`Field`]s, and [`Page::entries`] builds them. Every `Field` is in exactly
/// one section of one page, which a test holds — a setting on no page cannot be reached with the
/// mouse, and one on two is two rows to keep in step.
pub fn sections(category: Category) -> &'static [Section] {
    match category {
        Category::Appearance => APPEARANCE,
        Category::Editor => EDITOR,
        Category::Files => FILES,
        Category::Keys | Category::Languages | Category::Import => &[],
    }
}

/// The Keys page's headings: the help panel's groups, capitalised as the other pages' headings are.
fn group_title(group: Group) -> &'static str {
    match group {
        Group::Files => "Files",
        Group::Editing => "Editing",
        Group::Selection => "Selection and cursors",
        Group::Navigation => "Moving around",
        Group::Search => "Finding things",
        Group::Language => "The language server",
        Group::View => "The view",
        Group::Panes => "Panes and panels",
    }
}

/// How a setting is changed on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// On or off.
    Switch,
    /// A number, with a step down and a step up.
    Stepper,
    /// One of a fixed set of names, in the order a click cycles through them.
    Choice(&'static [&'static str]),
    /// One of a set too long to cycle through: a click opens a list of them. The theme's, whose
    /// list is grouped by appearance ([`theme_picks`]); the names are here for measuring.
    Dropdown(&'static [&'static str]),
}

/// The names a choice offers, taken from the lists the values themselves cycle through — written
/// out a second time here they would be a list that disagrees the first time a theme is added.
const THEMES: [&str; Theme::BUILT_IN.len()] = {
    let mut names = [""; Theme::BUILT_IN.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = Theme::BUILT_IN[at].1;
        at += 1;
    }
    names
};

const LINE_NUMBERS: [&str; crate::config::LineNumbers::ALL.len()] = {
    let all = crate::config::LineNumbers::ALL;
    let mut names = [""; crate::config::LineNumbers::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = all[at].name();
        at += 1;
    }
    names
};

const ANIMATIONS: [&str; crate::motion::Animations::ALL.len()] = {
    let all = crate::motion::Animations::ALL;
    let mut names = [""; crate::motion::Animations::ALL.len()];
    let mut at = 0;
    while at < names.len() {
        names[at] = all[at].name();
        at += 1;
    }
    names
};

impl Field {
    /// The row's name, as the page shows it.
    ///
    /// With the modal grammar off the switch stops calling itself Vim: someone who chose the
    /// conventional editor is shown what the switch does rather than a name from an editor they did
    /// not pick. Search still finds it by "vim", through [`Field::name`].
    pub fn label(self, modal: bool) -> &'static str {
        match self {
            Field::VimKeys if modal => "Vim keys",
            Field::VimKeys => "Modal editing",
            Field::FontSize => "Font size",
            Field::TabWidth => "Tab width",
            Field::SoftWrap => "Soft wrap",
            Field::FormatOnSave => "Format on save",
            Field::Theme => "Theme",
            Field::LineNumbers => "Line numbers",
            Field::StickyScroll => "Sticky scroll",
            Field::InlayHints => "Inlay hints",
            Field::Autosave => "Autosave",
            Field::Transparency => "Transparency",
            Field::Animations => "Animations",
        }
    }

    /// What the setting does and what each choice means, the way VSCode writes it under a setting:
    /// a sentence or two, at most three lines of the page (a test holds that, at the largest font).
    ///
    /// `None` where the name says everything — a theme, a font size and a tab width need no
    /// sentence, and one there would only push the rows that do need one further down.
    pub fn explanation(self, modal: bool) -> Option<&'static str> {
        Some(match self {
            Field::Theme | Field::FontSize | Field::TabWidth => return None,
            Field::VimKeys if modal => {
                "Normal, insert and visual modes: keys move and edit until you press i to type. \
                 Off: typing always inserts, as in most editors."
            }
            Field::VimKeys => {
                "On: keys become commands that move and edit, and you switch to a separate mode to \
                 type. Off: typing always inserts, as in most editors."
            }
            Field::SoftWrap => {
                "Long lines continue on the next row instead of scrolling sideways. Only the \
                 display changes; the file is untouched."
            }
            Field::FormatOnSave => {
                "Reformats the file each time it is saved, through the language server or a \
                 formatter such as rustfmt, ruff or prettier."
            }
            Field::LineNumbers if modal => {
                "Relative: each line shows its distance from the cursor, for Vim counts like 12j. \
                 Absolute: each line shows its own number. Off: no numbers."
            }
            Field::LineNumbers => {
                "Absolute: each line shows its own number. Relative: each line shows its distance \
                 from the cursor. Off: no numbers."
            }
            Field::StickyScroll => {
                "Keeps the opening line of each function or block you are inside pinned to the top \
                 of the view while you scroll through it."
            }
            Field::InlayHints => {
                "Shows the types and parameter names the language server works out, inline beside \
                 the code. Needs a server that offers them."
            }
            Field::Autosave => {
                "Saves the file a moment after you stop typing. Off: nothing is written until you \
                 save."
            }
            Field::Transparency => {
                "How much of the desktop shows through the window. Turning it on from opaque takes \
                 effect the next time DaCode starts."
            }
            Field::Animations => {
                "How much the interface moves. Full: panels and popups spring into place. \
                 Reduced: the same movement with no overshoot. Off: everything appears at once."
            }
        })
    }

    pub fn control(self) -> Control {
        match self {
            Field::VimKeys
            | Field::SoftWrap
            | Field::FormatOnSave
            | Field::StickyScroll
            | Field::InlayHints
            | Field::Autosave => Control::Switch,
            Field::FontSize | Field::TabWidth | Field::Transparency => Control::Stepper,
            Field::Theme => Control::Dropdown(&THEMES),
            Field::LineNumbers => Control::Choice(&LINE_NUMBERS),
            Field::Animations => Control::Choice(&ANIMATIONS),
        }
    }

    /// Whether the value is what DaCode would use if nothing had set it — what decides the ● and
    /// the ⟲ beside a row.
    pub fn is_default(self, values: &Values) -> bool {
        !values.differs(self, &Values::defaults(values.modal))
    }

    /// Whether the page offers a ● and a ⟲ for this setting when it differs from its default.
    ///
    /// Not for the grammar. Which editor someone wants is the first question the wizard asks, not a
    /// tweak away from a default, and a ⟲ beside "Modal editing" would turn Vim on in one click for
    /// someone who chose the conventional editor — after which typing no longer types. The switch
    /// is its own way back.
    pub fn resettable(self) -> bool {
        self != Field::VimKeys
    }
}

impl Values {
    /// What every setting is when nothing has set it.
    ///
    /// Read from [`crate::config::Settings::default`], which is what the editor starts from, rather
    /// than written out again — a default stated twice is a ● on a row the user never touched, the
    /// first time one of the two changes. `modal` is the grammar in force, because the gutter's
    /// default follows it exactly as `config::load` does: relative numbers under Vim's grammar,
    /// plain ones without.
    pub fn defaults(modal: bool) -> Values {
        let settings = crate::config::Settings::default();
        Values {
            modal: settings.modal,
            font_size: settings.typography.code_size,
            tab_width: settings.tab_width,
            soft_wrap: settings.soft_wrap,
            format_on_save: settings.format_on_save,
            theme: settings.theme.name(),
            sticky_scroll: settings.sticky_scroll,
            inlay_hints: settings.inlay_hints,
            autosave: settings.autosave,
            line_numbers: crate::config::LineNumbers::default_for(modal),
            transparency: settings.transparency,
            animations: settings.animations,
        }
    }

    /// Whether one field has a different value in `other`.
    pub fn differs(&self, field: Field, defaults: &Values) -> bool {
        match field {
            Field::VimKeys => self.modal != defaults.modal,
            Field::FontSize => self.font_size != defaults.font_size,
            Field::TabWidth => self.tab_width != defaults.tab_width,
            Field::SoftWrap => self.soft_wrap != defaults.soft_wrap,
            Field::FormatOnSave => self.format_on_save != defaults.format_on_save,
            Field::Theme => self.theme != defaults.theme,
            Field::LineNumbers => self.line_numbers != defaults.line_numbers,
            Field::StickyScroll => self.sticky_scroll != defaults.sticky_scroll,
            Field::InlayHints => self.inlay_hints != defaults.inlay_hints,
            Field::Autosave => self.autosave != defaults.autosave,
            Field::Transparency => self.transparency != defaults.transparency,
            Field::Animations => self.animations != defaults.animations,
        }
    }

    /// Puts one setting back to its default: what the ⟲ does.
    pub fn reset(&mut self, field: Field) {
        let defaults = Values::defaults(self.modal);
        self.copy(field, &defaults);
    }

    /// Takes one field's value from `from`.
    pub fn copy(&mut self, field: Field, from: &Values) {
        let defaults = from;
        match field {
            Field::VimKeys => self.modal = defaults.modal,
            Field::FontSize => self.font_size = defaults.font_size,
            Field::TabWidth => self.tab_width = defaults.tab_width,
            Field::SoftWrap => self.soft_wrap = defaults.soft_wrap,
            Field::FormatOnSave => self.format_on_save = defaults.format_on_save,
            Field::Theme => self.theme = defaults.theme,
            Field::LineNumbers => self.line_numbers = defaults.line_numbers,
            Field::StickyScroll => self.sticky_scroll = defaults.sticky_scroll,
            Field::InlayHints => self.inlay_hints = defaults.inlay_hints,
            Field::Autosave => self.autosave = defaults.autosave,
            Field::Transparency => self.transparency = defaults.transparency,
            Field::Animations => self.animations = defaults.animations,
        }
    }
}

impl Line {
    /// What the page calls a row that is not a setting.
    ///
    /// `None` for a setting, whose name is [`Field::label`], and for a detected program, whose text
    /// is the [`ServerRow`] it points at.
    pub fn label(self) -> Option<&'static str> {
        match self {
            // In the page's case rather than as file names; `Importable::name` stays what the old
            // panel and the import report print.
            Line::Import(Importable::Settings) => Some("Settings"),
            Line::Import(Importable::Theme) => Some("Color theme"),
            Line::Import(Importable::Keybindings) => Some("Keybindings"),
            Line::Import(Importable::Snippets) => Some("Snippets"),
            Line::RunImport => Some("Import the ticked items"),
            Line::LoadTheme => Some("Load a colour scheme"),
            Line::Server(_) | Line::Formatter(_) => None,
        }
    }

    /// The sentence under an action, as [`Field::explanation`] is under a setting.
    pub fn explanation(self) -> Option<&'static str> {
        match self {
            Line::RunImport => Some(
                "Copies the ticked items from your VSCode installation into DaCode. Nothing that \
                 is not ticked is touched.",
            ),
            Line::LoadTheme => Some(
                "Reads a colour scheme file in any format the importer understands, and keeps a \
                 copy so it survives a restart.",
            ),
            Line::Import(_) | Line::Server(_) | Line::Formatter(_) => None,
        }
    }
}

/// One entry in a page's list: a heading, or a row under one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A section heading. Not selectable. `category` is set during a search, when the list mixes
    /// pages and the heading has to say which one it came from: "Editor › Wrapping".
    Heading {
        title: &'static str,
        category: Option<Category>,
    },
    Field(Field),
    /// A row of the Keys page.
    Command(&'static Command),
    /// A row of the Languages or Import page.
    Line(Line),
    /// A row that takes you somewhere else: a link, not a setting.
    Link(Link),
}

/// Where a link row goes.
///
/// A variant of its own rather than another [`Line`], because it is not an import or a report and
/// the page treats it differently from both: it has no control, it is chosen like an action, and
/// what choosing it does — opening a file — belongs to the editor rather than to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The Keys page's way to change a shortcut, which the page itself does not offer: rebinding from
    /// the list is out of scope, and a list of keys with no word on how to change one reads as a
    /// list of keys that cannot be changed.
    RebindKeys,
}

impl Link {
    pub fn label(self) -> &'static str {
        match self {
            Link::RebindKeys => "Rebind a key in init.lua",
        }
    }

    /// What choosing it does, said on the status line when it is chosen — the row itself is one line,
    /// so the first row of the Keys page fits under the search box at the smallest sizes. Searched,
    /// so "dacode.map" finds the row.
    pub fn explanation(self) -> &'static str {
        match self {
            Link::RebindKeys => "a key is rebound with dacode.map(mode, keys, command) in init.lua",
        }
    }
}

impl Entry {
    fn is_heading(self) -> bool {
        matches!(self, Entry::Heading { .. })
    }
}

/// Which part of a row a point is over: what a click there does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The row itself: select it. For a setting, a click on the row away from the control only
    /// selects; for an import item or an action it is the action.
    Row,
    /// A setting's switch, stepper or choice, or an import item's tick box.
    Control,
    /// The ⟲.
    Reset,
    /// A stepper's − half: one step down.
    Decrease,
    /// A stepper's + half: one step up.
    Increase,
}

/// The settings tab's state: the open page, the search, the selection and the scroll.
///
/// **The fields are private so the model can keep its own invariants**: a new query or page resets
/// the scroll and puts the selection on the list's first row, a scroll is clamped to the list, and
/// a selection is never on a heading. With the fields public, typing a query while scrolled down
/// the Keys page left the scroll pointing past a four-row result, and the page went blank.
///
/// `scroll` is a pixel offset into the list, below the search box — the *target*, where the page
/// is going. The page scrolls through a spring (`motion::Key::SettingsScroll`), and drawing asks for
/// [`Page::geometry_at`] wherever the spring has got to.
///
/// What a frame needs from the list — the entries, their heights and offsets, which pages match —
/// is built once per page, query, grammar and width and kept in `flow`, so laying the page out
/// costs the rows on screen, not the ninety-odd commands on the Keys page.
#[derive(Debug, Clone)]
pub struct Page {
    category: Category,
    query: String,
    scroll: f32,
    selected: usize,
    servers: Vec<ServerRow>,
    formatters: Vec<ServerRow>,
    /// Which import items are ticked. Nothing, until asked — the same default the CLI has.
    ticked: Vec<Importable>,
    /// The theme dropdown, while it is open. On the page because it hangs from the page's theme
    /// row, and a page that is closed takes it with it.
    dropdown: Option<Dropdown>,
    flow: RefCell<Option<Flow>>,
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

/// Where everything on the page is. Read by the drawing and by the mouse, so they cannot disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct PageGeometry {
    pub search: Rect,
    /// Where the categories are. Six rows of a column on the left when the page has the room;
    /// otherwise the tree is **folded** into one button beside the search box, naming the open page,
    /// and a click on it moves to [`Category::next`] — every page is still a click away, which a
    /// strip of six tiles was not: it took two rows of the page at 640×480 and left none for
    /// settings.
    pub tree: Vec<(Category, Rect)>,
    pub folded: bool,
    /// The scrolled region under the search box. Drawing clips to it (`Frame::set_clip`); rows are
    /// placed at the scroll offset, so the first and last can lie partly outside it.
    pub list: Rect,
    /// The offset the rows are placed at, after clamping.
    pub scroll: f32,
    /// How far the list can scroll: its height less the room for it, never negative.
    pub max_scroll: f32,
    /// Every row that is at least partly in `list`, in order.
    pub rows: Vec<PageRow>,
    /// Where to say there is nothing to show — a search with no match, or no reports gathered —
    /// when the list is empty.
    pub empty: Option<Rect>,
}

/// One visible row of the page and the boxes inside it.
///
/// The boxes are where the row is laid out at the scroll offset, unclipped, so text is placed from
/// the box it was measured for; `clip` is the part of the row that is on screen. [`PageGeometry::hit`]
/// tests against the visible parts only — use it rather than these boxes for the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRow {
    pub entry: Entry,
    /// Its position in [`Page::entries`], which [`Page::selected`] counts in.
    pub index: usize,
    /// The whole row, for the selection highlight.
    pub rect: Rect,
    /// `rect` cut to the list: what is on screen.
    pub clip: Rect,
    /// The name. For a heading, the line its text sits on, below the space that separates it from
    /// the section before. For a detected program, the language and the program.
    pub label: Rect,
    /// Wrapped text under the name: a setting's or an action's explanation, exactly as many lines
    /// tall as it wraps to (wrap it with `chrome::wrap_at` at this width). For a detected program,
    /// the line saying where it is or how to install it.
    pub explanation: Option<Rect>,
    /// A setting's switch, stepper or choice; an import item's tick box. For a command, where its
    /// keys are drawn — not a control, since rebinding from the page is out of scope. Capped at half
    /// the row, so a long choice ("rose-pine-moon") is elided inside it, and a command's chips drop
    /// the alternatives that do not fit rather than squeezing one.
    pub control: Option<Rect>,
    /// The ● and, on hover, the ⟲: present only when a setting differs from its default and
    /// [`Field::resettable`] allows it.
    pub reset: Option<Rect>,
    /// The theme row's colour chip, inside `control` at its left: the theme shown rather than only
    /// named. The control is measured with room for it, so the name beside it is not cut shorter.
    pub swatch: Option<Rect>,
    /// A stepper's − and + buttons, at the two ends of `control`; its value sits between them.
    /// Boxes of their own because they go in opposite directions: a click on either was a click on
    /// "the control", which only knew one way, so − raised the font size.
    pub stepper: Option<(Rect, Rect)>,
}

impl PageRow {
    pub fn field(&self) -> Option<Field> {
        match self.entry {
            Entry::Field(field) => Some(field),
            _ => None,
        }
    }

    pub fn heading(&self) -> Option<&'static str> {
        match self.entry {
            Entry::Heading { title, .. } => Some(title),
            _ => None,
        }
    }

    /// A heading as drawn: "Wrapping" on its own page, "Editor › Wrapping" in a search.
    pub fn heading_text(&self) -> Option<String> {
        match self.entry {
            Entry::Heading {
                title,
                category: Some(category),
            } => Some(format!("{} › {title}", category.title())),
            Entry::Heading { title, .. } => Some(title.to_string()),
            _ => None,
        }
    }
}

impl PageGeometry {
    /// The row and part under a point, if any — the one hit test, against what is on screen.
    ///
    /// A part of a row scrolled out of the list is not hit, and a heading is not a target.
    pub fn hit(&self, x: f32, y: f32) -> Option<(usize, Part)> {
        if !contains(self.list, x, y) {
            return None;
        }
        let row = self
            .rows
            .iter()
            .find(|row| contains(row.clip, x, y) && !row.entry.is_heading())?;
        let part = if row.reset.is_some_and(|reset| contains(reset, x, y)) {
            Part::Reset
        } else if row.stepper.is_some_and(|(down, _)| contains(down, x, y)) {
            Part::Decrease
        } else if row.stepper.is_some_and(|(_, up)| contains(up, x, y)) {
            Part::Increase
        } else if !matches!(row.entry, Entry::Command(_))
            && row.control.is_some_and(|control| contains(control, x, y))
        {
            Part::Control
        } else {
            Part::Row
        };
        Some((row.index, part))
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

/// Where `needle` occurs in `text`, ignoring case, as ranges of *characters* — what the drawing
/// needs to place a wash under each, at a character's advance. The same case-folded `contains` the
/// search filters with, so what is lit is what made the row match. An empty needle matches nothing.
pub fn match_ranges(text: &str, needle: &str) -> Vec<std::ops::Range<usize>> {
    let needle: Vec<char> = needle.trim().chars().flat_map(char::to_lowercase).collect();
    let haystack: Vec<char> = text.chars().collect();
    let folded: Vec<char> = haystack
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut ranges = Vec::new();
    if needle.is_empty() || needle.len() > folded.len() {
        return ranges;
    }
    let mut at = 0;
    while at + needle.len() <= folded.len() {
        if folded[at..at + needle.len()] == needle[..] {
            ranges.push(at..at + needle.len());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    ranges
}

/// What the folded tree's button says during a search. Measured with the page titles when the button
/// is sized, so it is never cut to "All resul…".
const ALL_RESULTS: &str = "All results";

/// The widest the list gets. Past this a line of explanation runs to a hundred characters, too long
/// for the eye to find the start of the next one.
const PAGE_WIDTH: f32 = 640.0;

/// Below this width the tree is folded into one button beside the search box, so the rows keep
/// enough width for a name and a control side by side. It is folded too when the page is not tall
/// enough for its six rows.
const SIDE_TREE_MIN: f32 = 560.0;

/// The tree column's width, as a share of the page and within these bounds.
const TREE_SHARE: f32 = 0.22;
const TREE_MIN: f32 = 150.0;
const TREE_MAX: f32 = 210.0;

/// The page's measurements, from the type scale so a larger font grows the rows rather than
/// overflowing them.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    scale: f32,
    /// A name, in the interface face.
    label_line: f32,
    /// One line of an explanation.
    text_line: f32,
    ui_advance: f32,
    text_advance: f32,
    /// A control's height, the head of a setting row, a tree row and the search box.
    control: f32,
    /// Horizontal padding inside a row.
    inset: f32,
    /// Vertical padding inside a row, and the gap between boxes.
    gap: f32,
    small: f32,
    heading_space: f32,
}

impl Metrics {
    fn new(typography: &Typography, scale: f32) -> Self {
        let label_line = (typography.spec(Role::Ui).size_px * 1.5).ceil() * scale;
        Self {
            scale,
            label_line,
            text_line: (typography.spec(Role::Value).size_px * 1.5).ceil() * scale,
            ui_advance: crate::layout::measured_advance(typography, Role::Ui, scale),
            text_advance: crate::layout::measured_advance(typography, Role::Value, scale),
            control: label_line + space::XS * scale,
            inset: space::SM * scale,
            gap: space::XS * scale,
            small: space::XXS * scale,
            heading_space: space::MD * scale,
        }
    }

    /// How wide a setting's control wants to be: a switch is a fixed pill, a stepper holds the value
    /// between its two buttons, and a choice is as wide as its longest name — or the current value,
    /// which for a theme loaded from a file is "from a file" and on no list.
    fn control_width(&self, field: Field, values: &Values) -> f32 {
        match field.control() {
            Control::Switch => self.control * 1.8,
            Control::Stepper => self.control * 2.0 + self.ui_advance * 6.0 + self.gap * 2.0,
            Control::Choice(names) | Control::Dropdown(names) => {
                let longest = names
                    .iter()
                    .map(|name| name.chars().count())
                    .chain([values.get(field).chars().count()])
                    .max()
                    .unwrap_or(0);
                let swatch = if field == Field::Theme {
                    self.swatch() + self.gap
                } else {
                    0.0
                };
                longest as f32 * self.ui_advance + self.control + self.gap * 2.0 + swatch
            }
        }
    }

    /// The side of the theme's colour chip.
    fn swatch(&self) -> f32 {
        (self.control * 0.55).round()
    }

    /// How many lines `text` wraps to in `width`, by the same function the drawing wraps it with.
    fn lines(&self, text: &str, width: f32) -> usize {
        crate::chrome::wrap_at(text, width, self.text_advance).len()
    }

    /// A row's height in a list whose rows are `inner` wide inside their padding, and how many
    /// lines its text under the name takes.
    ///
    /// The first heading has no space above it, and on a folded page none at all: the folded tree's
    /// button already names the page, and those pixels are the difference between a setting fitting
    /// under the search box and not. A search's first heading is kept, folded or not: the button
    /// then says "All results", and only the heading says which page "Soft wrap" came from.
    fn height(&self, entry: Entry, leading: bool, folded: bool, inner: f32, modal: bool) -> Slot {
        let explained = |text: Option<&str>| text.map_or(0, |text| self.lines(text, inner));
        let (height, lines) = match entry {
            Entry::Heading { category: None, .. } if leading && folded => (0.0, 0),
            Entry::Heading { .. } if leading => (self.label_line + self.small, 0),
            Entry::Heading { .. } => (self.heading_space + self.label_line, 0),
            Entry::Command(_) => (self.label_line + self.gap, 0),
            Entry::Line(Line::Server(_) | Line::Formatter(_)) => {
                (self.gap * 2.0 + self.label_line + self.text_line, 1)
            }
            Entry::Field(_) | Entry::Line(_) | Entry::Link(_) => {
                let lines = match entry {
                    Entry::Field(field) => explained(field.explanation(modal)),
                    Entry::Line(line) => explained(line.explanation()),
                    // One line, with no sentence under it: it heads the Keys page, and at the
                    // smallest sizes the first row has to fit under the search box whole. What it
                    // does is said when it is chosen.
                    Entry::Link(_) => 0,
                    Entry::Heading { .. } | Entry::Command(_) => 0,
                };
                let text = if lines > 0 {
                    self.small + lines as f32 * self.text_line
                } else {
                    0.0
                };
                (self.gap * 2.0 + self.control + text, lines)
            }
        };
        Slot {
            top: 0.0,
            height,
            lines,
        }
    }
}

/// Where the page's fixed parts are — everything but the rows, which [`Flow`] places.
struct Skeleton {
    metrics: Metrics,
    folded: bool,
    tree: Vec<(Category, Rect)>,
    search: Rect,
    list: Rect,
}

impl Skeleton {
    fn new(rect: Rect, scale: f32, typography: &Typography, category: Category) -> Self {
        let metrics = Metrics::new(typography, scale);
        let rows = Category::ALL.len() as f32;
        let side_pad = space::LG * scale;
        let tree_height = side_pad * 2.0 + rows * metrics.control + (rows - 1.0) * metrics.small;
        let folded = rect.width < SIDE_TREE_MIN * scale || rect.height < tree_height;

        let pad = if folded { space::SM } else { space::LG } * scale;
        let bottom = (rect.bottom() - pad).max(rect.y);
        let top = (rect.y + pad).min(bottom);
        // Never taller than the page: the search box stays inside it at any size.
        let search_height = metrics.control.min(bottom - top);

        let mut tree = Vec::with_capacity(Category::ALL.len());
        let (x, width, search) = if folded {
            let x = rect.x + pad;
            let width = (rect.width - pad * 2.0).clamp(0.0, PAGE_WIDTH * scale);
            let longest = Category::ALL
                .iter()
                .map(|category| category.title())
                .chain([ALL_RESULTS])
                .map(|title| title.chars().count())
                .max()
                .unwrap_or(0);
            let button =
                (longest as f32 * metrics.ui_advance + metrics.control + metrics.gap * 2.0)
                    .min(width * 0.4);
            tree.push((category, Rect::new(x, top, button, search_height)));
            let search_x = x + button + metrics.gap;
            let search = Rect::new(
                search_x,
                top,
                (x + width - search_x).max(0.0),
                search_height,
            );
            (x, width, search)
        } else {
            let column = (rect.width * TREE_SHARE).clamp(TREE_MIN * scale, TREE_MAX * scale);
            for (at, category) in Category::ALL.into_iter().enumerate() {
                tree.push((
                    category,
                    Rect::new(
                        rect.x + metrics.gap,
                        top + at as f32 * (metrics.control + metrics.small),
                        (column - metrics.gap * 2.0).max(0.0),
                        metrics.control,
                    ),
                ));
            }
            let x = rect.x + column + pad;
            let width = (rect.right() - pad - x).clamp(0.0, PAGE_WIDTH * scale);
            (x, width, Rect::new(x, top, width, search_height))
        };

        let list_top = (search.bottom() + metrics.gap * if folded { 1.0 } else { 2.0 }).min(bottom);
        Self {
            metrics,
            folded,
            tree,
            search,
            list: Rect::new(x, list_top, width, bottom - list_top),
        }
    }
}

/// One entry's place in the list: its offset from the list's top, its height, and how many lines of
/// text it has under its name.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    top: f32,
    height: f32,
    lines: usize,
}

/// What the list's layout depends on. A change to any of these rebuilds the [`Flow`]; nothing else
/// does.
#[derive(Debug, Clone, PartialEq)]
struct FlowKey {
    category: Category,
    query: String,
    modal: bool,
    scale: f32,
    code_size: f32,
    width: f32,
    folded: bool,
}

/// The list, laid out once: its entries, where each one is, and which pages match the query.
#[derive(Debug, Clone)]
struct Flow {
    key: FlowKey,
    entries: Vec<Entry>,
    slots: Vec<Slot>,
    total: f32,
    matches: [bool; Category::ALL.len()],
    /// The defaults the ● is decided against, which follow the grammar and nothing else.
    defaults: Values,
}

impl Page {
    pub fn new() -> Self {
        Self {
            category: Category::Appearance,
            query: String::new(),
            scroll: 0.0,
            // Every page opens with a heading, and none is empty but Languages before its reports
            // are gathered — `set_reports` puts the selection right for that one.
            selected: 1,
            servers: Vec::new(),
            formatters: Vec::new(),
            ticked: Vec::new(),
            dropdown: None,
            flow: RefCell::new(None),
        }
    }

    pub fn category(&self) -> Category {
        self.category
    }

    /// Opens a page: from its top, with its first row selected. A query is left as it is — while
    /// one is typed the list is the search's, and the tree only marks the page.
    pub fn set_category(&mut self, category: Category, modal: bool) {
        self.category = category;
        self.restart(modal);
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Changes the search: the list starts again from its top with its first row selected, because
    /// the rows the scroll and the selection counted in are gone.
    pub fn set_query(&mut self, query: impl Into<String>, modal: bool) {
        self.query = query.into();
        self.restart(modal);
    }

    pub fn servers(&self) -> &[ServerRow] {
        &self.servers
    }

    pub fn formatters(&self) -> &[ServerRow] {
        &self.formatters
    }

    /// Hands the Languages page its reports, gathered by [`detect`] and [`detect_formatters`] when
    /// the tab opens — never while drawing, since finding a program walks `PATH`.
    pub fn set_reports(
        &mut self,
        servers: Vec<ServerRow>,
        formatters: Vec<ServerRow>,
        modal: bool,
    ) {
        self.servers = servers;
        self.formatters = formatters;
        *self.flow.get_mut() = None;
        self.select(self.selected, modal);
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn is_ticked(&self, item: Importable) -> bool {
        self.ticked.contains(&item)
    }

    /// Ticks an import item, or unticks it when it already is.
    pub fn tick(&mut self, item: Importable) {
        if let Some(at) = self.ticked.iter().position(|ticked| *ticked == item) {
            self.ticked.remove(at);
        } else {
            self.ticked.push(item);
        }
    }

    /// The checklist to hand the importer: what is ticked, and nothing else.
    pub fn checklist(&self) -> dacode_import::Checklist {
        self.ticked
            .iter()
            .fold(dacode_import::Checklist::nothing(), |list, item| {
                list.with(item.item())
            })
    }

    /// What the folded tree's one button says: the open page, or — while a search is showing rows
    /// from every page — that it is. Naming one page over a list that mixes all six read as though
    /// "Soft wrap" belonged under "Appearance".
    pub fn folded_label(&self) -> &'static str {
        if self.needle().is_some() {
            ALL_RESULTS
        } else {
            self.category.title()
        }
    }

    /// The selected entry, or `None` on a list with nothing to select.
    pub fn selected_entry(&self, modal: bool) -> Option<Entry> {
        self.entries(modal)
            .get(self.selected)
            .copied()
            .filter(|entry| !entry.is_heading())
    }

    /// The scroll target, in pixels from the top of the list.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    fn restart(&mut self, modal: bool) {
        // The row it hangs from may be gone from the new list.
        self.dropdown = None;
        self.scroll = 0.0;
        self.selected = 0;
        self.move_selection(0, modal);
    }

    /// The query as it is matched: trimmed, lower case, and `None` when there is nothing to find.
    fn needle(&self) -> Option<String> {
        let needle = self.query.trim().to_lowercase();
        (!needle.is_empty()).then_some(needle)
    }

    /// A page's rows under their headings, before any search.
    fn groups(&self, category: Category, modal: bool) -> Vec<(&'static str, Vec<Entry>)> {
        let lines = |lines: Vec<Line>| lines.into_iter().map(Entry::Line).collect::<Vec<_>>();
        match category {
            Category::Appearance | Category::Editor | Category::Files => sections(category)
                .iter()
                .map(|section| {
                    (
                        section.title,
                        section.fields.iter().copied().map(Entry::Field).collect(),
                    )
                })
                .collect(),
            // `available` rather than the whole table: with the grammar off there is no visual
            // mode to list a key for.
            // The way to change a key first, where someone who came to change one looks; then every
            // command under the help panel's groups.
            Category::Keys => std::iter::once(("Rebinding", vec![Entry::Link(Link::RebindKeys)]))
                .chain(Group::ALL.into_iter().map(|group| {
                    (
                        group_title(group),
                        commands::available(modal)
                            .filter(|command| command.group == group)
                            .map(Entry::Command)
                            .collect(),
                    )
                }))
                .collect(),
            Category::Languages => vec![
                (
                    "Language servers",
                    lines((0..self.servers.len()).map(Line::Server).collect()),
                ),
                (
                    "Formatters",
                    lines((0..self.formatters.len()).map(Line::Formatter).collect()),
                ),
            ],
            Category::Import => vec![
                (
                    "From VSCode",
                    lines(
                        Importable::ALL
                            .into_iter()
                            .map(Line::Import)
                            .chain([Line::RunImport])
                            .collect(),
                    ),
                ),
                ("From a file", lines(vec![Line::LoadTheme])),
            ],
        }
    }

    /// The report a Languages row points at.
    fn report(&self, line: Line) -> Option<&ServerRow> {
        match line {
            Line::Server(at) => self.servers.get(at),
            Line::Formatter(at) => self.formatters.get(at),
            _ => None,
        }
    }

    /// Whether an entry matches `needle`, which is already lower case.
    ///
    /// Anything matches by the page and the section it is under, as VSCode's "editor" lists the
    /// editor's settings. A setting matches by its name, its explanation and its config key — so
    /// "vim" still finds the modal switch when its label no longer says Vim, and someone copying a
    /// line out of `init.lua` finds the row it sets. A command matches by its name, what it does
    /// and its keys; a report by its language and program.
    fn entry_matches(
        &self,
        entry: Entry,
        (category, section): (Category, &str),
        needle: &str,
        modal: bool,
    ) -> bool {
        let found = |text: &str| text.to_lowercase().contains(needle);
        if entry.is_heading() {
            return false;
        }
        if found(category.title()) || found(section) {
            return true;
        }
        match entry {
            Entry::Heading { .. } => false,
            Entry::Field(field) => {
                found(field.label(modal))
                    || field.explanation(modal).is_some_and(found)
                    || found(field.name())
            }
            Entry::Command(command) => {
                found(command.name) || found(command.what) || found(&command.shown(modal))
            }
            Entry::Link(link) => found(link.label()) || found(link.explanation()),
            Entry::Line(line) => {
                line.label().is_some_and(found)
                    || line.explanation().is_some_and(found)
                    || self
                        .report(line)
                        .is_some_and(|row| found(&row.language) || found(&row.program))
            }
        }
    }

    /// Every entry of the list, headings included, in order — built fresh; drawing reads the copy
    /// cached with the layout.
    ///
    /// With no query, the open page's sections. With one, the matching rows of **every** page,
    /// each group under a heading naming its page and section, and a page with nothing matching
    /// contributes nothing at all — it collapses, and [`Page::has_matches`] lets the tree dim it.
    /// The tree keeps all six rows during a search: hiding them would move the ones left under the
    /// pointer as the query is typed.
    pub fn entries(&self, modal: bool) -> Vec<Entry> {
        let mut entries = Vec::new();
        let Some(needle) = self.needle() else {
            for (title, items) in self.groups(self.category, modal) {
                // A heading over nothing reads as a section that failed to load.
                if !items.is_empty() {
                    entries.push(Entry::Heading {
                        title,
                        category: None,
                    });
                    entries.extend(items);
                }
            }
            return entries;
        };
        for category in Category::ALL {
            for (title, items) in self.groups(category, modal) {
                let mut found = items
                    .into_iter()
                    .filter(|entry| self.entry_matches(*entry, (category, title), &needle, modal))
                    .peekable();
                if found.peek().is_some() {
                    entries.push(Entry::Heading {
                        title,
                        category: Some(category),
                    });
                    entries.extend(found);
                }
            }
        }
        entries
    }

    fn compute_has_matches(&self, category: Category, modal: bool) -> bool {
        let Some(needle) = self.needle() else {
            return true;
        };
        self.groups(category, modal)
            .into_iter()
            .any(|(title, items)| {
                items
                    .into_iter()
                    .any(|entry| self.entry_matches(entry, (category, title), &needle, modal))
            })
    }

    /// Whether a page has anything for the query — always, with no query. The tree dims a page
    /// that does not. Read from the layout's cache when it was built for this query.
    pub fn has_matches(&self, category: Category, modal: bool) -> bool {
        let at = Category::ALL
            .iter()
            .position(|other| *other == category)
            .unwrap_or(0);
        if let Some(flow) = self.flow.borrow().as_ref()
            && flow.key.query == self.query
            && flow.key.modal == modal
        {
            return flow.matches[at];
        }
        self.compute_has_matches(category, modal)
    }

    /// Whether a setting matches the query: case-insensitively, by its label, its explanation, its
    /// page and section, and its config key. Everything matches an empty query.
    pub fn matches(&self, field: Field, modal: bool) -> bool {
        let Some(needle) = self.needle() else {
            return true;
        };
        let home = Category::ALL.into_iter().find_map(|category| {
            sections(category)
                .iter()
                .find(|section| section.fields.contains(&field))
                .map(|section| (category, section.title))
        });
        let home = home.unwrap_or((Category::Appearance, ""));
        self.entry_matches(Entry::Field(field), home, &needle, modal)
    }

    /// Selects an entry — what a click on a row does — or the nearest row after it when it is a
    /// heading, or the last row when it is past the end. On a list with nothing to select the
    /// selection is 0, and [`Page::selected_entry`] says there is none.
    pub fn select(&mut self, index: usize, modal: bool) {
        self.selected = index;
        self.move_selection(0, modal);
    }

    /// Moves the selection `delta` rows, stepping over headings and stopping at either end. Follow
    /// it with [`Page::reveal`], which scrolls the page to keep the selection in view.
    ///
    /// A selection resting on a heading counts as the row after it, so `delta` 0 puts it somewhere
    /// it can be used.
    pub fn move_selection(&mut self, delta: isize, modal: bool) {
        let selectable: Vec<usize> = self
            .entries(modal)
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_heading())
            .map(|(index, _)| index)
            .collect();
        let Some(last) = selectable.len().checked_sub(1) else {
            self.selected = 0;
            return;
        };
        let at = selectable
            .iter()
            .position(|index| *index >= self.selected)
            .unwrap_or(last);
        let to = at.saturating_add_signed(delta).min(last);
        self.selected = selectable[to];
    }

    fn key(&self, skeleton: &Skeleton, modal: bool, typography: &Typography) -> FlowKey {
        FlowKey {
            category: self.category,
            query: self.query.clone(),
            modal,
            scale: skeleton.metrics.scale,
            code_size: typography.code_size,
            width: skeleton.list.width,
            folded: skeleton.folded,
        }
    }

    /// The list laid out for this skeleton, built only when something it depends on changed.
    fn flow(&self, skeleton: &Skeleton, modal: bool, typography: &Typography) -> Ref<'_, Flow> {
        let key = self.key(skeleton, modal, typography);
        let stale = self
            .flow
            .borrow()
            .as_ref()
            .is_none_or(|flow| flow.key != key);
        if stale {
            let built = self.build(key, &skeleton.metrics);
            *self.flow.borrow_mut() = Some(built);
        }
        Ref::map(self.flow.borrow(), |flow| {
            flow.as_ref().expect("the flow was built just above")
        })
    }

    fn build(&self, key: FlowKey, metrics: &Metrics) -> Flow {
        let entries = self.entries(key.modal);
        let inner = (key.width - metrics.inset * 2.0).max(0.0);
        let mut top = 0.0;
        let slots = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let slot = Slot {
                    top,
                    ..metrics.height(*entry, index == 0, key.folded, inner, key.modal)
                };
                top += slot.height;
                slot
            })
            .collect();
        Flow {
            matches: Category::ALL.map(|category| self.compute_has_matches(category, key.modal)),
            defaults: Values::defaults(key.modal),
            entries,
            slots,
            total: top,
            key,
        }
    }

    /// How far the list can scroll at this size.
    pub fn max_scroll(&self, rect: Rect, scale: f32, modal: bool, typography: &Typography) -> f32 {
        let skeleton = Skeleton::new(rect, scale, typography, self.category);
        let flow = self.flow(&skeleton, modal, typography);
        (flow.total - skeleton.list.height).max(0.0)
    }

    /// Scrolls the target by `delta` pixels, clamped to the list: what the wheel does.
    pub fn scroll_by(
        &mut self,
        delta: f32,
        rect: Rect,
        scale: f32,
        modal: bool,
        typography: &Typography,
    ) {
        let max = self.max_scroll(rect, scale, modal, typography);
        let to = self.scroll + if delta.is_finite() { delta } else { 0.0 };
        self.scroll = to.clamp(0.0, max);
    }

    /// Scrolls by as little as it takes to show the selected row whole — and its heading with it,
    /// when it is the first row of a section and both fit. A row taller than the list is shown
    /// from its top.
    ///
    /// The heights are the layout's own, so the arrow keys and the drawing cannot disagree about
    /// where a row is.
    pub fn reveal(&mut self, rect: Rect, scale: f32, modal: bool, typography: &Typography) {
        let skeleton = Skeleton::new(rect, scale, typography, self.category);
        let view = skeleton.list.height;
        let scroll = {
            let flow = self.flow(&skeleton, modal, typography);
            let max = (flow.total - view).max(0.0);
            let Some(slot) = flow.slots.get(self.selected).copied() else {
                let clamped = self.scroll.clamp(0.0, max);
                drop(flow);
                self.scroll = clamped;
                return;
            };
            let heading = self
                .selected
                .checked_sub(1)
                .and_then(|above| Some((flow.entries.get(above)?, flow.slots.get(above)?)))
                .filter(|(entry, _)| entry.is_heading())
                .map(|(_, above)| above.top);
            let bottom = slot.top + slot.height;
            let mut scroll = self.scroll;
            if bottom > scroll + view {
                scroll = bottom - view;
            }
            if slot.top < scroll || slot.height > view {
                scroll = slot.top;
            }
            if let Some(heading) = heading
                && heading < scroll
                && bottom - heading <= view
            {
                scroll = heading;
            }
            scroll.clamp(0.0, max)
        };
        self.scroll = scroll;
    }

    /// Lays the page out at its scroll target. See [`Page::geometry_at`].
    pub fn geometry(
        &self,
        rect: Rect,
        scale: f32,
        values: &Values,
        modal: bool,
        typography: &Typography,
    ) -> PageGeometry {
        self.geometry_at(self.scroll, rect, scale, values, modal, typography)
    }

    /// Lays the page out with the list scrolled by `offset` pixels — where the scroll spring has
    /// got to, while it moves.
    ///
    /// **One definition, read by the drawing and by the mouse**, as [`panel`] is and for the same
    /// reason. The list's entries and heights come from the cached [`Flow`]; a call finds the first
    /// visible row by binary search and lays out only the rows that touch the list, so its cost is
    /// the rows on screen whatever page or search is showing.
    pub fn geometry_at(
        &self,
        offset: f32,
        rect: Rect,
        scale: f32,
        values: &Values,
        modal: bool,
        typography: &Typography,
    ) -> PageGeometry {
        debug_assert_eq!(
            modal, values.modal,
            "the grammar the text is written for and the one the defaults follow must be one"
        );
        let skeleton = Skeleton::new(rect, scale, typography, self.category);
        let flow = self.flow(&skeleton, modal, typography);
        let list = skeleton.list;
        let metrics = skeleton.metrics;
        let max_scroll = (flow.total - list.height).max(0.0);
        let scroll = if offset.is_finite() {
            offset.clamp(0.0, max_scroll)
        } else {
            0.0
        };

        let layout = Layout {
            metrics: &metrics,
            values,
            defaults: &flow.defaults,
            modal,
            typography,
            list,
        };
        let first = flow
            .slots
            .partition_point(|slot| slot.top + slot.height <= scroll);
        let mut rows = Vec::new();
        for (index, (entry, slot)) in flow.entries.iter().zip(&flow.slots).enumerate().skip(first) {
            let y = list.y + slot.top - scroll;
            if y >= list.bottom() {
                break;
            }
            if slot.height > 0.0 {
                rows.push(layout.row(
                    *entry,
                    index,
                    Rect::new(list.x, y, list.width, slot.height),
                    slot.lines,
                ));
            }
        }

        let empty = flow.entries.is_empty().then(|| {
            Rect::new(
                list.x + metrics.inset,
                list.y,
                (list.width - metrics.inset * 2.0).max(0.0),
                metrics.label_line.min(list.height),
            )
        });

        PageGeometry {
            search: skeleton.search,
            tree: skeleton.tree,
            folded: skeleton.folded,
            list,
            scroll,
            max_scroll,
            rows,
            empty,
        }
    }
}

/// What laying out one row needs to know.
struct Layout<'a> {
    metrics: &'a Metrics,
    values: &'a Values,
    defaults: &'a Values,
    modal: bool,
    typography: &'a Typography,
    list: Rect,
}

impl Layout<'_> {
    /// The boxes inside one row, which is `rect`, with `lines` lines of text under its name.
    fn row(&self, entry: Entry, index: usize, rect: Rect, lines: usize) -> PageRow {
        let metrics = self.metrics;
        let x = rect.x + metrics.inset;
        let inner = (rect.width - metrics.inset * 2.0).max(0.0);
        let mut row = PageRow {
            entry,
            index,
            rect,
            clip: rect.intersect(self.list),
            label: Rect::new(x, rect.y, inner, 0.0),
            explanation: None,
            control: None,
            reset: None,
            swatch: None,
            stepper: None,
        };

        let text =
            |y: f32| (lines > 0).then(|| Rect::new(x, y, inner, lines as f32 * metrics.text_line));
        // A box at the right-hand end of the row's head, at most half the row and leaving `reserve`
        // to its left for the ⟲.
        let right = |y: f32, width: f32, height: f32, reserve: f32| {
            let width = width.min(inner * 0.5).min((inner - reserve).max(0.0));
            Rect::new(x + inner - width, y, width, height)
        };

        match entry {
            Entry::Heading { .. } => {
                let height = metrics.label_line.min(rect.height);
                row.label = Rect::new(x, rect.bottom() - height, inner, height);
            }
            Entry::Command(command) => {
                let y = rect.y + metrics.small;
                let keys = crate::layout::chips_width(
                    self.typography,
                    &command.shown(self.modal),
                    metrics.scale,
                );
                // A command with no key in this grammar has nothing to draw there; the palette is
                // its only way in, and the empty space says so.
                row.control = (keys > 0.0).then(|| right(y, keys, metrics.label_line, 0.0));
                let end = row.control.map_or(x + inner, |keys| keys.x - metrics.gap);
                row.label = Rect::new(x, y, (end - x).max(0.0), metrics.label_line);
            }
            Entry::Line(Line::Server(_) | Line::Formatter(_)) => {
                let y = rect.y + metrics.gap;
                row.label = Rect::new(x, y, inner, metrics.label_line);
                row.explanation = text(y + metrics.label_line);
            }
            Entry::Field(field) => {
                let y = rect.y + metrics.gap;
                let reset = field.resettable() && self.values.differs(field, self.defaults);
                let reserve = if reset {
                    metrics.control + metrics.gap
                } else {
                    0.0
                };
                let control = right(
                    y,
                    metrics.control_width(field, self.values),
                    metrics.control,
                    reserve,
                );
                row.control = Some(control);
                row.stepper = (field.control() == Control::Stepper).then(|| {
                    let side = control.height.min(control.width / 3.0);
                    (
                        Rect::new(control.x, control.y, side, control.height),
                        Rect::new(control.right() - side, control.y, side, control.height),
                    )
                });
                row.swatch = (field == Field::Theme).then(|| {
                    let side = metrics.swatch().min(control.height).min(control.width);
                    Rect::new(
                        control.x + metrics.gap,
                        control.y + (control.height - side) / 2.0,
                        side,
                        side,
                    )
                });
                row.reset = reset.then(|| {
                    Rect::new(
                        control.x - metrics.gap - metrics.control,
                        y,
                        metrics.control,
                        metrics.control,
                    )
                });
                let end = row.reset.unwrap_or(control).x - metrics.gap;
                row.label = Rect::new(x, y, (end - x).max(0.0), metrics.control);
                row.explanation = text(y + metrics.control + metrics.small);
            }
            Entry::Link(_) => {
                let y = rect.y + metrics.gap;
                row.label = Rect::new(x, y, inner, metrics.control);
            }
            Entry::Line(line) => {
                let y = rect.y + metrics.gap;
                // A tick box the width of a switch, so the two kinds of row line up.
                row.control = matches!(line, Line::Import(_))
                    .then(|| right(y, metrics.control * 1.8, metrics.control, 0.0));
                let end = row.control.map_or(x + inner, |tick| tick.x - metrics.gap);
                row.label = Rect::new(x, y, (end - x).max(0.0), metrics.control);
                row.explanation = text(y + metrics.control + metrics.small);
            }
        }
        row
    }
}

// ---- the theme dropdown -------------------------------------------------------------------------
//
// Thirteen themes were too many to cycle through one click at a time — the row offered "next" and
// nothing about what came after it — so the theme row's control opens a list instead, split into
// Dark and Light because that is the first thing anyone choosing a theme decides.

/// One line of the theme dropdown: a heading naming an appearance, or a built-in theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Heading(dacode_theme::Appearance),
    Theme(&'static str),
}

impl Pick {
    pub fn theme(self) -> Option<&'static str> {
        match self {
            Pick::Theme(name) => Some(name),
            Pick::Heading(_) => None,
        }
    }
}

/// What the dropdown lists: each appearance's heading, then its themes, in [`Theme::grouped`]'s
/// order — the order the arrows on the row and the wizard's cards walk too.
pub fn theme_picks() -> Vec<Pick> {
    dacode_theme::Appearance::ALL
        .into_iter()
        .flat_map(|appearance| {
            std::iter::once(Pick::Heading(appearance))
                .chain(Theme::built_ins(appearance).map(Pick::Theme))
        })
        .collect()
}

/// The open dropdown: which row is highlighted, and how far its list is scrolled, in pixels. The
/// highlight is never on a heading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dropdown {
    selected: usize,
    scroll: f32,
}

impl Dropdown {
    /// The highlighted row, as an index into [`theme_picks`].
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The scroll target, before [`theme_dropdown`] clamps it to the list.
    pub fn scroll(&self) -> f32 {
        self.scroll
    }
}

/// Where the dropdown and every row in it are. Read by the drawing and by the mouse, as
/// [`PageGeometry`] is and for the same reason.
#[derive(Debug, Clone, PartialEq)]
pub struct DropdownGeometry {
    /// The card, inside the editor area whatever the window's size.
    pub card: Rect,
    /// The scrolled region inside the card. Drawing clips to it.
    pub list: Rect,
    /// Every row — there are fifteen, so all of them rather than only the visible ones — placed at
    /// the scroll offset.
    pub rows: Vec<DropdownRow>,
    /// The offset the rows are placed at, after clamping.
    pub scroll: f32,
    pub max_scroll: f32,
    /// Whether it opened upwards, for want of room below the control: what it grows from.
    pub above: bool,
}

/// One row of the dropdown and the boxes inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropdownRow {
    pub pick: Pick,
    /// Its position in [`theme_picks`], which [`Dropdown::selected`] counts in.
    pub index: usize,
    /// The whole row at the scroll offset, unclipped.
    pub rect: Rect,
    /// `rect` cut to the list: what is on screen, and what [`DropdownGeometry::hit`] tests.
    pub clip: Rect,
    /// The name, or the heading's title.
    pub label: Rect,
    /// A theme's colour chip, the same chip the row's control shows.
    pub swatch: Option<Rect>,
    /// Where the mark for the theme in use goes, at the row's right.
    pub mark: Option<Rect>,
}

impl DropdownGeometry {
    /// The theme row under a point, if any. A heading is not a target, and nor is a row's part that
    /// is scrolled out of the list.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !contains(self.list, x, y) {
            return None;
        }
        self.rows
            .iter()
            .find(|row| row.pick.theme().is_some() && contains(row.clip, x, y))
            .map(|row| row.index)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        contains(self.card, x, y)
    }
}

/// Lays the dropdown out hanging from `anchor`, the theme row's control, inside `bounds`, with its
/// list scrolled by `scroll`.
///
/// It opens downwards, and upwards only when the room above is the larger and the list does not fit
/// below; either way it is cut to the room there is and its list scrolls. It is at least as wide as
/// the control and right-aligned with it, as the control sits at the row's right end.
pub fn theme_dropdown(
    anchor: Rect,
    bounds: Rect,
    scale: f32,
    typography: &Typography,
    scroll: f32,
) -> DropdownGeometry {
    let metrics = Metrics::new(typography, scale);
    let picks = theme_picks();
    let row = metrics.control;
    let heading = metrics.label_line;
    let pad = metrics.gap;
    let margin = space::XS * scale;
    let swatch = metrics.swatch();

    let height_of = |pick: &Pick| match pick {
        Pick::Heading(_) => heading,
        Pick::Theme(_) => row,
    };
    let total: f32 = picks.iter().map(height_of).sum();
    let longest = picks
        .iter()
        .map(|pick| match pick {
            Pick::Theme(name) => Theme::display_name_of(name).unwrap_or(name).chars().count(),
            Pick::Heading(appearance) => appearance.title().chars().count(),
        })
        .max()
        .unwrap_or(0);
    let natural = metrics.inset * 2.0
        + swatch
        + metrics.gap * 2.0
        + longest as f32 * metrics.ui_advance
        + row;

    let room_x = (bounds.width - margin * 2.0).max(0.0);
    let width = natural.max(anchor.width).min(room_x);
    let x = (anchor.right() - width)
        .min(bounds.right() - margin - width)
        .max(bounds.x + margin);

    let below = (bounds.bottom() - margin) - (anchor.bottom() + metrics.small);
    let over = (anchor.y - metrics.small) - (bounds.y + margin);
    let wanted = total + pad * 2.0;
    let above = below < wanted && over > below;
    let room = if above { over } else { below }.max(0.0);
    let height = wanted.min(room);
    let y = if above {
        anchor.y - metrics.small - height
    } else {
        anchor.bottom() + metrics.small
    };
    // An anchor scrolled half out of the page still leaves the card inside it.
    let y = y
        .min(bounds.bottom() - margin - height)
        .max(bounds.y + margin.min(bounds.height / 2.0));
    let card = Rect::new(x, y, width, height);
    let inner_pad = pad.min(height / 2.0);
    let list = Rect::new(
        card.x,
        card.y + inner_pad,
        card.width,
        (card.height - inner_pad * 2.0).max(0.0),
    );

    let max_scroll = (total - list.height).max(0.0);
    let scroll = if scroll.is_finite() {
        scroll.clamp(0.0, max_scroll)
    } else {
        0.0
    };

    let mut top = 0.0;
    let rows = picks
        .iter()
        .enumerate()
        .map(|(index, pick)| {
            let h = height_of(pick);
            let rect = Rect::new(list.x, list.y + top - scroll, list.width, h);
            top += h;
            let x = rect.x + metrics.inset;
            let inner = (rect.width - metrics.inset * 2.0).max(0.0);
            let (label, swatch, mark) = match pick {
                Pick::Heading(_) => (Rect::new(x, rect.y, inner, h), None, None),
                Pick::Theme(_) => {
                    let side = swatch.min(h);
                    let chip = Rect::new(x, rect.y + (h - side) / 2.0, side, side);
                    let mark = Rect::new(rect.right() - metrics.inset - h, rect.y, h, h);
                    let left = chip.right() + metrics.gap;
                    let label = Rect::new(left, rect.y, (mark.x - left).max(0.0), h);
                    (label, Some(chip), Some(mark))
                }
            };
            DropdownRow {
                pick: *pick,
                index,
                rect,
                clip: rect.intersect(list),
                label,
                swatch,
                mark,
            }
        })
        .collect();

    DropdownGeometry {
        card,
        list,
        rows,
        scroll,
        max_scroll,
        above,
    }
}

impl Page {
    /// The theme dropdown, when it is open.
    pub fn dropdown(&self) -> Option<Dropdown> {
        self.dropdown
    }

    /// Opens the theme dropdown with `current` highlighted — or the first theme, for a theme loaded
    /// from a file, which is on no list.
    pub fn open_dropdown(&mut self, current: Option<&str>) {
        let picks = theme_picks();
        let selected = current
            .and_then(|current| picks.iter().position(|pick| pick.theme() == Some(current)))
            .or_else(|| picks.iter().position(|pick| pick.theme().is_some()))
            .unwrap_or(0);
        self.dropdown = Some(Dropdown {
            selected,
            scroll: 0.0,
        });
    }

    pub fn close_dropdown(&mut self) {
        self.dropdown = None;
    }

    /// The highlighted theme.
    pub fn dropdown_choice(&self) -> Option<&'static str> {
        let dropdown = self.dropdown?;
        theme_picks().get(dropdown.selected)?.theme()
    }

    /// Moves the highlight `delta` themes, stepping over the headings and stopping at either end.
    /// Follow it with [`Page::reveal_dropdown`].
    pub fn move_dropdown(&mut self, delta: isize) {
        let Some(dropdown) = &mut self.dropdown else {
            return;
        };
        let themes: Vec<usize> = theme_picks()
            .iter()
            .enumerate()
            .filter(|(_, pick)| pick.theme().is_some())
            .map(|(index, _)| index)
            .collect();
        let Some(last) = themes.len().checked_sub(1) else {
            return;
        };
        let at = themes
            .iter()
            .position(|index| *index >= dropdown.selected)
            .unwrap_or(last);
        dropdown.selected = themes[at.saturating_add_signed(delta).min(last)];
    }

    /// Where the dropdown is, hanging from the theme row's control as `page` lays it out, inside
    /// `bounds` — the editor area, so it never covers the bars around it.
    ///
    /// If the theme row is not among `page`'s rows — the window was resized under it — the card
    /// hangs from the top right of the list rather than from nowhere.
    pub fn dropdown_geometry(
        &self,
        page: &PageGeometry,
        bounds: Rect,
        scale: f32,
        typography: &Typography,
    ) -> Option<DropdownGeometry> {
        let dropdown = self.dropdown?;
        let anchor = page
            .rows
            .iter()
            .find(|row| row.field() == Some(Field::Theme))
            .and_then(|row| row.control)
            .unwrap_or(Rect::new(page.list.right(), page.list.y, 0.0, 0.0));
        Some(theme_dropdown(
            anchor,
            bounds,
            scale,
            typography,
            dropdown.scroll,
        ))
    }

    /// Scrolls the dropdown's list by as little as it takes to show the highlighted row whole, and
    /// the heading above it with it when that is the first theme of its group and both fit.
    pub fn reveal_dropdown(
        &mut self,
        page: &PageGeometry,
        bounds: Rect,
        scale: f32,
        typography: &Typography,
    ) {
        let Some(geometry) = self.dropdown_geometry(page, bounds, scale, typography) else {
            return;
        };
        let Some(dropdown) = &mut self.dropdown else {
            return;
        };
        let Some(row) = geometry.rows.get(dropdown.selected) else {
            return;
        };
        let offset = |rect: Rect| rect.y - geometry.list.y + geometry.scroll;
        let view = geometry.list.height;
        let top = offset(row.rect);
        let bottom = top + row.rect.height;
        let mut scroll = geometry.scroll;
        if bottom > scroll + view {
            scroll = bottom - view;
        }
        if top < scroll {
            scroll = top;
        }
        if let Some(above) = dropdown
            .selected
            .checked_sub(1)
            .and_then(|above| geometry.rows.get(above))
            .filter(|above| above.pick.theme().is_none())
        {
            let heading = offset(above.rect);
            if heading < scroll && bottom - heading <= view {
                scroll = heading;
            }
        }
        dropdown.scroll = scroll.clamp(0.0, geometry.max_scroll);
    }

    /// Scrolls the dropdown's list by `delta` pixels, clamped to it: what the wheel does over it.
    pub fn scroll_dropdown(
        &mut self,
        delta: f32,
        page: &PageGeometry,
        bounds: Rect,
        scale: f32,
        typography: &Typography,
    ) {
        let Some(geometry) = self.dropdown_geometry(page, bounds, scale, typography) else {
            return;
        };
        if let Some(dropdown) = &mut self.dropdown {
            let delta = if delta.is_finite() { delta } else { 0.0 };
            dropdown.scroll = (geometry.scroll + delta).clamp(0.0, geometry.max_scroll);
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
            theme: Some("walnut"),
            line_numbers: crate::config::LineNumbers::Absolute,
            transparency: 0,
            animations: crate::motion::Animations::Full,
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

    fn values() -> Values {
        Values {
            modal: true,
            font_size: 13.5,
            tab_width: 4,
            soft_wrap: true,
            format_on_save: false,
            theme: Some("walnut"),
            line_numbers: crate::config::LineNumbers::Relative,
            transparency: 0,
            animations: crate::motion::Animations::Full,
            sticky_scroll: true,
            inlay_hints: false,
            autosave: false,
        }
    }

    #[test]
    fn a_click_on_animations_goes_round_full_reduced_off_and_back() {
        // Three states and one direction: a click must reach all of them and then wrap, or someone who
        // turned motion off with the mouse could never turn it back on.
        use crate::motion::Animations;
        let mut values = values();
        let mut seen = Vec::new();
        for _ in 0..4 {
            values.cycle(Field::Animations);
            seen.push(values.animations);
        }
        assert_eq!(
            seen,
            [
                Animations::Reduced,
                Animations::Off,
                Animations::Full,
                Animations::Reduced
            ]
        );
        assert_eq!(values.get(Field::Animations), "reduced");
    }

    #[test]
    fn the_animations_row_is_written_back_under_its_config_name() {
        let mut values = values();
        values.cycle(Field::Animations);
        let written = write_back("", &values);
        assert!(written.contains("animations = \"reduced\""), "{written}");

        // And what was written is something the config reader accepts, closing the loop.
        let config = dacode_lua::eval(&written, "written.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.animations.as_deref(), Some("reduced"));
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
        // By slug, the name config knows each by.
        let mut seen = vec![values.theme.unwrap()];

        for _ in 1..Theme::BUILT_IN.len() {
            values.adjust(Field::Theme, 1);
            seen.push(values.theme.unwrap());
        }
        let mut distinct = seen.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), Theme::BUILT_IN.len());
        assert!(seen.contains(&"rose-pine"));
        assert!(seen.contains(&"true-black"));

        // Wherever it started, one more step comes back to it, and one back from there is the last
        // one seen — the wrap, in both directions, without the test knowing the list's order.
        values.adjust(Field::Theme, 1);
        assert_eq!(values.theme.unwrap(), seen[0], "and wraps round");
        values.adjust(Field::Theme, -1);
        assert_eq!(
            values.theme.unwrap(),
            *seen.last().unwrap(),
            "backwards too"
        );
        assert_eq!(
            values.get(Field::Theme),
            Theme::display_name_of(seen.last().unwrap()).unwrap()
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
    fn writing_to_an_empty_config_produces_a_readable_block() {
        let written = write_back("", &values());
        assert!(written.starts_with(MARKER));
        assert!(written.contains("tab_width = 4"));
        assert!(written.contains("theme = \"walnut\""));
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
    fn a_block_written_by_vex_is_replaced_rather_than_kept_beside_a_new_one() {
        // The upgrade case: init.lua copied from ~/.config/vex still has vex's marker and calls
        // `vex.set`. Saving from the panel must leave one block, under the new marker.
        let old = "-- vex: written by the settings panel (ctrl-,). Edit freely — rewritten on save.\n\
                   vex.set {\n  tab_width = 4,\n  theme = \"rose-pine\",\n}\n";
        let written = write_back(old, &values());
        assert_eq!(written.matches("dacode.set {").count(), 1);
        assert!(
            !written.contains("vex.set"),
            "the old block is gone:\n{written}"
        );
        assert!(written.starts_with(MARKER));
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
        // The step lands wherever the grouped order puts the next theme; what matters is that the
        // name written is the name read.
        assert_ne!(original.theme, Some("walnut"), "the theme did move");
        assert_eq!(config.theme_name.as_deref(), original.theme);
        assert_eq!(
            config.font_size.map(|size| format!("{size:.1}")),
            Some(original.get(Field::FontSize))
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
            theme: Some("walnut"),
            line_numbers: crate::config::LineNumbers::Relative,
            transparency: 0,
            animations: crate::motion::Animations::Full,
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

#[cfg(test)]
mod page_tests {
    use super::*;

    /// Windows from the smallest DaCode is laid out for to the largest common desktop, including a
    /// short wide one, where a side tree once squeezed its rows below the height of their text.
    const WINDOWS: [(f32, f32); 7] = [
        (640.0, 480.0),
        (800.0, 600.0),
        (1024.0, 768.0),
        (1280.0, 720.0),
        (1600.0, 480.0),
        (1600.0, 1000.0),
        (2560.0, 1440.0),
    ];

    /// The rectangle the settings tab is really given: the editor area, with the sidebar closed and
    /// open. The first version of these tests swept whole windows, and passed while the page Task 11
    /// would draw — 236 px shorter at scale 2 — showed no rows at all.
    fn pages() -> Vec<(Rect, f32, String)> {
        let mut pages = Vec::new();
        for (width, height) in WINDOWS {
            for scale in [1.0_f32, 2.0] {
                for sidebar in [0.0_f32, 1.0] {
                    let window = Rect::new(0.0, 0.0, width, height);
                    let editor = crate::layout::Layout::of(window, scale, None, sidebar).editor;
                    pages.push((
                        editor,
                        scale,
                        format!("{width}x{height} at {scale}, sidebar {sidebar}"),
                    ));
                }
            }
        }
        pages
    }

    fn servers() -> Vec<ServerRow> {
        vec![
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
        ]
    }

    fn page(category: Category, query: &str, modal: bool) -> Page {
        let mut page = Page::new();
        page.set_reports(servers(), servers(), modal);
        page.set_category(category, modal);
        page.set_query(query, modal);
        page
    }

    /// Every value changed from its default, so every resettable row carries a reset box too.
    fn changed(modal: bool) -> Values {
        let mut values = Values::defaults(true);
        for field in Field::ALL {
            if field != Field::VimKeys {
                values.adjust(field, 1);
            }
        }
        values.modal = modal;
        values
    }

    fn inside(inner: Rect, outer: Rect) -> bool {
        const SLACK: f32 = 0.01;
        inner.x >= outer.x - SLACK
            && inner.y >= outer.y - SLACK
            && inner.right() <= outer.right() + SLACK
            && inner.bottom() <= outer.bottom() + SLACK
            && inner.width >= 0.0
            && inner.height >= 0.0
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        const SLACK: f32 = 0.01;
        a.x < b.right() - SLACK
            && b.x < a.right() - SLACK
            && a.y < b.bottom() - SLACK
            && b.y < a.bottom() - SLACK
    }

    fn category_of(field: Field) -> Category {
        Category::ALL
            .into_iter()
            .find(|category| {
                sections(*category)
                    .iter()
                    .any(|section| section.fields.contains(&field))
            })
            .unwrap_or_else(|| panic!("{field:?} is on no page"))
    }

    /// The editor area of a window, sidebar closed.
    fn editor(width: f32, height: f32, scale: f32) -> Rect {
        crate::layout::Layout::of(Rect::new(0.0, 0.0, width, height), scale, None, 0.0).editor
    }

    #[test]
    fn every_field_appears_in_exactly_one_section() {
        // A setting on no page cannot be changed with the mouse at all, and one on two pages is two
        // rows that must be kept in step. Both have happened to settings screens that kept their
        // lists by hand.
        for field in Field::ALL {
            let homes: Vec<(Category, &str)> = Category::ALL
                .into_iter()
                .flat_map(|category| {
                    sections(category)
                        .iter()
                        .filter(move |section| section.fields.contains(&field))
                        .map(move |section| (category, section.title))
                })
                .collect();
            assert_eq!(homes.len(), 1, "{field:?} is in {homes:?}");
        }
        for category in Category::ALL {
            for section in sections(category) {
                assert!(
                    !section.fields.is_empty(),
                    "a heading over nothing: {}",
                    section.title
                );
            }
        }
    }

    #[test]
    fn a_search_for_wrap_finds_soft_wrap_and_no_setting_in_another_category() {
        // Search is across every page, so a query that matched loosely would fill the page with
        // rows that have nothing to do with it. Upper case too: nobody types a search in the case
        // the label happens to be set in.
        for query in ["wrap", "WRAP", " Wrap "] {
            for modal in [true, false] {
                let page = page(Category::Appearance, query, modal);
                let found: Vec<Field> = Field::ALL
                    .into_iter()
                    .filter(|field| page.matches(*field, modal))
                    .collect();
                assert_eq!(found, [Field::SoftWrap], "{query:?}, modal {modal}");

                let fields: Vec<Field> = page
                    .entries(modal)
                    .into_iter()
                    .filter_map(|entry| match entry {
                        Entry::Field(field) => Some(field),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    fields,
                    [Field::SoftWrap],
                    "the page shows it, whatever category was open"
                );
                assert!(
                    page.entries(modal).contains(&Entry::Heading {
                        title: "Wrapping",
                        category: Some(Category::Editor),
                    }),
                    "under the page and section it lives in"
                );

                for category in [
                    Category::Appearance,
                    Category::Files,
                    Category::Languages,
                    Category::Import,
                ] {
                    assert!(
                        !page.has_matches(category, modal),
                        "{category:?} has nothing for {query:?}, so it collapses"
                    );
                }
                assert!(page.has_matches(Category::Editor, modal));
                // The one other page with a hit, deliberately: the command that toggles soft wrap
                // is on the Keys page, and someone searching for "wrap" wants its key as much as
                // its switch.
                assert!(page.has_matches(Category::Keys, modal));
            }
        }

        // A page's own name finds its settings, as VSCode's "editor" lists the editor's.
        let editor = page(Category::Keys, "editor", true);
        for field in sections(Category::Editor).iter().flat_map(|s| s.fields) {
            assert!(editor.matches(*field, true), "{field:?} for \"editor\"");
        }
        assert!(page(Category::Keys, "appearance", true).matches(Field::Theme, true));
    }

    #[test]
    fn the_page_fits_the_editor_area_at_every_size() {
        // Every page, two searches, both grammars, scrolled to the top, part way and past the end.
        let typography = Typography::default();
        for (rect, scale, at) in pages() {
            for (category, query) in Category::ALL
                .into_iter()
                .map(|category| (category, ""))
                .chain([(Category::Appearance, "e"), (Category::Keys, "s")])
            {
                for modal in [true, false] {
                    for scroll in [0.0_f32, 130.0, 1e9] {
                        let mut page = page(category, query, modal);
                        page.scroll_by(scroll, rect, scale, modal, &typography);
                        let geometry =
                            page.geometry(rect, scale, &changed(modal), modal, &typography);
                        let at =
                            format!("{at}, {category:?} {query:?}, scroll {scroll}, modal {modal}");

                        assert!(
                            inside(geometry.search, rect),
                            "search outside the page: {at}"
                        );
                        assert!(geometry.search.height > 0.0, "no search box: {at}");
                        assert!(inside(geometry.list, rect), "list outside the page: {at}");
                        assert!(
                            !overlap(geometry.search, geometry.list),
                            "search on list: {at}"
                        );
                        if geometry.folded {
                            assert_eq!(geometry.tree.len(), 1, "{at}");
                            assert_eq!(
                                geometry.tree[0].0, category,
                                "the button names the page: {at}"
                            );
                        } else {
                            assert_eq!(geometry.tree.len(), Category::ALL.len(), "{at}");
                        }
                        for (index, (_, tree)) in geometry.tree.iter().enumerate() {
                            assert!(inside(*tree, rect), "tree outside: {at}");
                            assert!(tree.height > 0.0, "a tree row with no height: {at}");
                            assert!(!overlap(*tree, geometry.search), "tree on search: {at}");
                            assert!(!overlap(*tree, geometry.list), "tree on list: {at}");
                            for (_, other) in &geometry.tree[index + 1..] {
                                assert!(!overlap(*tree, *other), "tree rows overlap: {at}");
                            }
                        }
                        assert!(
                            geometry.scroll >= 0.0 && geometry.scroll <= geometry.max_scroll,
                            "scroll {} past {}: {at}",
                            geometry.scroll,
                            geometry.max_scroll
                        );

                        for (index, row) in geometry.rows.iter().enumerate() {
                            assert!(
                                inside(row.clip, geometry.list),
                                "on screen outside the list: {at}"
                            );
                            assert!(row.clip.height > 0.0, "a row not on screen: {at}");
                            for other in &geometry.rows[index + 1..] {
                                assert!(!overlap(row.rect, other.rect), "rows overlap: {at}");
                                assert!(other.index > row.index, "rows out of order: {at}");
                            }
                            let parts: Vec<Rect> =
                                [Some(row.label), row.explanation, row.control, row.reset]
                                    .into_iter()
                                    .flatten()
                                    .collect();
                            for (at_part, part) in parts.iter().enumerate() {
                                assert!(
                                    inside(*part, row.rect),
                                    "a part outside its row: {at}, {row:?}"
                                );
                                for other in &parts[at_part + 1..] {
                                    assert!(
                                        !overlap(*part, *other),
                                        "parts overlap: {at}, {row:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_setting_fits_whenever_the_page_has_room_for_the_search_box_and_one_row() {
        // The threshold is the search box, the space under it, the page's first setting — and on a
        // page with room for the tree, that setting's heading — and the padding below. At 640×480
        // and scale 2 the editor area is 244 px tall; before the tree folded into a button and the
        // leading heading gave up its space, the strip and the search box took 272 of them.
        let typography = Typography::default();
        let mut met = Vec::new();
        for (rect, scale, at) in pages() {
            for category in Category::ALL {
                let page = page(category, "", true);
                let geometry = page.geometry(rect, scale, &changed(true), true, &typography);
                let skeleton = Skeleton::new(rect, scale, &typography, category);
                let flow = page.flow(&skeleton, true, &typography);
                let first = flow
                    .entries
                    .iter()
                    .position(|entry| !entry.is_heading())
                    .expect("every page has a row");
                let slot = flow.slots[first];
                if skeleton.folded {
                    assert_eq!(slot.top, 0.0, "a folded page starts with its setting: {at}");
                }
                let threshold = (geometry.list.y - rect.y)
                    + slot.top
                    + slot.height
                    + (rect.bottom() - geometry.list.bottom());
                if rect.height >= threshold {
                    let row = geometry
                        .rows
                        .iter()
                        .find(|row| row.index == first)
                        .unwrap_or_else(|| panic!("no first row at {at}, {category:?}"));
                    assert!(
                        inside(row.rect, geometry.list),
                        "{category:?} has room ({} ≥ {threshold}) and its first setting is cut: {at}",
                        rect.height
                    );
                    met.push((at.clone(), category));
                }
            }
        }
        // Not vacuous: the smallest window, at scale 2, still has room for a setting on the pages
        // whose first setting needs no sentence under it.
        for at in ["640x480 at 2, sidebar 0", "640x480 at 2, sidebar 1"] {
            for category in [Category::Appearance, Category::Keys, Category::Import] {
                assert!(
                    met.iter().any(|(m, c)| m == at && *c == category),
                    "{category:?} at {at} is below the threshold"
                );
            }
        }
        for category in Category::ALL {
            assert!(
                met.iter()
                    .any(|(m, c)| m == "640x480 at 1, sidebar 1" && *c == category)
            );
        }
    }

    #[test]
    fn a_changed_setting_offers_a_reset_and_one_at_its_default_does_not() {
        // The ● and ⟲ answer "what have I changed?", the question a list of forty values cannot.
        // A reset on a row that is already at its default would be a button that does nothing.
        let typography = Typography::default();
        let rect = editor(1600.0, 1000.0, 1.0);
        let defaults = Values::defaults(true);

        for field in Field::ALL {
            assert!(
                field.is_default(&defaults),
                "{field:?} is not at its own default"
            );
            let page = page(category_of(field), "", true);
            let geometry = page.geometry(rect, 1.0, &defaults, true, &typography);
            for row in &geometry.rows {
                assert_eq!(row.reset, None, "a reset on a default row: {row:?}");
            }

            let mut values = defaults.clone();
            values.adjust(field, 1);
            assert!(
                !field.is_default(&values),
                "{field:?} changed and still default"
            );
            let page = page_for(field, values.modal);
            let geometry = page.geometry(rect, 1.0, &values, values.modal, &typography);
            let row = geometry
                .rows
                .iter()
                .find(|row| row.field() == Some(field))
                .unwrap_or_else(|| panic!("{field:?} is not on its own page"));
            // The grammar is the exception: a ⟲ there turns Vim on in one click for someone who
            // chose the conventional editor, so it has none.
            // Named here rather than read from `resettable`, so the test cannot agree with a
            // regression in it.
            assert_eq!(row.reset.is_some(), field != Field::VimKeys, "{field:?}");
            for other in &geometry.rows {
                if let Some(other_field) = other.field() {
                    assert_eq!(
                        other.reset.is_some(),
                        other_field != Field::VimKeys && !other_field.is_default(&values),
                        "{other_field:?} after changing {field:?}"
                    );
                }
            }

            values.reset(field);
            assert!(
                field.is_default(&values),
                "{field:?} reset and still changed"
            );
        }
    }

    fn page_for(field: Field, modal: bool) -> Page {
        page(category_of(field), "", modal)
    }

    #[test]
    fn no_heading_is_a_source_caption() {
        // "init.lua" and "imported" over a value were the old panel's answer to where it came
        // from. Readers took them for section names, and they explained nothing.
        let captions: Vec<String> = [Source::Default, Source::Config, Source::Imported]
            .into_iter()
            .map(|source| source.label().to_lowercase())
            .chain(["init.lua", "imported", "default"].map(String::from))
            .collect();
        let typography = Typography::default();
        let rect = editor(2560.0, 1440.0, 1.0);
        let mut checked = 0;
        for category in Category::ALL {
            for query in ["", "a", "e", "i"] {
                for modal in [true, false] {
                    let page = page(category, query, modal);
                    let headings =
                        page.entries(modal)
                            .into_iter()
                            .filter_map(|entry| match entry {
                                Entry::Heading { title, .. } => Some(title),
                                _ => None,
                            });
                    let drawn = page
                        .geometry(rect, 1.0, &changed(modal), modal, &typography)
                        .rows
                        .into_iter()
                        .filter_map(|row| row.heading());
                    for heading in headings.chain(drawn) {
                        checked += 1;
                        assert!(
                            !captions.contains(&heading.trim().to_lowercase()),
                            "{heading:?} is a source caption"
                        );
                    }
                }
            }
        }
        assert!(checked > 0, "and there were headings to check");
    }

    #[test]
    fn the_keys_page_lays_out_only_the_rows_on_screen() {
        // Ninety-odd commands: laying all of them out to draw the forty on screen is per-frame work
        // that grows with the table rather than the window.
        let typography = Typography::default();
        for (rect, scale, at) in pages() {
            for scroll in [0.0_f32, 900.0] {
                let mut page = page(Category::Keys, "", true);
                page.scroll_by(scroll, rect, scale, true, &typography);
                let total = page.entries(true).len();
                assert!(total > 90, "the whole table is the list: {total}");

                let geometry = page.geometry(rect, scale, &changed(true), true, &typography);
                if geometry.list.height <= 0.0 {
                    continue;
                }
                assert!(
                    !geometry.rows.is_empty(),
                    "nothing on the Keys page at {at}"
                );
                let smallest = geometry
                    .rows
                    .iter()
                    .map(|row| row.rect.height)
                    .fold(f32::MAX, f32::min);
                // A part-shown row at either end, and whole rows between.
                let room = (geometry.list.height / smallest).ceil() + 1.0;
                assert!(
                    geometry.rows.len() as f32 <= room,
                    "{} rows laid out in room for {room} at {at}",
                    geometry.rows.len(),
                );
                assert!(geometry.rows.len() < total, "the whole table at {at}");
            }
        }
    }

    #[test]
    fn nothing_says_vim_to_someone_who_turned_it_off() {
        // Someone who chose the conventional editor should not be taught Vim's ideas by the one
        // screen that describes the editor they chose.
        let mut texts: Vec<&str> = Vec::new();
        for field in Field::ALL {
            texts.push(field.label(false));
            texts.extend(field.explanation(false));
        }
        for line in [Line::RunImport, Line::LoadTheme]
            .into_iter()
            .chain(Importable::ALL.map(Line::Import))
        {
            texts.extend(line.label());
            texts.extend(line.explanation());
        }
        for category in Category::ALL {
            for entry in page(category, "", false).entries(false) {
                if let Entry::Heading { title, .. } = entry {
                    texts.push(title);
                }
            }
        }
        assert!(texts.len() > Field::ALL.len(), "there was text to check");
        for text in texts {
            assert!(!text.to_lowercase().contains("vim"), "{text:?}");
        }

        // And the switch itself is still findable by the name people know it by.
        let page = page(Category::Files, "vim", false);
        assert!(page.matches(Field::VimKeys, false));
    }

    #[test]
    fn every_explanation_fits_three_lines_of_the_page() {
        // An explanation is a sentence or two, the way VSCode writes them. One that runs to a
        // paragraph turns a list of settings into a document nobody scans — so the limit is a
        // test, at the largest font the editor allows, where the lines are shortest, and at the
        // page's full width.
        for typography in [
            Typography::default(),
            Typography::with_code_size(Typography::MAX_CODE_SIZE),
        ] {
            let metrics = Metrics::new(&typography, 1.0);
            let rect = editor(2560.0, 1440.0, 1.0);
            for category in [
                Category::Appearance,
                Category::Editor,
                Category::Files,
                Category::Import,
            ] {
                for modal in [true, false] {
                    let geometry = page(category, "", modal).geometry(
                        rect,
                        1.0,
                        &changed(modal),
                        modal,
                        &typography,
                    );
                    assert_eq!(
                        geometry.list.width, PAGE_WIDTH,
                        "measured at the page's width"
                    );
                    for row in &geometry.rows {
                        if let Some(explanation) = row.explanation {
                            assert!(
                                explanation.height <= metrics.text_line * 3.0 + 0.01,
                                "{row:?} explains itself in {} lines",
                                explanation.height / metrics.text_line
                            );
                        }
                    }
                }
            }
        }

        // The settings whose names say everything say nothing more.
        for field in Field::ALL {
            let silent = matches!(field, Field::Theme | Field::FontSize | Field::TabWidth);
            for modal in [true, false] {
                assert_eq!(field.explanation(modal).is_none(), silent, "{field:?}");
            }
        }
    }

    #[test]
    fn the_selection_steps_over_headings_and_stays_on_the_list() {
        // A heading is not something to change, and a selection resting on one would make the
        // arrow keys look broken.
        let mut page = page(Category::Editor, "", true);
        let entries = page.entries(true);
        assert!(matches!(
            entries[page.selected()],
            Entry::Field(Field::VimKeys)
        ));
        for _ in 0..entries.len() * 2 {
            page.move_selection(1, true);
            assert!(!entries[page.selected()].is_heading());
        }
        assert_eq!(page.selected(), entries.len() - 1);
        page.move_selection(-99, true);
        assert!(matches!(
            entries[page.selected()],
            Entry::Field(Field::VimKeys)
        ));

        // A click on a heading selects the row under it.
        page.select(3, true);
        assert!(matches!(entries[3], Entry::Heading { .. }));
        assert_eq!(page.selected(), 4);

        // A list with nothing to select says so rather than pointing past its end.
        let mut empty = Page::new();
        empty.set_category(Category::Languages, true);
        assert_eq!(empty.selected_entry(true), None);
    }

    #[test]
    fn a_search_typed_while_scrolled_down_starts_from_the_top() {
        // The defect: scroll the Keys page down, type "wrap", and the scroll still pointed past the
        // four rows the search left — a blank page — while the selection pointed at nothing.
        let typography = Typography::default();
        let rect = editor(1024.0, 768.0, 1.0);
        let mut page = page(Category::Keys, "", true);
        page.scroll_by(1e9, rect, 1.0, true, &typography);
        for _ in 0..80 {
            page.move_selection(1, true);
        }
        assert!(page.scroll() > 0.0 && page.selected() > 60);

        page.set_query("wrap", true);
        assert_eq!(page.scroll(), 0.0);
        let selected = page.selected_entry(true).expect("a match is selected");
        assert_eq!(selected, Entry::Field(Field::SoftWrap));
        let geometry = page.geometry(rect, 1.0, &changed(true), true, &typography);
        assert!(geometry.rows.iter().any(|row| row.index == page.selected()));

        // And a page change does the same.
        page.set_query("", true);
        page.scroll_by(1e9, rect, 1.0, true, &typography);
        page.set_category(Category::Files, true);
        assert_eq!(page.scroll(), 0.0);
        assert_eq!(
            page.selected_entry(true),
            Some(Entry::Field(Field::Autosave))
        );
    }

    #[test]
    fn stepping_through_the_keys_page_keeps_the_selection_on_screen() {
        // Pressing ↓ past the last row on screen has to scroll, by heights only the layout knows;
        // `reveal` reads the same ones the drawing does, at every size.
        let typography = Typography::default();
        for (width, height, scale) in [
            (1024.0, 768.0, 1.0),
            (640.0, 480.0, 1.0),
            (640.0, 480.0, 2.0),
        ] {
            let rect = editor(width, height, scale);
            let mut page = page(Category::Keys, "", true);
            let total = page.entries(true).len();
            let check = |page: &Page| {
                let geometry = page.geometry(rect, scale, &changed(true), true, &typography);
                let row = geometry
                    .rows
                    .iter()
                    .find(|row| row.index == page.selected())
                    .unwrap_or_else(|| {
                        panic!("row {} is off screen at {width}x{height}", page.selected())
                    });
                assert!(
                    inside(row.rect, geometry.list),
                    "row {} is cut at {width}x{height}",
                    page.selected()
                );
            };
            for _ in 0..total {
                page.move_selection(1, true);
                page.reveal(rect, scale, true, &typography);
                check(&page);
            }
            assert!(page.scroll() > 0.0);
            for _ in 0..total {
                page.move_selection(-1, true);
                page.reveal(rect, scale, true, &typography);
                check(&page);
            }
            assert_eq!(page.scroll(), 0.0, "back at the top, its heading shown too");
        }
    }

    #[test]
    fn the_scroll_is_clamped_to_the_list() {
        // The wheel past the end must stop with the last row at the bottom, not scroll into empty
        // space — and a page shorter than the window does not scroll at all.
        let typography = Typography::default();
        let rect = editor(1024.0, 768.0, 1.0);
        let mut page = page(Category::Keys, "", true);
        let max = page.max_scroll(rect, 1.0, true, &typography);
        assert!(max > 0.0);
        page.scroll_by(1e9, rect, 1.0, true, &typography);
        assert_eq!(page.scroll(), max);
        let geometry = page.geometry(rect, 1.0, &changed(true), true, &typography);
        assert_eq!(geometry.max_scroll, max);
        let last = geometry.rows.last().expect("rows at the end");
        assert_eq!(last.index, page.entries(true).len() - 1);
        assert!((last.rect.bottom() - geometry.list.bottom()).abs() < 0.01);

        // A spring that overshoots is drawn clamped too.
        let geometry = page.geometry_at(max + 500.0, rect, 1.0, &changed(true), true, &typography);
        assert_eq!(geometry.scroll, max);

        page.scroll_by(-1e9, rect, 1.0, true, &typography);
        assert_eq!(page.scroll(), 0.0);
        page.scroll_by(f32::NAN, rect, 1.0, true, &typography);
        assert_eq!(page.scroll(), 0.0);

        let mut short = page_for(Field::Autosave, true);
        assert_eq!(
            short.max_scroll(editor(1600.0, 1000.0, 1.0), 1.0, true, &typography),
            0.0
        );
        short.scroll_by(300.0, editor(1600.0, 1000.0, 1.0), 1.0, true, &typography);
        assert_eq!(short.scroll(), 0.0);
    }

    #[test]
    fn a_click_lands_on_what_is_on_screen() {
        // The mouse reads the geometry's own hit test, which knows a row scrolled half out of the
        // list is only half there.
        let typography = Typography::default();
        let rect = editor(800.0, 600.0, 1.0);
        let mut page = page(Category::Appearance, "", true);
        page.scroll_by(60.0, rect, 1.0, true, &typography);
        let geometry = page.geometry(rect, 1.0, &changed(true), true, &typography);
        let centre = |r: Rect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
        let mut tried = 0;
        for row in &geometry.rows {
            if row.entry.is_heading() {
                continue;
            }
            let (x, _) = centre(row.label);
            let y = row.clip.y + row.clip.height / 2.0;
            assert_eq!(geometry.hit(x, y), Some((row.index, Part::Row)));
            for (part, kind) in [(row.control, Part::Control), (row.reset, Part::Reset)] {
                if let Some(part) = part.filter(|part| inside(*part, geometry.list)) {
                    let (x, y) = centre(part);
                    assert_eq!(geometry.hit(x, y), Some((row.index, kind)));
                    tried += 1;
                }
            }
            // The part of a row scrolled above the list is not there to click.
            if row.rect.y < geometry.list.y {
                assert_eq!(geometry.hit(x, row.rect.y + 1.0), None);
                tried += 1;
            }
        }
        assert!(tried > 2, "the test clicked something");

        // Every page is a click away on a folded tree too: its one button steps through them all.
        let mut seen = vec![Category::Appearance];
        while seen.len() < Category::ALL.len() + 1 {
            seen.push(seen.last().unwrap().next());
        }
        assert_eq!(&seen[..Category::ALL.len()], Category::ALL);
        assert_eq!(seen.last(), Some(&Category::Appearance));
    }

    #[test]
    fn the_layout_is_built_once_and_not_per_frame() {
        // What a frame costs must be the rows on screen. The list, its heights and the match flags
        // are rebuilt when the page, query, grammar or width changes — and only then.
        let typography = Typography::default();
        let rect = editor(1024.0, 768.0, 1.0);
        let page = page(Category::Keys, "s", true);
        page.geometry(rect, 1.0, &changed(true), true, &typography);
        let built = page
            .flow
            .borrow()
            .as_ref()
            .map(|flow| flow.entries.as_ptr());
        page.geometry_at(40.0, rect, 1.0, &changed(true), true, &typography);
        page.has_matches(Category::Files, true);
        assert_eq!(
            page.flow
                .borrow()
                .as_ref()
                .map(|flow| flow.entries.as_ptr()),
            built
        );
        // A width that changes how explanations wrap does rebuild it. (1600 would not: the list is
        // 640 wide there as at 1024, and nothing about the layout differs.)
        page.geometry(
            editor(800.0, 600.0, 1.0),
            1.0,
            &changed(true),
            true,
            &typography,
        );
        assert_ne!(
            page.flow
                .borrow()
                .as_ref()
                .map(|flow| flow.entries.as_ptr()),
            built
        );
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;

    fn editor(width: f32, height: f32, scale: f32) -> Rect {
        crate::layout::Layout::of(Rect::new(0.0, 0.0, width, height), scale, None, 0.0).editor
    }

    #[test]
    fn nothing_is_ticked_until_asked() {
        // The rule the import CLI already follows: an import that replaces your theme because you
        // wanted your keybindings is an import nobody runs twice.
        let mut page = Page::new();
        assert!(page.checklist().is_empty());

        page.tick(Importable::Theme);
        assert!(page.is_ticked(Importable::Theme));
        assert!(!page.is_ticked(Importable::Settings));
        assert!(page.checklist().wants(dacode_import::ImportItem::Theme));
        assert!(!page.checklist().wants(dacode_import::ImportItem::Settings));

        page.tick(Importable::Theme);
        assert!(!page.is_ticked(Importable::Theme), "ticking again unticks");
    }

    #[test]
    fn the_keys_page_says_how_to_rebind_from_a_row_that_scrolls_with_the_list() {
        // Rebinding from the list is out of scope, and a list of keys with no word on how to change
        // one reads as keys that cannot be changed. The way is a row: it is selected, scrolled and
        // clicked like any other, rather than a box drawn somewhere the mouse was not told about.
        let modal = true;
        let mut page = Page::new();
        page.set_category(Category::Keys, modal);
        let link = Entry::Link(Link::RebindKeys);
        assert_eq!(
            page.selected_entry(modal),
            Some(link),
            "it is the first thing on the page"
        );

        let rect = editor(1600.0, 1000.0, 1.0);
        let values = Values::defaults(modal);
        let typography = Typography::default();
        let at_top = page.geometry(rect, 1.0, &values, modal, &typography);
        let row = at_top
            .rows
            .iter()
            .find(|row| row.entry == link)
            .expect("on screen at the top");
        let middle = |rect: Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let (x, y) = middle(row.label);
        assert_eq!(at_top.hit(x, y), Some((row.index, Part::Row)));

        page.scroll_by(10_000.0, rect, 1.0, modal, &typography);
        let scrolled = page.geometry(rect, 1.0, &values, modal, &typography);
        assert!(
            scrolled.rows.iter().all(|row| row.entry != link),
            "and it scrolls away with the list"
        );

        page.set_query("rebind", modal);
        assert!(
            page.entries(modal).contains(&link),
            "a search for how to rebind finds it"
        );
    }

    #[test]
    fn a_folded_tree_names_every_page_while_a_search_shows_them_all() {
        // At 640x480 a search for "wrap" showed Soft wrap under a button saying "Appearance": the
        // list is every page's during a search, and the button must not claim it for one.
        let modal = true;
        let mut page = Page::new();
        assert_eq!(page.folded_label(), "Appearance");
        page.set_query("wrap", modal);
        assert_eq!(page.folded_label(), "All results");
        page.set_query("  ", modal);
        assert_eq!(
            page.folded_label(),
            "Appearance",
            "only whitespace is no search"
        );

        // Folded, the button has room for the whole of it, and the first group keeps its heading:
        // the button no longer names a page, so only the heading says where "Soft wrap" lives.
        page.set_query("wrap", modal);
        let typography = Typography::default();
        for scale in [1.0, 2.0] {
            let rect = editor(540.0 * scale, 480.0 * scale, scale);
            let geometry = page.geometry(rect, scale, &Values::defaults(modal), modal, &typography);
            assert!(geometry.folded, "folded at 540 wide");
            let button = geometry.tree[0].1;
            let advance = crate::layout::measured_advance(&typography, Role::Ui, scale);
            assert!(
                button.width >= ALL_RESULTS.chars().count() as f32 * advance,
                "\"{ALL_RESULTS}\" fits its button at {scale}"
            );
            let first = geometry.rows.first().expect("a row");
            assert_eq!(first.heading(), Some("Wrapping"));
            assert!(first.label.height > 0.0, "and it is drawn at {scale}");
        }
    }

    #[test]
    fn a_steppers_two_buttons_are_its_own_and_hit_as_their_direction() {
        // Drawn as − value +, and the mouse must see the same three parts: one box for the whole
        // control made − a step up.
        let modal = true;
        let mut page = Page::new();
        page.set_category(Category::Appearance, modal);
        let values = Values::defaults(modal);
        let typography = Typography::default();
        for scale in [1.0, 2.0] {
            let rect = editor(1600.0 * scale, 1000.0 * scale, scale);
            let geometry = page.geometry(rect, scale, &values, modal, &typography);
            for row in &geometry.rows {
                let stepper = row.field().map(|field| field.control()) == Some(Control::Stepper);
                assert_eq!(row.stepper.is_some(), stepper, "{:?}", row.entry);
                let Some((down, up)) = row.stepper else {
                    continue;
                };
                let control = row.control.unwrap();
                for half in [down, up] {
                    assert!(half.x >= control.x && half.right() <= control.right() + 0.01);
                }
                assert!(down.right() <= up.x, "the halves do not overlap");
                let middle = |rect: Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
                let (x, y) = middle(down);
                assert_eq!(geometry.hit(x, y), Some((row.index, Part::Decrease)));
                let (x, y) = middle(up);
                assert_eq!(geometry.hit(x, y), Some((row.index, Part::Increase)));
                let (x, y) = middle(control);
                assert_eq!(
                    geometry.hit(x, y),
                    Some((row.index, Part::Control)),
                    "the value"
                );
            }
        }
    }

    #[test]
    fn a_line_added_to_a_hand_edited_block_leaves_init_lua_valid() {
        // Lua lets a table's last field go without a comma, and someone editing the block by hand
        // leaves it off. A new line appended after it made the whole file fail to load.
        let mut values = Values::defaults(true);
        values.font_size = 15.0;
        let edited = format!("{MARKER}\ndacode.set {{\n  tab_width = 4\n}}\n");
        let written = write_touched(&edited, &values, &[Field::FontSize], &[]);
        let config = dacode_lua::eval(&written, "init.lua");
        assert!(
            config.problems.is_empty(),
            "{:?}\n{written}",
            config.problems
        );
        assert_eq!(config.font_size, Some(15.0));
        assert_eq!(config.tab_width, Some(4), "the hand-edited line is kept");
    }

    #[test]
    fn a_search_marks_every_place_it_matched_ignoring_case() {
        assert_eq!(match_ranges("Soft wrap", "WRAP"), vec![5..9]);
        assert_eq!(
            match_ranges("wrap long lines, or wrap", "wrap"),
            vec![0..4, 20..24]
        );
        assert_eq!(
            match_ranges("Soft wrap", "  "),
            Vec::<std::ops::Range<usize>>::new()
        );
        assert!(match_ranges("Theme", "font").is_empty());
        // Characters, not bytes: the drawing places a wash at a character's advance.
        assert_eq!(match_ranges("Editor › Wrapping", "wrap"), vec![9..13]);
    }

    #[test]
    fn the_theme_row_has_room_for_its_swatch_inside_its_control() {
        // The swatch is drawn inside the control, from this box — so it must be there, inside it,
        // and leave the rest of the control for the name.
        let modal = true;
        let mut page = Page::new();
        page.set_category(Category::Appearance, modal);
        let values = Values::defaults(modal);
        let typography = Typography::default();
        for scale in [1.0, 2.0] {
            let geometry = page.geometry(
                editor(1600.0, 1000.0, scale),
                scale,
                &values,
                modal,
                &typography,
            );
            let row = geometry
                .rows
                .iter()
                .find(|row| row.field() == Some(Field::Theme))
                .expect("the theme row is on screen");
            let (control, swatch) = (row.control.unwrap(), row.swatch.expect("a swatch"));
            assert!(
                swatch.x >= control.x
                    && swatch.right() <= control.right()
                    && swatch.y >= control.y
                    && swatch.bottom() <= control.bottom(),
                "{swatch:?} outside {control:?}"
            );
            assert!(
                geometry
                    .rows
                    .iter()
                    .filter(|other| other.field() != Some(Field::Theme))
                    .all(|other| other.swatch.is_none()),
                "only the theme has one"
            );
        }
    }
}

#[cfg(test)]
mod dropdown_tests {
    use super::*;
    use dacode_theme::Appearance;

    fn values(theme: Option<&'static str>) -> Values {
        Values {
            theme,
            ..Values::defaults(true)
        }
    }

    /// The editor area at the sizes the page is checked at, small and short ones first.
    fn editors() -> Vec<(Rect, f32, String)> {
        let mut editors = Vec::new();
        for (width, height) in [
            (640.0, 480.0),
            (800.0, 600.0),
            (1600.0, 480.0),
            (1600.0, 1000.0),
            (2560.0, 1440.0),
        ] {
            for scale in [1.0_f32, 2.0] {
                let window = Rect::new(0.0, 0.0, width, height);
                let editor = crate::layout::Layout::of(window, scale, None, 0.0).editor;
                editors.push((editor, scale, format!("{width}x{height} at {scale}")));
            }
        }
        editors
    }

    fn inside(outer: Rect, inner: Rect) -> bool {
        let slack = 0.01;
        inner.x >= outer.x - slack
            && inner.y >= outer.y - slack
            && inner.right() <= outer.right() + slack
            && inner.bottom() <= outer.bottom() + slack
    }

    /// The page at the top of Appearance, with the dropdown open on `theme`.
    fn opened(theme: Option<&'static str>) -> Page {
        let mut page = Page::new();
        page.set_category(Category::Appearance, true);
        page.open_dropdown(theme);
        page
    }

    fn geometry(page: &Page, editor: Rect, scale: f32) -> DropdownGeometry {
        let typography = Typography::default();
        let values = values(page.dropdown_choice());
        let rows = page.geometry(editor, scale, &values, true, &typography);
        page.dropdown_geometry(&rows, editor, scale, &typography)
            .expect("the dropdown is open")
    }

    #[test]
    fn the_dropdown_lists_every_theme_under_the_heading_of_its_appearance() {
        // "Dark" over a white page is the failure this split exists to prevent, so each theme is
        // checked against the heading it sits under rather than against a list written out here.
        let picks = theme_picks();
        assert_eq!(picks.first(), Some(&Pick::Heading(Appearance::Dark)));
        let headings: Vec<Appearance> = picks
            .iter()
            .filter_map(|pick| match pick {
                Pick::Heading(appearance) => Some(*appearance),
                Pick::Theme(_) => None,
            })
            .collect();
        assert_eq!(headings, vec![Appearance::Dark, Appearance::Light]);

        let mut under = Appearance::Dark;
        let mut listed = Vec::new();
        for pick in &picks {
            match pick {
                Pick::Heading(appearance) => under = *appearance,
                Pick::Theme(name) => {
                    assert_eq!(
                        Theme::by_name(name).unwrap().appearance,
                        under,
                        "{name} sits under {}",
                        under.title()
                    );
                    listed.push(*name);
                }
            }
        }
        let mut every: Vec<&str> = Theme::BUILT_IN.iter().map(|(name, _, _)| *name).collect();
        every.sort_unstable();
        listed.sort_unstable();
        assert_eq!(listed, every, "every built-in, once");
    }

    #[test]
    fn opening_the_dropdown_highlights_the_theme_in_use() {
        assert_eq!(opened(Some("snow")).dropdown_choice(), Some("snow"));
        assert_eq!(opened(Some("abyss")).dropdown_choice(), Some("abyss"));
        // A theme from a file is on no list; the highlight starts on the first theme, not a heading.
        assert_eq!(opened(None).dropdown_choice(), Some(Theme::grouped()[0]));
    }

    #[test]
    fn the_highlight_steps_over_the_headings_and_stops_at_the_ends() {
        let mut page = opened(Some("abyss"));
        // The last dark theme, then across the "Light" heading to the first light one.
        page.move_dropdown(1);
        assert_eq!(page.dropdown_choice(), Some("linen"));
        page.move_dropdown(-1);
        assert_eq!(page.dropdown_choice(), Some("abyss"));

        page.move_dropdown(isize::MIN / 2);
        assert_eq!(page.dropdown_choice(), Some(Theme::grouped()[0]));
        page.move_dropdown(isize::MAX / 2);
        assert_eq!(page.dropdown_choice(), Theme::grouped().last().copied());
        for _ in 0..40 {
            page.move_dropdown(-1);
            assert!(page.dropdown_choice().is_some(), "never on a heading");
        }
    }

    #[test]
    fn the_dropdown_hangs_under_the_theme_control_and_stays_inside_the_window() {
        for (editor, scale, at) in editors() {
            let page = opened(Some("slate"));
            let typography = Typography::default();
            let rows = page.geometry(editor, scale, &values(Some("slate")), true, &typography);
            let control = rows
                .rows
                .iter()
                .find(|row| row.field() == Some(Field::Theme))
                .and_then(|row| row.control)
                .expect("the theme row is on screen");
            let dropdown = geometry(&page, editor, scale);
            let card = dropdown.card;

            assert!(
                card.x >= editor.x && card.right() <= editor.right() + 0.01,
                "{at}: {card:?}"
            );
            assert!(
                card.y >= editor.y && card.bottom() <= editor.bottom() + 0.01,
                "{at}: {card:?}"
            );
            assert!(card.height > 0.0 && card.width > 0.0, "{at}");
            if !dropdown.above {
                assert!(
                    card.y >= control.bottom(),
                    "{at}: under the control, not over it"
                );
            }
            assert!(card.width >= control.width.min(editor.width), "{at}");
            for row in &dropdown.rows {
                assert!(
                    row.clip.height == 0.0 || inside(dropdown.list, row.clip),
                    "{at}: {row:?} is drawn outside the list"
                );
            }
            // A dropdown too short for its list scrolls rather than running off the window.
            let wanted: f32 = dropdown.rows.iter().map(|row| row.rect.height).sum();
            assert_eq!(
                dropdown.max_scroll > 0.0,
                wanted > dropdown.list.height + 0.01,
                "{at}"
            );
        }
    }

    #[test]
    fn a_heading_is_not_a_target_and_a_theme_row_is() {
        let (editor, scale) = (Rect::new(0.0, 0.0, 1400.0, 900.0), 1.0);
        let page = opened(Some("slate"));
        let dropdown = geometry(&page, editor, scale);
        for row in &dropdown.rows {
            let (x, y) = (
                row.clip.x + row.clip.width / 2.0,
                row.clip.y + row.clip.height / 2.0,
            );
            match row.pick {
                Pick::Heading(_) => assert_eq!(dropdown.hit(x, y), None, "{row:?}"),
                Pick::Theme(_) => assert_eq!(dropdown.hit(x, y), Some(row.index), "{row:?}"),
            }
        }
        assert_eq!(dropdown.hit(dropdown.card.x - 5.0, dropdown.card.y), None);
    }

    #[test]
    fn the_dropdown_scrolls_to_keep_the_highlight_whole() {
        // At 640×480 the list does not fit; walking to the last light theme must scroll the card's
        // list, or Enter picks a row nobody can see.
        let window = Rect::new(0.0, 0.0, 640.0, 480.0);
        let editor = crate::layout::Layout::of(window, 1.0, None, 0.0).editor;
        let typography = Typography::default();
        let mut page = opened(Some("slate"));
        assert!(
            geometry(&page, editor, 1.0).max_scroll > 0.0,
            "it has to scroll here"
        );
        for step in 0..Theme::BUILT_IN.len() {
            page.move_dropdown(1);
            let rows = page.geometry(editor, 1.0, &values(Some("slate")), true, &typography);
            page.reveal_dropdown(&rows, editor, 1.0, &typography);
            let dropdown = geometry(&page, editor, 1.0);
            let selected = page.dropdown().unwrap().selected();
            let row = dropdown.rows[selected];
            assert_eq!(row.clip, row.rect, "step {step}: the highlight is whole");
        }
        assert_eq!(page.dropdown_choice(), Theme::grouped().last().copied());
    }
}
