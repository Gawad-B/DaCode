//! The first-run setup.
//!
//! Three questions, in the order that matters most: how you want to type, which theme, and whether to
//! bring your VSCode settings over. Then it writes the answers to `init.lua` and never appears again.
//!
//! It exists because the two decisions that most affect whether someone can use this editor at all —
//! modal keys, and a theme they can read — are decisions a new user cannot discover. An editor that
//! opens in a modal mode with no explanation has already lost the person who did not choose it.
//!
//! **Themes are shown, not named.** "rose-pine" tells you nothing; six lines of syntax-coloured code
//! in it tells you everything. That is the whole reason this is a screen rather than a prompt.

use vex_theme::Theme;

/// Which question is on screen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Step {
    /// Modal keys or conventional ones.
    Keys,
    /// Which of the built-in themes.
    Theme,
    /// Whether to import from VSCode, and what.
    Import,
    /// Everything answered; the caller writes the config and closes.
    Done,
}

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::Keys => "how do you want to type?",
            Step::Theme => "pick a theme",
            Step::Import => "bring anything over from vscode?",
            Step::Done => "ready",
        }
    }

    /// One-based position, for "2 of 3".
    pub fn number(self) -> usize {
        match self {
            Step::Keys => 1,
            Step::Theme => 2,
            Step::Import => 3,
            Step::Done => 3,
        }
    }

    pub const COUNT: usize = 3;
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers {
    /// True for Vim's modal grammar.
    pub modal: bool,
    pub theme: &'static str,
    /// What to import; empty means nothing.
    pub import: Vec<crate::settings_ui::Importable>,
}

impl Default for Answers {
    fn default() -> Self {
        Self {
            // Conventional keys are the default *here* even though the editor's own default is modal:
            // someone being asked the question by a wizard is someone who has not chosen Vim.
            modal: false,
            theme: "dark",
            import: Vec::new(),
        }
    }
}

pub struct Welcome {
    pub step: Step,
    /// Which option on the current step is highlighted.
    pub selected: usize,
    pub answers: Answers,
    /// Set once the import has run, so the last step can report it.
    pub import_report: Option<String>,
}

impl Default for Welcome {
    fn default() -> Self {
        Self::new()
    }
}

impl Welcome {
    pub fn new() -> Self {
        Self {
            step: Step::Keys,
            selected: 0,
            answers: Answers::default(),
            import_report: None,
        }
    }

    /// How many options the current step offers.
    pub fn options(&self) -> usize {
        match self.step {
            Step::Keys => 2,
            Step::Theme => Theme::BUILT_IN.len(),
            // The four importable things, plus "skip".
            Step::Import => crate::settings_ui::Importable::ALL.len() + 1,
            Step::Done => 0,
        }
    }

    /// Moves the highlight, wrapping — a row of five cards is a ring, not a list with ends.
    pub fn move_selection(&mut self, delta: isize) {
        let count = self.options();
        if count == 0 {
            return;
        }
        let at = self.selected as isize + delta;
        self.selected = at.rem_euclid(count as isize) as usize;

        // On the theme step the highlight *is* the answer: moving it previews that theme live, so the
        // choice is made by looking rather than by committing and finding out.
        if self.step == Step::Theme {
            self.answers.theme = Theme::BUILT_IN[self.selected].0;
        }
    }

    /// Whether an import item is ticked.
    pub fn is_ticked(&self, item: crate::settings_ui::Importable) -> bool {
        self.answers.import.contains(&item)
    }

