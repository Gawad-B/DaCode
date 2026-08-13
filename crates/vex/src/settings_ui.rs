//! The settings panel.
//!
//! A list of every option, its current value, and **where that value came from** — default, imported
//! from VSCode, or your own `init.lua`. That last column is the reason this exists rather than being
//! only a file: when a setting is not what you expect, the useful question is which of three places
//! last set it, and no text editor answers that by showing you one of them.
//!
//! Changes apply immediately and are written back to `init.lua` on request, into a clearly marked
//! block at the end of the file. Rewriting the user's own lines is not attempted: config is *code*
//! here, a `vex.set` call may be inside a loop or a condition, and an editor that reformats your
//! program to store a checkbox has overstepped. Appending wins because `init.lua` applies in order,
//! so the panel's block is simply the last word.

use vex_theme::Theme;

/// Where a value came from. The point of the panel.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Source {
    /// The built-in default — nothing has set it.
    Default,
    /// Set by `init.lua`, or by the panel writing to it.
    Config,
    /// Written by `vex import` from a VSCode installation.
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
    StickyScroll,
    InlayHints,
}

impl Field {
    pub const ALL: [Field; 8] = [
        Field::VimKeys,
        Field::FontSize,
        Field::TabWidth,
        Field::SoftWrap,
        Field::FormatOnSave,
        Field::Theme,
        Field::StickyScroll,
        Field::InlayHints,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Field::VimKeys => "vim_keys",
            Field::FontSize => "font_size",
            Field::TabWidth => "tab_width",
            Field::SoftWrap => "soft_wrap",
            Field::FormatOnSave => "format_on_save",
            Field::Theme => "theme",
            Field::StickyScroll => "sticky_scroll",
            Field::InlayHints => "inlay_hints",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Field::VimKeys => "modal editing; off means typing always inserts",
            Field::FontSize => "size of the code face, in pixels",
            Field::TabWidth => "columns a tab stands for",
            Field::SoftWrap => "wrap long lines instead of scrolling sideways",
            Field::FormatOnSave => "reformat through the language server after each save",
            Field::Theme => "one of the five that ship with vex",
            Field::StickyScroll => "pin enclosing block heads above the viewport",
            Field::InlayHints => "show inferred types and parameter names inline",
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

    pub fn item(self) -> vex_import::ImportItem {
        match self {
            Importable::Settings => vex_import::ImportItem::Settings,
            Importable::Theme => vex_import::ImportItem::Theme,
            Importable::Keybindings => vex_import::ImportItem::Keybindings,
            Importable::Snippets => vex_import::ImportItem::Snippets,
        }
    }
}

/// Every row in the panel: the options, then the import checklist, then the action that runs it.
///
/// One list rather than two panes, so `j` and `k` walk the whole thing and there is no second focus
/// to keep track of.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Line {
    Option(Field),
    Import(Importable),
    RunImport,
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
        }
    }

    /// Every line, in display order.
    pub fn lines() -> Vec<Line> {
        Field::ALL
            .iter()
            .map(|field| Line::Option(*field))
            .chain(Importable::ALL.iter().map(|item| Line::Import(*item)))
            .chain(std::iter::once(Line::RunImport))
            .collect()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = Self::lines().len() - 1;
        self.selected = match delta {
            d if d < 0 => self.selected.saturating_sub(d.unsigned_abs()),
            d => (self.selected + d as usize).min(last),
        };
    }

    /// The selected line.
    pub fn line(&self) -> Line {
        let lines = Self::lines();
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
    pub fn checklist(&self) -> vex_import::Checklist {
        self.ticked
            .iter()
            .fold(vex_import::Checklist::nothing(), |list, item| {
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
}

impl Values {
    pub fn get(&self, field: Field) -> String {
        match field {
            Field::VimKeys => on_off(self.modal),
            Field::FontSize => format!("{:.1}", self.font_size),
            Field::TabWidth => self.tab_width.to_string(),
            Field::SoftWrap => on_off(self.soft_wrap),
            Field::FormatOnSave => on_off(self.format_on_save),
            Field::Theme => self.theme.unwrap_or("from a file").to_string(),
            Field::StickyScroll => on_off(self.sticky_scroll),
            Field::InlayHints => on_off(self.inlay_hints),
        }
    }

    /// Changes a field. `delta` is +1 or -1; a toggle ignores its sign.
    pub fn adjust(&mut self, field: Field, delta: i32) {
        match field {
            // Bounds match `Typography::with_code_size`, which clamps anyway — but a panel that lets
            // you set 400 and then silently shows 99 is lying about what it did.
            Field::VimKeys => self.modal = !self.modal,
            Field::FontSize => {
                self.font_size = (self.font_size + delta as f32).clamp(8.0, 32.0);
            }
            Field::TabWidth => {
                self.tab_width = (self.tab_width as i32 + delta).clamp(1, 16) as usize;
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
            Field::StickyScroll => self.sticky_scroll = !self.sticky_scroll,
            Field::InlayHints => self.inlay_hints = !self.inlay_hints,
        }
    }
}

fn on_off(value: bool) -> String {
    if value { "on".into() } else { "off".into() }
}

/// The marker the panel's block is written under.
const MARKER: &str =
    "-- vex: written by the settings panel (ctrl-,). Edit freely — rewritten on save.";

/// Rewrites `init.lua` so the panel's block reflects `values`.
///
/// Everything the user wrote is preserved verbatim; only the block under [`MARKER`] is replaced, and
/// it is appended if absent. `init.lua` applies in order, so the panel's block being last is what
/// makes it authoritative — and moving a setting out of it back into your own code is as simple as
/// deleting the line, because your line runs first only until the block runs after it.
pub fn write_back(existing: &str, values: &Values) -> String {
    let block = format!(
        "{MARKER}\nvex.set {{\n  \
         vim_keys = {},\n  \
         font_size = {:.1},\n  \
         tab_width = {},\n  \
         soft_wrap = {},\n  \
         format_on_save = {},\n  \
         theme = \"{}\",\n  \
         sticky_scroll = {},\n  \
         inlay_hints = {},\n}}\n",
        values.modal,
        values.font_size,
        values.tab_width,
        values.soft_wrap,
        values.format_on_save,
        values.theme.unwrap_or("dark"),
        values.sticky_scroll,
        values.inlay_hints,
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

    fn values() -> Values {
        Values {
            modal: true,
            font_size: 13.5,
            tab_width: 4,
            soft_wrap: true,
            format_on_save: false,
            theme: Some("dark"),
            sticky_scroll: true,
            inlay_hints: false,
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
        assert_eq!(values.font_size, 32.0);
        assert_eq!(values.tab_width, 16);

        for _ in 0..100 {
            values.adjust(Field::FontSize, -1);
            values.adjust(Field::TabWidth, -1);
        }
        assert_eq!(values.font_size, 8.0);
        assert_eq!(values.tab_width, 1, "never zero, which would divide by it");
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

        values.adjust(Field::Theme, 1);
        assert_eq!(values.get(Field::Theme), "dark", "and wraps round");
        values.adjust(Field::Theme, -1);
        assert_eq!(values.get(Field::Theme), "amoled", "backwards too");
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
        assert_eq!(settings.selected, Settings::lines().len() - 1);
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
    fn the_users_own_lines_are_preserved() {
        // The whole reason for a marked block: config is code, and rewriting someone's program to
        // store a checkbox would be overstepping.
        let existing =
            "-- my config\nvex.set { tab_width = 8 }\nvex.map(\"normal\", \"x\", \"save\")\n";
        let written = write_back(existing, &values());

        assert!(written.contains("-- my config"));
        assert!(written.contains("vex.map(\"normal\", \"x\", \"save\")"));
        assert!(
            written.find("-- my config") < written.find(MARKER),
            "the panel's block goes last, so it is the last word"
        );
    }

    #[test]
    fn writing_twice_replaces_the_block_rather_than_stacking_them() {
        let once = write_back("vex.map(\"normal\", \"x\", \"save\")\n", &values());

        let mut changed = values();
        changed.adjust(Field::TabWidth, -2);
        let twice = write_back(&once, &changed);

        assert_eq!(twice.matches(MARKER).count(), 1, "one block, not two");
        assert!(twice.contains("tab_width = 2"));
        assert!(!twice.contains("tab_width = 4"));
        assert!(twice.contains("vex.map"), "and the user's line survived");
    }

    #[test]
    fn lines_added_after_the_block_are_kept() {
        // People do add things below whatever a tool wrote, and losing them would be unforgivable.
        let once = write_back("", &values());
        let edited = format!("{once}\nvex.map(\"normal\", \"z\", \"quit\")\n");
        let again = write_back(&edited, &values());

        assert!(
            again.contains("vex.map(\"normal\", \"z\", \"quit\")"),
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
        let config = vex_lua::eval(&source, "settings.lua");

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
        let lines = Settings::lines();
        assert_eq!(lines.len(), Field::ALL.len() + Importable::ALL.len() + 1);
        assert_eq!(
            lines[0],
            Line::Option(Field::VimKeys),
            "the modal switch comes first: it changes what every other key does"
        );
        assert_eq!(lines[Field::ALL.len()], Line::Import(Importable::Settings));
        assert_eq!(*lines.last().unwrap(), Line::RunImport);
    }

    #[test]
    fn the_selection_walks_the_whole_list_and_stops() {
        let mut settings = Settings::new();
        settings.move_selection(99);
        assert_eq!(settings.line(), Line::RunImport);
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
        assert!(settings.checklist().wants(vex_import::ImportItem::Theme));
        assert!(!settings.checklist().wants(vex_import::ImportItem::Settings));

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
        let config = vex_lua::eval(&source, "settings.lua");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.modal, Some(false));
    }
}
