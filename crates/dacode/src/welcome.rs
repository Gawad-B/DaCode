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

use dacode_render::Rect;
use dacode_theme::{Theme, Typography};

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
            // The editor's own default, asked of the type that decides it. This was `"dark"`, so
            // the first screen a new user ever saw was the old warm-brown palette that nothing
            // else draws in any more, and answering it with Enter wrote that into their config.
            theme: Theme::default()
                .name()
                .expect("the default theme is a built-in"),
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
    /// Where the pointer is, so a card can light up under it.
    ///
    /// The wizard draws before there is an editor to ask, and a card that does not react to the
    /// mouse does not look like something you can click — on the one screen where clicking is the
    /// answer to the question being asked.
    pub pointer: (f32, f32),
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
            pointer: (-1.0, -1.0),
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
            self.answers.theme = Theme::grouped()[self.selected];
        }
    }

    /// Moves the highlight a row of the theme grid up or down — `columns` cards — stopping at the
    /// first or last card rather than wrapping into another column.
    pub fn move_row(&mut self, delta: isize, columns: usize) {
        let count = self.options();
        if count == 0 {
            return;
        }
        let to = self.selected as isize + delta * columns.max(1) as isize;
        self.selected = to.clamp(0, count as isize - 1) as usize;
        if self.step == Step::Theme {
            self.answers.theme = Theme::grouped()[self.selected];
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
                self.answers.theme = Theme::grouped()[self.selected];
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
            Step::Theme => Theme::grouped()
                .iter()
                .position(|name| *name == self.answers.theme)
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
            Step::Theme => Theme::grouped()
                .iter()
                .position(|name| *name == self.answers.theme)
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

/// Where the wizard's clickable things are.
///
/// The first question is "do you want the conventional editor or Vim's keys", and the wizard asking
/// it could only be answered with the arrow keys and return. Someone reaching for the mouse to pick
/// *the mouse-friendly option* found nothing to click — the worst possible first impression, and one
/// nobody who already knew the answer would ever notice.
///
/// One definition, read by the drawing code and by the click handler, like everything else here.
pub struct Geometry {
    /// One rectangle per option, in the order they are drawn.
    pub options: Vec<Rect>,
    /// The button that moves on: "continue" on the last step, absent elsewhere because choosing an
    /// option already advances.
    pub next: Option<Rect>,
}

/// How tall a step's content is, below its heading: what the wizard is centred on. One function for
/// [`geometry`] and `chrome::draw_welcome`, which both walk the same stack.
pub fn content_height(step: Step, bounds: Rect, scale: f32, typography: &Typography) -> f32 {
    let px = |v: f32| v * scale;
    match step {
        Step::Keys => px(200.0),
        Step::Theme => {
            ThemeGrid::new(content_width(bounds, scale), bounds, scale, typography).height()
        }
        Step::Import => px(230.0),
        Step::Done => 0.0,
    }
}

/// How wide the wizard's content is: 1000 px at most, with 48 px either side. One function for
/// [`geometry`], [`content_height`] and `chrome::draw_welcome`, which each had a copy.
pub fn content_width(bounds: Rect, scale: f32) -> f32 {
    (1000.0 * scale).min(bounds.width - 96.0 * scale)
}

/// How many columns of theme cards the wizard lays out in this window: what ↑ and ↓ step by.
pub fn theme_columns(bounds: Rect, scale: f32, typography: &Typography) -> usize {
    ThemeGrid::new(content_width(bounds, scale), bounds, scale, typography).columns
}

/// The theme step's cards, as a grid: as many columns as fit cards wide enough for the longest
/// display name, and rows as tall as the window has room for, up to the 200 px a card has when
/// nothing is short. In [`Theme::grouped`]'s order, the dark themes first.
///
/// Thirteen cards in one row — six cards' layout, stretched — were 29 px wide at 640, too narrow for
/// a name. A fixed minimum width then cut four names at 1600×1000, where there was room for all of
/// them; the width is measured from the names now. When the window is too short for the rows that
/// width makes, the grid takes more, narrower columns rather than running off the bottom, and those
/// names are elided — the one case where the window, not the layout, is the limit. A card shorter
/// than its sample shows as many lines of it as fit, and at the smallest window only its colours
/// and its name: honest, if not much of a preview.
struct ThemeGrid {
    columns: usize,
    rows: usize,
    card: (f32, f32),
    gap: f32,
}

impl ThemeGrid {
    /// The narrowest a card gets, whatever its name: room for six lines of sample.
    const MIN_CARD: f32 = 112.0;
    /// The shortest a card gets: its name and its colours.
    const MIN_HEIGHT: f32 = 40.0;

    fn new(width: f32, bounds: Rect, scale: f32, typography: &Typography) -> Self {
        let px = |v: f32| v * scale;
        let count = Theme::BUILT_IN.len();
        let gap = px(14.0);
        // The name is drawn 12 px in from either side, in the label face — measured as the drawing
        // elides it, so a name that fits here is not cut there.
        let longest = Theme::BUILT_IN
            .iter()
            .map(|(_, title, _)| title.chars().count())
            .max()
            .unwrap_or(0);
        let named = longest as f32
            * crate::layout::measured_advance(typography, dacode_theme::Role::Label, scale)
            + px(24.0)
            + 1.0;
        let narrowest = named.max(px(Self::MIN_CARD));
        // The room under the heading when the wizard sits as high as it goes (56 px down, then the
        // mark, the title and the step's heading: 134), less the line of keys 64 px from the bottom
        // and a gap above it.
        let room = bounds.height - px(56.0 + 134.0 + 64.0 + 14.0);
        let fits = |columns: usize| {
            let rows = count.div_ceil(columns) as f32;
            rows * px(Self::MIN_HEIGHT) + (rows - 1.0) * gap <= room
        };
        let mut columns = (((width + gap) / (narrowest + gap)).floor() as usize).clamp(1, count);
        while columns < count && !fits(columns) {
            columns += 1;
        }
        let rows = count.div_ceil(columns);
        let card_width = ((width - gap * (columns as f32 - 1.0)) / columns as f32).floor();
        let card_height = ((room - gap * (rows as f32 - 1.0)) / rows as f32)
            .floor()
            .clamp(px(Self::MIN_HEIGHT), px(200.0));
        Self {
            columns,
            rows,
            card: (card_width, card_height),
            gap,
        }
    }

    fn height(&self) -> f32 {
        self.rows as f32 * self.card.1 + (self.rows as f32 - 1.0) * self.gap
    }
}

/// The layout of the current step. `bounds` is the whole window.
pub fn geometry(welcome: &Welcome, bounds: Rect, scale: f32, typography: &Typography) -> Geometry {
    let px = |v: f32| v * scale;
    let content_width = content_width(bounds, scale);
    let left = (bounds.x + (bounds.width - content_width) / 2.0).round();

    // The same stack `draw_welcome` walks: mark, heading, then the step's own content.
    let content_height = content_height(welcome.step, bounds, scale, typography);
    let header = px(34.0) + px(48.0) + px(26.0) + px(52.0);
    let mark = px(34.0);
    let top = (bounds.y + ((bounds.height - header - content_height) / 2.0).max(px(56.0))).round()
        + mark
        + px(48.0)
        + px(52.0);

    let options = match welcome.step {
        Step::Keys => (0..2)
            .map(|index| {
                let height = px(92.0);
                Rect::new(
                    left,
                    top + index as f32 * (height + px(16.0)),
                    content_width,
                    height,
                )
            })
            .collect(),
        Step::Theme => {
            let grid = ThemeGrid::new(content_width, bounds, scale, typography);
            (0..Theme::BUILT_IN.len())
                .map(|index| {
                    let (row, column) = (index / grid.columns, index % grid.columns);
                    Rect::new(
                        left + column as f32 * (grid.card.0 + grid.gap),
                        top + row as f32 * (grid.card.1 + grid.gap),
                        grid.card.0,
                        grid.card.1,
                    )
                })
                .collect()
        }
        // The list starts below the "found a vscode installation" line.
        Step::Import => (0..crate::settings_ui::Importable::ALL.len())
            .map(|index| {
                let line = px(34.0);
                Rect::new(
                    left,
                    top + px(40.0) + index as f32 * line,
                    content_width,
                    line,
                )
            })
            .collect(),
        Step::Done => Vec::new(),
    };

    let next = (welcome.step == Step::Import).then(|| {
        Rect::new(
            left,
            top + px(40.0) + px(34.0) * crate::settings_ui::Importable::ALL.len() as f32 + px(14.0),
            // Wide enough for its longest label, "start without importing", in the face's real
            // advance; it was 200 and cut that to "start without impor".
            px(260.0).min(content_width),
            px(34.0),
        )
    });

    Geometry { options, next }
}

/// The config the answers amount to.
///
/// Written through the settings tab's own block so there is one format for machine-written
/// configuration, and so opening the settings tab later shows exactly these values.
pub fn config_from(answers: &Answers, existing: &str) -> String {
    let values = crate::settings_ui::Values {
        modal: answers.modal,
        font_size: 13.5,
        tab_width: 4,
        soft_wrap: true,
        format_on_save: false,
        theme: Some(answers.theme),
        // The gutter follows the answer to the first question: someone who chose the conventional
        // editor has no `12j` to type and no use for distances.
        line_numbers: crate::config::LineNumbers::default_for(answers.modal),
        transparency: 0,
        animations: crate::motion::Animations::Full,
        updates: crate::update::Policy::Automatic,
        interface_zoom: crate::chrome::Zoom::default(),
        sticky_scroll: true,
        bracket_colors: true,
        minimap: false,
        inlay_hints: false,
        inline_blame: true,
        // Chosen for them, and it is the right choice: a person who picks the conventional editor is
        // telling you they do not think about saving, and losing their work to that would be the
        // editor's fault, not theirs. Someone who picked Vim knows where `:w` is.
        autosave: !answers.modal,
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
    fn the_wizard_opens_in_the_theme_the_editor_defaults_to() {
        // The first screen a new user sees is drawn in the highlighted answer, so a default other
        // than the editor's is a first impression of a palette nothing else uses.
        let welcome = Welcome::new();
        assert_eq!(welcome.answers.theme, Theme::default().name().unwrap());
        assert_eq!(
            welcome.preview_theme().bg.to_hex(),
            Theme::default().bg.to_hex()
        );

        let mut welcome = Welcome::new();
        welcome.choose();
        assert_eq!(welcome.step, Step::Theme);
        assert_eq!(
            Theme::grouped()[welcome.selected],
            welcome.answers.theme,
            "the theme step starts on it too"
        );
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
    fn the_theme_cards_fit_the_window_at_every_size() {
        // Thirteen cards in one row was six cards' layout stretched: at 640 wide each was 29 px, too
        // narrow for a name, and the row would only get worse with every theme added. They wrap into
        // a grid now, dark themes first, and shrink in height before they leave the window.
        let mut welcome = Welcome::new();
        welcome.choose();
        assert_eq!(welcome.step, Step::Theme);
        for (width, height) in [
            (640.0, 480.0),
            (800.0, 600.0),
            (1280.0, 720.0),
            (1600.0, 1000.0),
        ] {
            for scale in [1.0_f32, 2.0] {
                let bounds = Rect::new(0.0, 0.0, width * scale, height * scale);
                let cards = geometry(&welcome, bounds, scale, &Typography::default()).options;
                assert_eq!(cards.len(), Theme::BUILT_IN.len(), "{width}x{height}");
                // Above the line of keys at the bottom, which sits 64 px up.
                let floor = bounds.bottom() - 64.0 * scale;
                for (at, card) in cards.iter().enumerate() {
                    assert!(
                        card.width >= 100.0 * scale,
                        "{width}x{height} card {at}: {card:?}"
                    );
                    assert!(
                        card.x >= bounds.x && card.right() <= bounds.right(),
                        "{card:?}"
                    );
                    assert!(
                        card.y >= bounds.y && card.bottom() <= floor,
                        "{width}x{height}: {card:?}"
                    );
                    for other in &cards[at + 1..] {
                        assert!(
                            card.intersect(*other).width * card.intersect(*other).height == 0.0,
                            "{width}x{height}: {card:?} overlaps {other:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_theme_names_fit_their_cards_where_the_window_has_room() {
        // At the default size the window has room to spare, and four names were cut anyway because
        // the cards were sized to a fixed minimum rather than to the longest name.
        let mut welcome = Welcome::new();
        welcome.choose();
        let typography = Typography::default();
        for (width, height) in [(1600.0, 1000.0), (1280.0, 720.0)] {
            let bounds = Rect::new(0.0, 0.0, width, height);
            let cards = geometry(&welcome, bounds, 1.0, &typography).options;
            let advance =
                crate::layout::measured_advance(&typography, dacode_theme::Role::Label, 1.0);
            for (card, name) in cards.iter().zip(Theme::grouped()) {
                let title = Theme::display_name_of(name).unwrap();
                let room = card.width - 24.0;
                assert!(
                    title.chars().count() as f32 * advance <= room,
                    "{width}x{height}: `{title}` needs {} px in {room}",
                    title.chars().count() as f32 * advance
                );
            }
        }
    }

    #[test]
    fn up_and_down_on_the_theme_grid_move_by_a_row() {
        // The cards are a grid, so ↓ goes to the card below rather than the one beside it.
        let mut welcome = Welcome::new();
        welcome.choose();
        welcome.selected = 0;
        let columns = 4;
        welcome.move_row(1, columns);
        assert_eq!(welcome.selected, columns);
        assert_eq!(
            welcome.answers.theme,
            Theme::grouped()[columns],
            "and previews it"
        );
        welcome.move_row(-1, columns);
        assert_eq!(welcome.selected, 0);
        // Off the top or the bottom it stops at the first or last card rather than wrapping into a
        // column it was not in.
        welcome.move_row(-1, columns);
        assert_eq!(welcome.selected, 0);
        for _ in 0..10 {
            welcome.move_row(1, columns);
        }
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
            Theme::grouped()[welcome.selected],
            chosen,
            "and the highlight is on it"
        );

        assert!(welcome.back());
        assert_eq!(welcome.step, Step::Keys);
        assert_eq!(welcome.selected, 1, "the keys answer is highlighted too");
        assert!(!welcome.back(), "there is nowhere before the first step");
    }

    #[test]
    fn the_answers_become_config_the_settings_tab_agrees_with() {
        // One format for machine-written configuration: the wizard writes what the panel writes, so
        // opening the panel afterwards shows exactly these values rather than defaults.
        let answers = Answers {
            modal: true,
            theme: "rose-pine-moon",
            import: Vec::new(),
        };
        let source = config_from(&answers, "");
        let config = dacode_lua::eval(&source, "init.lua");

        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.modal, Some(true));
        assert_eq!(config.theme_name.as_deref(), Some("rose-pine-moon"));
    }

    #[test]
    fn a_users_existing_config_is_preserved_if_the_wizard_ever_runs_again() {
        let existing = "dacode.map(\"normal\", \"x\", \"save\")\n";
        let written = config_from(&Answers::default(), existing);
        assert!(written.contains("dacode.map"));
    }

    #[test]
    fn the_absence_of_a_config_file_is_what_makes_it_a_first_run() {
        let dir = std::env::temp_dir().join(format!("dacode-welcome-{}", std::process::id()));
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