    /// Acts on the highlighted option. Returns true when the step advanced.
    pub fn choose(&mut self) -> bool {
        match self.step {
            Step::Keys => {
                self.answers.modal = self.selected == 1;
                self.advance();
                true
            }
            Step::Theme => {
                self.answers.theme = Theme::BUILT_IN[self.selected].0;
                self.advance();
                true
            }
            Step::Import => {
                let items = crate::settings_ui::Importable::ALL;
                match items.get(self.selected) {
                    // Ticking is not advancing: several things can be wanted at once, and a step that
                    // ended on the first tick would make choosing two of them impossible.
                    Some(item) => {
                        let item = *item;
                        if let Some(at) = self.answers.import.iter().position(|had| *had == item) {
                            self.answers.import.remove(at);
                        } else {
                            self.answers.import.push(item);
                        }
                        false
                    }
                    // The last row: done, whether or not anything is ticked.
                    None => {
                        self.advance();
                        true
                    }
                }
            }
            Step::Done => false,
        }
    }

    /// Goes back a step, or reports that there is nowhere to go.
    pub fn back(&mut self) -> bool {
        let previous = match self.step {
            Step::Keys => return false,
            Step::Theme => Step::Keys,
            Step::Import => Step::Theme,
            Step::Done => Step::Import,
        };
        self.step = previous;
        self.selected = match previous {
            Step::Keys => usize::from(self.answers.modal),
            Step::Theme => Theme::BUILT_IN
                .iter()
                .position(|(name, _)| *name == self.answers.theme)
                .unwrap_or(0),
            _ => 0,
        };
        true
    }

    fn advance(&mut self) {
        self.step = match self.step {
            Step::Keys => Step::Theme,
            Step::Theme => Step::Import,
            Step::Import | Step::Done => Step::Done,
        };
        // Landing on the theme step highlights the theme already chosen, so going back and forth does
        // not silently reset it.
        self.selected = match self.step {
            Step::Theme => Theme::BUILT_IN
                .iter()
                .position(|(name, _)| *name == self.answers.theme)
                .unwrap_or(0),
            _ => 0,
        };
    }

    pub fn is_done(&self) -> bool {
        self.step == Step::Done
    }

    /// The theme to show behind the wizard: whatever is highlighted, so choosing is looking.
    pub fn preview_theme(&self) -> Theme {
        Theme::by_name(self.answers.theme).unwrap_or_default()
    }
}

/// The config the answers amount to.
///
/// Written through the settings panel's own block so there is one format for machine-written
/// configuration, and so opening the settings panel later shows exactly these values.
pub fn config_from(answers: &Answers, existing: &str) -> String {
    let values = crate::settings_ui::Values {
        modal: answers.modal,
        font_size: 13.5,
        tab_width: 4,
        soft_wrap: true,
        format_on_save: false,
        theme: Some(answers.theme),
        sticky_scroll: true,
        inlay_hints: false,
    };
    crate::settings_ui::write_back(existing, &values)
}

