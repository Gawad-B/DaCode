//! The checklist: what a source offers, and what the user chose to take.
//!
//! Import is opt-in per item. Discovering what is available is separated from importing it, so the
//! editor can show a list and let you tick boxes — and so an import can never quietly rewrite a
//! setting you wanted to keep.

use std::fmt;

/// One thing that can be imported, independently of the others.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImportItem {
    /// Editor preferences from `settings.json`.
    Settings,
    /// Key bindings from `keybindings.json`.
    Keybindings,
    /// A color theme.
    Theme,
    /// Snippet definitions.
    Snippets,
    /// Bracket pairs, comment tokens and auto-closing rules for a language.
    LanguageConfig,
    /// A language server the extension bundles or requires, to be driven over LSP directly.
    LanguageServer,
}

impl ImportItem {
    pub const ALL: [ImportItem; 6] = [
        ImportItem::Settings,
        ImportItem::Keybindings,
        ImportItem::Theme,
        ImportItem::Snippets,
        ImportItem::LanguageConfig,
        ImportItem::LanguageServer,
    ];

    /// The name used on the command line: `--settings`, `--theme`, …
    pub fn flag(self) -> &'static str {
        match self {
            ImportItem::Settings => "settings",
            ImportItem::Keybindings => "keybindings",
            ImportItem::Theme => "theme",
            ImportItem::Snippets => "snippets",
            ImportItem::LanguageConfig => "language-config",
            ImportItem::LanguageServer => "language-server",
        }
    }

    pub fn parse(flag: &str) -> Option<Self> {
        ImportItem::ALL
            .into_iter()
            .find(|item| item.flag() == flag.trim_start_matches('-'))
    }

    pub fn describe(self) -> &'static str {
        match self {
            ImportItem::Settings => "editor preferences (font, tabs, wrap, rulers)",
            ImportItem::Keybindings => "key bindings, where a DaCode equivalent exists",
            ImportItem::Theme => "color theme, converted to DaCode's perceptual palette",
            ImportItem::Snippets => "snippet definitions",
            ImportItem::LanguageConfig => "brackets, comment tokens, auto-close pairs",
            ImportItem::LanguageServer => "a language server, driven over LSP",
        }
    }
}

impl fmt::Display for ImportItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.flag())
    }
}

/// What was selected for import. Empty by default: nothing is taken unless asked for.
#[derive(Debug, Default, Clone)]
pub struct Checklist {
    selected: Vec<ImportItem>,
}

impl Checklist {
    pub fn nothing() -> Self {
        Self::default()
    }

    pub fn everything() -> Self {
        Self {
            selected: ImportItem::ALL.to_vec(),
        }
    }

    pub fn of(items: impl IntoIterator<Item = ImportItem>) -> Self {
        let mut selected: Vec<ImportItem> = items.into_iter().collect();
        selected.sort_unstable();
        selected.dedup();
        Self { selected }
    }

    pub fn with(mut self, item: ImportItem) -> Self {
        if !self.selected.contains(&item) {
            self.selected.push(item);
            self.selected.sort_unstable();
        }
        self
    }

    pub fn wants(&self, item: ImportItem) -> bool {
        self.selected.contains(&item)
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn items(&self) -> &[ImportItem] {
        &self.selected
    }
}

/// What happened to one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Imported, with a count of what came across.
    Imported {
        count: usize,
    },
    /// Offered by the source but not selected.
    Skipped,
    /// Selected, but the source has none of it.
    NotPresent,
    /// Cannot be supported, with the reason. Reported rather than hidden: silence here would look
    /// like a bug in DaCode instead of a deliberate boundary.
    Unsupported {
        reason: String,
    },
    Failed {
        error: String,
    },
}

/// The result of an import, item by item, plus per-entry notes.
#[derive(Debug, Default, Clone)]
pub struct ImportReport {
    entries: Vec<(ImportItem, Outcome)>,
    notes: Vec<String>,
}

impl ImportReport {
    pub fn record(&mut self, item: ImportItem, outcome: Outcome) {
        self.entries.push((item, outcome));
    }

    /// A detail worth telling the user — an unmapped binding, an ignored contribution.
    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    pub fn entries(&self) -> &[(ImportItem, Outcome)] {
        &self.entries
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn imported_count(&self) -> usize {
        self.entries
            .iter()
            .filter_map(|(_, outcome)| match outcome {
                Outcome::Imported { count } => Some(count),
                _ => None,
            })
            .sum()
    }

    pub fn had_failures(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, outcome)| matches!(outcome, Outcome::Failed { .. }))
    }

    /// A human-readable summary, for the terminal.
    pub fn render(&self) -> String {
        let mut out = String::new();

        for (item, outcome) in &self.entries {
            let line = match outcome {
                Outcome::Imported { count } => format!("  {item:<16} imported ({count})"),
                Outcome::Skipped => format!("  {item:<16} skipped (not selected)"),
                Outcome::NotPresent => format!("  {item:<16} nothing to import"),
                Outcome::Unsupported { reason } => format!("  {item:<16} unsupported — {reason}"),
                Outcome::Failed { error } => format!("  {item:<16} failed — {error}"),
            };
            out.push_str(&line);
            out.push('\n');
        }

        if !self.notes.is_empty() {
            out.push_str("\nnotes:\n");
            for note in &self.notes {
                out.push_str("  · ");
                out.push_str(note);
                out.push('\n');
            }
        }

        out
    }
}