/// Whether the wizard should run: true when there is no configuration yet.
///
/// The *absence of a config file* is the signal, rather than a separate "have we run" marker. One
/// fewer piece of state, and it means deleting your config gets you the wizard back — which is what
/// someone who deleted it probably wanted.
pub fn is_first_run(config_dir: &std::path::Path) -> bool {
    !config_dir.join("init.lua").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_ui::Importable;

    #[test]
    fn the_default_answer_is_the_gentler_one() {
        // Someone being asked by a wizard is someone who has not chosen Vim, even though modal is the
        // editor's own default for everyone who never sees this screen.
        assert!(!Answers::default().modal);
    }

    #[test]
    fn the_three_steps_run_in_order_and_end() {
        let mut welcome = Welcome::new();
        assert_eq!(welcome.step, Step::Keys);

        assert!(welcome.choose());
        assert_eq!(welcome.step, Step::Theme);
        assert!(welcome.choose());
        assert_eq!(welcome.step, Step::Import);

        // The import step ends on its last row, not on the first tick.
        welcome.selected = Importable::ALL.len();
        assert!(welcome.choose());
        assert!(welcome.is_done());
    }

    #[test]
    fn choosing_the_second_option_on_the_first_step_means_vim() {
        let mut welcome = Welcome::new();
        welcome.selected = 1;
        welcome.choose();
        assert!(welcome.answers.modal);

        let mut plain = Welcome::new();
        plain.selected = 0;
        plain.choose();
        assert!(!plain.answers.modal);
    }

    #[test]
    fn moving_the_highlight_on_the_theme_step_previews_that_theme() {
        // The whole reason this is a screen: the choice is made by looking, so moving has to change
        // what you are looking at.
        let mut welcome = Welcome::new();
        welcome.choose();
        assert_eq!(welcome.step, Step::Theme);

        let first = welcome.answers.theme;
        welcome.move_selection(1);
        assert_ne!(welcome.answers.theme, first);
        assert_eq!(
            welcome.preview_theme().name(),
            Some(welcome.answers.theme),
            "the preview is the highlighted theme"
        );
    }

    #[test]
    fn the_highlight_wraps_rather_than_stopping() {
        let mut welcome = Welcome::new();
        welcome.choose();

        welcome.selected = Theme::BUILT_IN.len() - 1;
        welcome.move_selection(1);
        assert_eq!(welcome.selected, 0, "past the end comes round");
        welcome.move_selection(-1);
        assert_eq!(welcome.selected, Theme::BUILT_IN.len() - 1);
    }

    #[test]
    fn several_import_items_can_be_ticked() {
        let mut welcome = Welcome::new();
        welcome.choose();
        welcome.choose();
        assert_eq!(welcome.step, Step::Import);

        welcome.selected = 0;
        assert!(!welcome.choose(), "ticking does not advance");
        welcome.selected = 1;
        welcome.choose();

        assert!(welcome.is_ticked(Importable::Settings));
        assert!(welcome.is_ticked(Importable::Theme));
        assert!(!welcome.is_ticked(Importable::Snippets));

        // And ticking again unticks.
        welcome.selected = 0;
        welcome.choose();
        assert!(!welcome.is_ticked(Importable::Settings));
    }

    #[test]
    fn going_back_keeps_what_was_already_answered() {
        let mut welcome = Welcome::new();
        welcome.selected = 1;
        welcome.choose();
        welcome.move_selection(2);
        let chosen = welcome.answers.theme;
        welcome.choose();

        assert!(welcome.back());
        assert_eq!(welcome.step, Step::Theme);
        assert_eq!(
            welcome.answers.theme, chosen,
            "the theme is still the one that was picked"
        );
        assert_eq!(
            Theme::BUILT_IN[welcome.selected].0,
            chosen,
            "and the highlight is on it"
        );

        assert!(welcome.back());
        assert_eq!(welcome.step, Step::Keys);
        assert_eq!(welcome.selected, 1, "the keys answer is highlighted too");
        assert!(!welcome.back(), "there is nowhere before the first step");
    }

    #[test]
    fn the_answers_become_config_the_settings_panel_agrees_with() {
        // One format for machine-written configuration: the wizard writes what the panel writes, so
        // opening the panel afterwards shows exactly these values rather than defaults.
        let answers = Answers {
            modal: true,
            theme: "rose-pine-moon",
            import: Vec::new(),
        };
        let source = config_from(&answers, "");
        let config = vex_lua::eval(&source, "init.lua");

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.modal, Some(true));
        assert_eq!(config.theme_name.as_deref(), Some("rose-pine-moon"));
    }

    #[test]
    fn a_users_existing_config_is_preserved_if_the_wizard_ever_runs_again() {
        let existing = "vex.map(\"normal\", \"x\", \"save\")\n";
        let written = config_from(&Answers::default(), existing);
        assert!(written.contains("vex.map"));
    }

    #[test]
    fn the_absence_of_a_config_file_is_what_makes_it_a_first_run() {
        let dir = std::env::temp_dir().join(format!("vex-welcome-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        assert!(is_first_run(&dir));
        std::fs::write(dir.join("init.lua"), "-- mine\n").unwrap();
        assert!(
            !is_first_run(&dir),
            "a config that exists means setup is done"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
