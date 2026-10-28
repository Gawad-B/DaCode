//! The editor chrome: title bar, activity bar, sidebar, tab bar, gutter, code area, status bar.
//!
//! A pure function of state into a [`Frame`], so it is testable without a GPU and can move into
//! `crates/ui` unchanged. The design's geometry is guidance: proportions are honored, but pixel
//! values derive from the type scale so the whole chrome scales with font size.
//!
//! The code area draws **display rows**, not buffer lines — with soft wrap one line can occupy
//! several rows, and the gutter figure belongs only to the first of them.

use crate::motion::Key;
use crate::palette::{Palette, PaletteKind};
use crate::prompt::Prompt;
use crate::results::Results;
use dacode_buffer::Document;
use dacode_display::{DisplayMap, ScrollAnchor, WrapMode};
use dacode_render::{Align, Frame, Icon, Layer, Rect, TextRun};
use dacode_syntax::{Highlight, HighlightSpan};
use dacode_text::{Bias, CharOffset, Point, SelectionSet, Text, Version};
use dacode_theme::{CursorShape, Mode, Oklch, Role, Theme, Typography, frame as border, space};

/// One pane's view of a buffer, for drawing.
///
/// Only the focused pane's document is the live one in [`EditorState`]; the rest are borrowed from
/// the buffer list. Passing them in as views rather than reaching for them here keeps the chrome a
/// pure function of what it is given, which is what makes it testable without an editor around it.
pub struct PaneView<'a> {
    pub document: &'a Document,
    pub scroll: ScrollAnchor,
    pub path: &'a [String],
    pub focused: bool,
    /// Highlight spans for this buffer.
    ///
    /// An unfocused pane cannot be edited — keys only reach the focused one — so the spans computed
    /// when it was last focused are still correct for its text. Missing spans render as plain text,
    /// which degrades honestly rather than painting one file's colors onto another's.
    pub highlights: &'a [HighlightSpan],
}

/// The selections each `expand_selection` started from, so `shrink_selection` can retrace them.
///
/// The seventh thing outside the document that holds offsets into it (see `CLAUDE.md`), and it is
/// **cleared** by [`EditorState::mutate`] rather than moved: a history of how far a selection grew
/// is worth nothing once the text under it has changed, and shrinking from wherever the selection
/// is now is what the tree fallback already does. It is also cleared on a buffer change, and it
/// is valid only while the selections are still exactly the ones the last expansion left — any
/// other move, a click, a `vaf`, ends the run without anything having to say so, the same way
/// [`EditorState::vertical_goal`] ends.
#[derive(Debug, Clone, Default)]
pub struct Expansions {
    /// What the selections were before each expansion, oldest first.
    before: Vec<SelectionSet>,
    /// What the last expansion left them as.
    after: Option<SelectionSet>,
}

impl Expansions {
    /// Records one expansion: from `before` to `after`. A history whose end no longer matches
    /// `before` belonged to some earlier run and is dropped first.
    pub fn push(&mut self, before: SelectionSet, after: SelectionSet) {
        if self.after.as_ref() != Some(&before) {
            self.before.clear();
        }
        self.before.push(before);
        self.after = Some(after);
    }

    /// The selections to go back to, when `current` is still what the last expansion left.
    pub fn pop(&mut self, current: &SelectionSet) -> Option<SelectionSet> {
        if self.after.as_ref() != Some(current) {
            self.clear();
            return None;
        }
        let previous = self.before.pop()?;
        self.after = (!self.before.is_empty()).then(|| previous.clone());
        Some(previous)
    }

    pub fn clear(&mut self) {
        self.before.clear();
        self.after = None;
    }
}

pub struct EditorState {
    pub mode: Mode,
    pub theme: Theme,
    /// The theme `toggle_theme` last left, when its counterpart does not lead back to it: True black
    /// and Slate toggle to Linen, whose own other half is Walnut. A second toggle goes back to this
    /// instead.
    pub theme_toggled_from: Option<Theme>,
    pub typography: Typography,

    pub document: Document,
    /// Bracket pairing and indentation rules for the active buffer's language.
    pub pairs: dacode_buffer::PairConfig,
    /// Reformat through the language server after each save.
    pub format_on_save: bool,
    pub display: DisplayMap,
    pub scroll: ScrollAnchor,
    pub path: Vec<String>,
    /// The file on disk, if any. `None` for a scratch buffer.
    pub file_path: Option<String>,

    /// Partially-typed grammar (count, register, operator) — cleared when it resolves.
    pub pending: String,

    pub syntax_version: Version,
    pub diagnostics_version: Version,
    /// Highlight spans for the visible range. Only ever set from a result whose version matched
    /// the buffer — a stale result is discarded, never painted.
    pub highlights: Vec<HighlightSpan>,
    /// The tree-sitter ancestor chain at the cursor.
    pub node_path: Vec<String>,
    pub syntax_error: bool,
    /// The parse tree the last highlight result was read from, for indentation and the structural
    /// text objects. **Neither moved nor cleared by [`EditorState::mutate`]**, unlike the spans:
    /// it is tagged with the version it was parsed from, and every reader goes through
    /// [`EditorState::fresh_structure`], which refuses it the moment an edit moves the buffer on.
    /// Moving a tree by hand would be re-implementing tree-sitter's incremental edit on the UI
    /// thread; ignoring it until the worker sends the next one costs nothing.
    pub structure: Option<dacode_syntax::Structure>,
    /// How the selections got to be the size they are, for `shrink_selection`. Cleared by
    /// [`EditorState::mutate`]; see [`Expansions`].
    pub expansions: Expansions,

    pub frame_ms: f32,
    pub worst_ms: f32,

    /// The command palette, when open. It overlays everything and owns the keyboard.
    pub palette: Option<Palette>,
    /// The `/` or `:` prompt, when open. Takes over the statusline.
    pub prompt: Option<Prompt>,
    /// Project-search results, when the panel is open.
    pub results: Option<Results>,
    /// Matches of the last search, highlighted in the text.
    pub search_matches: Vec<std::ops::Range<CharOffset>>,
    /// A hover reply from the language server, shown near the cursor until dismissed.
    pub hover: Option<String>,
    /// The open completion session, if any.
    pub completion: Option<crate::completion::Completion>,
    /// A request the executor wants sent, picked up by the caller that owns the LSP client.
    pub pending_request: Option<dacode_input::Action>,
    /// The project root, for showing paths relatively.
    pub project_root: std::path::PathBuf,
    /// Diagnostics for the active buffer, and the version they describe.
    pub diagnostics: dacode_lsp::Diagnostics,
    /// What the language server is called, or why there isn't one.
    pub server_status: String,
    /// How many buffers are open, for the tab strip.
    pub buffer_count: usize,
    /// The name of each open buffer, in order, and whether it has unsaved changes.
    ///
    /// Refreshed when the buffer list changes rather than read through a borrow, because the
    /// drawing code is handed `&EditorState` and the buffers live beside it on the app.
    pub buffer_tabs: Vec<(String, bool)>,
    /// Index of the active buffer.
    pub active_buffer: usize,
    /// Whether Vim's modal grammar is in force. Shown in the statusline, since with it off there is
    /// no mode to report and saying "NORMAL" would be a lie.
    pub modal: bool,
    /// Whether to pin enclosing block heads above the viewport.
    pub sticky_scroll: bool,
    /// The column each caret is keeping to through a run of up and down moves, beside the offsets
    /// the run left the carets at. Valid only while the carets are still exactly there: any other
    /// move, a click or an edit changes them, which ends the run without anything having to say so.
    pub vertical_goal: Option<(Vec<dacode_text::CharOffset>, Vec<usize>)>,
    /// The search view in the sidebar, kept while it is closed so reopening it finds the last
    /// query and its results where they were.
    pub search: Option<crate::search_panel::SearchPanel>,
    /// The source-control view: what git last said, the commit message, the selection. Kept while
    /// the view is closed, like the search's, and asked about at startup so the status line can
    /// name the branch without the view ever having been opened.
    pub scm: crate::source_control::View,
    /// What the active buffer is, when it is a diff or the history rather than a file: **read
    /// only**, which [`EditorState::mutate`] enforces. Travels with its buffer, as the highlights do.
    pub review: Option<crate::scm::Show>,
    /// The macro register being recorded into, if any.
    pub recording: Option<char>,
    /// What changed since the last commit, per line.
    pub diff: crate::git::Diff,
    /// Who last changed each line, and whether to show it. **Neither moved nor cleared** by
    /// [`EditorState::mutate`], which only appends each change's line arithmetic to its log: a line is
    /// looked up by walking that log back to where it was when the blame was taken. See
    /// [`crate::blame`] for why that, rather than moving every entry per keystroke.
    pub blame: crate::blame::Tracker,
    /// Whether the sidebar shows the diagnostics list instead of the structure panel.
    pub show_diagnostics: bool,
    /// Every problem the language server has reported, across the project.
    pub problems: crate::problems::Store,
    /// Inlay hints for the visible range, when they are on.
    pub inlay_hints: Vec<dacode_lsp::lsp_types::InlayHint>,
    /// Whether to ask for and draw them. Off by default: hints move the code sideways as you type,
    /// and that is a preference, not a default worth imposing.
    pub show_inlay_hints: bool,
    /// First-run setup, when it is showing. It owns the whole window while it does.
    pub welcome: Option<crate::welcome::Welcome>,
    /// The settings tab, while it is open: its page, search, selection and scroll.
    ///
    /// **A tab that is not a buffer.** It is not in the buffer list and has no index there, so
    /// nothing that counts buffers — `buffer_key`, `stored_index`, the panes — knows it exists, and
    /// opening or closing it cannot renumber a file. The tab strip draws it after the files (see
    /// [`EditorState::settings_tab`]); whether it is the tab on screen is `settings_active`.
    pub settings_page: Option<crate::settings_ui::Page>,
    /// Whether the settings tab is the one the editor area shows. Only meaningful while
    /// `settings_page` is open; [`EditorState::settings_showing`] asks both.
    ///
    /// It takes the keys as the text would — focus stays [`crate::focus::Focus::Editor`] — because it
    /// occupies the place the text does: a click on it is a click in the editor area, `ctrl-w` closes
    /// it as it would a file, and the sidebar and the panel keep their own focus beside it.
    pub settings_active: bool,
    /// The file tree, when open. It takes the sidebar over rather than adding a third column: the
    /// window is finite, and a tree squeezed beside the structure panel would be too narrow to read
    /// a path in.
    pub tree: Option<crate::tree::FileTree>,
    /// Where the keys go. A view being open does not make it the target — clicking it does.
    pub focus: crate::focus::Focus,
    /// The terminal panel: whether it is shown and what it last showed. The shell itself belongs to
    /// the event loop, which owns every process.
    pub terminal: crate::terminal::Panel,
    /// Write the file a moment after typing stops, rather than waiting to be asked.
    ///
    /// Off by default, because writing someone's file without being told to is not a default an
    /// editor gets to choose for them — but on for anyone who says so, since half the people who
    /// open an editor have never pressed ctrl-s in their lives and should not lose work for it.
    pub autosave: bool,
    /// What the gutter counts: absolute numbers, distance from the cursor, or nothing.
    pub line_numbers: crate::config::LineNumbers,
    /// Where the pointer is, in physical pixels.
    ///
    /// The chrome needs it to light a button under the cursor. A toolbar that does not react to the
    /// pointer does not read as clickable, and a control nobody believes is clickable is one nobody
    /// clicks — which would defeat the entire point of having it.
    pub pointer: (f32, f32),
    /// The help panel, when open. `F1`, and the answer to "how does this thing work".
    pub help: Option<crate::help::Help>,
    /// The menu bar or a context menu, when one is open.
    pub menu: Option<crate::menu::Menu>,
    /// What the window chrome remembers that the document does not.
    pub chrome: Chrome,
}

/// Overlays that have closed and are still fading out.
///
/// The palette, a menu and the help panel drop their state the moment they close, and a fade-out
/// needs something to fade. What closed is moved here by [`EditorState::close_palette`] and its
/// siblings — a move, not a copy — drawn at the fading value, and forgotten once it reaches zero or
/// the overlay opens again. **Nothing here takes input**: the mouse and the keys read the live
/// fields, so a card on its way out cannot be clicked or typed into.
#[derive(Debug, Default)]
pub struct Leaving {
    pub palette: Option<Box<Palette>>,
    pub menu: Option<crate::menu::Menu>,
    pub help: Option<crate::help::Help>,
}

/// State belonging to the window's chrome rather than to any document.
#[derive(Debug, Default)]
pub struct Chrome {
    /// The card of an overlay open over the page, where the pointer belongs to the overlay.
    ///
    /// Drawing reads `pointer` for every wash, and the theme dropdown is drawn after the page it
    /// covers, so a row behind the list lit up as the pointer crossed the list — two highlights at
    /// once, one of them on a row a click could not reach. Set by the event loop beside the motion
    /// targets, from the same geometry the click reads.
    pub covered: Option<Rect>,
    /// The sidebar view chosen from the activity bar, for the views that keep no state of their own.
    /// The explorer, problems and settings are open when their own state is, as they always were.
    pub activity: Option<crate::layout::Activity>,
    /// Every message the editor has shown, oldest first, so one that vanished at the next keystroke
    /// can still be read from the bell. Bounded: this is a log for a person, not a history.
    pub notifications: Vec<String>,
    /// How many of them arrived since the bell was last opened.
    pub unread: usize,
    pub show_notifications: bool,
    /// The `updates` setting.
    pub updates: crate::update::Policy,
    /// An update on offer, drawn at the top of the bell's panel with its buttons. Set by the event
    /// loop, which owns the updater; drawing and the click both read [`notifications_panel`].
    pub update: Option<crate::update::Offer>,
    /// How see-through the window's backgrounds are: 0 is opaque.
    pub transparency: f32,
    /// Every animated quantity. Drawing reads it at [`EditorState::now`]; the executor and the event
    /// loop retarget it, at the same `now()`, which outside a frame is the wall clock.
    pub motion: crate::motion::Motion,
    /// The instant the frame being drawn is *for*, set only while a frame is drawn. `redraw` sets it to
    /// the wall clock just before building the frame and puts it back to `None` afterwards, so input
    /// handlers and the executor, which run between frames, read the real clock and never the time of a
    /// frame drawn minutes ago. A capture sets it to a chosen moment (`--at-ms`) and never clears it,
    /// which is how a transition is photographed part-way. `None` means "now".
    pub now: Option<std::time::Instant>,
    /// The focused pane's scrolling that has not yet made a whole row: a touchpad's pixels, and a wheel
    /// glide's as it travels. Drawn as how far the top row has moved up out of view. Here rather than
    /// on the event loop because drawing reads it, and a capture (`--scroll-px`) has no event loop.
    pub scroll_input: crate::scroll::ScrollInput,
    /// What is fading out. See [`Leaving`].
    pub leaving: Leaving,
    /// Whether the activity bar's marker was showing at the last [`EditorState::sync_motion`], so
    /// that a marker appearing is put in place rather than slid in from wherever it last was.
    pub marker_shown: bool,
    /// How many tabs there were at the last sync, so a new one can be grown in. `None` before the
    /// first, which is a window opening and not a tab.
    pub tabs_seen: Option<usize>,
}

impl Chrome {
    /// How many messages the bell keeps.
    pub const KEEP: usize = 100;

    /// Records a message for the bell. A repeat of the last one is not recorded twice: autosave
    /// saying "saved" every pause in typing would otherwise fill the log with one word.
    pub fn notify(&mut self, message: &str) {
        if message.is_empty()
            || self
                .notifications
                .last()
                .is_some_and(|last| last == message)
        {
            return;
        }
        self.notifications.push(message.to_string());
        if self.notifications.len() > Self::KEEP {
            self.notifications.remove(0);
        }
        if !self.show_notifications {
            self.unread += 1;
        }
        // A message is an event, not a state, so the bell is kicked here rather than synced. The
        // frame's instant while one is drawn, the wall clock otherwise — the same rule as `now()`.
        let now = self.now.unwrap_or_else(std::time::Instant::now);
        self.motion.kick(
            crate::motion::Key::Bell,
            crate::motion::Spring::WOBBLE_KICK,
            crate::motion::Spring::WOBBLE,
            now,
        );
    }
}

/// Where an overlay grows from as it arrives: the edge it hangs from, or its middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Top,
    Bottom,
    Centre,
}

/// The scale an overlay at `progress` is drawn at, about `anchor` on `rect`: 0.96 at nothing, 1 at
/// arrival. A spring overshooting past 1 grows it a hair past full size, which is the "expressive"
/// part; anything wilder is clamped.
pub fn appear_zoom(rect: Rect, progress: f32, anchor: Anchor) -> dacode_render::Zoom {
    let progress = if progress.is_finite() {
        progress.clamp(0.0, 1.25)
    } else {
        1.0
    };
    let x = rect.x + rect.width / 2.0;
    let y = match anchor {
        Anchor::Top => rect.y,
        Anchor::Bottom => rect.bottom(),
        Anchor::Centre => rect.y + rect.height / 2.0,
    };
    dacode_render::Zoom::about([x, y], 0.96 + 0.04 * progress)
}

/// `rect` scaled about its `anchor` from 0.96 to 1 as `progress` goes from 0 to 1.
///
/// Callers fade every colour by `progress.clamp(0.0, 1.0)` as well, which [`overlay`] does through
/// [`Frame::set_opacity`] for everything a card draws at once.
pub fn appear(rect: Rect, progress: f32, anchor: Anchor) -> Rect {
    appear_zoom(rect, progress, anchor).rect(rect)
}

/// Draws an overlay that fades in and out: everything `draw` adds is faded by `fade`, and nothing at
/// all is drawn at zero. `draw` scales its own card with [`appear_zoom`] once it knows where the card
/// is; both are lifted here, so an early return inside it cannot leave the rest of the frame faded.
fn overlay(frame: &mut Frame, fade: f32, draw: impl FnOnce(&mut Frame)) {
    let fade = fade.clamp(0.0, 1.0);
    if fade <= 0.0 {
        return;
    }
    frame.set_opacity(fade);
    draw(frame);
    frame.set_opacity(1.0);
    frame.set_zoom(None);
}

/// What the settings tab is called in the tab strip.
pub const SETTINGS_TAB: &str = "Settings";

/// The corner radius of every card, at scale 1.
pub const CARD_RADIUS: f32 = 8.0;

/// The one card every overlay is drawn on: a soft shadow, the panel surface, a hairline outline.
///
/// **Every overlay goes through here** — the palette, the menus, completion, the hover box, the
/// which-key hint, the help panel, the bell's log — so they read as one family rather than as the
/// separately-invented rectangles they were, each with its own shadow offset and a border on three
/// sides out of four.
///
/// The shadow is two translucent rounded fills offset downward: one close and firmer, one wide and
/// faint, at a fraction of the theme's `shadow` so it stays a hint in a light theme and does not
/// turn into a black halo in a dark one. It is drawing only — nothing tests a click against it, and
/// a click that lands on it is a click outside the card. It goes through [`Frame::fill_rounded`],
/// not [`Frame::fill_backdrop`]: with the window made translucent the card stays solid, as every
/// overlay does, and the shadow only darkens what is behind it rather than punching through it.
///
/// `alpha` fades all of it together, shadow included. An overlay already fades through the frame's
/// opacity (see [`overlay`]) and passes 1; this is for a card drawn outside one.
pub fn card(frame: &mut Frame, theme: &Theme, rect: Rect, scale: f32, alpha: f32) {
    let alpha = if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if alpha <= 0.0 || !rect.is_visible() {
        return;
    }
    let px = |v: f32| v * scale;
    let radius = px(CARD_RADIUS);
    let faded = |color: Oklch, by: f32| color.with_alpha(color.alpha * by * alpha);

    frame.fill_rounded(
        Rect::new(
            rect.x - px(6.0),
            rect.y - px(1.0),
            rect.width + px(12.0),
            rect.height + px(13.0),
        ),
        faded(theme.shadow, 0.22),
        radius + px(6.0),
    );
    frame.fill_rounded(
        Rect::new(
            rect.x - px(1.0),
            rect.y + px(1.0),
            rect.width + px(2.0),
            rect.height + px(3.0),
        ),
        faded(theme.shadow, 0.35),
        radius + px(1.0),
    );
    frame.fill_rounded(rect, faded(theme.bg_panel, 1.0), radius);
    frame.outline_rounded(rect, faded(theme.rule, 1.0), radius, border::BORDER * scale);
}

/// Keys, drawn as small rounded chips ending at `right` and centred in the row `y..y + height`.
/// Returns the left edge of the first chip, so the caller can fit its text before it; `right`
/// itself when there are no keys.
///
/// `keys` is what [`dacode_input::commands::Command::shown`] gives — desktop notation, or Vim's
/// with Vim keys on — and one chip is one press (see [`crate::layout::key_presses`]).
pub fn key_chip(
    frame: &mut Frame,
    state: &EditorState,
    keys: &str,
    right: f32,
    y: f32,
    height: f32,
    scale: f32,
) -> f32 {
    let theme = &state.theme;
    let advance = crate::layout::measured_advance(&state.typography, Role::Value, scale);
    let chip_height = (18.0 * scale).min(height);
    let top = y + (height - chip_height) / 2.0;
    let mut left = right;
    for (index, press) in crate::layout::key_presses(keys).iter().rev().enumerate() {
        if index > 0 {
            left -= crate::layout::CHIP_GAP * scale;
        }
        let width = press.chars().count() as f32 * advance + crate::layout::CHIP_PAD * 2.0 * scale;
        left -= width;
        let chip = Rect::new(left, top, width, chip_height);
        frame.fill_rounded(chip, theme.bg, 4.0 * scale);
        frame.outline_rounded(chip, theme.rule_soft, 4.0 * scale, border::BORDER * scale);
        frame.text(run(
            state,
            Role::Value,
            *press,
            chip,
            theme.ink_dim,
            Align::Center,
            scale,
        ));
    }
    left
}

/// A key hint along a card's foot: chips, then what they do, starting at `x` in `row`. Returns
/// where the next one starts, or `None` when this one does not fit before the row's right edge —
/// and then nothing is drawn, because half a hint is worse than one fewer.
fn hint(
    frame: &mut Frame,
    state: &EditorState,
    keys: &str,
    words: &str,
    x: f32,
    row: Rect,
    scale: f32,
) -> Option<f32> {
    let limit = row.right();
    let chips = crate::layout::chips_width(&state.typography, keys, scale);
    let text = words.chars().count() as f32
        * crate::layout::measured_advance(&state.typography, Role::Ui, scale);
    let end = x + chips + 6.0 * scale + text;
    if end > limit {
        return None;
    }
    key_chip(frame, state, keys, x + chips, row.y, row.height, scale);
    frame.text(ui(
        state,
        words,
        Rect::new(
            x + chips + 6.0 * scale,
            row.y,
            text + 2.0 * scale,
            row.height,
        ),
        state.theme.ink_faint,
        Align::Left,
        scale,
    ));
    Some(end + 16.0 * scale)
}

/// A row's highlight inside a card: the selection, or a lighter wash under the pointer.
///
/// Both are translucent over the card rather than a surface colour of the theme's: `row_active` is
/// the line under the caret, and in the light themes it is *lighter* than the panel a card is made
/// of, so a selection painted in it was all but invisible. The mode colour, faint, reads as "this
/// one" on every theme, and the ink at a few percent as "the pointer is here".
fn row_wash(frame: &mut Frame, state: &EditorState, row: Rect, selected: bool, scale: f32) {
    let theme = &state.theme;
    if selected {
        let accent = theme.accent(state.mode);
        frame.fill_rounded(row, accent.with_alpha(0.16), 5.0 * scale);
    } else if hovered(state, row) {
        frame.fill_rounded(row, theme.ink.with_alpha(0.06), 5.0 * scale);
    }
}

/// [`row_wash`] for a row of the overlay that is itself the covering card, which [`hovered`] would
/// refuse.
fn overlay_row_wash(frame: &mut Frame, state: &EditorState, row: Rect, selected: bool, scale: f32) {
    let theme = &state.theme;
    if selected {
        frame.fill_rounded(row, theme.accent(state.mode).with_alpha(0.16), 5.0 * scale);
    } else if pointer_in(state, row) {
        frame.fill_rounded(row, theme.ink.with_alpha(0.06), 5.0 * scale);
    }
}

/// A card's text field: the query, or a placeholder saying what to type, with a caret.
///
/// `reserve` is room kept free at the right for something the caller draws there — the palette's
/// "4 of 1284" — so a long query gives way to it rather than running underneath it. Returns that
/// strip.
fn draw_field(
    frame: &mut Frame,
    state: &EditorState,
    field: Rect,
    query: &str,
    placeholder: &str,
    reserve: f32,
    scale: f32,
) -> Rect {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill_rounded(field, theme.bg, 6.0 * scale);
    frame.outline_rounded(
        field,
        accent.with_alpha(accent.alpha * 0.8),
        6.0 * scale,
        border::BORDER * scale,
    );
    let icon = Rect::new(field.x + 10.0 * scale, field.y, 14.0 * scale, field.height);
    frame.icon(Icon::Search, centred(icon, 14.0 * scale), theme.ink_faint);
    let whole = Rect::new(
        icon.right() + 8.0 * scale,
        field.y,
        (field.right() - 10.0 * scale - (icon.right() + 8.0 * scale)).max(0.0),
        field.height,
    );
    let (reserved, text) = whole.split_right(reserve.min(whole.width));
    let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);
    let typed = if query.is_empty() {
        frame.text(ui(
            state,
            elide(state, Role::Ui, placeholder, text.width, scale),
            text,
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        0
    } else {
        // The end of what was typed is the part being worked on, so a query too long for the field
        // loses its start rather than its end.
        let shown = elide_start(state, Role::Ui, query, text.width - 4.0 * scale, scale);
        frame.text(ui(
            state,
            shown.clone(),
            text,
            theme.ink,
            Align::Left,
            scale,
        ));
        shown.chars().count()
    };
    // The caret says where typing goes — before the placeholder, which is what it replaces.
    let caret = if typed == 0 {
        text.x - 3.0 * scale
    } else {
        (text.x + typed as f32 * advance).min(text.right())
    };
    frame.fill(
        Rect::new(
            caret,
            field.y + (field.height - 18.0 * scale) / 2.0,
            1.5 * scale,
            18.0 * scale,
        ),
        accent,
    );
    reserved
}

impl EditorState {
    /// The time this frame is drawn at. Drawing code asks here instead of calling `Instant::now`, so a
    /// frame is a pure function of the state and one instant, and a test can draw any instant it likes.
    pub fn now(&self) -> std::time::Instant {
        self.chrome.now.unwrap_or_else(std::time::Instant::now)
    }

    /// Whether any view is open in the sidebar.
    pub fn sidebar_open(&self) -> bool {
        self.tree.is_some() || self.show_diagnostics || self.chrome.activity.is_some()
    }

    /// Whether the sidebar is showing a view that takes keys of its own: the tree, or the search
    /// view's boxes. The problems list and the placeholders are looked at, so giving them the
    /// keyboard would only make it disappear.
    pub fn sidebar_takes_keys(&self) -> bool {
        self.tree.is_some() || self.search_showing() || self.source_control_showing()
    }

    /// Whether the sidebar is showing the source-control view.
    pub fn source_control_showing(&self) -> bool {
        self.tree.is_none()
            && !self.show_diagnostics
            && self.chrome.activity == Some(crate::layout::Activity::SourceControl)
    }

    /// Shows the source-control view with the keys in its message box, as VSCode's key does.
    pub fn open_source_control(&mut self) {
        self.close_sidebar();
        self.chrome.activity = Some(crate::layout::Activity::SourceControl);
        self.scm.field = crate::source_control::Field::Message;
        self.focus = crate::focus::Focus::Sidebar;
    }

    /// Shows the search view with the keys in its query box, searching for `query` unless it is
    /// empty or already what the view is searching for.
    pub fn open_search(&mut self, query: &str) {
        let root = self.project_root.clone();
        self.close_sidebar();
        self.chrome.activity = Some(crate::layout::Activity::Search);
        let panel = self
            .search
            .get_or_insert_with(|| crate::search_panel::SearchPanel::new(""));
        if !query.is_empty() && query != panel.query {
            panel.query = query.to_string();
            panel.restart(&root);
        }
        panel.field = crate::search_panel::Field::Query;
        self.focus = crate::focus::Focus::Sidebar;
    }

    /// Whether the sidebar is showing the search view.
    pub fn search_showing(&self) -> bool {
        self.tree.is_none()
            && !self.show_diagnostics
            && self.chrome.activity == Some(crate::layout::Activity::Search)
            && self.search.is_some()
    }

    /// Whether the editor area is showing the settings page rather than code.
    pub fn settings_showing(&self) -> bool {
        self.settings_active && self.settings_page.is_some()
    }

    /// The blame column for [`crate::layout::Layout::with_blame`]: in the focused pane, at its
    /// wanted width, while it is switched on and there is a blame for it — or one on its way.
    ///
    /// The one answer drawing, the mouse and the capture all build their layout from. A column that
    /// one of them reserved and another did not would be the click-lands-elsewhere defect
    /// `layout.rs` exists to prevent, by a column's width. Absent where git has nothing to say, so an
    /// untracked file is not given an empty strip.
    pub fn blame_column(&self, focused: usize, scale: f32) -> Option<(usize, f32)> {
        let has = self.blame.blame().is_some() || self.blame.is_pending();
        (self.blame.column && has && !self.settings_showing()).then(|| {
            (
                focused,
                crate::layout::blame_column_width(&self.typography, scale),
            )
        })
    }

    /// The settings tab's place in the tab strip, while it is open: after every file, one past the
    /// last buffer's number. A number the buffer list never hands out, so a tab carrying it cannot
    /// be mistaken for a file by anything that switches to or closes one.
    pub fn settings_tab(&self) -> Option<usize> {
        // After the files, which with nothing open is first.
        let files = if self.nothing_open() {
            0
        } else {
            self.buffer_count.max(1)
        };
        self.settings_page.as_ref().map(|_| files)
    }

    /// The tab the strip shows as active: the settings tab while it is showing, the live buffer
    /// otherwise.
    pub fn active_tab(&self) -> usize {
        match self.settings_tab() {
            Some(tab) if self.settings_active => tab,
            // No file's tab is drawn, so none is active; past the end marks none.
            _ if self.nothing_open() => usize::MAX,
            _ => self.active_buffer,
        }
    }

    /// What each tab is called, in strip order: every open buffer, then "Settings" while the
    /// settings tab is open. The one list both the drawing and the mouse lay the strip out from.
    ///
    /// The buffers' own list can be shorter than the count in the single-pane drawing path, and a
    /// missing name is filled from the path.
    pub fn tab_names(&self) -> Vec<String> {
        // Nothing open is an empty strip, as in any editor, rather than one tab called `untitled`.
        let tabs = if self.nothing_open() {
            0
        } else {
            self.buffer_count.max(1)
        };
        let mut names: Vec<String> = (0..tabs)
            .map(|index| {
                self.buffer_tabs
                    .get(index)
                    .map(|(name, _)| name.clone())
                    .or_else(|| self.path.last().cloned())
                    .unwrap_or_else(|| "untitled".to_string())
            })
            .collect();
        if self.settings_page.is_some() {
            names.push(SETTINGS_TAB.to_string());
        }
        names
    }

    /// Whether the one buffer is the empty scratch buffer the editor starts with: no file, no text,
    /// no other buffer beside it. A file opened after it replaces it rather than sitting beside it.
    pub fn nothing_open(&self) -> bool {
        self.buffer_count <= 1
            && self.file_path.is_none()
            && !self.document.is_dirty()
            && self.document.text().len_chars() == dacode_text::CharOffset(0)
    }

    /// Puts the settings page's scroll where its target is, with no glide: for a new page or a new
    /// search, whose rows are not the ones the scroll was travelling through.
    pub fn snap_settings_scroll(&mut self) {
        if let Some(page) = &self.settings_page {
            self.chrome
                .motion
                .snap(crate::motion::Key::SettingsScroll, page.scroll());
        }
    }

    /// The values the settings page shows and edits, read from the live editor rather than from a
    /// copy — by the drawing and by the code that changes them, so the two cannot disagree.
    pub fn setting_values(&self) -> crate::settings_ui::Values {
        crate::settings_ui::Values {
            font_size: self.typography.code_size,
            tab_width: self.display.tab_width,
            soft_wrap: self.display.mode == WrapMode::Word,
            format_on_save: self.format_on_save,
            modal: self.modal,
            theme: self.theme.name(),
            line_numbers: self.line_numbers,
            sticky_scroll: self.sticky_scroll,
            inlay_hints: self.show_inlay_hints,
            inline_blame: self.blame.inline,
            autosave: self.autosave,
            transparency: (self.chrome.transparency * 100.0).round() as u8,
            animations: self.chrome.motion.animations(),
            updates: self.chrome.updates,
        }
    }

    /// Drops a focus whose surface is gone, so a stale one can never eat a key.
    ///
    /// Called when a key is routed rather than at every site that closes something: there are a
    /// dozen of those (toggles, the activity bar, replace, open-folder, a hit being opened), and
    /// "remember to reset focus too" is exactly the rule that does not survive a year.
    ///
    /// It also closes a completion popup the text no longer has the keys for. The popup belongs to
    /// the caret, and one left open behind a focused tree went on taking ↓ and Enter from it and
    /// accepting a suggestion into a buffer nobody was looking at. Closing it here, rather than at
    /// each site that moves focus (a click, `ctrl-shift-e`, the search results arriving, the
    /// settings tab coming forward), is the same choice as above for the same reason.
    pub fn settle_focus(&mut self) {
        use crate::focus::Focus;
        let stale = match self.focus {
            Focus::Sidebar => !self.sidebar_takes_keys(),
            Focus::Panel => self.results.is_none(),
            Focus::Terminal => !self.terminal.open,
            Focus::Editor => false,
        };
        if stale {
            self.focus = Focus::Editor;
        }
        if self.focus != Focus::Editor || self.settings_showing() {
            self.completion = None;
        }
    }

    /// Shows search results and gives them the keys, as opening any view does.
    pub fn show_results(&mut self, results: crate::results::Results) {
        self.results = Some(results);
        self.focus = crate::focus::Focus::Panel;
    }

    /// Which activity icon is lit: the view the sidebar is showing, in the order it would choose.
    pub fn active_activity(&self) -> Option<crate::layout::Activity> {
        use crate::layout::Activity;
        if self.tree.is_some() {
            Some(Activity::Explorer)
        } else if self.show_diagnostics {
            Some(Activity::Problems)
        } else if self.chrome.activity.is_some() {
            self.chrome.activity
        } else if self.results.is_some() {
            Some(Activity::Search)
        } else {
            None
        }
    }

    /// Closes whatever the sidebar is showing, so the next view opened replaces it.
    pub fn close_sidebar(&mut self) {
        self.tree = None;
        self.show_diagnostics = false;
        self.chrome.activity = None;
        // Nothing is left to hold the keys, and a focus that outlived its view would swallow the
        // next view's first key before anyone had chosen it.
        self.focus = crate::focus::Focus::Editor;
    }

    /// How far open the sidebar is drawn at the frame's instant: 0 closed, 1 open. Drawing and the
    /// mouse read this; soft wrap reads [`Self::sidebar_open`].
    pub fn sidebar_fraction(&self) -> f32 {
        self.sidebar_fraction_at(self.now())
    }

    /// How far open the sidebar is drawn at `now`.
    ///
    /// Clamped to `[0, 1]`: the panel spring's overshoot would widen the slot past the width its view
    /// is laid out at, and the view would reflow every frame of it. Past 1 there is nothing to show
    /// but a reflow, so the overshoot shows as the slide settling instead.
    pub fn sidebar_fraction_at(&self, now: std::time::Instant) -> f32 {
        let open = if self.sidebar_open() { 1.0 } else { 0.0 };
        self.chrome
            .motion
            .movement(crate::motion::Key::Sidebar, open, now)
            .clamp(0.0, 1.0)
    }

    /// Closes the palette, leaving it to fade out. Setting the field to `None` also closes it, only
    /// without the fade.
    pub fn close_palette(&mut self) {
        if let Some(palette) = self.palette.take() {
            self.chrome.leaving.palette = Some(Box::new(palette));
        }
    }

    /// Opens a menu — the bar or a context menu — and makes it the one thing taking keys.
    ///
    /// **Every menu opens here.** Key routing offers the palette, a prompt, a hover box and the
    /// completion popup a key before the menu, so a menu opened over any of them could be seen and
    /// not driven: a click on Edit with completion showing, then Enter, accepted the completion
    /// into the buffer with the menu still open. So the things that would take its keys are
    /// dismissed, and with a prompt up — which is half-typed input, not something to throw away —
    /// the menu does not open. Returns whether it did.
    pub fn open_menu(&mut self, menu: crate::menu::Menu) -> bool {
        if self.prompt.is_some() {
            return false;
        }
        self.completion = None;
        self.hover = None;
        self.close_palette();
        self.close_help();
        self.menu = Some(menu);
        true
    }

    /// Closes the menu, leaving it to fade out.
    pub fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            self.chrome.leaving.menu = Some(menu);
        }
    }

    /// Closes the help panel, leaving it to fade out.
    pub fn close_help(&mut self) {
        if let Some(help) = self.help.take() {
            self.chrome.leaving.help = Some(help);
        }
    }

    /// Points every animated value at where the state says it should be, at `now`.
    ///
    /// **The one place targets are set from state.** The event loop calls it after every batch of
    /// events and before every frame; asking for a target a value already has changes nothing, so
    /// calling it often costs a few hash lookups and cannot restart anything. That is what makes it
    /// impossible to miss a site: there are dozens of places that open or close a view, and a
    /// transition started at each of them would be one more thing to remember at the next. Drawing
    /// only ever reads. Events that are not states — a message arriving, a tab being opened — are
    /// kicked where they happen or detected here by comparing with the last sync.
    ///
    /// `bounds` and `scale` place the activity icons, so the one under the pointer can lift.
    pub fn sync_motion(&mut self, bounds: Rect, scale: f32, now: std::time::Instant) {
        use crate::motion::{Key, Spring};
        let flag = |open: bool| if open { 1.0 } else { 0.0 };
        // Arriving overshoots; leaving does not. A value that rang below zero on its way out would
        // come back up past it, and a closed palette would blink back into view for a frame.
        let spring = |open: bool, spring: Spring| {
            if open {
                spring
            } else {
                Spring {
                    damping_ratio: spring.damping_ratio.max(1.0),
                    ..spring
                }
            }
        };

        let fading = [
            (Key::Sidebar, self.sidebar_open(), Spring::PANEL),
            (Key::Palette, self.palette.is_some(), Spring::PANEL),
            (Key::Help, self.help.is_some(), Spring::PANEL),
            (Key::Menu, self.menu.is_some(), Spring::POPUP),
            (
                Key::Notifications,
                self.chrome.show_notifications,
                Spring::POPUP,
            ),
        ];
        // These two fade in and leave at once. Typing closes them — a keystroke past the end of a
        // word, a caret moved off what was hovered — and typing is never animated; their anchor is
        // the caret, which has already moved, so a fade-out would drift across the text. The info
        // box fades in for a hover reply only: the diagnostic under the caret is found by walking
        // every diagnostic, and this runs after every batch of events. A diagnostic's box simply
        // appears with the caret that lands on it, which is caret-following and never animated.
        //
        // Drawing reads both as "open" and lets the draw function decide whether there is anything
        // to show, so a key snapped to 0 here while a diagnostic box is up falls back to the state in
        // `Motion::fade` and is drawn whole. That fallback is load-bearing for this, not only for
        // "nothing has synced yet".
        //
        // The theme dropdown is the same: picking a theme changes every colour on screen at once, and
        // a list fading out in the old theme's colours over the new one would be the last thing
        // anyone needed to see.
        let dropdown = self.settings_showing()
            && self
                .settings_page
                .as_ref()
                .is_some_and(|page| page.dropdown().is_some());
        let cut = [
            (Key::Completion, self.completion.is_some()),
            (Key::Info, self.hover.is_some()),
            (Key::Dropdown, dropdown),
        ];

        let hovers: Vec<(u32, bool)> = {
            let layout = crate::layout::Layout::of(bounds, scale, None, 0.0);
            let (x, y) = self.pointer;
            crate::layout::activity_items(layout.activity, scale)
                .into_iter()
                .filter_map(|(item, cell)| {
                    let index = crate::layout::ACTIVITIES
                        .iter()
                        .position(|candidate| candidate.activity == item.activity)?;
                    Some((index as u32, crate::layout::contains(cell, x, y)))
                })
                .collect()
        };
        let marker = self.active_activity().and_then(|active| {
            crate::layout::ACTIVITIES
                .iter()
                .position(|item| item.activity == active)
        });
        let tabs = self.buffer_count.max(1) + usize::from(self.settings_page.is_some());
        // With no tab active the indicator stays where it is rather than gliding off past the end.
        let active_tab = self.active_tab().min(tabs) as f32;
        let settings_scroll = self.settings_page.as_ref().map(|page| page.scroll());
        let step = self
            .welcome
            .as_ref()
            .map(|welcome| welcome.step.number() as f32);

        let chrome = &mut self.chrome;
        let motion = &mut chrome.motion;
        // Under reduced motion a value that only moves things is never drawn anywhere but at its
        // target, so it is put there: a spring running underneath would keep the loop drawing frames
        // that do not change, on every pass of the pointer over the activity bar.
        let still = motion.animations() == crate::motion::Animations::Reduced;
        for (key, open, kind) in fading {
            if still && key == Key::Sidebar {
                motion.snap(key, flag(open));
            } else {
                motion.to(key, flag(open), spring(open, kind), now);
            }
        }
        for (key, open) in cut {
            if open {
                motion.to(key, 1.0, Spring::POPUP, now);
            } else {
                motion.snap(key, 0.0);
            }
        }
        for (index, over) in hovers {
            if still {
                motion.snap(Key::Hover(index), flag(over));
            } else {
                motion.to(Key::Hover(index), flag(over), Spring::HOVER, now);
            }
        }

        // Something that slides between places is put in its first place, not slid in from zero.
        let glide = |motion: &mut crate::motion::Motion, key: Key, to: f32, fresh: bool| {
            if still || fresh || motion.target(key).is_none() {
                motion.snap(key, to);
            } else {
                motion.to(key, to, Spring::POPUP, now);
            }
        };
        if let Some(index) = marker {
            glide(
                motion,
                Key::ActivityIndicator,
                index as f32,
                !chrome.marker_shown,
            );
        }
        chrome.marker_shown = marker.is_some();
        glide(motion, Key::TabIndicator, active_tab, false);
        // The page's scroll follows its target. A new page or search snaps it where they change
        // (`snap_settings_scroll`), so this only ever carries the wheel and the arrow keys.
        if let Some(scroll) = settings_scroll {
            if still || motion.target(Key::SettingsScroll).is_none() {
                motion.snap(Key::SettingsScroll, scroll);
            } else {
                motion.to(Key::SettingsScroll, scroll, Spring::SCROLL, now);
            }
        }
        if chrome.tabs_seen.is_some_and(|seen| tabs > seen) {
            motion.snap(Key::TabGrow, 0.0);
            motion.to(Key::TabGrow, 1.0, Spring::POPUP, now);
        }
        chrome.tabs_seen = Some(tabs);
        if let Some(step) = step {
            if motion.target(Key::Welcome).is_none() {
                motion.snap(Key::Welcome, step);
            } else {
                // Without overshoot: past its place, a page would fade out again as it settled.
                let settle = Spring {
                    damping_ratio: 1.0,
                    ..Spring::PANEL
                };
                motion.to(Key::Welcome, step, settle, now);
            }
        }

        // What has finished leaving is forgotten, and what has come back is not leaving. Nor does
        // anything keep leaving once another modal is open: they all draw on one layer, where every
        // quad goes down before any glyph, so a menu fading out behind the palette it opened would
        // print its rows through the palette's card.
        let modal = self.palette.is_some() || self.menu.is_some() || self.help.is_some();
        let gone = |motion: &crate::motion::Motion, key: Key, open: bool| {
            modal || open || motion.get(key, now) <= 0.0
        };
        if gone(motion, Key::Palette, self.palette.is_some()) {
            chrome.leaving.palette = None;
        }
        if gone(motion, Key::Menu, self.menu.is_some()) {
            chrome.leaving.menu = None;
        }
        if gone(motion, Key::Help, self.help.is_some()) {
            chrome.leaving.help = None;
        }
    }

    /// Mutates the document and moves everything outside it that names a position in it.
    ///
    /// [`Document::apply`] is the single funnel for changing text, but the folds live out here in
    /// the display map, and nothing was moving them — so they went on naming lines the edit had
    /// shifted or deleted. Since [`dacode_display::Folds::is_hidden`] asks only whether a line falls
    /// inside a range, a fold whose end had run off the end of the document hid *everything below
    /// its head*: delete a function above a closed fold and the rest of the file disappeared.
    ///
    /// Takes a closure rather than a transaction because two of the mutation sites are in
    /// [`crate::edits`], which builds the transaction itself and is tested against a bare
    /// `Document`. `crates/dacode/tests/conventions.rs` enforces going through here, since "remember
    /// to call the other thing too" is not a rule that survives a year.
    /// The matches of the last search are the one thing here that is *not* moved, and it is a
    /// deliberate refusal rather than an oversight: they cover the file where the spans cover the
    /// viewport, so walking them per keystroke measured 24.6x on a hundred-thousand-line document
    /// against a thousand-line one. `latency.rs` holds that measurement. They drift until the search
    /// is re-run, and moving them needs to be lazy — at the viewport when drawing, once per `n`.
    /// The syntax tree, if it describes the buffer as it is now — and `None` if an edit has moved
    /// the buffer past it, which the next parse will put right.
    pub fn fresh_structure(&self) -> Option<&dacode_syntax::Structure> {
        self.structure
            .as_ref()
            .filter(|structure| structure.is_fresh_for(self.document.version()))
    }

    pub fn mutate<T>(&mut self, change: impl FnOnce(&mut Document) -> T) -> T {
        // A diff or the history is git's answer, not a file: an edit to it would be an edit to
        // nothing, saved nowhere. The change is run against a copy and thrown away, so whatever
        // asked gets an answer of the type it expects and the text on screen stays git's.
        if self.review.is_some() {
            self.hover = Some(
                "this tab is git's answer, not a file — open the file to change it".to_string(),
            );
            let mut copy = Document::from_str(&self.document.text().to_string());
            return change(&mut copy);
        }
        // Cleared, not moved — see `Expansions`. Before the early return below, which would
        // otherwise skip it in every buffer with nothing folded or coloured.
        self.expansions.clear();
        // Nothing folded, colored or underlined, and this costs three `is_empty` calls.
        if self.display.folds.is_empty()
            && self.highlights.is_empty()
            && self.diagnostics.is_empty()
            && !self.blame.is_tracking()
        {
            return change(&mut self.document);
        }

        let mut before = self.document.text().clone();
        let mark = self.document.version();
        let outcome = change(&mut self.document);

        let moved = self.document.version() != mark;
        let mut replayed = false;

        // Each change's edits are expressed against the previous change's text, so they are walked
        // in order rather than concatenated — the same reason the syntax worker replays them.
        for applied in self.document.changes_since(mark) {
            self.display
                .folds
                .transform(&before, &applied.text, &applied.edits);
            shift_highlights(&mut self.highlights, &applied.edits);
            self.diagnostics.transform(&before, &applied.edits);
            self.blame.record(&before, &applied.edits);
            before = applied.text.clone();
            replayed = true;
        }

        if moved && !replayed {
            // The queue was truncated, so how far the lines moved is no longer knowable. Folds that
            // cannot be trusted are worse than no folds: they hide code silently, and spans that
            // cannot be trusted paint one piece of text in another's colors.
            self.display.folds.clear();
            self.highlights.clear();
            self.diagnostics.clear();
            self.blame.lose();
        }
        outcome
    }

    /// The block heads sticky scroll pins above a viewport, outermost first.
    ///
    /// Only when the head has actually scrolled off — pinning a line that is already visible would
    /// show it twice, which is the thing that makes this feature annoying elsewhere.
    ///
    /// Read by `draw_code` to place them and, through [`crate::layout::CodeArea`], by the mouse to
    /// take their strip off the top before turning a pixel into a display row. It lived inside the
    /// drawing code, where nothing else could see it.
    pub fn sticky_heads(&self, text: &Text, scroll_line: usize, focused: bool) -> Vec<usize> {
        if !focused || !self.sticky_scroll {
            return Vec::new();
        }
        dacode_display::enclosing_heads(text, scroll_line, self.display.tab_width, 3)
            .into_iter()
            .filter(|head| *head < scroll_line)
            .collect()
    }

    /// Changes the editor mode, unless there is only one.
    ///
    /// With Vim's grammar off there is exactly one mode and it is `Insert`. Escape resolved to
    /// `CollapseSelections`, which set `Normal` unconditionally — so a non-modal user pressing the
    /// key every editor uses for "never mind" got a blue block cursor and a blue gutter, while the
    /// statusline went on saying EDITING because its label is chosen by `modal` rather than by the
    /// mode. A mode they could see, could not name, and could not leave: nothing `feed_flat` can
    /// type is `i`.
    ///
    /// Refusing here rather than at each of the six call sites, because five of them are only
    /// reachable *from* visual mode and reasoning about that at every site is how the sixth was
    /// missed.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.modal {
            self.mode = mode;
        }
    }

    pub fn cursor(&self) -> CharOffset {
        self.document.selections().primary().head
    }

    pub fn cursor_point(&self) -> Point {
        self.document.text().char_to_point(self.cursor())
    }

    /// Whether there is nothing to show: an untouched scratch buffer.
    ///
    /// Not merely "the text is empty" — a file that happens to be empty is still a file you opened,
    /// and replacing its viewport with a splash would hide the fact that you are editing it.
    pub fn is_empty(&self) -> bool {
        self.file_path.is_none()
            && self.document.text().len_chars().get() == 0
            && !self.document.is_dirty()
    }

    /// How far the focused pane's top row has scrolled up out of view, in pixels.
    ///
    /// The one reading of [`Chrome::scroll_input`] that drawing and the mouse share, through
    /// [`crate::layout::CodeGrid::with_sub_row`]. Clamped to a row: the remainder is kept inside one by
    /// `scroll::by_pixels`, and a font-size change between that and this frame must not turn a few
    /// pixels of an old row height into a whole row of gap.
    pub fn sub_row_px(&self, row_height: f32) -> f32 {
        let px = self.chrome.scroll_input.remainder();
        if px.is_finite() && px > 0.0 {
            px.min(row_height)
        } else {
            0.0
        }
    }

    /// How many display rows fit in a viewport of this pixel height.
    pub fn visible_rows(&self, height_px: f32, row_height: f32) -> usize {
        ((height_px / row_height).floor() as usize).max(1)
    }

    /// Height of the code area, given the whole window — the chrome's vertical stack in reverse.
    pub fn code_height(&self, window_height: f32, scale: f32) -> f32 {
        let chrome = (window_height - (72.0 + 34.0 + 34.0) * scale).max(0.0);
        // The terminal takes its share from the bottom of the code, and the caret has to be revealed
        // above it rather than underneath. Its height is the layout's, which depends on the window's
        // height alone, so any width will do here.
        let terminal = crate::layout::Layout::of(
            Rect::new(0.0, 0.0, 1000.0 * scale, window_height),
            scale,
            None,
            0.0,
        )
        .with_terminal(self.terminal.open, self.terminal.fraction, scale)
        .terminal
        .map_or(0.0, |panel| panel.height);
        (chrome - terminal).max(0.0)
    }
}

/// Moves highlight spans over one change's edits, so they keep naming the text they describe.
///
/// The spans are the second thing outside the document that holds bare offsets across an edit, and
/// unlike the folds nothing was moving them. They are only ever *set* from a parse whose version
/// matched the buffer — but the next keystroke makes that set stale, and a parse takes a few
/// milliseconds to answer, so the spans were painted against text that had already moved. Typing a
/// hundred characters into a line shifted every span after it a hundred characters early: a comment's
/// italic ran from the middle of the line above to the middle of the comment, and the whole file
/// below the cursor rendered as a smear of the wrong colors until the parse landed. It went away
/// the moment you stopped typing, which is exactly why it read as a rendering fault rather than a
/// stale-data one.
///
/// Both ends take [`Bias::Left`], which keeps adjacent spans from overlapping: text typed at a span's
/// end is left uncolored until the parse arrives rather than being claimed by both that span and the
/// one after it.
fn shift_highlights(spans: &mut Vec<HighlightSpan>, edits: &[dacode_text::Edit]) {
    spans.retain_mut(|span| shift(&mut span.range, edits));
}

/// Moves one range over the edits, reporting whether anything of it is left.
fn shift(range: &mut std::ops::Range<CharOffset>, edits: &[dacode_text::Edit]) -> bool {
    range.start = dacode_text::transform_offset(range.start, Bias::Left, edits);
    range.end = dacode_text::transform_offset(range.end, Bias::Left, edits);
    // A range whose text was deleted collapses to empty. Dropping it is the whole of what
    // "this no longer describes anything" means.
    range.start < range.end
}

/// How the welcome step on screen is arriving at `now`: how faded in it is, and how far it still has
/// to slide, in logical pixels. A step comes in from the side it lies on — forwards from the right,
/// back from the left — over the one before, which is gone the moment the answer is given.
pub fn welcome_arrival(
    motion: &crate::motion::Motion,
    welcome: &crate::welcome::Welcome,
    now: std::time::Instant,
) -> (f32, f32) {
    let step = welcome.step.number() as f32;
    let away = step - motion.fade(Key::Welcome, step, now);
    let moving = step - motion.movement(Key::Welcome, step, now);
    ((1.0 - away.abs()).clamp(0.0, 1.0), moving * 48.0)
}

/// The first-run setup screen.
///
/// A function of the wizard's state and a theme, like the splash — it runs before there is anything
/// else worth drawing, and it must be legible in whichever theme is being previewed. `arrival` is
/// [`welcome_arrival`]: the step's question and options fade and slide in; the mark and the way out
/// at the bottom stay put, since they do not change between steps.
pub fn draw_welcome(
    frame: &mut Frame,
    welcome: &crate::welcome::Welcome,
    typography: &Typography,
    bounds: Rect,
    scale: f32,
    arrival: (f32, f32),
) {
    use crate::welcome::Step;

    let theme = welcome.preview_theme();
    frame.fill(bounds, theme.bg);

    let px = |v: f32| v * scale;
    let content_width = crate::welcome::content_width(bounds, scale);
    let left = (bounds.x + (bounds.width - content_width) / 2.0).round();

    // Centred vertically, because a wizard pinned to the top of a tall window reads as a page that
    // failed to load. The content's height is known per step, so this is arithmetic rather than a
    // guess.
    let content_height = crate::welcome::content_height(welcome.step, bounds, scale, typography);
    let header = px(34.0) + px(48.0) + px(26.0) + px(52.0);

    let mark = px(34.0);
    let mut y =
        (bounds.y + ((bounds.height - header - content_height) / 2.0).max(px(56.0))).round();
    frame.mark(Rect::new(left, y, mark, mark), theme.ink, theme.brand());

    let mut wordmark = typography.spec(Role::Display);
    wordmark.size_px = px(30.0);
    frame.text(TextRun {
        text: "welcome to DaCode".into(),
        spec: wordmark,
        color: theme.ink,
        rect: Rect::new(left + mark + px(18.0), y, content_width, mark),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    });

    y += mark + px(48.0);

    frame.set_opacity(arrival.0);
    frame.set_zoom(Some(dacode_render::Zoom::shifted([px(arrival.1), 0.0])));

    // Which question, and where in the sequence — three steps is short enough to be worth saying.
    let mut heading = typography.spec(Role::Prose);
    heading.size_px = px(19.0);
    frame.text(TextRun {
        text: welcome.step.title().into(),
        spec: heading,
        color: theme.ink,
        rect: Rect::new(left, y, content_width * 0.7, px(26.0)),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    });
    frame.text(welcome_label(
        typography,
        format!("step {} of {}", welcome.step.number(), Step::COUNT),
        Rect::new(left, y, content_width, px(26.0)),
        theme.ink_faint,
        Align::Right,
        scale,
    ));

    y += px(52.0);

    let geometry = crate::welcome::geometry(welcome, bounds, scale, typography);
    let context = WelcomeFrame {
        welcome,
        typography,
        theme: &theme,
        left,
        top: y,
        width: content_width,
        scale,
        options: &geometry.options,
        next: geometry.next,
    };
    match welcome.step {
        Step::Keys => draw_welcome_keys(frame, &context),
        Step::Theme => draw_theme_cards(frame, &context),
        Step::Import => draw_welcome_import(frame, &context),
        Step::Done => {}
    }
    frame.set_opacity(1.0);
    frame.set_zoom(None);

    // What the keys do, at the bottom. A wizard nobody can leave is a trap.
    frame.text(welcome_label(
        typography,
        // Leads with the mouse, because the first question this screen asks is whether you want the
        // editor that expects one — and it used to answer that question by being keyboard-only.
        match welcome.step {
            Step::Import => "click to tick · ↑↓ move · enter tick · → finish · esc skip setup",
            _ => "click to choose · ↑↓ move · enter choose · ← back · esc skip setup",
        },
        Rect::new(left, bounds.bottom() - px(64.0), content_width, px(22.0)),
        theme.ink_faint,
        Align::Left,
        scale,
    ));
}

/// Everything a welcome step needs to draw itself.
struct WelcomeFrame<'a> {
    welcome: &'a crate::welcome::Welcome,
    typography: &'a Typography,
    theme: &'a Theme,
    left: f32,
    top: f32,
    width: f32,
    scale: f32,
    /// Where each option is — from `welcome::geometry`, which the mouse reads too. Passed in rather
    /// than recomputed per step, so a card cannot be drawn anywhere other than where it is clicked.
    options: &'a [Rect],
    /// The "continue" button, on the step that has one.
    next: Option<Rect>,
}

/// The two ways of typing, as cards with a sentence each.
fn draw_welcome_keys(frame: &mut Frame, context: &WelcomeFrame<'_>) {
    let WelcomeFrame {
        welcome,
        typography,
        theme,
        scale,
        ..
    } = *context;
    let px = |v: f32| v * scale;
    let choices = [
        (
            "like every other editor",
            "typing inserts. ctrl-c, ctrl-v, ctrl-z, ctrl-d, alt-arrows. double-click a word, \
             alt-click for another cursor. pick this if you are unsure — f1 shows every key.",
        ),
        (
            "vim keys",
            "modal editing: normal, insert, visual. counts, operators, text objects, macros.",
        ),
    ];

    for (index, (name, description)) in choices.iter().enumerate() {
        let Some(card) = context.options.get(index).copied() else {
            continue;
        };
        // Highlighted by the keyboard *or* by the pointer, so a card lights up under the mouse and
        // reads as something you can click — which is the entire question this step is asking.
        let selected = index == welcome.selected
            || crate::layout::contains(card, welcome.pointer.0, welcome.pointer.1);
        draw_welcome_card(frame, theme, card, selected, scale);

        let mut title = typography.spec(Role::Prose);
        title.size_px = px(17.0);
        frame.text(TextRun {
            text: (*name).into(),
            spec: title,
            color: if selected { theme.ink } else { theme.ink_dim },
            rect: Rect::new(
                card.x + px(24.0),
                card.y + px(18.0),
                card.width - px(48.0),
                px(24.0),
            ),
            align: Align::Left,
            center_vertically: true,
            wrap: false,
            layer: Layer::default(),
            clip: None,
        });

        // Two lines, wrapped here so the second can end in an ellipsis when the card is too narrow
        // for the sentence. The renderer wrapped it before and clipped the third line at the card's
        // edge, so "pick this if you are unsure" lost the half that says why.
        let mut body = typography.spec(Role::Value);
        body.size_px = px(13.0);
        let width = card.width - px(48.0);
        let advance = crate::layout::measured_advance_at(body.size_px);
        let lines = wrap_lines(description, width, advance, 2);
        for (row, line) in lines.iter().enumerate() {
            frame.text(TextRun {
                text: line.clone(),
                spec: body,
                color: theme.ink_faint,
                rect: Rect::new(
                    card.x + px(24.0),
                    card.y + px(44.0) + row as f32 * px(18.0),
                    width,
                    px(18.0),
                ),
                align: Align::Left,
                center_vertically: true,
                wrap: false,
                layer: Layer::default(),
                clip: None,
            });
        }
    }
}

/// The five themes, each shown as a few lines of code in itself.
///
/// Shown rather than named, which is the whole reason this screen exists: "rose-pine" tells you
/// nothing, and six coloured lines tell you everything.
fn draw_theme_cards(frame: &mut Frame, context: &WelcomeFrame<'_>) {
    let WelcomeFrame {
        welcome,
        typography,
        theme: current,
        scale,
        ..
    } = *context;
    let px = |v: f32| v * scale;

    // A sample with one of everything the palette colours, so a card shows the whole theme rather
    // than whichever role happens to appear in a random line.
    /// A line of the preview: the text, and which palette role colours it.
    type SampleLine = (&'static str, fn(&Theme) -> Oklch);

    let sample: [SampleLine; 6] = [
        ("// a comment", |theme| theme.syntax.comment),
        ("fn main() {", |theme| theme.syntax.keyword),
        ("  let n = 42;", |theme| theme.syntax.number),
        ("  greet(\"hi\");", |theme| theme.syntax.string),
        ("  Thing::new()", |theme| theme.syntax.type_),
        ("}", |theme| theme.ink),
    ];

    // In the order the options are laid out: the dark themes first, as the settings tab lists them.
    for (index, name) in Theme::grouped().into_iter().enumerate() {
        let Some(theme) = Theme::by_name(name) else {
            continue;
        };
        let Some(card) = context.options.get(index).copied() else {
            continue;
        };
        // Under the pointer counts as selected, so the theme previews as you move over it — the
        // same "choosing is looking" the arrow keys give.
        let selected = index == welcome.selected
            || crate::layout::contains(card, welcome.pointer.0, welcome.pointer.1);

        // The card is painted in *its own* theme, which is what makes it a preview — on the card
        // shape every overlay uses, outlined in its own rule.
        let radius = CARD_RADIUS * scale;
        frame.fill_rounded(card, theme.bg, radius);
        frame.outline_rounded(card, theme.rule, radius, border::BORDER * scale);

        let line = px(21.0);
        let mut code = typography.spec(Role::Code);
        code.size_px = px(11.5);
        for (row, (text, color)) in sample.iter().enumerate() {
            // As many lines as the card has room for above its name: a short window gets short
            // cards, and a line drawn into the name's place would hide the name.
            let rect = Rect::new(
                card.x + px(12.0),
                card.y + px(14.0) + row as f32 * line,
                card.width - px(16.0),
                line,
            );
            if rect.bottom() > card.bottom() - px(32.0) {
                break;
            }
            frame.text(TextRun {
                text: (*text).into(),
                spec: code,
                color: color(&theme),
                rect,
                align: Align::Left,
                center_vertically: true,
                wrap: false,
                layer: Layer::default(),
                clip: None,
            });
        }

        // The name goes on the card, in the card's own ink — whole, or with an ellipsis on a card
        // too narrow for it, never cut at the card's edge mid-letter. The display name: the card's
        // own colours say which half of a pair it is.
        let name_width = card.width - px(24.0);
        let name = elide_to(
            Theme::display_name_of(name).unwrap_or(name),
            name_width,
            crate::layout::measured_advance(typography, Role::Label, scale),
        );
        frame.text(welcome_label(
            typography,
            name,
            Rect::new(
                card.x + px(12.0),
                card.bottom() - px(30.0),
                card.width - px(24.0),
                px(20.0),
            ),
            theme.ink_dim,
            Align::Left,
            scale,
        ));

        // The selection ring is drawn in the *current* theme's accent so it is visible against every
        // card, including the black one.
        if selected {
            let accent = current.accent(Mode::Normal);
            frame.outline_rounded(card, accent, radius, px(2.0));
        }
    }
}

/// The import checklist, and the row that finishes.
fn draw_welcome_import(frame: &mut Frame, context: &WelcomeFrame<'_>) {
    let WelcomeFrame {
        welcome,
        typography,
        theme,
        left,
        top,
        width,
        scale,
        ..
    } = *context;
    let px = |v: f32| v * scale;
    let present = crate::import_cli::vscode_is_present();

    frame.text(welcome_label(
        typography,
        if present {
            "found a vscode installation"
        } else {
            "no vscode installation found — nothing to import"
        },
        Rect::new(left, top, width, px(22.0)),
        if present {
            theme.fresh()
        } else {
            theme.ink_faint
        },
        Align::Left,
        scale,
    ));

    let line = px(34.0);

    for (index, item) in crate::settings_ui::Importable::ALL.iter().enumerate() {
        let Some(row) = context.options.get(index).copied() else {
            continue;
        };
        let selected = index == welcome.selected
            || crate::layout::contains(row, welcome.pointer.0, welcome.pointer.1);
        if selected {
            frame.fill_rounded(row, theme.row_active, px(5.0));
        }

        let mut body = typography.spec(Role::Value);
        body.size_px = px(14.0);
        frame.text(TextRun {
            text: format!(
                "[{}] {}",
                if welcome.is_ticked(*item) { "x" } else { " " },
                item.name()
            ),
            spec: body,
            color: match (present, welcome.is_ticked(*item)) {
                (false, _) => theme.ink_faint,
                (true, true) => theme.ink,
                (true, false) => theme.ink_dim,
            },
            rect: Rect::new(row.x + px(16.0), row.y, row.width - px(32.0), line),
            align: Align::Left,
            center_vertically: true,
            wrap: false,
            layer: Layer::default(),
            clip: None,
        });
    }

    // The finishing row, which is where the last option lands — and the one thing on this screen
    // that has to look like a button, because it is the only way out that is not a keyboard key.
    let row = context.next.unwrap_or(Rect::new(left, top, width, line));
    let selected = welcome.selected >= crate::settings_ui::Importable::ALL.len()
        || crate::layout::contains(row, welcome.pointer.0, welcome.pointer.1);
    // A button, on the card shape: lifted when it is the one Enter would press.
    draw_welcome_card(frame, theme, row, selected, scale);
    let mut body = typography.spec(Role::Value);
    body.size_px = px(14.0);
    let words = if welcome.answers.import.is_empty() {
        "start without importing".to_string()
    } else {
        format!("import {} and start", welcome.answers.import.len())
    };
    frame.text(TextRun {
        text: elide_to(
            &words,
            row.width - px(32.0),
            crate::layout::measured_advance_at(body.size_px),
        ),
        spec: body,
        color: if selected {
            theme.accent(Mode::Normal)
        } else {
            theme.ink_dim
        },
        rect: Rect::new(row.x + px(16.0), row.y, row.width - px(32.0), line),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    });
}

/// An option on the wizard: the overlay card's surface, radius and outline, with the accent as its
/// outline when it is the one chosen.
///
/// The card itself, shadow and all, only for the chosen one — the shadow lifts it off the page — so
/// the rest sit flat in the surface they are on, outlined.
fn draw_welcome_card(frame: &mut Frame, theme: &Theme, rect: Rect, selected: bool, scale: f32) {
    let radius = CARD_RADIUS * scale;
    if selected {
        card(frame, theme, rect, scale, 1.0);
        let accent = theme.accent(Mode::Normal);
        frame.outline_rounded(rect, accent, radius, border::ACCENT_BORDER * scale);
    } else {
        frame.fill_rounded(rect, theme.bg_alt, radius);
        frame.outline_rounded(rect, theme.rule_soft, radius, border::BORDER * scale);
    }
}

/// A small uppercase label, for a screen that has no `EditorState` to read typography from.
fn welcome_label(
    typography: &Typography,
    text: impl Into<String>,
    rect: Rect,
    color: Oklch,
    align: Align,
    scale: f32,
) -> TextRun {
    let mut spec = typography.spec(Role::Label);
    spec.size_px *= scale;
    TextRun {
        text: text.into(),
        spec,
        color,
        rect,
        align,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    }
}

/// The cold-start frame: the stacked lockup on the editor's own background.
///
/// A function of the theme alone, because it is drawn before there is a document, a language or a
/// mode. Deliberately not animated and with no progress bar — the startup budget is supposed to make
/// this frame nearly invisible, and anything that draws attention to it is admitting it is not.
pub fn draw_splash(
    frame: &mut Frame,
    theme: &Theme,
    typography: &Typography,
    bounds: Rect,
    scale: f32,
) {
    frame.fill(bounds, theme.bg);

    let mark_size = 46.0 * scale;
    let wordmark_size = 34.0 * scale;
    let gap = space::SM * scale;

    // Left edges aligned, to the mark's *ink* rather than to its path — see MARK_INK_LEFT.
    let block_height = mark_size + gap + wordmark_size;

    let left = (bounds.x + (bounds.width - mark_size) / 2.0).round();
    let top = (bounds.y + (bounds.height - block_height) / 2.0).round();

    frame.mark(
        Rect::new(left, top, mark_size, mark_size),
        theme.ink,
        theme.brand(),
    );

    let mut spec = typography.spec(Role::Display);
    spec.size_px = wordmark_size;
    frame.text(TextRun {
        text: "dacode".into(),
        spec,
        color: theme.ink,
        rect: Rect::new(
            left + mark_size * dacode_render::MARK_INK_LEFT,
            top + mark_size + gap,
            bounds.width,
            // Tight: a taller rect centres the glyphs lower and the 8px gap becomes 40px of air.
            wordmark_size,
        ),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    });
}

/// How panes divide the editor area.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum PaneLayout {
    /// Side by side.
    #[default]
    Vertical,
    /// Stacked.
    Horizontal,
}

pub fn draw(frame: &mut Frame, state: &EditorState, bounds: Rect, scale: f32) {
    // The single-pane case, which is what the tests and the screenshot path use.
    let pane = PaneView {
        document: &state.document,
        scroll: state.scroll,
        path: &state.path,
        focused: true,
        highlights: &state.highlights,
    };
    draw_panes(frame, state, &[pane], PaneLayout::Vertical, bounds, scale);
}

pub fn draw_panes(
    frame: &mut Frame,
    state: &EditorState,
    panes: &[PaneView<'_>],
    layout: PaneLayout,
    bounds: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    frame.set_transparency(state.chrome.transparency);
    let row_height = state.typography.row_height() * scale;

    // The same regions the mouse reads. This used to re-derive the stack itself — a second copy of
    // the arithmetic `layout.rs` exists to hold once, and the one place this module was breaking
    // its own rule.
    let regions = crate::layout::Layout::of(
        bounds,
        scale,
        state.results.as_ref().map(|results| results.wanted_rows()),
        state.sidebar_fraction(),
    )
    .with_terminal(state.terminal.open, state.terminal.fraction, scale)
    .with_blame(state.blame_column(
        panes.iter().position(|pane| pane.focused).unwrap_or(0),
        scale,
    ));

    draw_title(frame, state, regions.title, scale);
    draw_activity(frame, state, regions.activity, scale);
    draw_tabs(frame, state, regions.tabs, scale);
    frame.fill_backdrop(regions.editor, theme.bg);

    // Panes divide the editor area evenly along one axis, through the same `Layout::pane` the mouse
    // asks — a click in the third pane of a split lands in the third pane's text.
    let count = panes.len().max(1);
    let vertical = layout == PaneLayout::Vertical;
    let mut focused_code = regions.editor;
    // The settings tab takes the whole editor area, splits and all: it is the tab on screen, and a
    // page of settings beside half a file would be neither.
    let settings = state
        .settings_page
        .as_ref()
        .filter(|_| state.settings_active);
    if let Some(page) = settings {
        draw_settings_page(frame, state, page, regions.editor, scale);
    }
    let panes = if settings.is_some() { &[][..] } else { panes };
    for (index, pane) in panes.iter().enumerate() {
        let rects = regions.pane(index, count, vertical, scale);

        // A hairline between panes, on the leading edge of every pane after the first. Without it
        // two files sitting side by side read as one document with a strange gap in it.
        if index > 0 {
            let line = border::BORDER * scale;
            frame.fill(
                if vertical {
                    Rect::new(rects.slot.x - line, rects.slot.y, line, rects.slot.height)
                } else {
                    Rect::new(rects.slot.x, rects.slot.y - line, rects.slot.width, line)
                },
                theme.rule,
            );
        }

        // Sticky scroll takes its strip before anything measures against the code area — including
        // the info box, the completion popup and the mouse, all of which read `focused_code` below.
        let area = crate::layout::CodeArea::new(
            rects.code,
            state.sticky_heads(pane.document.text(), pane.scroll.line, pane.focused),
            row_height,
        );
        draw_code(
            frame,
            state,
            pane,
            rects.blame,
            rects.gutter,
            &area,
            row_height,
            scale,
        );

        // With more than one pane, each takes a footer naming its file. A reserved strip rather than
        // a label floating over the text: an overlay would sit on top of the last row of code, which
        // is exactly where the eye goes when reading down.
        if let Some(footer) = rects.footer {
            frame.fill_backdrop(footer, if pane.focused { theme.bg_alt } else { theme.bg });
            frame.rule_top(footer, theme.rule_soft, border::BORDER);
            frame.text(run(
                state,
                Role::Ui,
                pane.path.last().cloned().unwrap_or_default(),
                Rect::new(
                    footer.x + space::MD * scale,
                    footer.y,
                    footer.width - space::MD * 2.0 * scale,
                    footer.height,
                ),
                // The focused pane's name is the brighter one: with two files open it is the
                // fastest way to see where typing will land.
                if pane.focused {
                    theme.ink
                } else {
                    theme.ink_faint
                },
                Align::Left,
                scale,
            ));
        }

        if pane.focused {
            focused_code = area.code;
        }
    }

    let code = focused_code;
    if let Some(sidebar) = regions.sidebar {
        draw_sidebar(frame, state, sidebar, regions.sidebar_width, scale);
    }
    if let (Some(results), Some(area)) = (&state.results, regions.results) {
        draw_results(frame, state, results, area, scale);
    }
    if let Some(panel) = regions.terminal {
        draw_terminal(frame, state, panel, scale);
    }
    draw_statusline(frame, state, regions.status, scale);

    // Info boxes sit above the code but below the palette: a hover is contextual, a palette is
    // modal. The layer is what makes them opaque — within one layer, later text still shows through
    // an earlier panel, because all of a layer's rectangles are drawn before any of its glyphs.
    frame.set_layer(Layer::Panel);
    // Neither of these fades out, so each is drawn as if open and returns early when it is not —
    // which spares asking here whether the caret is on a diagnostic, a walk over all of them.
    //
    // All three describe the code, so none is drawn while the settings tab covers it. `code` is then
    // the bare editor region, and the message under a caret nobody could see sat on the page.
    if settings.is_none() {
        let (fade, travel) = arrival(state, Key::Info, true);
        overlay(frame, fade, |frame| {
            draw_info(frame, state, code, row_height, travel, scale)
        });
        let (fade, travel) = arrival(state, Key::Completion, true);
        overlay(frame, fade, |frame| {
            draw_completion(frame, state, code, row_height, travel, scale)
        });

        // The which-key hint sits with the info boxes: it is contextual, it appears beside what
        // you are doing, and a scrim over the document would be absurd for something that lasts
        // one keystroke.
        draw_which_key(frame, state, code, scale);
    }
    // The log outlives the panel, so it fades out on its own with nothing kept aside for it.
    let (fade, travel) = arrival(state, Key::Notifications, state.chrome.show_notifications);
    overlay(frame, fade, |frame| {
        draw_notifications(frame, state, regions.title, bounds, travel, scale)
    });

    // Last, so they cover everything: these are modal overlays. Each is drawn from its live state
    // when open and from what it left behind while it fades out — never both.
    frame.set_layer(Layer::Modal);
    // The theme dropdown first, and only with nothing else modal open: they share this layer, where
    // every quad goes down before any glyph, so its names would print through a palette's card.
    if let Some(page) =
        settings.filter(|_| state.palette.is_none() && state.help.is_none() && state.menu.is_none())
    {
        let (fade, travel) = arrival(state, Key::Dropdown, page.dropdown().is_some());
        overlay(frame, fade, |frame| {
            draw_theme_dropdown(frame, state, page, regions.editor, travel, scale)
        });
    }
    let leaving = &state.chrome.leaving;
    let (fade, travel) = arrival(state, Key::Palette, state.palette.is_some());
    if let Some(palette) = state.palette.as_ref().or(leaving.palette.as_deref()) {
        overlay(frame, fade, |frame| {
            draw_palette(frame, state, palette, bounds, travel, scale)
        });
    }
    let (fade, travel) = arrival(state, Key::Help, state.help.is_some());
    if let Some(help) = state.help.as_ref().or(leaving.help.as_ref()) {
        overlay(frame, fade, |frame| {
            draw_help(frame, state, help, bounds, travel, scale)
        });
    }
    let (fade, travel) = arrival(state, Key::Menu, state.menu.is_some());
    if let Some(menu) = state.menu.as_ref().or(leaving.menu.as_ref()) {
        overlay(frame, fade, |frame| {
            draw_menu(frame, state, menu, bounds, travel, scale)
        });
    }
    // A control's name and key, under the pointer. On top of everything, and only when nothing
    // modal is open — a tooltip over the palette would be describing a button you cannot reach.
    if state.palette.is_none() && state.help.is_none() && state.menu.is_none() {
        draw_tooltip(frame, state, &regions, bounds, scale);
    }
    frame.set_layer(Layer::Base);
}

/// The terminal panel: a header with its heading, tabs and buttons, and the shown shell's visible
/// rows.
///
/// Reads the [`crate::terminal::Panel::screen`] the event loop copied out, which is the viewport's
/// cells and nothing more, so this costs the panel's size whatever the scrollback holds. Each row
/// is drawn as runs of one colour, each placed at its own column, rather than one run per row: a
/// run has one colour, and placing every span at its column keeps a line of mixed colours on the
/// grid the cursor and the shell both count in.
fn draw_terminal(frame: &mut Frame, state: &EditorState, panel: Rect, scale: f32) {
    use dacode_terminal::Screen;

    let theme = &state.theme;
    let rects = crate::layout::terminal_panel(panel, &state.typography, scale);
    let focused = state.focus == crate::focus::Focus::Terminal;
    frame.fill(panel, theme.bg_panel);
    frame.rule_top(panel, theme.rule, border::BORDER);

    // The header says what this is and offers the two things you can do to it. Its rule takes the
    // mode accent while the terminal has the keys, the one place to look to know where typing goes.
    let header = rects.header;
    frame.rule_bottom(
        header,
        if focused {
            theme.accent(state.mode)
        } else {
            theme.rule_soft
        },
        border::BORDER,
    );
    frame.text(label(
        state,
        "terminal",
        rects.heading,
        if focused { theme.ink } else { theme.ink_dim },
        Align::Left,
        scale,
    ));
    // One tab per shell. The shown one is lifted onto the code's background and keeps its title in
    // full ink; the others are dim until hovered. Every tab carries its own close, which ends that
    // shell, where the header's close only hides the panel.
    let tabs = crate::layout::terminal_tabs(&rects, &state.terminal.tabs, &state.typography, scale);
    for (index, (tab, title)) in tabs.iter().zip(&state.terminal.tabs).enumerate() {
        let shown = index == state.terminal.active;
        let over = hovered(state, tab.tab);
        if shown {
            frame.fill_rounded(tab.tab, theme.bg, 6.0 * scale);
        } else if over {
            frame.fill_rounded(tab.tab, theme.row_active, 6.0 * scale);
        }
        frame.text(ui(
            state,
            elide(state, Role::Ui, title, tab.label.width, scale),
            tab.label,
            if shown || over {
                theme.ink
            } else {
                theme.ink_faint
            },
            Align::Left,
            scale,
        ));
        if tab.close.width >= 8.0 * scale {
            frame.icon(
                Icon::Close,
                centred(tab.close, crate::layout::size::ICON * 0.75 * scale),
                if hovered(state, tab.close) {
                    theme.ink
                } else {
                    theme.ink_faint
                },
            );
        }
    }
    icon_button(frame, state, rects.new, Icon::Plus, theme.ink_dim, scale);
    icon_button(frame, state, rects.close, Icon::Close, theme.ink_dim, scale);

    let blank;
    let screen = match &state.terminal.screen {
        Some(screen) => screen,
        None => {
            blank = Screen::blank(dacode_terminal::GridSize::new(rects.columns, rects.rows));
            &blank
        }
    };
    let palette = theme.ansi();
    let mut spec = state.typography.spec(Role::Code);
    spec.size_px *= scale;
    spec.line_height = crate::terminal::LINE_HEIGHT;
    let cursor = screen.cursor;
    let line = (scale).max(1.0).round();

    // Only the rows the panel has room for, however many the copy holds: a resize reaches the
    // shell a frame after the panel changes, and the rows it has not yet re-wrapped are cut here
    // rather than drawn over the status bar.
    for (row, cells) in screen.rows.iter().enumerate().take(rects.rows) {
        let columns = cells.len().min(rects.columns);
        let mut column = 0;
        while column < columns {
            let cell = &cells[column];
            if cell.spacer {
                column += 1;
                continue;
            }
            let on_cursor = cursor.is_some_and(|at| at.row == row && at.column == column);
            let (ink, fill) = crate::terminal::paint(theme, &palette, cell);
            let style = cell.style;

            // A span: this cell and every following one painted the same way. The cursor's cell and
            // a double-width character are spans of their own, the first because it is drawn
            // differently and the second because a fallback face's advance is not the grid's.
            let mut end = column + 1;
            if !on_cursor && !cell.wide {
                while end < columns {
                    let next = &cells[end];
                    if next.spacer
                        || next.wide
                        || cursor.is_some_and(|at| at.row == row && at.column == end)
                        || next.style.underline != style.underline
                        || next.style.strikeout != style.strikeout
                        || next.style.hidden != style.hidden
                        || crate::terminal::paint(theme, &palette, next) != (ink, fill)
                    {
                        break;
                    }
                    end += 1;
                }
            }
            let width_cells = if cell.wide { 2 } else { end - column };
            let first = rects.cell(row, column);
            let span = Rect::new(
                first.x,
                first.y,
                rects.cell_width * width_cells as f32,
                rects.cell_height,
            )
            .intersect(rects.grid);

            if let Some(fill) = fill {
                frame.fill(span, fill);
            }
            let mut ink = ink;
            if on_cursor && focused {
                // A block in the text's colour, with the character under it in the panel's: the
                // cursor every terminal draws, and the one shape that reads at any size.
                frame.fill(span, theme.ink);
                ink = fill.unwrap_or(theme.bg_panel);
            } else if on_cursor {
                // Unfocused, an outline: the shell still has a cursor, but the keys are elsewhere.
                frame.outline_rounded(span, theme.ink_dim, 0.0, line);
            }
            if style.underline {
                frame.fill(
                    Rect::new(span.x, span.bottom() - 3.0 * scale, span.width, line),
                    ink,
                );
            }
            if style.strikeout {
                frame.fill(
                    Rect::new(span.x, span.y + span.height / 2.0, span.width, line),
                    ink,
                );
            }

            let text: String = cells[column..end]
                .iter()
                .filter(|cell| !cell.spacer)
                .map(|cell| cell.c)
                .collect();
            if !style.hidden && !text.trim().is_empty() {
                frame.text(TextRun {
                    text: text.trim_end().to_string(),
                    spec,
                    color: ink,
                    // A cell of slack on the right, so the last glyph of a span is not shaved by a
                    // clip measured in the grid's arithmetic rather than the shaper's.
                    rect: Rect::new(span.x, span.y, span.width + rects.cell_width, span.height)
                        .intersect(rects.panel),
                    align: Align::Left,
                    center_vertically: true,
                    wrap: false,
                    layer: Layer::default(),
                    clip: None,
                });
            }
            column = end.max(column + 1);
        }
    }
}

/// How far an overlay has faded in, and how far it has travelled (grown), at the frame's instant.
/// The two differ only under reduced motion, where it fades and does not move.
fn arrival(state: &EditorState, key: Key, open: bool) -> (f32, f32) {
    let expected = if open { 1.0 } else { 0.0 };
    let now = state.now();
    let motion = &state.chrome.motion;
    (
        motion.fade(key, expected, now),
        motion.movement(key, expected, now),
    )
}

/// The completion popup, anchored under the word being completed.
fn draw_completion(
    frame: &mut Frame,
    state: &EditorState,
    code: Rect,
    row_height: f32,
    travel: f32,
    scale: f32,
) {
    let Some(completion) = &state.completion else {
        return;
    };
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let advance = column_advance(state, scale);

    let (visible, cursor) = completion.visible();
    let row = 20.0 * scale;
    let height = if completion.pending || visible.is_empty() {
        row
    } else {
        visible.len() as f32 * row
    };

    // Anchored at the word, not at the cursor: the list lines up with what it is completing.
    let anchor_point =
        state
            .display
            .display_point(state.document.text(), state.scroll, 512, completion.anchor);
    let Some(point) = anchor_point else { return };

    // Wide enough for the label and what sits beside it. A popup sized to the label alone puts the
    // type — half of why you are reading the list at all — off the right edge.
    let widest = visible
        .iter()
        .map(|item| item.label().chars().count() + 2 + tail_of(item).chars().count().min(TAIL))
        .max()
        .unwrap_or(12)
        .max(12);
    let width = ((widest as f32 + 4.0) * advance).min(code.width * 0.6);

    let x = (code.x + space::SM * scale + point.column as f32 * advance)
        .min(code.right() - width - space::SM * scale);
    let cursor_y = row_grid(state, code, row_height, scale).y_of(point.row);
    // Below the line normally, above it when there is no room — the same rule as the hover box.
    let below = cursor_y + row_height;
    let y = if below + height <= code.bottom() {
        below
    } else {
        (cursor_y - height).max(code.y)
    };

    let popup = Rect::new(x, y, width, height);
    // It grows from the edge nearest the word it completes.
    let anchor = if y >= below {
        Anchor::Top
    } else {
        Anchor::Bottom
    };
    frame.set_zoom(Some(appear_zoom(popup, travel, anchor)));
    card(frame, theme, popup, scale, 1.0);

    if completion.pending {
        frame.text(run(
            state,
            Role::Value,
            "…",
            Rect::new(popup.x + space::XS * scale, popup.y, popup.width, row),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        return;
    }

    if visible.is_empty() {
        frame.text(run(
            state,
            Role::Value,
            "no suggestions",
            Rect::new(popup.x + space::XS * scale, popup.y, popup.width, row),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        return;
    }

    for (index, item) in visible.iter().enumerate() {
        let item_y = popup.y + index as f32 * row;
        let selected = index == cursor;
        row_wash(
            frame,
            state,
            Rect::new(
                popup.x + 3.0 * scale,
                item_y,
                popup.width - 6.0 * scale,
                row,
            ),
            selected,
            scale,
        );

        let text_x = popup.x + space::XS * scale;
        let inner = popup.width - space::XS * 2.0 * scale;

        // The type on the right, in the faint ink. It gives way to the label rather than the other
        // way round: a truncated type is still a hint, a truncated name is not something you can
        // accept.
        let tail = tail_of(item);
        let label_room = if tail.is_empty() {
            inner
        } else {
            let tail = elide(
                state,
                Role::Value,
                &tail,
                (TAIL as f32 * advance).min(inner * 0.62),
                scale,
            );
            frame.text(run(
                state,
                Role::Value,
                tail.clone(),
                Rect::new(text_x, item_y, inner, row),
                theme.ink_faint,
                Align::Right,
                scale,
            ));
            inner - (tail.chars().count() as f32 + 1.0) * advance
        };

        let label = elide(state, Role::Value, item.label(), label_room, scale);
        frame.text(run(
            state,
            Role::Value,
            label.clone(),
            Rect::new(text_x, item_y, label_room, row),
            match (selected, item.offer.deprecated) {
                // Deprecated stays readable but stops looking like the answer. Striking it through
                // would need a rule per row the renderer has no cheap way to place.
                (_, true) => theme.ink_faint,
                (true, false) => theme.ink,
                (false, false) => theme.ink_dim,
            },
            Align::Left,
            scale,
        ));

        // The typed prefix, in the mode color, over the label's own glyphs.
        // At the face's real advance, as the palette's are: the type scale's 0.60 put a highlight
        // a little further left of its letter with every character along a long label.
        let prefix_advance = crate::layout::measured_advance(&state.typography, Role::Value, scale);
        for position in &item.highlights {
            let Some(c) = label.chars().nth(*position as usize) else {
                continue;
            };
            frame.text(run(
                state,
                Role::Value,
                c.to_string(),
                Rect::new(
                    text_x + *position as f32 * prefix_advance,
                    item_y,
                    prefix_advance * 2.0,
                    row,
                ),
                accent,
                Align::Left,
                scale,
            ));
        }
    }
}

/// Characters of type signature a completion row will show before eliding.
///
/// The popup is capped at 60% of the code area, so this is what keeps a single 200-character
/// generic signature from setting the width for every other row in the list.
const TAIL: usize = 34;

/// What sits to the right of a suggestion's name.
///
/// The type when the server gave one, and the kind only when it did not. Showing both was worse
/// than either: at this width the kind was elided to `m…`, which names nothing — and a signature
/// starting `fn(` already says what a `method` tag would.
fn tail_of(item: &crate::completion::Item) -> String {
    match &item.offer.detail {
        Some(detail) => detail.clone(),
        None => item.offer.kind.to_string(),
    }
}

/// The focused pane's rows, for a popup that sits against the cursor's line. Through the grid
/// `draw_code` placed that line with, so a popup follows a part-scrolled row rather than sitting a
/// fraction of a row away from it.
fn row_grid(
    state: &EditorState,
    code: Rect,
    row_height: f32,
    scale: f32,
) -> crate::layout::CodeGrid {
    crate::layout::CodeGrid::new(code, column_advance(state, scale), row_height, 0, scale)
        .with_sub_row(state.sub_row_px(row_height))
}

/// A hover reply, or the message of the diagnostic under the cursor.
///
/// Hover wins when both apply: it was asked for explicitly, while the diagnostic message appears
/// merely because the cursor is on the line.
fn draw_info(
    frame: &mut Frame,
    state: &EditorState,
    code: Rect,
    row_height: f32,
    travel: f32,
    scale: f32,
) {
    let theme = &state.theme;

    let (text, accent) = match &state.hover {
        Some(hover) => (hover.clone(), theme.accent(state.mode)),
        None => {
            let (message, colour) = match line_note(state, crate::blame::now()) {
                Some(LineNote::Diagnostic { message, is_error }) => (
                    message,
                    if is_error {
                        theme.over_budget()
                    } else {
                        theme.stale()
                    },
                ),
                Some(LineNote::Blame(text)) => (text, theme.ink_faint),
                None => return,
            };
            draw_after_line(frame, state, code, row_height, scale, &message, colour);
            return;
        }
    };

    // A cap on *rendered* rows, not source lines. It used to be `.take(6)` over `str::lines`, which
    // for a one-paragraph message is one line however long it is — and the box was then a fixed 620
    // wide with `wrap: false`, so the sentence simply stopped where the rectangle did. No wrap, no
    // ellipsis, no growth: the reader is told the beginning of why something failed and left
    // believing that was all of it.
    const MAX_ROWS: usize = 10;

    let line_height = 20.0 * scale;
    let widest = (620.0 * scale).min(code.width - space::XL * scale);
    let inner = widest - space::SM * 2.0 * scale;

    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && line.trim() != "```")
        .flat_map(|line| wrap_to(&state.typography, Role::Value, line.trim(), inner, scale))
        .collect();
    if lines.is_empty() {
        return;
    }

    // The original intent, kept: a hover reply can be pages of markdown, and a panel that grew to
    // fit one would cover the code it describes. What changed is that the cut now says it happened.
    if lines.len() > MAX_ROWS {
        lines.truncate(MAX_ROWS);
        if let Some(last) = lines.last_mut() {
            *last = elide(state, Role::Value, &format!("{last} …"), inner, scale);
        }
    }

    // Sized to what is actually drawn, so a short message gets a short box rather than 620px of
    // panel with three words in it.
    let advance = crate::layout::measured_advance(&state.typography, Role::Value, scale);
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let width = ((longest as f32 * advance) + space::SM * 2.0 * scale).clamp(0.0, widest);
    let height = lines.len() as f32 * line_height + space::MD * scale;

    // Below the cursor line where there is room, above it otherwise, so the box never hides the
    // thing it is describing.
    let cursor_y = state
        .display
        .display_point(state.document.text(), state.scroll, 512, state.cursor())
        .map(|point| row_grid(state, code, row_height, scale).y_of(point.row))
        .unwrap_or(code.y);
    let below = cursor_y + row_height;
    let y = if below + height <= code.bottom() {
        below
    } else {
        (cursor_y - height).max(code.y)
    };

    let box_rect = Rect::new(code.x + space::SM * scale, y, width, height);
    let anchor = if y >= below {
        Anchor::Top
    } else {
        Anchor::Bottom
    };
    frame.set_zoom(Some(appear_zoom(box_rect, travel, anchor)));
    card(frame, theme, box_rect, scale, 1.0);
    // What kind of box it is — a hover answer in the mode colour, an error or a warning in theirs —
    // as a short bar inside the card's left edge, clear of its rounded corners.
    let stripe = 8.0 * scale;
    frame.fill_rounded(
        Rect::new(
            box_rect.x + 4.0 * scale,
            box_rect.y + stripe,
            border::ACCENT_BORDER * scale,
            (box_rect.height - stripe * 2.0).max(border::ACCENT_BORDER * scale),
        ),
        accent,
        scale,
    );

    for (index, line) in lines.iter().enumerate() {
        frame.text(run(
            state,
            Role::Value,
            line.clone(),
            Rect::new(
                box_rect.x + space::SM * scale,
                box_rect.y + space::XS * scale + index as f32 * line_height,
                box_rect.width - space::SM * 2.0 * scale,
                line_height,
            ),
            theme.ink,
            Align::Left,
            scale,
        ));
    }
}

/// What is written after the end of the caret's line, when anything is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineNote {
    /// The problem under the caret.
    Diagnostic { message: String, is_error: bool },
    /// Who last changed the line, when, and why: GitLens's inline blame.
    Blame(String),
}

/// The note for the caret's line: its diagnostic if it has one, otherwise its blame.
///
/// **The diagnostic always wins.** Both want the same place, and a problem is something to act on
/// while a blame is something to know; hiding the error behind its author would be the wrong way
/// round. One function decides, and drawing reads it, so the rule cannot be applied in one place
/// and forgotten in another. `now` is seconds since the epoch, a parameter so the relative date can
/// be tested.
pub fn line_note(state: &EditorState, now: i64) -> Option<LineNote> {
    if let Some((message, is_error)) = diagnostic_at_cursor(state) {
        return Some(LineNote::Diagnostic { message, is_error });
    }
    if !state.blame.inline {
        return None;
    }
    let attribution = state.blame.at(state.cursor_point().line)?;
    Some(LineNote::Blame(crate::blame::inline_text(attribution, now)))
}

/// One entry of the gutter's blame column: the author on the left, how long ago on the right.
///
/// Elided to the column rather than clipped by it, so a long name ends in an ellipsis rather than
/// mid-letter. The age is kept whole — it is the shorter half and the one read at a glance.
fn draw_blame_entry(
    frame: &mut Frame,
    state: &EditorState,
    attribution: crate::blame::Attribution<'_>,
    cell: Rect,
    on_cursor_line: bool,
    now: i64,
    scale: f32,
) {
    let theme = &state.theme;
    let colour = if on_cursor_line {
        theme.ink_dim
    } else {
        theme.ink_faint
    };
    let pad = space::SM * scale;
    let inner = Rect::new(
        cell.x + pad,
        cell.y,
        (cell.width - pad * 1.5).max(0.0),
        cell.height,
    );
    let (author, age) = match attribution {
        crate::blame::Attribution::Commit(commit) => (
            commit.author.clone(),
            crate::blame::ago_short(commit.time, now),
        ),
        crate::blame::Attribution::Uncommitted => ("not committed".to_string(), String::new()),
    };
    let advance = crate::layout::measured_advance(&state.typography, Role::LineNumber, scale);
    let age_width = age.chars().count() as f32 * advance;
    let room = (inner.width - age_width - if age.is_empty() { 0.0 } else { advance }).max(0.0);
    frame.text(run(
        state,
        Role::LineNumber,
        elide(state, Role::LineNumber, &author, room, scale),
        Rect::new(inner.x, inner.y, room, inner.height),
        colour,
        Align::Left,
        scale,
    ));
    if !age.is_empty() && inner.width > age_width {
        frame.text(run(
            state,
            Role::LineNumber,
            age,
            // A character of slack on the left, so the estimate being a hair short clips nothing.
            Rect::new(
                inner.right() - age_width - advance,
                inner.y,
                age_width + advance,
                inner.height,
            ),
            colour,
            Align::Right,
            scale,
        ));
    }
}

/// A note written after the end of the caret's line: the diagnostic under the caret, or its blame.
///
/// It was a card on the row below the caret, which hid that row — the line under an unused import
/// is usually the next import, and it vanished while the caret sat there. On the line's own row,
/// past its last character, it covers nothing. The cost is length: a message longer than the room
/// left on the row is elided, and the whole of it is in the problems list and in a hover.
fn draw_after_line(
    frame: &mut Frame,
    state: &EditorState,
    code: Rect,
    row_height: f32,
    scale: f32,
    message: &str,
    colour: Oklch,
) {
    let text = state.document.text();
    let line = text.char_to_line(state.cursor());
    let Some(end) = state.display.display_point(
        text,
        state.scroll,
        state.visible_rows(code.height, row_height),
        text.line_range(line).end,
    ) else {
        return;
    };
    let offset = if state.display.mode == WrapMode::None {
        state.scroll.column
    } else {
        0
    };
    let grid = crate::layout::CodeGrid::new(
        code,
        column_advance(state, scale),
        row_height,
        offset,
        scale,
    )
    .with_sub_row(state.sub_row_px(row_height));
    // The grid steps 0.60 em and the shaper draws Fira Code at 0.615, so a long line's last glyph
    // ends to the right of where the grid places its column. Without the drift the gap closed up
    // and the message ran on from the code as if it were part of the line.
    let drift = end.column.saturating_sub(offset) as f32
        * (crate::layout::measured_advance(&state.typography, Role::Code, scale) - grid.advance())
            .max(0.0);
    let x = (grid.x_of(end.column) + drift + grid.advance() * 3.0).max(code.x + space::SM * scale);
    let width = code.right() - x - space::SM * scale;
    // Fewer than a dozen columns is a word and an ellipsis, which says less than nothing.
    if width < grid.advance() * 12.0 {
        return;
    }
    let first = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // Base, under every panel: within one layer glyphs draw after every quad, so on the panel
    // layer this would show through the completion list whenever the two met.
    let layer = frame.layer();
    frame.set_layer(Layer::Base);
    frame.text(run(
        state,
        Role::Code,
        elide(state, Role::Code, first, width, scale),
        Rect::new(x, grid.y_of(end.row), width, row_height),
        colour,
        Align::Left,
        scale,
    ));
    frame.set_layer(layer);
}

/// The message of the diagnostic covering the cursor, and whether it is an error.
pub fn diagnostic_at_cursor(state: &EditorState) -> Option<(String, bool)> {
    let text = state.document.text();
    let cursor = state.cursor();

    state
        .diagnostics
        .items()
        .iter()
        .find(|diagnostic| {
            let range = dacode_lsp::range_from_lsp(text, diagnostic.range);
            // `<=` on the end so a diagnostic about a single position still matches when the cursor
            // sits on it.
            range.start <= cursor && cursor <= range.end
        })
        .map(|diagnostic| {
            (
                diagnostic.message.clone(),
                diagnostic.severity == Some(dacode_lsp::lsp_types::DiagnosticSeverity::ERROR),
            )
        })
}

/// The project-search results panel.
fn draw_results(
    frame: &mut Frame,
    state: &EditorState,
    results: &crate::results::Results,
    rect: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(rect, theme.bg_panel);
    frame.rule_top(rect, theme.rule, border::BORDER);

    let pad = space::XL * scale;
    let row_height = crate::results::ROW * scale;
    let geometry = results.geometry(rect, scale);
    let (header, rows_area) = (geometry.header, geometry.list);

    // What was searched for, and how much was found: the one heading this panel needs, since a
    // panel of hits with no query over it does not say what they are hits *for*. In the interface's
    // own face rather than the uppercase label, which would have shouted the query back in capitals
    // it was not typed in.
    let summary = results.summary();
    let summary_width = summary.chars().count() as f32
        * crate::layout::measured_advance(&state.typography, Role::Value, scale);
    let query_room = (header.width - pad * 2.0 - summary_width - space::MD * scale).max(0.0);
    frame.text(ui(
        state,
        elide(
            state,
            Role::Ui,
            &format!("search: {}", results.query),
            query_room,
            scale,
        ),
        Rect::new(header.x + pad, header.y, query_room, header.height),
        theme.ink_dim,
        Align::Left,
        scale,
    ));
    frame.text(run(
        state,
        Role::Value,
        summary,
        Rect::new(
            header.x + pad,
            header.y,
            header.width - pad * 2.0,
            header.height,
        ),
        theme.ink_faint,
        Align::Right,
        scale,
    ));

    let inner = (rows_area.width - pad * 2.0).max(0.0);
    let location_width = (320.0 * scale).min(inner * 0.4);
    let line_x = rows_area.x + pad + location_width + space::MD * scale;
    let line_room = (rows_area.right() - pad - line_x).max(0.0);
    for (offset, row) in geometry.rows.iter().enumerate() {
        let index = geometry.first + offset;
        let Some(hit) = results.hits.get(index) else {
            continue;
        };
        let (row, y) = (*row, row.y);
        let selected = index == results.selected;
        row_wash(
            frame,
            state,
            Rect::new(row.x + pad / 2.0, y, (row.width - pad).max(0.0), row_height),
            selected,
            scale,
        );

        // `path:line` first, then the matching line — the order that lets you scan the list. A
        // long path loses its front, since its end is the file.
        let location = format!(
            "{}:{}",
            results.relative(hit, &state.project_root),
            hit.line_number
        );
        frame.text(run(
            state,
            Role::Value,
            elide_start(state, Role::Value, &location, location_width, scale),
            Rect::new(row.x + pad, y, location_width, row_height),
            if selected { accent } else { theme.ink_dim },
            Align::Left,
            scale,
        ));

        // The matched line, trimmed: leading indentation wastes the width that shows context.
        frame.text(run(
            state,
            Role::Value,
            elide(state, Role::Value, hit.line.trim_start(), line_room, scale),
            Rect::new(line_x, y, line_room, row_height),
            if selected { theme.ink } else { theme.ink_faint },
            Align::Left,
            scale,
        ));
    }

    if results.hits.is_empty() {
        frame.text(run(
            state,
            Role::Value,
            if results.finished {
                "no matches"
            } else {
                "searching…"
            },
            Rect::new(rows_area.x + pad, rows_area.y, rows_area.width, row_height),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }
}

/// The command palette: a card dropped from the command centre in the title bar.
///
/// Laid out by [`Palette::geometry`], which the mouse reads too. No scrim: it hangs from the box
/// that opened it, the way VSCode's and Zed's do, and the shadow is what separates it from the code.
/// The query field is the card's only caption; the rows are headed by their group only where the
/// list is grouped (see [`Palette::lines`]). The footer of hints it once had — enter, arrows, escape
/// — told nobody anything, and the match score each file row carried was the editor showing its
/// workings, so both are gone.
fn draw_palette(
    frame: &mut Frame,
    state: &EditorState,
    palette: &Palette,
    bounds: Rect,
    travel: f32,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let anchor = crate::palette::anchor(bounds, scale, &state.typography);
    let geometry = palette.geometry(bounds, anchor, scale);

    frame.set_zoom(Some(appear_zoom(geometry.card, travel, Anchor::Top)));
    card(frame, theme, geometry.card, scale, 1.0);

    // How much the query has narrowed, once there is something to narrow: "4 of 1284".
    let count = (!palette.query.is_empty() && palette.total > 0)
        .then(|| format!("{} of {}", palette.rows.len(), palette.total));
    let reserve = count.as_ref().map_or(0.0, |count| {
        count.chars().count() as f32
            * crate::layout::measured_advance(&state.typography, Role::Value, scale)
            + 12.0 * scale
    });
    let strip = draw_field(
        frame,
        state,
        geometry.input,
        &palette.query,
        &placeholder(palette.kind),
        reserve,
        scale,
    );
    if let Some(count) = count {
        frame.text(run(
            state,
            Role::Value,
            count,
            strip,
            theme.ink_faint,
            Align::Right,
            scale,
        ));
    }

    let pad = 10.0 * scale;
    for (rect, title) in &geometry.headings {
        draw_heading(frame, state, title, *rect, scale);
    }

    // The face's real advance, so a highlight lands on its letter all the way along a long name.
    let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);
    for (rect, item) in geometry.rows.iter().zip(&geometry.items) {
        let Some(row) = palette.rows.get(*item) else {
            continue;
        };
        let selected = *item == palette.selected;
        row_wash(frame, state, *rect, selected, scale);

        let left = rect.x + pad;
        let mut right = rect.right() - pad;
        // A command's key is a column of chips on the right, so the list reads as the table that
        // teaches the shortcuts.
        if palette.kind == PaletteKind::Commands && !row.context.is_empty() {
            right = key_chip(
                frame,
                state,
                &row.context,
                right,
                rect.y,
                rect.height,
                scale,
            ) - 12.0 * scale;
        }

        let name = elide(state, Role::Ui, &row.name, (right - left).max(0.0), scale);
        let length = name.chars().count();
        frame.text(ui(
            state,
            name.clone(),
            Rect::new(left, rect.y, (right - left).max(0.0), rect.height),
            if selected { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));
        // The matched characters in the mode colour, over the name's own glyphs — only those still
        // on screen, so a highlight never lands on the ellipsis or past it.
        for position in &row.highlights {
            let position = *position as usize;
            if position + 1 >= length && name.ends_with('…') {
                continue;
            }
            let Some(c) = name.chars().nth(position) else {
                continue;
            };
            frame.text(ui(
                state,
                c.to_string(),
                Rect::new(
                    left + position as f32 * advance,
                    rect.y,
                    advance * 2.0,
                    rect.height,
                ),
                accent,
                Align::Left,
                scale,
            ));
        }

        // A file's directory, a symbol's kind: dimmed after the name, given what room is left. A
        // directory loses its front first, since its end is the part that tells two apart.
        if palette.kind != PaletteKind::Commands && !row.context.is_empty() {
            let x = left + (length as f32 + 2.0) * advance;
            let room = right - x;
            if room > advance * 4.0 {
                let shown = match palette.kind {
                    PaletteKind::Files | PaletteKind::Buffers => {
                        elide_start(state, Role::Value, &row.context, room, scale)
                    }
                    _ => elide(state, Role::Value, &row.context, room, scale),
                };
                frame.text(run(
                    state,
                    Role::Value,
                    shown,
                    Rect::new(x, rect.y, room, rect.height),
                    theme.ink_faint,
                    Align::Left,
                    scale,
                ));
            }
        }
    }

    if geometry.rows.is_empty() {
        frame.text(ui(
            state,
            "no matches",
            Rect::new(
                geometry.list.x + pad,
                geometry.list.y,
                geometry.list.width - pad * 2.0,
                geometry.list.height,
            ),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }
}

/// What an empty palette's field says it is for.
fn placeholder(kind: PaletteKind) -> String {
    let prompt = kind.prompt();
    let mut chars = prompt.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The which-key hint: what can follow the keys typed so far.
///
/// The statusline already showed the prefix — `<c-k>` sat there on its own, which tells you the
/// editor is waiting and nothing about what for. This is the thing Helix is repeatedly praised for
/// and which people bolt onto Neovim with a plugin: press a prefix, read the menu, learn the key.
///
/// Sits at the top left of the code area, not at the cursor: it appears and disappears within one
/// keystroke, and a popup that jumped around the window as you typed would be harder to read than a
/// still one in a place you learn. Drawn without a scrim, because it is a hint and not a decision —
/// it goes away the moment the command resolves, with nothing to dismiss.
fn draw_which_key(frame: &mut Frame, state: &EditorState, code: Rect, scale: f32) {
    if state.pending.is_empty() {
        return;
    }
    // What the prefix leads to *now*: a continuation the user moved elsewhere is not offered, and
    // one they bound under it is.
    let entries = dacode_input::shortcuts::continuations(&state.pending, state.modal);
    if entries.is_empty() {
        return;
    }

    let theme = &state.theme;
    let row_height = 24.0 * scale;
    let pad = space::SM * scale;
    // The keys in the notation of the grammar in use, like everywhere else they are shown.
    let shown: Vec<(String, &str)> = entries
        .iter()
        .map(|(key, what)| {
            let key = if state.modal {
                key.clone()
            } else {
                dacode_input::commands::pretty(key)
            };
            (key, what.as_str())
        })
        .collect();

    // Measured with the same advance the text is drawn at, so nothing is clipped — the popup is
    // small and the measurement is over a handful of static strings. The key column is only as
    // wide as the widest key: `v` and `<c-o>` next to a description half a window away read as two
    // unrelated lists.
    let key_column = shown
        .iter()
        .map(|(key, _)| crate::layout::chips_width(&state.typography, key, scale))
        .fold(0.0, f32::max);
    let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);
    let longest = shown
        .iter()
        .map(|(_, what)| what.chars().count())
        .max()
        .unwrap_or(20) as f32;
    let width = (key_column + longest * advance + pad * 3.0).min(code.width - pad * 2.0);
    let height = shown.len() as f32 * row_height + pad;

    // Kept inside the code area: the popup must not fall off the bottom of the window, which is
    // exactly where the cursor is when a file is longer than the screen.
    let cursor_y = code.y + row_height;
    let y = if cursor_y + height > code.bottom() {
        (code.bottom() - height).max(code.y)
    } else {
        cursor_y
    };
    let rect = Rect::new(code.x + pad, y, width, height);
    card(frame, theme, rect, scale, 1.0);

    for (index, (key, what)) in shown.iter().enumerate() {
        let y = rect.y + pad / 2.0 + index as f32 * row_height;
        let keys_right = rect.x + pad + key_column;
        // Right-aligned in their column, so the descriptions start on one edge.
        key_chip(frame, state, key, keys_right, y, row_height, scale);
        let room = (rect.right() - pad - (keys_right + pad)).max(0.0);
        frame.text(ui(
            state,
            elide(state, Role::Ui, what, room, scale),
            Rect::new(keys_right + pad, y, room, row_height),
            theme.ink_dim,
            Align::Left,
            scale,
        ));
    }
}

/// The help panel: every key that works right now, grouped and filterable, and each one a row that
/// runs its command when clicked or chosen with Enter.
///
/// A dialog, so it keeps the scrim the palette dropped. Laid out by [`crate::help::Help::geometry`],
/// which the mouse reads too. The section headings stay — "files", "the language server" — because
/// in a list of seventy keys they are how you find your place. The title the panel had is folded
/// into the field's placeholder, which still says which grammar these keys are for: that is the
/// question underneath the question, and a non-modal user needs to know they are not being shown
/// Vim's keys.
fn draw_help(
    frame: &mut Frame,
    state: &EditorState,
    help: &crate::help::Help,
    bounds: Rect,
    travel: f32,
    scale: f32,
) {
    use crate::help::Line;

    let theme = &state.theme;
    frame.fill(bounds, theme.scrim);
    let geometry = help.geometry(bounds, scale);
    frame.set_zoom(Some(appear_zoom(geometry.card, travel, Anchor::Centre)));
    card(frame, theme, geometry.card, scale, 1.0);

    draw_field(
        frame,
        state,
        geometry.input,
        &help.query,
        if state.modal {
            "Filter the keys of the Vim grammar"
        } else {
            "Filter the keys"
        },
        0.0,
        scale,
    );

    let pad = 10.0 * scale;
    let lines = help.lines();
    for (offset, rect) in geometry.rows.iter().enumerate() {
        let index = geometry.first + offset;
        match lines.get(index) {
            Some(Line::Heading(title)) => draw_heading(frame, state, title, *rect, scale),
            Some(Line::Binding { name, what, .. }) => {
                let selected = index == help.selected;
                row_wash(frame, state, *rect, selected, scale);
                let keys = dacode_input::commands::by_name(name)
                    .map(|command| command.shown(state.modal))
                    .unwrap_or_default();
                let right = key_chip(
                    frame,
                    state,
                    &keys,
                    rect.right() - pad,
                    rect.y,
                    rect.height,
                    scale,
                ) - 12.0 * scale;
                let room = (right - rect.x - pad).max(0.0);
                frame.text(ui(
                    state,
                    elide(state, Role::Ui, what, room, scale),
                    Rect::new(rect.x + pad, rect.y, room, rect.height),
                    if selected { theme.ink } else { theme.ink_dim },
                    Align::Left,
                    scale,
                ));
            }
            None => {}
        }
    }
    if geometry.rows.is_empty() || lines.is_empty() {
        frame.text(ui(
            state,
            "no key does that",
            Rect::new(
                geometry.input.x + pad,
                geometry.input.bottom() + 6.0 * scale,
                geometry.input.width - pad * 2.0,
                26.0 * scale,
            ),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }

    // The panel's own keys, as chips, then the way to everything that has no key at all. Each
    // hint is drawn whole or not at all, so a narrow card drops the last rather than cutting it.
    let footer = geometry.footer;
    frame.rule_top(footer, theme.rule_soft, border::BORDER * scale);
    let enter = if state.modal { "<cr>" } else { "Enter" };
    let escape = if state.modal { "<esc>" } else { "Esc" };
    let palette = key_hint(state, "command_palette");
    let strip = Rect::new(footer.x, footer.y, footer.width - pad, footer.height);
    let mut x = Some(footer.x + pad);
    for (keys, words) in [
        ("↑↓", "move"),
        (enter, "run"),
        (escape, "close"),
        (palette.as_str(), "every command"),
    ] {
        let Some(at) = x else { break };
        x = hint(frame, state, keys, words, at, strip, scale);
    }
}

/// The open menu's card, under its title in the bar or at the pointer. The bar's titles are not
/// drawn here — see [`draw_menu_titles`].
///
/// The one heading a menu has is the name of a menu opened from behind "…", which does not say
/// which it is. An entry whose label cannot say what it does has a line under it
/// ([`crate::menu::note`]); the rest say it in their label.
fn draw_menu(
    frame: &mut Frame,
    state: &EditorState,
    menu: &crate::menu::Menu,
    bounds: Rect,
    travel: f32,
    scale: f32,
) {
    let theme = &state.theme;

    // Geometry comes from the menu, not from here: the mouse reads the same function, and a second
    // copy of this arithmetic is how clicks start landing a row off.
    let geometry = menu.geometry(bounds, scale, &state.typography);
    let pad = geometry.pad;
    let inset = 4.0 * scale;
    let line = 26.0 * scale;

    // The card hangs from its title, or from the pointer that opened it.
    frame.set_zoom(Some(appear_zoom(geometry.card, travel, Anchor::Top)));
    card(frame, theme, geometry.card, scale, 1.0);

    if let Some(header) = geometry.header {
        frame.text(label(
            state,
            crate::menu::TITLES[menu.open.min(crate::menu::TITLES.len() - 1)],
            Rect::new(
                header.x + pad,
                header.y,
                header.width - pad * 2.0,
                header.height,
            ),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }
    for (index, (item, row)) in menu.items.iter().zip(&geometry.rows).enumerate() {
        let selected = index == menu.selected;
        let wash = Rect::new(row.x + inset, row.y, row.width - inset * 2.0, row.height);
        row_wash(frame, state, wash, selected, scale);
        let label_row = Rect::new(row.x, row.y, row.width, line.min(row.height));

        // The key, as chips on the right: the menu's whole purpose is that you read this and stop
        // needing the menu.
        let right = key_chip(
            frame,
            state,
            &item.keys,
            row.right() - pad,
            label_row.y,
            label_row.height,
            scale,
        );
        let room = (right - 12.0 * scale - (row.x + pad)).max(0.0);
        frame.text(ui(
            state,
            elide(state, Role::Ui, &item.label, room, scale),
            Rect::new(row.x + pad, label_row.y, room, label_row.height),
            if selected { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));
        if let Some(note) = item.note {
            let width = row.width - pad * 2.0;
            frame.text(run(
                state,
                Role::Value,
                elide(state, Role::Value, note, width, scale),
                Rect::new(
                    row.x + pad,
                    label_row.bottom() - 4.0 * scale,
                    width,
                    crate::menu::NOTE_HEIGHT * scale,
                ),
                theme.ink_faint,
                Align::Left,
                scale,
            ));
        }
    }
}

fn run(
    state: &EditorState,
    role: Role,
    text: impl Into<String>,
    rect: Rect,
    color: Oklch,
    align: Align,
    scale: f32,
) -> TextRun {
    let mut spec = state.typography.spec(role);
    spec.size_px *= scale;
    TextRun {
        text: text.into(),
        spec,
        color,
        rect,
        align,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    }
}

fn label(
    state: &EditorState,
    text: impl Into<String>,
    rect: Rect,
    color: Oklch,
    align: Align,
    scale: f32,
) -> TextRun {
    run(state, Role::Label, text, rect, color, align, scale)
}

/// Shortens text to fit a width, ending in an ellipsis.
///
/// The renderer clips a run to its rectangle, which means a string that is too long simply stops —
/// a narrow window showed a setting's caption as `modal editing; off means typing a`, cut mid-word
/// with no sign there was more. An ellipsis is the difference between text that is abbreviated and
/// text that looks broken.
fn elide(state: &EditorState, role: Role, text: &str, width: f32, scale: f32) -> String {
    elide_to(
        text,
        width,
        crate::layout::measured_advance(&state.typography, role, scale),
    )
}

/// [`elide`] for text set at an advance of its own — the wizard's, which has no `EditorState` and
/// sets some of its text at sizes off the type scale.
fn elide_to(text: &str, width: f32, advance: f32) -> String {
    if advance <= 0.0 || width <= 0.0 {
        return text.to_string();
    }
    let fits = (width / advance).floor().max(0.0) as usize;
    let length = text.chars().count();
    if length <= fits {
        return text.to_string();
    }
    if fits <= 1 {
        return "…".to_string();
    }
    text.chars().take(fits - 1).collect::<String>() + "…"
}

/// Breaks text into lines that fit a width, at word boundaries where it can.
///
/// The counterpart to [`elide`], for the one place where losing the tail is not acceptable: a
/// message. `elide` is right for a label in a row of a fixed height, and wrong for a sentence
/// explaining why something did not work — the box that carried "a language server is separate from
/// the Python you have installed…" stopped at "you have i", with no ellipsis and nothing to say it
/// had been cut. The reader is left believing they read the whole thing.
///
/// Measured with the same type-scale estimate `elide` uses, and deliberately: it is the
/// approximation the code grid and every other measurement here already make. Over-estimating a
/// character costs one word moved to the next line; under-estimating costs a character shaved by the
/// renderer's clip. Neither loses a sentence.
///
/// A word longer than the whole line is broken rather than dropped, because a path or a URL is
/// exactly the kind of thing an error message contains.
fn wrap_to(typography: &Typography, role: Role, text: &str, width: f32, scale: f32) -> Vec<String> {
    wrap_at(
        text,
        width,
        crate::layout::measured_advance(typography, role, scale),
    )
}

/// `text` wrapped to at most `max` lines, the last ending in an ellipsis when there was more.
///
/// The one way a paragraph is fitted into a fixed number of rows. There were two, and one of them
/// put a lone "…" on its second row whenever a message's first line fitted on the first.
fn wrap_lines(text: &str, width: f32, advance: f32, max: usize) -> Vec<String> {
    let mut lines = wrap_at(text, width, advance);
    if lines.len() > max.max(1) {
        // Greedy wrapping means what follows the kept lines is more than one line's worth, so the
        // elision always ends in an ellipsis.
        let rest = lines.split_off(max.max(1) - 1).join(" ");
        lines.push(elide_to(&rest, width, advance));
    }
    lines
}

/// A group's heading inside a card's list: the palette's command groups, the help panel's sections.
fn draw_heading(frame: &mut Frame, state: &EditorState, title: &str, rect: Rect, scale: f32) {
    let pad = 10.0 * scale;
    frame.text(label(
        state,
        elide(state, Role::Label, title, rect.width - pad * 2.0, scale),
        Rect::new(
            rect.x + pad,
            rect.y + 4.0 * scale,
            rect.width - pad * 2.0,
            rect.height - 4.0 * scale,
        ),
        state.theme.ink_faint,
        Align::Left,
        scale,
    ));
}

/// [`wrap_to`] at an advance of the caller's, for text set off the type scale.
pub(crate) fn wrap_at(text: &str, width: f32, advance: f32) -> Vec<String> {
    let per_line = if advance > 0.0 && width > 0.0 {
        (width / advance).floor().max(1.0) as usize
    } else {
        return vec![text.to_string()];
    };

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;

    for word in text.split_whitespace() {
        let length = word.chars().count();

        // Longer than a whole line on its own: break it across as many as it needs.
        if length > per_line {
            if used > 0 {
                lines.push(std::mem::take(&mut current));
            }
            let mut rest: Vec<char> = word.chars().collect();
            while rest.len() > per_line {
                lines.push(rest.drain(..per_line).collect());
            }
            current = rest.into_iter().collect();
            used = current.chars().count();
            continue;
        }

        let needed = if used == 0 { length } else { length + 1 };
        if used + needed > per_line {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        if used > 0 {
            current.push(' ');
            used += 1;
        }
        current.push_str(word);
        used += length;
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Shortens text to fit by dropping the **front**, for values whose end is the identifying part.
///
/// A path is the case: `…\.cargo\bin\rust-analyzer.exe` still names the program, while eliding it
/// the usual way gives `C:\Users\Gawa…` — every character of which the reader already knew.
fn elide_start(state: &EditorState, role: Role, text: &str, width: f32, scale: f32) -> String {
    let advance = crate::layout::measured_advance(&state.typography, role, scale);
    if advance <= 0.0 || width <= 0.0 {
        return text.to_string();
    }
    let fits = (width / advance).floor().max(0.0) as usize;
    let length = text.chars().count();
    if length <= fits {
        return text.to_string();
    }
    if fits <= 1 {
        return "…".to_string();
    }
    let skip = length - (fits - 1);
    "…".to_string() + &text.chars().skip(skip).collect::<String>()
}

/// What the viewport shows when no file is open: the mark, and one line telling you how to open one.
///
/// Nothing else. A recent-files list or a tips carousel here would be the first place the editor
/// started competing with the work instead of holding it.
fn draw_empty_state(frame: &mut Frame, state: &EditorState, code: Rect, scale: f32) {
    let theme = &state.theme;
    let size = 64.0 * scale;
    let line_height = 28.0 * scale;
    let block = size + line_height * 2.4;

    let top = (code.y + (code.height - block) / 2.0).round();
    let left = (code.x + (code.width - size) / 2.0).round();

    // Faint, over the canvas it sits on: the mark is present, not insistent.
    frame.mark(
        Rect::new(left, top, size, size),
        theme.ink_faint.with_alpha(0.4),
        theme.brand().with_alpha(0.4),
    );

    // The one line of teaching in the whole editor, and it goes here because this is the screen
    // someone is looking at when they have no idea what to do next. Not a tips carousel, not a
    // tour: two keys, as the chips every card draws them as, and the second answers everything else.
    let open = if state.modal { "<c-p>" } else { "Ctrl+P" };
    let help = key_hint(state, "help");
    for (index, (keys, words)) in [
        (open, "open a file"),
        (help.as_str(), "every key that works"),
    ]
    .into_iter()
    .enumerate()
    {
        let chips = crate::layout::chips_width(&state.typography, keys, scale);
        let gap = 10.0 * scale;
        let text = words.chars().count() as f32
            * crate::layout::measured_advance(&state.typography, Role::Ui, scale);
        let total = chips + gap + text;
        // Centred as one unit; on a code area too narrow for it, the words give way, not the keys.
        let x = (code.x + (code.width - total) / 2.0).max(code.x + space::SM * scale);
        let row = Rect::new(
            x,
            top + size + space::SM * scale + index as f32 * line_height * 1.2,
            total,
            line_height,
        );
        key_chip(frame, state, keys, x + chips, row.y, row.height, scale);
        let room = (code.right() - space::SM * scale - (x + chips + gap)).max(0.0);
        frame.text(ui(
            state,
            elide(state, Role::Ui, words, room, scale),
            Rect::new(x + chips + gap, row.y, room, row.height),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }
}

/// The settings tab, drawn in the editor area in place of code.
///
/// **Every rectangle comes from [`crate::settings_ui::Page::geometry_at`]**, at the same scroll
/// offset the mouse reads (the spring's value at the frame's instant), so what is drawn under the
/// pointer is what a click there changes. Nothing here measures a row: the list is clipped to the
/// geometry's `list`, and a row partly scrolled out is drawn whole and cut by the clip.
fn draw_settings_page(
    frame: &mut Frame,
    state: &EditorState,
    page: &crate::settings_ui::Page,
    rect: Rect,
    scale: f32,
) {
    use crate::settings_ui::Entry;

    let theme = &state.theme;
    let now = state.now();
    let motion = &state.chrome.motion;
    let values = state.setting_values();
    let offset = motion.movement(Key::SettingsScroll, page.scroll(), now);
    let geometry = page.geometry_at(offset, rect, scale, &values, state.modal, &state.typography);

    draw_settings_tree(frame, state, page, &geometry, rect, scale);
    draw_field(
        frame,
        state,
        geometry.search,
        page.query(),
        "Search settings",
        0.0,
        scale,
    );

    // A new page fades in — the old one is gone at once, so a fade-in rather than a cross-fade; a
    // search does not fade, since it changes with every key and a list that faded on each one would
    // never be still enough to read.
    let fade = motion.fade(Key::SettingsCategory, 1.0, now).clamp(0.0, 1.0);
    frame.set_clip(Some(geometry.list));
    frame.set_opacity(fade);
    let selected = page.selected();
    for row in &geometry.rows {
        match row.entry {
            Entry::Heading { .. } => draw_page_heading(frame, state, row, scale),
            _ => {
                draw_page_row(
                    frame,
                    state,
                    page,
                    row,
                    row.index == selected,
                    &values,
                    scale,
                );
            }
        }
    }
    frame.set_opacity(1.0);
    frame.set_clip(None);

    if let Some(empty) = geometry.empty {
        let says = if page.query().trim().is_empty() {
            "Nothing found on this machine yet.".to_string()
        } else {
            format!("No setting matches \"{}\".", page.query().trim())
        };
        frame.text(ui(
            state,
            elide(state, Role::Ui, &says, empty.width, scale),
            empty,
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }

    // A thumb at the list's right edge when it scrolls: a list that runs past the bottom with no sign
    // of it reads as a page that is missing settings.
    if geometry.max_scroll > 0.0 && geometry.list.height > 0.0 {
        let list = geometry.list;
        let total = list.height + geometry.max_scroll;
        let height = (list.height * list.height / total)
            .max(24.0 * scale)
            .min(list.height);
        let y = list.y + (list.height - height) * (geometry.scroll / geometry.max_scroll);
        let width = 3.0 * scale;
        frame.fill_rounded(
            Rect::new(list.right() - width, y, width, height),
            theme.ink.with_alpha(0.18),
            width / 2.0,
        );
    }
}

/// The category tree: a column of pages beside the list, or — folded — one button that names the
/// page and moves to the next.
fn draw_settings_tree(
    frame: &mut Frame,
    state: &EditorState,
    page: &crate::settings_ui::Page,
    geometry: &crate::settings_ui::PageGeometry,
    rect: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let searching = !page.query().trim().is_empty();
    let inset = space::SM * scale;

    if geometry.folded {
        let Some((_, button)) = geometry.tree.first().copied() else {
            return;
        };
        frame.fill_rounded(button, theme.bg_panel, 6.0 * scale);
        if hovered(state, button) {
            frame.fill_rounded(button, theme.ink.with_alpha(0.06), 6.0 * scale);
        }
        frame.outline_rounded(button, theme.rule, 6.0 * scale, border::BORDER * scale);
        let chevron = Rect::new(
            button.right() - button.height,
            button.y,
            button.height.min(button.width),
            button.height,
        );
        frame.icon(
            Icon::ChevronRight,
            centred(chevron, 12.0 * scale),
            theme.ink_dim,
        );
        let text = Rect::new(
            button.x + inset,
            button.y,
            (chevron.x - button.x - inset).max(0.0),
            button.height,
        );
        frame.text(ui(
            state,
            elide(state, Role::Ui, page.folded_label(), text.width, scale),
            text,
            theme.ink,
            Align::Left,
            scale,
        ));
        return;
    }

    // The column is a surface of its own, as JetBrains draws it, so the list reads as the page and
    // the tree as the way between pages.
    if let Some((_, last)) = geometry.tree.last() {
        // The tiles are inset by the same margin on both sides of the column.
        let margin = last.x - rect.x;
        let column = Rect::new(rect.x, rect.y, last.right() + margin - rect.x, rect.height);
        frame.fill(column, theme.bg_panel.with_alpha(0.55));
        frame.fill(
            Rect::new(
                column.right() - border::BORDER * scale,
                column.y,
                border::BORDER * scale,
                column.height,
            ),
            theme.rule_soft,
        );
    }
    for (category, tile) in &geometry.tree {
        // During a search the list is every page's; the tree marks none of them as open, and dims
        // the ones with nothing to show.
        let open = !searching && *category == page.category();
        let empty = searching && !page.has_matches(*category, state.modal);
        if open {
            frame.fill_rounded(*tile, accent.with_alpha(0.16), 5.0 * scale);
            frame.fill_rounded(
                Rect::new(
                    tile.x,
                    tile.y + tile.height * 0.25,
                    2.0 * scale,
                    tile.height * 0.5,
                ),
                accent,
                1.0 * scale,
            );
        } else if hovered(state, *tile) {
            frame.fill_rounded(*tile, theme.ink.with_alpha(0.06), 5.0 * scale);
        }
        let text = Rect::new(
            tile.x + inset,
            tile.y,
            (tile.width - inset * 2.0).max(0.0),
            tile.height,
        );
        frame.text(ui(
            state,
            elide(state, Role::Ui, category.title(), text.width, scale),
            text,
            if open {
                theme.ink
            } else if empty {
                theme.ink_faint
            } else {
                theme.ink_dim
            },
            Align::Left,
            scale,
        ));
    }
}

/// A section heading in the list: "Wrapping", or "Editor › Wrapping" in a search.
fn draw_page_heading(
    frame: &mut Frame,
    state: &EditorState,
    row: &crate::settings_ui::PageRow,
    scale: f32,
) {
    let Some(text) = row.heading_text() else {
        return;
    };
    let mut spec = state.typography.spec(Role::Prose);
    spec.size_px *= scale;
    let advance = crate::layout::measured_advance(&state.typography, Role::Prose, scale);
    frame.text(TextRun {
        text: elide_to(&text, row.label.width, advance),
        spec,
        color: state.theme.ink,
        rect: row.label,
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
        clip: None,
    });
}

/// A row of the list that is not a heading: a setting, a command, an import or a report.
/// The ● in the margin of a row that differs from its default, and the ⟲ that puts it back — the
/// same pair on a setting and on a command whose keys were moved.
fn draw_row_reset(
    frame: &mut Frame,
    state: &EditorState,
    row: &crate::settings_ui::PageRow,
    reset: Rect,
    accent: Oklch,
    scale: f32,
) {
    let theme = &state.theme;
    // The ● says "changed" in the margin, where the eye scanning the names meets it; the ⟲ beside
    // the control is what puts it back.
    let inset = row.label.x - row.rect.x;
    frame.disc(
        [
            row.rect.x + inset / 2.0,
            row.label.y + row.label.height / 2.0,
        ],
        3.0 * scale,
        accent,
    );
    let over = hovered(state, reset);
    if over {
        frame.fill_rounded(reset, theme.ink.with_alpha(0.08), 5.0 * scale);
    }
    frame.icon(
        Icon::Reset,
        centred(reset, 14.0 * scale),
        if over { theme.ink } else { theme.ink_dim },
    );
}

fn draw_page_row(
    frame: &mut Frame,
    state: &EditorState,
    page: &crate::settings_ui::Page,
    row: &crate::settings_ui::PageRow,
    selected: bool,
    values: &crate::settings_ui::Values,
    scale: f32,
) {
    use crate::settings_ui::{Entry, Line};

    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    row_wash(frame, state, row.rect, selected, scale);
    // A change glows and fades, so the row that moved is the one the eye finds — most of all when
    // it was changed by a key from the other end of the page.
    if let Some(field) = row.field() {
        let glow = state.chrome.motion.get(
            Key::Glow(crate::settings_ui::glow_index(field)),
            state.now(),
        );
        if glow > 0.0 {
            frame.fill_rounded(
                row.rect,
                accent.with_alpha(0.22 * glow.min(1.0)),
                5.0 * scale,
            );
        }
    }

    // The name, set heavier than the text under it.
    // What a search matched, lit under the characters that matched it: the list says which rows
    // matched, and this says why — "wrap" in "Soft wrap" and in its sentence.
    let query = page.query();
    let mark = |frame: &mut Frame, shown: &str, area: Rect, role: Role| {
        let advance = crate::layout::measured_advance(&state.typography, role, scale);
        for range in crate::settings_ui::match_ranges(shown, query) {
            let x = area.x + range.start as f32 * advance;
            let width = (range.len() as f32 * advance).min(area.right() - x);
            if width > 0.0 {
                frame.fill_rounded(
                    Rect::new(x, area.y + area.height * 0.12, width, area.height * 0.76),
                    accent.with_alpha(0.30),
                    3.0 * scale,
                );
            }
        }
    };
    let name = |frame: &mut Frame, text: &str, color: Oklch| {
        let mut spec = state.typography.spec(Role::Ui);
        spec.size_px *= scale;
        let shown = elide(state, Role::Ui, text, row.label.width, scale);
        mark(frame, &shown, row.label, Role::Ui);
        frame.text(TextRun {
            text: shown,
            spec,
            color,
            rect: row.label,
            align: Align::Left,
            center_vertically: true,
            wrap: false,
            layer: Layer::default(),
            clip: None,
        });
    };
    // Wrapped exactly as the geometry counted it, line by line, each line a row of its own.
    let explain = |frame: &mut Frame, text: &str, color: Oklch| {
        let Some(area) = row.explanation else {
            return;
        };
        let advance = crate::layout::measured_advance(&state.typography, Role::Value, scale);
        let lines = wrap_at(text, area.width, advance);
        let line_height = area.height / lines.len().max(1) as f32;
        for (at, line) in lines.iter().enumerate() {
            let rect = Rect::new(
                area.x,
                area.y + at as f32 * line_height,
                area.width,
                line_height,
            );
            mark(frame, line, rect, Role::Value);
            frame.text(run(
                state,
                Role::Value,
                line.clone(),
                rect,
                color,
                Align::Left,
                scale,
            ));
        }
    };

    match row.entry {
        Entry::Heading { .. } => {}
        Entry::Field(field) => {
            name(frame, field.label(state.modal), theme.ink);
            if let Some(text) = field.explanation(state.modal) {
                explain(frame, text, theme.ink_dim);
            }
            if let Some(control) = row.control {
                draw_control(frame, state, field, values, row, control, scale);
            }
            if let Some(reset) = row.reset {
                draw_row_reset(frame, state, row, reset, accent, scale);
            }
        }
        Entry::Command(command) => {
            // A row waiting for its key is lit as a change is, and says what it is waiting for
            // under its name; the keys it has now stay where they were until the new ones land.
            let waiting = page
                .capture()
                .filter(|capture| capture.command.name == command.name);
            if waiting.is_some() {
                frame.fill_rounded(row.rect, accent.with_alpha(0.16), 5.0 * scale);
            }
            name(
                frame,
                command.what,
                if waiting.is_some() {
                    theme.ink
                } else {
                    theme.ink_dim
                },
            );
            if let Some(capture) = waiting {
                explain(frame, &capture.text(state.modal), theme.ink);
            }
            if let Some(keys) = row.control {
                let shown = fit_keys(state, &command.shown(state.modal), keys.width, scale);
                key_chip(
                    frame,
                    state,
                    &shown,
                    keys.right(),
                    keys.y,
                    keys.height,
                    scale,
                );
            }
            if let Some(reset) = row.reset {
                draw_row_reset(frame, state, row, reset, accent, scale);
            }
        }
        Entry::Link(link) => name(frame, link.label(), accent),
        Entry::Line(line) => match line {
            Line::Import(item) => {
                name(frame, line.label().unwrap_or_default(), theme.ink);
                if let Some(control) = row.control {
                    let ticked = page.is_ticked(item);
                    let side = (control.height * 0.62).round();
                    let tick = Rect::new(
                        control.right() - side,
                        control.y + (control.height - side) / 2.0,
                        side,
                        side,
                    );
                    if ticked {
                        frame.fill_rounded(tick, accent, 4.0 * scale);
                        frame.fill_rounded(tick.inset(side * 0.3), theme.bg, 2.0 * scale);
                    } else {
                        // Outline only: filled, an empty box on the selected row's wash read as
                        // a ticked one.
                        frame.outline_rounded(tick, theme.ink_faint, 4.0 * scale, 1.5 * scale);
                    }
                }
            }
            Line::RunImport | Line::LoadTheme => {
                let available = line != Line::RunImport || crate::import_cli::vscode_is_present();
                name(
                    frame,
                    line.label().unwrap_or_default(),
                    if available { accent } else { theme.ink_faint },
                );
                let text = if available {
                    line.explanation().unwrap_or_default()
                } else {
                    "No VSCode installation was found on this machine, so there is nothing to import."
                };
                explain(frame, text, theme.ink_dim);
            }
            Line::Server(at) | Line::Formatter(at) => {
                let list = if matches!(line, Line::Formatter(_)) {
                    page.formatters()
                } else {
                    page.servers()
                };
                let Some(report) = list.get(at) else {
                    return;
                };
                let found = if report.found { "found" } else { "missing" };
                let found_width =
                    crate::layout::measured_advance(&state.typography, Role::Value, scale)
                        * (found.chars().count() as f32 + 1.0);
                let (status, label) = row.label.split_right(found_width.min(row.label.width));
                let mark = if report.configured {
                    " · configured"
                } else {
                    ""
                };
                frame.text(ui(
                    state,
                    elide(
                        state,
                        Role::Ui,
                        &format!("{} · {}{mark}", report.language, report.program),
                        label.width,
                        scale,
                    ),
                    label,
                    theme.ink,
                    Align::Left,
                    scale,
                ));
                frame.text(run(
                    state,
                    Role::Value,
                    found,
                    status,
                    if report.found {
                        theme.fresh()
                    } else {
                        theme.ink_faint
                    },
                    Align::Right,
                    scale,
                ));
                // A path loses its front, where the part that names the program is its end; an
                // install line loses its end, since its start is the command.
                if let Some(area) = row.explanation {
                    let shown = if report.found {
                        elide_start(state, Role::Value, &report.status, area.width, scale)
                    } else {
                        elide(state, Role::Value, &report.status, area.width, scale)
                    };
                    frame.text(run(
                        state,
                        Role::Value,
                        shown,
                        area,
                        theme.ink_dim,
                        Align::Left,
                        scale,
                    ));
                }
            }
        },
    }
}

/// A setting's control: a switch, a stepper or a choice, drawn in `control`.
fn draw_control(
    frame: &mut Frame,
    state: &EditorState,
    field: crate::settings_ui::Field,
    values: &crate::settings_ui::Values,
    row: &crate::settings_ui::PageRow,
    control: Rect,
    scale: f32,
) {
    use crate::settings_ui::Control;
    let swatch = row.swatch;

    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let value = values.get(field);
    match field.control() {
        Control::Switch => {
            let on = value == "on";
            let height = control.height.min(22.0 * scale);
            let pill = Rect::new(
                control.right() - (height * 1.8).min(control.width),
                control.y + (control.height - height) / 2.0,
                (height * 1.8).min(control.width),
                height,
            );
            if on {
                frame.fill_rounded(pill, accent, height / 2.0);
            } else {
                frame.fill_rounded(pill, theme.bg_alt, height / 2.0);
                frame.outline_rounded(pill, theme.rule, height / 2.0, border::BORDER * scale);
            }
            let knob = height * 0.36;
            let x = if on {
                pill.right() - height / 2.0
            } else {
                pill.x + height / 2.0
            };
            frame.disc(
                [x, pill.y + height / 2.0],
                knob,
                if on { theme.bg } else { theme.ink_faint },
            );
        }
        Control::Stepper => {
            // The two buttons are the geometry's, the same boxes a click is tested against.
            let Some((down, up)) = row.stepper else {
                return;
            };
            let middle = Rect::new(
                down.right(),
                control.y,
                (up.x - down.right()).max(0.0),
                control.height,
            );
            for (button, glyph) in [(down, "-"), (up, "+")] {
                frame.fill_rounded(button, theme.bg_alt, 5.0 * scale);
                if hovered(state, button) {
                    frame.fill_rounded(button, theme.ink.with_alpha(0.08), 5.0 * scale);
                }
                frame.outline_rounded(button, theme.rule, 5.0 * scale, border::BORDER * scale);
                frame.text(run(
                    state,
                    Role::Ui,
                    glyph,
                    button,
                    theme.ink_dim,
                    Align::Center,
                    scale,
                ));
            }
            frame.text(run(
                state,
                Role::Value,
                elide(state, Role::Value, &value, middle.width, scale),
                middle,
                theme.ink,
                Align::Center,
                scale,
            ));
        }
        kind @ (Control::Choice(_) | Control::Dropdown(_)) => {
            // A dropdown's control is outlined in the accent while its list is open, so the card
            // under it reads as coming from it.
            let open = matches!(kind, Control::Dropdown(_))
                && state
                    .settings_page
                    .as_ref()
                    .is_some_and(|page| page.dropdown().is_some());
            frame.fill_rounded(control, theme.bg_alt, 5.0 * scale);
            frame.outline_rounded(
                control,
                if open { accent } else { theme.rule },
                5.0 * scale,
                border::BORDER * scale,
            );
            if hovered(state, control) {
                frame.fill_rounded(control, theme.ink.with_alpha(0.05), 5.0 * scale);
            }
            let chevron = Rect::new(
                control.right() - control.height.min(control.width),
                control.y,
                control.height.min(control.width),
                control.height,
            );
            // Right for a choice, which steps to the next; down for one that opens a list.
            frame.icon(
                if matches!(kind, Control::Dropdown(_)) {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                centred(chevron, 12.0 * scale),
                theme.ink_dim,
            );
            let mut left = control.x + space::SM * scale;
            if let Some(swatch) = swatch {
                draw_swatch(frame, theme, values.theme, swatch, scale);
                left = swatch.right() + space::XS * scale;
            }
            let text = Rect::new(left, control.y, (chevron.x - left).max(0.0), control.height);
            frame.text(ui(
                state,
                elide(state, Role::Ui, &value, text.width, scale),
                text,
                theme.ink,
                Align::Left,
                scale,
            ));
        }
    }
}

/// The settings tab's theme dropdown: the themes under "Dark" and "Light", each with its colour
/// chip, the one in use marked and the highlighted or hovered one washed.
///
/// Laid out by [`crate::settings_ui::Page::dropdown_geometry`] from the same page geometry the
/// drawing above it used, which is what the mouse reads too.
fn draw_theme_dropdown(
    frame: &mut Frame,
    state: &EditorState,
    page: &crate::settings_ui::Page,
    editor: Rect,
    travel: f32,
    scale: f32,
) {
    use crate::settings_ui::Pick;
    let Some(dropdown) = page.dropdown() else {
        return;
    };
    let theme = &state.theme;
    let now = state.now();
    let values = state.setting_values();
    let offset = state
        .chrome
        .motion
        .movement(Key::SettingsScroll, page.scroll(), now);
    let rows = page.geometry_at(
        offset,
        editor,
        scale,
        &values,
        state.modal,
        &state.typography,
    );
    let Some(geometry) = page.dropdown_geometry(&rows, editor, scale, &state.typography) else {
        return;
    };

    let anchor = if geometry.above {
        Anchor::Bottom
    } else {
        Anchor::Top
    };
    frame.set_zoom(Some(appear_zoom(geometry.card, travel, anchor)));
    card(frame, theme, geometry.card, scale, 1.0);

    frame.set_clip(Some(geometry.list));
    let inset = 4.0 * scale;
    for row in &geometry.rows {
        if !row.clip.is_visible() {
            continue;
        }
        match row.pick {
            Pick::Heading(appearance) => {
                frame.text(label(
                    state,
                    elide(
                        state,
                        Role::Label,
                        appearance.title(),
                        row.label.width,
                        scale,
                    ),
                    row.label,
                    theme.ink_faint,
                    Align::Left,
                    scale,
                ));
            }
            Pick::Theme(name) => {
                let wash = Rect::new(
                    row.rect.x + inset,
                    row.rect.y,
                    (row.rect.width - inset * 2.0).max(0.0),
                    row.rect.height,
                );
                let selected = row.index == dropdown.selected();
                // `row_wash` asks whether the pointer is over the row, which must be the part on
                // screen: a row scrolled half out of the list is not hovered by a pointer below it.
                if selected || pointer_in(state, row.clip) {
                    overlay_row_wash(frame, state, wash, selected, scale);
                }
                if let Some(swatch) = row.swatch {
                    draw_swatch(frame, theme, Some(name), swatch, scale);
                }
                let current = values.theme == Some(name);
                frame.text(ui(
                    state,
                    // The display name: the heading above already says which half of a pair it is.
                    elide(
                        state,
                        Role::Ui,
                        Theme::display_name_of(name).unwrap_or(name),
                        row.label.width,
                        scale,
                    ),
                    row.label,
                    if selected || current {
                        theme.ink
                    } else {
                        theme.ink_dim
                    },
                    Align::Left,
                    scale,
                ));
                if let (true, Some(mark)) = (current, row.mark) {
                    frame.icon(
                        Icon::Dot,
                        centred(mark, 10.0 * scale),
                        theme.accent(state.mode),
                    );
                }
            }
        }
    }
    frame.set_clip(None);

    // A thumb when the list scrolls, as the page has: a list cut off with no sign of it reads as a
    // short list.
    if geometry.max_scroll > 0.0 && geometry.list.height > 0.0 {
        let list = geometry.list;
        let total = list.height + geometry.max_scroll;
        let height = (list.height * list.height / total)
            .max(16.0 * scale)
            .min(list.height);
        let y = list.y + (list.height - height) * (geometry.scroll / geometry.max_scroll);
        let width = 3.0 * scale;
        frame.fill_rounded(
            Rect::new(list.right() - width - 2.0 * scale, y, width, height),
            theme.ink.with_alpha(0.18),
            width / 2.0,
        );
    }
}

/// A theme's colours, small: its background with two of its syntax colours and its accent on it.
fn draw_swatch(frame: &mut Frame, current: &Theme, name: Option<&str>, rect: Rect, scale: f32) {
    let theme = name.and_then(Theme::by_name).unwrap_or(*current);
    let radius = 4.0 * scale;
    frame.fill_rounded(rect, theme.bg, radius);
    let dot = rect.width * 0.16;
    let y = rect.y + rect.height / 2.0;
    for (at, color) in [
        theme.syntax.keyword,
        theme.syntax.string,
        theme.accent(Mode::Normal),
    ]
    .into_iter()
    .enumerate()
    {
        frame.disc(
            [rect.x + rect.width * (0.25 + 0.25 * at as f32), y],
            dot,
            color,
        );
    }
    frame.outline_rounded(rect, current.rule, radius, border::BORDER * scale);
}

/// A command's keys cut to the alternatives that fit `width`, the first always kept: one key you
/// can read beats two squeezed past the name beside them.
fn fit_keys(state: &EditorState, keys: &str, width: f32, scale: f32) -> String {
    let presses = crate::layout::key_presses(keys);
    let mut kept: Vec<&str> = Vec::new();
    for press in presses {
        let candidate = kept
            .iter()
            .copied()
            .chain([press])
            .collect::<Vec<_>>()
            .join("  ");
        if !kept.is_empty()
            && crate::layout::chips_width(&state.typography, &candidate, scale) > width
        {
            break;
        }
        kept.push(press);
    }
    kept.join("  ")
}

/// Every problem in the **project**, as a list.
///
/// The underlines say *where* in this file; this says what is wrong anywhere. Both are needed: `]d`
/// walks one file's problems one at a time, which cannot answer "did my change break anything?"
/// without a build in the terminal.
///
/// Ordered by [`crate::problems::Store::ordered`]: the file you are in first, then files with an
/// error, then the rest. Grouping by file is what makes it scannable, because going somewhere means
/// opening a file.
fn draw_diagnostics_list(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    let pad = space::LG * scale;
    let line = 20.0 * scale;
    let width = rect.width - pad * 2.0;
    let mut y = rect.y + pad;

    let (errors, warnings) = state.problems.counts();
    frame.text(run(
        state,
        Role::Prose,
        "Problems",
        Rect::new(rect.x + pad, y, width, line),
        theme.ink,
        Align::Left,
        scale,
    ));
    y += line * 1.4;

    let summary = match state.problems.dropped() {
        // Silently truncating is a panel that has lied about being complete.
        0 => format!("{errors} errors · {warnings} warnings"),
        dropped => format!("{errors} errors · {warnings} warnings · {dropped} not shown"),
    };
    frame.text(label(
        state,
        elide(state, Role::Label, &summary, width, scale),
        Rect::new(rect.x + pad, y, width, line),
        theme.ink_faint,
        Align::Left,
        scale,
    ));
    y += line * 1.6;

    let problems = state.problems.ordered();

    if problems.is_empty() {
        frame.text(run(
            state,
            Role::Value,
            "nothing to report",
            Rect::new(rect.x + pad, y, width, line),
            theme.fresh(),
            Align::Left,
            scale,
        ));
        return;
    }

    let mut last_file: Option<&std::path::Path> = None;

    for problem in problems {
        if y + line * 2.0 > rect.bottom() - pad {
            break;
        }

        // A heading each time the file changes, so a list spanning six files reads as six groups
        // rather than as one column of unrelated line numbers.
        if last_file != Some(problem.path.as_path()) {
            last_file = Some(problem.path.as_path());
            let name = problem
                .path
                .strip_prefix(&state.project_root)
                .unwrap_or(&problem.path)
                .to_string_lossy()
                .replace('\\', "/");
            frame.text(label(
                state,
                elide(state, Role::Label, &name, width, scale),
                Rect::new(rect.x + pad, y, width, line),
                theme.ink_dim,
                Align::Left,
                scale,
            ));
            y += line * 1.2;
            if y + line * 2.0 > rect.bottom() - pad {
                break;
            }
        }

        let color = match problem.severity {
            crate::problems::Severity::Error => theme.over_budget(),
            crate::problems::Severity::Warning => theme.stale(),
            _ => theme.ink_faint,
        };

        let on_cursor_line = problem.line == state.cursor_point().line;
        if on_cursor_line {
            frame.fill_rounded(
                Rect::new(
                    rect.x + pad / 2.0,
                    y - 2.0 * scale,
                    rect.width - pad,
                    line * 2.0,
                ),
                theme.row_active,
                5.0 * scale,
            );
        }

        // The line number in the severity's colour, then the message. Position first, because the
        // list is read to decide where to go.
        frame.text(run(
            state,
            Role::Value,
            format!("{}", problem.line + 1),
            Rect::new(rect.x + pad, y, 36.0 * scale, line),
            color,
            Align::Left,
            scale,
        ));

        // The whole message, with its lines joined by spaces, wrapped across the two rows this entry
        // has. Server messages run to paragraphs; the box that appears when the cursor reaches the diagnostic is where the
        // whole text belongs, and a list is for deciding where to go.
        //
        // Wrapped here rather than by the renderer: it wrapped the whole paragraph into a box two
        // rows tall and clipped the rest, so a long message stopped mid-word at the foot of the
        // second row with nothing to say it went on. Two lines, and the second ends in an ellipsis
        // when there was more.
        let room = (width - 40.0 * scale).max(0.0);
        let advance = crate::layout::measured_advance(&state.typography, Role::Value, scale);
        let lines = wrap_lines(&problem.message, room, advance, 2);
        for (offset, text) in lines.iter().enumerate() {
            frame.text(run(
                state,
                Role::Value,
                text.clone(),
                Rect::new(
                    rect.x + pad + 40.0 * scale,
                    y + offset as f32 * line,
                    room,
                    line,
                ),
                if on_cursor_line {
                    theme.ink
                } else {
                    theme.ink_dim
                },
                Align::Left,
                scale,
            ));
        }

        y += line * 2.0;
    }
}

/// The file tree, in the sidebar.
/// The explorer: a "New +" button, then the project's files with their icons.
fn draw_explorer(
    frame: &mut Frame,
    state: &EditorState,
    tree: &crate::tree::FileTree,
    rect: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    // One walk, shared with the click handler. The two used to be written out separately and had
    // already drifted a pad apart, which is a click that opens the file below the one you pointed
    // at as soon as the tree is longer than the panel.
    let panel = crate::tree::panel(rect, scale, tree);

    let button = panel.header;
    frame.fill_rounded(
        button,
        if hovered(state, button) {
            theme.row_active
        } else {
            theme.rule_soft
        },
        6.0 * scale,
    );
    frame.text(ui(state, "New  +", button, theme.ink, Align::Center, scale));

    if tree.is_empty() {
        frame.text(ui(
            state,
            "Indexing…",
            Rect::new(
                rect.x + 16.0 * scale,
                button.bottom() + 12.0 * scale,
                rect.width - 32.0 * scale,
                24.0 * scale,
            ),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        return;
    }

    let icon = 16.0 * scale;
    for (index, (row, box_)) in panel.rows.iter().enumerate() {
        let selected = panel.cursor == Some(index);
        let highlight = Rect::new(
            box_.x + 8.0 * scale,
            box_.y + 1.0 * scale,
            box_.width - 16.0 * scale,
            box_.height - 2.0 * scale,
        );
        if selected {
            // The selection says whose it is. Outlined, the arrow keys are the tree's; filled with
            // no outline, they are the text's and the tree is only being looked at. Both keep the
            // `row_active` fill and hover keeps `rule_soft`, so a selected row never reads as a
            // hovered one — which is what the quiet state used to look like.
            frame.fill_rounded(highlight, theme.row_active, 5.0 * scale);
            if state.focus == crate::focus::Focus::Sidebar {
                frame.outline_rounded(highlight, theme.primary, 5.0 * scale, 1.0 * scale);
            }
        } else if hovered(state, *box_) {
            frame.fill_rounded(highlight, theme.rule_soft, 5.0 * scale);
        }

        // A chevron for a directory and the space it would take for a file, so names at one depth
        // line up whichever they are.
        let indent = box_.x + 14.0 * scale + row.depth as f32 * 14.0 * scale;
        let chevron = Rect::new(indent, box_.y, 12.0 * scale, box_.height);
        let glyph = Rect::new(chevron.right() + 4.0 * scale, box_.y, icon, box_.height);
        if row.is_directory {
            frame.icon(
                if row.expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                centred(chevron, 12.0 * scale),
                theme.ink_faint,
            );
            frame.icon(Icon::Folder, centred(glyph, icon), theme.ink_dim);
        } else {
            frame.icon(
                Icon::File,
                centred(glyph, icon),
                file_color(theme, &row.name),
            );
        }

        let name = Rect::new(
            glyph.right() + 8.0 * scale,
            box_.y,
            (box_.right() - 12.0 * scale - (glyph.right() + 8.0 * scale)).max(0.0),
            box_.height,
        );
        frame.text(ui(
            state,
            elide(state, Role::Ui, &row.name, name.width, scale),
            name,
            if selected { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));
    }
}

/// A sidebar view for a feature that is not built yet, saying so plainly.
///
/// Better than an icon that does nothing — which reads as broken — and better than a view that
/// pretends: it names what will be here and what to use meanwhile.
/// The search view: query and replacement boxes, the toggles, and the hits grouped by file. Every
/// rectangle comes from [`crate::search_panel::SearchPanel::geometry`], which the mouse reads too.
fn draw_search_panel(
    frame: &mut Frame,
    state: &EditorState,
    panel: &crate::search_panel::SearchPanel,
    rect: Rect,
    scale: f32,
) {
    use crate::search_panel::{Field, Row};
    let theme = &state.theme;
    let geometry = panel.geometry(rect, scale);
    let focused = state.focus == crate::focus::Focus::Sidebar;
    let radius = 5.0 * scale;
    let inner = 8.0 * scale;

    frame.text(label(
        state,
        "SEARCH",
        geometry.heading,
        theme.ink_faint,
        Align::Left,
        scale,
    ));

    // A box: its value, or what it is for in faint ink when empty, and an outline in the accent
    // while the keys are typing into it.
    let field =
        |frame: &mut Frame, rect: Rect, value: &str, placeholder: &str, which: Field, room: f32| {
            frame.fill_rounded(rect, theme.bg, radius);
            let active = focused && panel.field == which;
            frame.outline_rounded(
                rect,
                if active { theme.primary } else { theme.rule },
                radius,
                1.0 * scale,
            );
            let text_rect = Rect::new(rect.x + inner, rect.y, (room - inner).max(0.0), rect.height);
            if value.is_empty() {
                frame.text(ui(
                    state,
                    placeholder,
                    text_rect,
                    theme.ink_faint,
                    Align::Left,
                    scale,
                ));
            } else {
                // The end is what is being typed, so a long value keeps its end in view.
                let shown = elide_start(state, Role::Ui, value, text_rect.width, scale);
                frame.text(ui(state, shown, text_rect, theme.ink, Align::Left, scale));
            }
            if active {
                let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);
                let typed = value
                    .chars()
                    .count()
                    .min((text_rect.width / advance) as usize);
                let x = text_rect.x + typed as f32 * advance;
                frame.fill(
                    Rect::new(
                        x,
                        rect.y + 6.0 * scale,
                        1.5 * scale,
                        rect.height - 12.0 * scale,
                    ),
                    theme.primary,
                );
            }
        };

    let toggles_left = geometry.toggles[0].1.x;
    field(
        frame,
        geometry.query,
        &panel.query,
        "Search",
        Field::Query,
        toggles_left - geometry.query.x - 4.0 * scale,
    );
    for (toggle, cell) in geometry.toggles {
        let on = panel.is_on(toggle);
        if on {
            frame.fill_rounded(cell, theme.row_active, 4.0 * scale);
            frame.outline_rounded(cell, theme.primary, 4.0 * scale, 1.0 * scale);
        } else if hovered(state, cell) {
            frame.fill_rounded(cell, theme.rule_soft, 4.0 * scale);
        }
        frame.text(ui(
            state,
            toggle.glyph(),
            cell,
            if on { theme.ink } else { theme.ink_dim },
            Align::Center,
            scale,
        ));
    }

    field(
        frame,
        geometry.replace,
        &panel.replace,
        "Replace",
        Field::Replace,
        geometry.replace.width - inner,
    );
    let button = geometry.replace_all;
    let ready = !panel.query.is_empty() && !panel.hits().is_empty();
    if panel.confirming {
        frame.fill_rounded(button, theme.primary, radius);
    } else {
        frame.fill_rounded(
            button,
            if hovered(state, button) && ready {
                theme.row_active
            } else {
                theme.bg_alt
            },
            radius,
        );
        frame.outline_rounded(button, theme.rule, radius, 1.0 * scale);
    }
    frame.text(ui(
        state,
        "All",
        button,
        if panel.confirming {
            theme.bg
        } else if ready {
            theme.ink
        } else {
            theme.ink_faint
        },
        Align::Center,
        scale,
    ));

    let summary = if panel.replacing() {
        "replacing…".to_string()
    } else if panel.confirming {
        "press All again to replace — not undoable".to_string()
    } else {
        panel.summary()
    };
    frame.text(label(
        state,
        elide(state, Role::Label, &summary, geometry.summary.width, scale),
        geometry.summary,
        theme.ink_faint,
        Align::Left,
        scale,
    ));

    let pad = 12.0 * scale;
    let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);
    let hits = panel.hits();
    for (index, row_rect) in &geometry.rows {
        let row_rect = *row_rect;
        match &panel.rows()[*index] {
            Row::File { path, count } => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let folder = path
                    .parent()
                    .and_then(|parent| parent.strip_prefix(&state.project_root).ok())
                    .map(|parent| parent.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                let count = count.to_string();
                let count_width = (count.len() as f32 + 1.0) * advance;
                let width = row_rect.width - pad * 2.0 - count_width;
                let name_shown = elide(state, Role::Ui, &name, width, scale);
                let name_width = name_shown.chars().count() as f32 * advance;
                frame.text(ui(
                    state,
                    name_shown,
                    Rect::new(row_rect.x + pad, row_rect.y, width, row_rect.height),
                    theme.ink,
                    Align::Left,
                    scale,
                ));
                let folder_room = width - name_width - advance;
                if !folder.is_empty() && folder_room > advance * 4.0 {
                    frame.text(ui(
                        state,
                        elide_start(state, Role::Ui, &folder, folder_room, scale),
                        Rect::new(
                            row_rect.x + pad + name_width + advance,
                            row_rect.y,
                            folder_room,
                            row_rect.height,
                        ),
                        theme.ink_faint,
                        Align::Left,
                        scale,
                    ));
                }
                frame.text(ui(
                    state,
                    count,
                    Rect::new(
                        row_rect.right() - pad - count_width,
                        row_rect.y,
                        count_width,
                        row_rect.height,
                    ),
                    theme.ink_dim,
                    Align::Right,
                    scale,
                ));
            }
            Row::Hit(hit_index) => {
                let Some(hit) = hits.get(*hit_index) else {
                    continue;
                };
                let selected = *hit_index == panel.selected;
                let highlight = Rect::new(
                    row_rect.x + 4.0 * scale,
                    row_rect.y + 1.0 * scale,
                    row_rect.width - 8.0 * scale,
                    row_rect.height - 2.0 * scale,
                );
                if selected {
                    frame.fill_rounded(highlight, theme.row_active, radius);
                    if focused {
                        frame.outline_rounded(highlight, theme.primary, radius, 1.0 * scale);
                    }
                } else if hovered(state, row_rect) {
                    frame.fill_rounded(highlight, theme.rule_soft, radius);
                }
                // The line from a little before the match, so a match far along a long line is
                // still in view, with the match itself marked.
                let indent = pad * 2.0;
                let room = row_rect.width - indent - pad;
                let columns = (room / advance).max(0.0) as usize;
                let start = hit.range.start.min(hit.line.len());
                let before: String = hit.line[..start].trim_start().to_string();
                let lead = before.chars().count().saturating_sub(12);
                let before: String = before.chars().skip(lead).collect();
                let end = hit.range.end.clamp(start, hit.line.len());
                let matched = &hit.line[start..end];
                let after = &hit.line[end..];
                let shown: String = format!("{before}{matched}{after}")
                    .chars()
                    .take(columns)
                    .collect();
                let x = row_rect.x + indent;
                let mark_from = before.chars().count();
                let mark_len = matched
                    .chars()
                    .count()
                    .min(columns.saturating_sub(mark_from));
                if mark_len > 0 {
                    frame.fill_rounded(
                        Rect::new(
                            x + mark_from as f32 * advance,
                            row_rect.y + 3.0 * scale,
                            mark_len as f32 * advance,
                            row_rect.height - 6.0 * scale,
                        ),
                        theme.primary.with_alpha(0.28),
                        2.0 * scale,
                    );
                }
                frame.text(ui(
                    state,
                    shown,
                    Rect::new(x, row_rect.y, room, row_rect.height),
                    if selected { theme.ink } else { theme.ink_dim },
                    Align::Left,
                    scale,
                ));
            }
        }
    }
}

/// The colour of a change's letter: added green, modified in the modified hue, deleted and
/// conflicted red, moved in the primary — the gutter's colours, so a letter here and a mark there
/// mean the same thing.
pub fn change_color(theme: &Theme, letter: char) -> Oklch {
    match letter {
        'A' | 'U' | '?' => theme.fresh(),
        'M' | 'T' => theme.stale(),
        'D' => theme.over_budget(),
        'C' => theme.over_budget(),
        _ => theme.primary,
    }
}

/// What a line of a diff tab is, by its first character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLine {
    Added,
    Removed,
    Hunk,
}

/// `start` is the line's first few characters: `+++ ` and `--- ` name the two files rather than
/// add or remove a line, and are left plain.
fn diff_line_kind(start: impl Iterator<Item = char>) -> Option<DiffLine> {
    let start: String = start.collect();
    if start.starts_with("+++ ") || start.starts_with("--- ") {
        return None;
    }
    match start.chars().next()? {
        '+' => Some(DiffLine::Added),
        '-' => Some(DiffLine::Removed),
        '@' => Some(DiffLine::Hunk),
        _ => None,
    }
}

/// The source-control view: the branch, the commit box, and the changes in their groups.
fn draw_source_control(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    use crate::scm::Group;
    use crate::source_control::{Confirm, Field, Row, State};
    let theme = &state.theme;
    let view = &state.scm;
    let geometry = view.geometry(rect, scale, &state.typography);
    let focused = state.focus == crate::focus::Focus::Sidebar;
    let radius = 5.0 * scale;
    let inner = 8.0 * scale;

    frame.text(label(
        state,
        "SOURCE CONTROL",
        geometry.heading,
        theme.ink_faint,
        Align::Left,
        scale,
    ));
    for (tool, cell) in &geometry.tools {
        small_icon_button(frame, state, *cell, tool.icon(), theme.ink_dim, scale);
    }

    let notice = match &view.state {
        State::Ready(_) => None,
        State::Unknown => Some("Asking git about this folder…".to_string()),
        State::NotARepository => Some(
            "This folder is not in a git repository. Initialize one to start tracking its \
             changes, or open a folder that already is one."
                .to_string(),
        ),
        State::NoGit => Some(
            "git is not installed, or not on PATH, so there is nothing to ask. Install it and \
             refresh."
                .to_string(),
        ),
        State::Failed(why) => Some(format!("git could not read this repository: {why}")),
    };
    if let Some(text) = notice {
        if let Some(button) = geometry.init {
            let over = hovered(state, button);
            frame.fill_rounded(
                button,
                if over {
                    theme.primary.with_alpha(0.85)
                } else {
                    theme.primary
                },
                radius,
            );
            frame.text(ui(
                state,
                elide(
                    state,
                    Role::Ui,
                    "Initialize Repository",
                    button.width,
                    scale,
                ),
                button,
                theme.bg,
                Align::Center,
                scale,
            ));
        }
        let line = 20.0 * scale;
        let mut y = geometry.notice.y;
        for text in wrap_to(
            &state.typography,
            Role::Ui,
            &text,
            geometry.notice.width,
            scale,
        ) {
            if y + line > geometry.notice.bottom() {
                break;
            }
            frame.text(ui(
                state,
                text,
                Rect::new(geometry.notice.x, y, geometry.notice.width, line),
                theme.ink_dim,
                Align::Left,
                scale,
            ));
            y += line;
        }
        return;
    }
    let Some(status) = view.status() else {
        return;
    };
    let advance = crate::layout::measured_advance(&state.typography, Role::Ui, scale);

    // Where you are: the branch, and how far it is from its upstream, both of them buttons.
    if hovered(state, geometry.branch) {
        frame.fill_rounded(geometry.branch, theme.row_active, 4.0 * scale);
    }
    let icon = 14.0 * scale;
    frame.icon(
        Icon::Branch,
        Rect::new(
            geometry.branch.x + 4.0 * scale,
            geometry.branch.y + (geometry.branch.height - icon) / 2.0,
            icon,
            icon,
        ),
        theme.ink_dim,
    );
    let name_rect = Rect::new(
        geometry.branch.x + icon + 10.0 * scale,
        geometry.branch.y,
        (geometry.branch.width - icon - 12.0 * scale).max(0.0),
        geometry.branch.height,
    );
    frame.text(ui(
        state,
        elide(state, Role::Ui, &status.head_name(), name_rect.width, scale),
        name_rect,
        theme.ink,
        Align::Left,
        scale,
    ));
    for (cell, arrow, count) in [
        (geometry.pull, Icon::ArrowDown, status.behind),
        (geometry.push, Icon::ArrowUp, status.ahead),
    ] {
        if hovered(state, cell) {
            frame.fill_rounded(cell, theme.row_active, 4.0 * scale);
        }
        let color = if count > 0 {
            theme.ink
        } else {
            theme.ink_faint
        };
        let side = 13.0 * scale;
        frame.icon(
            arrow,
            Rect::new(
                cell.x + 4.0 * scale,
                cell.y + (cell.height - side) / 2.0,
                side,
                side,
            ),
            color,
        );
        frame.text(ui(
            state,
            count.to_string(),
            Rect::new(
                cell.x + 4.0 * scale + side + 2.0 * scale,
                cell.y,
                cell.width,
                cell.height,
            ),
            color,
            Align::Left,
            scale,
        ));
    }

    // The message box.
    let typing = focused && view.field == Field::Message;
    frame.fill_rounded(geometry.message, theme.bg, radius);
    frame.outline_rounded(
        geometry.message,
        if typing { theme.primary } else { theme.rule },
        radius,
        1.0 * scale,
    );
    let text_left = geometry.message.x + inner;
    let text_width = (geometry.message.width - inner * 2.0).max(0.0);
    let line = crate::source_control::MESSAGE_LINE * scale;
    let top = geometry.message.y + 5.0 * scale;
    if view.message.is_empty() {
        let placeholder = format!("Message (ctrl-enter to commit on '{}')", status.head_name());
        frame.text(ui(
            state,
            elide(state, Role::Ui, &placeholder, text_width, scale),
            Rect::new(text_left, top, text_width, line),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    } else {
        let first = view.message_top();
        for (slot, text) in view
            .message
            .split('\n')
            .skip(first)
            .take(crate::source_control::MESSAGE_LINES)
            .enumerate()
        {
            frame.text(ui(
                state,
                elide(state, Role::Ui, text, text_width, scale),
                Rect::new(text_left, top + slot as f32 * line, text_width, line),
                theme.ink,
                Align::Left,
                scale,
            ));
        }
    }
    if typing {
        let (row, column) = view.caret_line_column();
        let slot = row.saturating_sub(view.message_top());
        let x = (text_left + column as f32 * advance).min(text_left + text_width);
        frame.fill(
            Rect::new(
                x,
                top + slot as f32 * line + 3.0 * scale,
                1.5 * scale,
                line - 6.0 * scale,
            ),
            theme.primary,
        );
    }

    // The commit button: filled while there is something it would commit.
    let committable = view.count(Group::Staged) > 0 || view.count(Group::Changes) > 0 || view.amend;
    let smart = view.confirm == Some(Confirm::StageAllAndCommit);
    let over = hovered(state, geometry.commit);
    frame.fill_rounded(
        geometry.commit,
        if !committable {
            theme.bg_alt
        } else if over {
            theme.primary.with_alpha(0.85)
        } else {
            theme.primary
        },
        radius,
    );
    if !committable {
        frame.outline_rounded(geometry.commit, theme.rule, radius, 1.0 * scale);
    }
    let words = if smart {
        "Stage All and Commit"
    } else if view.amend {
        "Commit (Amend)"
    } else {
        "Commit"
    };
    let check = 14.0 * scale;
    let words_width = words.chars().count() as f32 * advance;
    let start = geometry.commit.x
        + ((geometry.commit.width - words_width - check - 6.0 * scale) / 2.0).max(inner);
    let ink = if committable {
        theme.bg
    } else {
        theme.ink_faint
    };
    frame.icon(
        Icon::Check,
        Rect::new(
            start,
            geometry.commit.y + (geometry.commit.height - check) / 2.0,
            check,
            check,
        ),
        ink,
    );
    let words_rect = Rect::new(
        start + check + 6.0 * scale,
        geometry.commit.y,
        (geometry.commit.right() - start - check - 6.0 * scale).max(0.0),
        geometry.commit.height,
    );
    frame.text(ui(
        state,
        elide(state, Role::Ui, words, words_rect.width, scale),
        words_rect,
        ink,
        Align::Left,
        scale,
    ));

    // What is happening, or the question a second press answers.
    let summary = if let Some(doing) = view.busy {
        (doing.to_string(), theme.ink_dim)
    } else if let Some(confirm) = &view.confirm {
        let question = match confirm {
            Confirm::Discard(paths) if paths.len() == 1 => format!(
                "discard {}? press again — not undoable",
                paths[0].rsplit('/').next().unwrap_or(&paths[0])
            ),
            Confirm::Discard(paths) => {
                format!("discard {} files? press again — not undoable", paths.len())
            }
            Confirm::DiscardAll => "discard every change? press again — not undoable".to_string(),
            Confirm::StageAllAndCommit => {
                "nothing is staged — press again to commit every change".to_string()
            }
        };
        (question, theme.over_budget())
    } else if status.truncated() {
        (
            format!(
                "showing {} of {} changes",
                status.entries.len(),
                status.total
            ),
            theme.ink_faint,
        )
    } else {
        (String::new(), theme.ink_faint)
    };
    frame.text(run(
        state,
        Role::Label,
        elide(
            state,
            Role::Label,
            &summary.0,
            geometry.summary.width,
            scale,
        ),
        geometry.summary,
        summary.1,
        Align::Left,
        scale,
    ));

    // The list. Only the rows on screen: the geometry holds no others.
    let pad = 12.0 * scale;
    let letter_column = 22.0 * scale;
    for (index, row_rect) in &geometry.rows {
        let (index, row_rect) = (*index, *row_rect);
        let selected = view.selected == Some(index);
        let over = hovered(state, row_rect);
        if selected {
            frame.fill(
                row_rect,
                if focused && view.field == Field::List {
                    theme.row_active
                } else {
                    theme.rule_soft
                },
            );
        } else if over {
            frame.fill(row_rect, theme.rule_soft);
        }
        let buttons = if over || (selected && focused) {
            view.action_rects(index, row_rect, scale)
        } else {
            Vec::new()
        };
        let room_right = buttons
            .first()
            .map_or(row_rect.right() - pad - letter_column, |(_, at)| at.x)
            - 4.0 * scale;

        match view.rows()[index] {
            Row::Header(group) => {
                let collapsed = view.collapsed[group.index()];
                let chevron = 14.0 * scale;
                frame.icon(
                    if collapsed {
                        Icon::ChevronRight
                    } else {
                        Icon::ChevronDown
                    },
                    Rect::new(
                        row_rect.x + 6.0 * scale,
                        row_rect.y + (row_rect.height - chevron) / 2.0,
                        chevron,
                        chevron,
                    ),
                    theme.ink_dim,
                );
                let title = Rect::new(
                    row_rect.x + 24.0 * scale,
                    row_rect.y,
                    (room_right - row_rect.x - 24.0 * scale).max(0.0),
                    row_rect.height,
                );
                frame.text(ui(
                    state,
                    elide(state, Role::Ui, group.title(), title.width, scale),
                    title,
                    theme.ink,
                    Align::Left,
                    scale,
                ));
                // The count, in a pill at the end of the row, as VSCode draws it.
                let count = view.count(group).to_string();
                let width = (count.len() as f32 * advance + 10.0 * scale).max(18.0 * scale);
                let pill = Rect::new(
                    row_rect.right() - pad - width,
                    row_rect.y + 3.0 * scale,
                    width,
                    row_rect.height - 6.0 * scale,
                );
                frame.fill_rounded(pill, theme.rule, pill.height / 2.0);
                frame.text(ui(state, count, pill, theme.ink_dim, Align::Center, scale));
            }
            Row::File { group, entry } => {
                let Some(entry) = status.entries.get(entry) else {
                    continue;
                };
                let letter = entry.letter(group);
                let left = row_rect.x + 26.0 * scale;
                let width = (room_right - left).max(0.0);
                let name = elide(state, Role::Ui, entry.name(), width, scale);
                let name_width = name.chars().count() as f32 * advance;
                // A deletion is drawn dim, being a file that is no longer there.
                let ink = if letter == 'D' {
                    theme.ink_faint
                } else {
                    theme.ink
                };
                frame.text(ui(
                    state,
                    name,
                    Rect::new(left, row_rect.y, width, row_rect.height),
                    ink,
                    Align::Left,
                    scale,
                ));
                let folder_room = width - name_width - advance;
                if !entry.directory().is_empty() && folder_room > advance * 3.0 {
                    frame.text(ui(
                        state,
                        elide_start(state, Role::Ui, entry.directory(), folder_room, scale),
                        Rect::new(
                            left + name_width + advance,
                            row_rect.y,
                            folder_room,
                            row_rect.height,
                        ),
                        theme.ink_faint,
                        Align::Left,
                        scale,
                    ));
                }
                frame.text(ui(
                    state,
                    letter.to_string(),
                    Rect::new(
                        row_rect.right() - pad - letter_column,
                        row_rect.y,
                        letter_column,
                        row_rect.height,
                    ),
                    change_color(theme, letter),
                    Align::Center,
                    scale,
                ));
            }
        }

        for (action, cell) in buttons {
            if hovered(state, cell) {
                frame.fill_rounded(cell, theme.row_active, 4.0 * scale);
            }
            frame.icon(
                action.icon(),
                centred(cell, 14.0 * scale),
                if hovered(state, cell) {
                    theme.ink
                } else {
                    theme.ink_dim
                },
            );
        }
    }
}

/// A toolbar button at the size of a sidebar's heading: [`icon_button`]'s, with a smaller icon.
fn small_icon_button(
    frame: &mut Frame,
    state: &EditorState,
    rect: Rect,
    icon: Icon,
    color: Oklch,
    scale: f32,
) {
    let over = hovered(state, rect);
    if over {
        frame.fill_rounded(rect, state.theme.row_active, 5.0 * scale);
    }
    frame.icon(
        icon,
        centred(rect, 15.0 * scale),
        if over { state.theme.ink } else { color },
    );
}

/// Advance of one code column.
///
/// Derived from the type scale for now. At the point where a proportional fallback glyph can
/// appear in code, this must come from the shaper's measured advance, or the gutter and the text
/// will drift out of registration.
/// The part of `content` inside the grid's visible columns, and the x it starts at.
///
/// `content` must already have been through `expand_tabs`, so one char is one column. Returns
/// `None` when the segment is entirely off screen — the common case for the left of a long line
/// once the view has scrolled, and the reason this is worth doing at all rather than drawing
/// everything and letting the clip sort it out.
fn visible_slice(
    grid: &crate::layout::CodeGrid,
    start_column: usize,
    content: &str,
) -> Option<(f32, String)> {
    let first = grid.offset();
    let last = first + grid.columns() + 1;
    let end = start_column + content.chars().count();

    if end <= first || start_column >= last {
        return None;
    }
    if start_column >= first && end <= last {
        return Some((grid.x_of(start_column), content.to_string()));
    }

    let skip = first.saturating_sub(start_column);
    let take = (last.min(end)).saturating_sub(start_column.max(first));
    let sliced: String = content.chars().skip(skip).take(take).collect();
    if sliced.is_empty() {
        return None;
    }
    Some((grid.x_of(start_column + skip), sliced))
}

fn column_advance(state: &EditorState, scale: f32) -> f32 {
    advance_for(state.typography.code_size * scale)
}

/// Monospace advance for a given font size.
///
/// Any overlay that draws per-character on top of a run must use *that run's* size: mixing the code
/// advance with a smaller UI font puts every highlighted character a fraction of a glyph off, which
/// reads as garbled text rather than as a highlight.
fn advance_for(size_px: f32) -> f32 {
    size_px * 0.6
}

/// Text in the interface's own face.
fn ui(
    state: &EditorState,
    text: impl Into<String>,
    rect: Rect,
    color: Oklch,
    align: Align,
    scale: f32,
) -> TextRun {
    run(state, Role::Ui, text, rect, color, align, scale)
}

/// Whether the pointer is over a rectangle and not over an overlay covering it.
fn hovered(state: &EditorState, rect: Rect) -> bool {
    pointer_in(state, rect)
        && !state
            .chrome
            .covered
            .is_some_and(|card| crate::layout::contains(card, state.pointer.0, state.pointer.1))
}

/// Whether the pointer is over a rectangle, covered or not: for drawing the covering overlay.
fn pointer_in(state: &EditorState, rect: Rect) -> bool {
    crate::layout::contains(rect, state.pointer.0, state.pointer.1)
}

/// An icon button: the icon, and a rounded wash behind it under the pointer.
fn icon_button(
    frame: &mut Frame,
    state: &EditorState,
    rect: Rect,
    icon: Icon,
    color: Oklch,
    scale: f32,
) {
    icon_button_turned(frame, state, rect, icon, color, 0.0, scale);
}

/// [`icon_button`] with the icon turned about its centre.
fn icon_button_turned(
    frame: &mut Frame,
    state: &EditorState,
    rect: Rect,
    icon: Icon,
    color: Oklch,
    radians: f32,
    scale: f32,
) {
    let theme = &state.theme;
    let over = hovered(state, rect);
    if over {
        frame.fill_rounded(rect, theme.row_active, 6.0 * scale);
    }
    frame.icon_rotated(
        icon,
        centred(rect, crate::layout::size::ICON * scale),
        if over { theme.ink } else { color },
        radians,
    );
}

/// A square of `side` centred in `rect`.
fn centred(rect: Rect, side: f32) -> Rect {
    Rect::new(
        rect.x + (rect.width - side) / 2.0,
        rect.y + (rect.height - side) / 2.0,
        side,
        side,
    )
}

/// The colour a file's icon takes, by what kind of file it is.
///
/// The hues people already associate with each language — Rust's rust, Python's yellow, Dart's
/// blue — because an icon is read before the name beside it, and the colour is most of what makes
/// it readable at sixteen pixels.
pub fn file_color(theme: &Theme, name: &str) -> Oklch {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let hue = |l: f32, c: f32, h: f32| Oklch::new(l, c, h);
    match (lower.as_str(), extension) {
        (".gitignore" | ".gitattributes" | ".gitmodules", _) => hue(0.72, 0.15, 145.0),
        (_, "rs") => hue(0.70, 0.14, 45.0),
        (_, "py" | "pyi") => hue(0.82, 0.14, 90.0),
        (_, "js" | "mjs" | "cjs" | "jsx") => hue(0.86, 0.15, 100.0),
        (_, "ts" | "tsx" | "mts") => hue(0.66, 0.15, 250.0),
        (_, "dart") => hue(0.72, 0.12, 225.0),
        (_, "go") => hue(0.76, 0.12, 210.0),
        (_, "c" | "h" | "cpp" | "cc" | "hpp" | "cxx") => hue(0.66, 0.12, 255.0),
        (_, "html" | "htm") => hue(0.66, 0.18, 35.0),
        (_, "css" | "scss" | "sass") => hue(0.64, 0.15, 285.0),
        (_, "lua") => hue(0.58, 0.15, 265.0),
        (_, "json" | "toml" | "yaml" | "yml") => hue(0.80, 0.11, 75.0),
        (_, "md" | "markdown" | "txt") => hue(0.74, 0.05, 250.0),
        (_, "sh" | "bash" | "zsh" | "fish") => hue(0.74, 0.14, 140.0),
        _ => theme.ink_dim,
    }
}

/// The window's top bar: the logo and name, the menu bar, the command centre and the bell.
fn draw_title(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    frame.fill_backdrop(rect, theme.bg_alt);
    frame.rule_bottom(rect, theme.rule_soft, border::BORDER);
    let bar = crate::layout::title_bar(rect, scale, &state.typography);

    // The mark and the wordmark, as the logo sheet sets them: "Da" in the brand's amber, "Code" in
    // the ink. Two runs placed by the face's own advance — one monospaced face, so the second word
    // starts exactly where the first ends.
    frame.mark(bar.logo, theme.ink, theme.brand());
    let advance = state.typography.advance(Role::Brand) * scale;
    // On a bar too narrow for it the wordmark gives its room to the menus.
    if bar.name.width > 0.0 {
        frame.text(run(
            state,
            Role::Brand,
            "Da",
            bar.name,
            theme.brand(),
            Align::Left,
            scale,
        ));
        frame.text(run(
            state,
            Role::Brand,
            "Code",
            Rect::new(
                bar.name.x + advance * 2.0,
                bar.name.y,
                (bar.name.width - advance * 2.0).max(0.0),
                bar.name.height,
            ),
            theme.ink,
            Align::Left,
            scale,
        ));
    }
    draw_menu_titles(frame, state, &bar, scale);

    // The command centre, as VSCode and Zed both have it: a box that looks like a search field,
    // opens the palette, and says what it is for and which key does the same.
    if let Some(search) = bar.search {
        let over = hovered(state, search);
        frame.fill_rounded(
            search,
            if over { theme.row_active } else { theme.bg },
            6.0 * scale,
        );
        frame.outline_rounded(search, theme.rule_soft, 6.0 * scale, border::BORDER * scale);
        let icon = Rect::new(
            search.x + 10.0 * scale,
            search.y,
            14.0 * scale,
            search.height,
        );
        frame.icon(Icon::Search, centred(icon, 14.0 * scale), theme.ink_faint);
        let key = key_hint(state, "command_palette");
        let text = Rect::new(
            icon.right() + 8.0 * scale,
            search.y,
            (search.right() - 10.0 * scale - (icon.right() + 8.0 * scale)).max(0.0),
            search.height,
        );
        let project = state
            .project_root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let prompt = if project.is_empty() {
            "Search files and commands".to_string()
        } else {
            format!("Search {project}")
        };
        // The key gives way first: at the box's narrowest the two met in the middle and printed
        // over each other. The prompt says what the box is; the key is a bonus.
        let key_width = key.chars().count() as f32 * state.typography.advance(Role::Value) * scale;
        let beside_key = text.width - key_width - 12.0 * scale;
        let show_key =
            !key.is_empty() && beside_key >= 10.0 * state.typography.advance(Role::Ui) * scale;
        let prompt_width = if show_key { beside_key } else { text.width };
        frame.text(ui(
            state,
            elide(state, Role::Ui, &prompt, prompt_width, scale),
            text,
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        if show_key {
            frame.text(run(
                state,
                Role::Value,
                key,
                text,
                theme.ink_faint,
                Align::Right,
                scale,
            ));
        }
    }

    // The bell swings when a message arrives, and rings down in under a second.
    let swing = state
        .chrome
        .motion
        .movement(Key::Bell, 0.0, state.now())
        .clamp(-1.5, 1.5);
    icon_button_turned(
        frame,
        state,
        bar.bell,
        Icon::Bell,
        theme.ink_dim,
        swing * 12f32.to_radians(),
        scale,
    );
    // Unread messages: a dot on the bell, the way every desktop says "something happened".
    if state.chrome.unread > 0 {
        frame.disc(
            [bar.bell.right() - 8.0 * scale, bar.bell.y + 8.0 * scale],
            3.5 * scale,
            theme.primary,
        );
    }
}

/// The menu bar's titles, always on the title bar. The open one is lit, and under the pointer one
/// is washed the way an icon button is, because every one is a button.
///
/// Drawn here rather than with the menu: the titles used to belong to the overlay, so the bar
/// existed only while a menu was open and faded with it. Only the card fades now.
fn draw_menu_titles(
    frame: &mut Frame,
    state: &EditorState,
    bar: &crate::layout::TitleBar,
    scale: f32,
) {
    let theme = &state.theme;
    // Only the bar's own menu lights a title; a context menu has nothing to do with the bar.
    let open = state
        .menu
        .as_ref()
        .filter(|menu| menu.kind == crate::menu::Kind::Bar)
        .map(|menu| menu.open);
    let shown = bar.menus.len();
    let cells = bar
        .menus
        .iter()
        .zip(crate::menu::TITLES)
        .enumerate()
        .map(|(index, (rect, text))| (*rect, text, open == Some(index)))
        .chain(bar.overflow.map(|rect| {
            (
                rect,
                crate::layout::OVERFLOW,
                open.is_some_and(|open| open >= shown),
            )
        }));
    for (rect, text, lit) in cells {
        let over = hovered(state, rect);
        if lit || over {
            frame.fill_rounded(rect, theme.row_active, 5.0 * scale);
        }
        frame.text(ui(
            state,
            elide(state, Role::Ui, text, rect.width, scale),
            rect,
            if lit || over {
                theme.ink
            } else {
                theme.ink_dim
            },
            Align::Center,
            scale,
        ));
    }
}

/// The column of view icons down the left edge.
fn draw_activity(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    frame.fill_backdrop(rect, theme.bg_panel);
    frame.fill(
        Rect::new(
            rect.right() - border::BORDER * scale,
            rect.y,
            border::BORDER * scale,
            rect.height,
        ),
        theme.rule_soft,
    );

    let active = state.active_activity();
    let items = crate::layout::activity_items(rect, scale);
    let now = state.now();
    let motion = &state.chrome.motion;
    let index_of = |activity: crate::layout::Activity| {
        crate::layout::ACTIVITIES
            .iter()
            .position(|item| item.activity == activity)
    };

    // The bar on the open view's edge, as both editors mark it. It slides between icons rather than
    // jumping, through the cells it passes: placed by the activity's position in the table, so the
    // way from the explorer down to settings, pinned at the bottom, follows the column.
    if let Some(lit) = active.and_then(index_of) {
        let at = motion.movement(Key::ActivityIndicator, lit as f32, now);
        let y_of = |index: usize| {
            items
                .iter()
                .find(|(item, _)| index_of(item.activity) == Some(index))
                .map(|(_, cell)| cell.y)
        };
        let (low, high) = (at.floor().max(0.0) as usize, at.ceil().max(0.0) as usize);
        let y = match (y_of(low), y_of(high)) {
            (Some(a), Some(b)) => a + (b - a) * (at - low as f32),
            _ => y_of(lit).unwrap_or(rect.y),
        };
        if let Some((_, cell)) = items.first() {
            frame.fill_rounded(
                Rect::new(
                    rect.x,
                    y + cell.height * 0.22,
                    2.5 * scale,
                    cell.height * 0.56,
                ),
                theme.primary,
                1.25 * scale,
            );
        }
    }

    for (item, cell) in items.iter().copied() {
        let lit = active == Some(item.activity);
        let over = hovered(state, cell);
        // Under the pointer an icon rises a little, the way a key gives under a finger.
        let lift = index_of(item.activity).map_or(0.0, |index| {
            motion.movement(Key::Hover(index as u32), if over { 1.0 } else { 0.0 }, now)
        });
        let icon = centred(cell, 22.0 * scale);
        frame.icon(
            item.icon,
            Rect::new(icon.x, icon.y - 2.0 * scale * lift, icon.width, icon.height),
            if lit {
                theme.ink
            } else if over {
                theme.ink_dim
            } else {
                theme.ink_faint
            },
        );

        // A count on Problems, so a broken project is visible without opening anything.
        if item.activity == crate::layout::Activity::Problems {
            let (errors, warnings) = state.problems.counts();
            let total = errors + warnings;
            if total > 0 {
                let badge = Rect::new(
                    cell.right() - 20.0 * scale,
                    cell.y + 7.0 * scale,
                    16.0 * scale,
                    14.0 * scale,
                );
                frame.fill_rounded(
                    badge,
                    if errors > 0 {
                        theme.over_budget()
                    } else {
                        theme.stale()
                    },
                    7.0 * scale,
                );
                frame.text(run(
                    state,
                    Role::Label,
                    if total > 99 {
                        "99".to_string()
                    } else {
                        total.to_string()
                    },
                    badge,
                    theme.bg_panel,
                    Align::Center,
                    scale,
                ));
            }
        }
    }
}

/// The key that runs a command in the grammar the user chose, from the one table that knows —
/// written in Vim's notation for someone who chose Vim, and as `Ctrl+Shift+P` for everyone else.
fn key_hint(state: &EditorState, command: &str) -> String {
    dacode_input::commands::COMMANDS
        .iter()
        .find(|row| row.name == command)
        .map(|row| row.shown(state.modal))
        .unwrap_or_default()
}

/// The name and key of whatever control is under the pointer.
///
/// Every icon in the chrome teaches its shortcut this way, which is the bargain the mouse paths in
/// this editor make: the button is there so that, after using it twice, you no longer need it.
fn draw_tooltip(
    frame: &mut Frame,
    state: &EditorState,
    regions: &crate::layout::Layout,
    bounds: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let (x, y) = state.pointer;
    let mut found: Option<(&str, &str, Rect)> = None;

    for (item, cell) in crate::layout::activity_items(regions.activity, scale) {
        if crate::layout::contains(cell, x, y) {
            found = Some((item.label, item.command, cell));
        }
    }
    let bell = crate::layout::title_bar(regions.title, scale, &state.typography).bell;
    if crate::layout::contains(bell, x, y) {
        found = Some(("Notifications", "notifications", bell));
    }
    let names = state.tab_names();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let tabs = crate::layout::tab_bar(
        regions.tabs,
        &names,
        state.active_tab(),
        &state.typography,
        scale,
    );
    let buttons = [tabs.home, tabs.back, tabs.forward, tabs.split, tabs.run];
    for ((command, _, label_text), rect) in crate::layout::TAB_BAR_COMMANDS.iter().zip(buttons) {
        if crate::layout::contains(rect, x, y) {
            found = Some((label_text, command, rect));
        }
    }

    if let Some(sidebar) = regions
        .sidebar
        .filter(|sidebar| state.source_control_showing() && crate::layout::contains(*sidebar, x, y))
    {
        let view = &state.scm;
        let geometry = view.geometry(sidebar, scale, &state.typography);
        use crate::source_control::Hit;
        let hit = view.hit(&geometry, x, y, scale);
        found = match hit {
            Some(Hit::Tool(tool)) => geometry
                .tools
                .iter()
                .find(|(at, _)| *at == tool)
                .map(|(_, rect)| (tool.label(), tool.command(), *rect)),
            Some(Hit::Branch) => Some(("Checkout branch…", "git_checkout", geometry.branch)),
            Some(Hit::Pull) => Some(("Pull", "git_pull", geometry.pull)),
            Some(Hit::Push) => Some(("Push", "git_push", geometry.push)),
            Some(Hit::Action(row, action)) => geometry
                .rows
                .iter()
                .find(|(index, _)| *index == row)
                .and_then(|(_, rect)| {
                    view.action_rects(row, *rect, scale)
                        .into_iter()
                        .find(|(at, _)| *at == action)
                })
                .map(|(_, rect)| {
                    let (name, command) = action.label();
                    (name, command, rect)
                }),
            _ => found,
        };
    }
    if let Some(branch) = state.scm.status().and_then(|status| {
        crate::layout::status_branch(
            regions.status,
            &status.head_name(),
            &state.typography,
            scale,
        )
    }) {
        if crate::layout::contains(branch, x, y) {
            found = Some(("Checkout branch…", "git_checkout", branch));
        }
    }

    let Some((name, command, anchor)) = found else {
        return;
    };
    let key = key_hint(state, command);
    let text = if key.is_empty() {
        name.to_string()
    } else {
        format!("{name}   {key}")
    };
    let advance = state.typography.advance(Role::Ui) * scale;
    let width = text.chars().count() as f32 * advance + 20.0 * scale;
    let height = 26.0 * scale;
    // Beside the activity bar, below everything else — and never off the window.
    let (mut tip_x, mut tip_y) = if crate::layout::contains(regions.activity, x, y) {
        (
            anchor.right() + 6.0 * scale,
            anchor.y + (anchor.height - height) / 2.0,
        )
    } else {
        (
            anchor.x + (anchor.width - width) / 2.0,
            anchor.bottom() + 6.0 * scale,
        )
    };
    tip_x = tip_x.clamp(
        bounds.x + 4.0 * scale,
        (bounds.right() - width - 4.0 * scale).max(0.0),
    );
    // Above the control when there is no room below it — the status line's branch, at the foot.
    if tip_y + height > bounds.bottom() - 4.0 * scale {
        tip_y = (anchor.y - height - 6.0 * scale).max(bounds.y);
    }
    let tip = Rect::new(tip_x, tip_y, width, height);
    frame.fill_rounded(tip, theme.row_active, 5.0 * scale);
    frame.outline_rounded(tip, theme.rule, 5.0 * scale, border::BORDER * scale);
    frame.text(ui(state, text, tip, theme.ink, Align::Center, scale));
}

/// How many messages the bell's panel lists.
const NOTIFICATIONS_SHOWN: usize = 12;

/// Where the bell's panel and everything on it are: an update on offer and its buttons at the top,
/// then the recent messages. Read by drawing and by [`notifications_button_at`], so a click lands on
/// the button drawn under it.
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationsPanel {
    pub card: Rect,
    pub heading: Rect,
    pub rows: Vec<Rect>,
    pub offer: Option<OfferBox>,
}

/// The update offer's part of the panel.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferBox {
    pub rect: Rect,
    pub lines: Vec<(String, Rect)>,
    pub buttons: Vec<(Rect, crate::update::Button)>,
}

pub fn notifications_panel(
    state: &EditorState,
    title: Rect,
    bounds: Rect,
    scale: f32,
) -> NotificationsPanel {
    let typography = &state.typography;
    let bar = crate::layout::title_bar(title, scale, typography);
    // Wider while an update is on offer, so its three buttons sit on one row at any usual size.
    let wide = if state.chrome.update.is_some() {
        440.0
    } else {
        380.0
    };
    let width = (wide * scale).min(bounds.width - 16.0 * scale);
    let line = 22.0 * scale;
    let pad = 12.0 * scale;
    let x = (bar.bell.right() - width).max(bounds.x + 8.0 * scale);
    let top = title.bottom() + 6.0 * scale;
    let inner = Rect::new(x + pad, top, width - 2.0 * pad, 0.0);
    let mut y = top + 6.0 * scale;

    let offer = state.chrome.update.as_ref().map(|offer| {
        let start = y;
        let advance = crate::layout::measured_advance(typography, Role::Ui, scale);
        let lines: Vec<(String, Rect)> = wrap_lines(&offer.message, inner.width, advance, 3)
            .into_iter()
            .map(|text| {
                let rect = Rect::new(inner.x, y, inner.width, line);
                y += line;
                (text, rect)
            })
            .collect();
        y += 6.0 * scale;
        let height = 26.0 * scale;
        let gap = 6.0 * scale;
        let mut at = inner.x;
        let buttons = offer
            .buttons
            .iter()
            .map(|button| {
                let want = button.label.chars().count() as f32 * advance + 20.0 * scale;
                let w = want.min(inner.width);
                // Onto a row of its own when it does not fit beside the last: a narrow window
                // gets a column of buttons rather than one cut off at the card's edge.
                if at > inner.x && at + w > inner.right() {
                    at = inner.x;
                    y += height + gap;
                }
                let rect = Rect::new(at, y, w, height);
                at += w + gap;
                (rect, *button)
            })
            .collect();
        y += height + 10.0 * scale;
        OfferBox {
            rect: Rect::new(inner.x, start, inner.width, y - start - 4.0 * scale),
            lines,
            buttons,
        }
    });

    let heading = Rect::new(inner.x, y, inner.width, line);
    let first = y + line * 0.95;
    let count = state
        .chrome
        .notifications
        .len()
        .clamp(1, NOTIFICATIONS_SHOWN);
    let rows: Vec<Rect> = (0..count)
        .map(|index| Rect::new(inner.x, first + line * index as f32, inner.width, line))
        .collect();
    let bottom = first + line * count as f32 + line * 0.4;
    NotificationsPanel {
        card: Rect::new(x, top, width, bottom - top),
        heading,
        rows,
        offer,
    }
}

/// The command of the offer's button under a point, if the panel is open and one is there.
pub fn notifications_button_at(
    state: &EditorState,
    title: Rect,
    bounds: Rect,
    scale: f32,
    x: f32,
    y: f32,
) -> Option<&'static str> {
    if !state.chrome.show_notifications {
        return None;
    }
    let panel = notifications_panel(state, title, bounds, scale);
    panel
        .offer?
        .buttons
        .into_iter()
        .find_map(|(rect, button)| crate::layout::contains(rect, x, y).then_some(button.command))
}

/// The bell's panel: an update on offer, then the editor's recent messages, newest first.
fn draw_notifications(
    frame: &mut Frame,
    state: &EditorState,
    title: Rect,
    bounds: Rect,
    travel: f32,
    scale: f32,
) {
    let theme = &state.theme;
    let panel = notifications_panel(state, title, bounds, scale);
    let card = panel.card;
    frame.set_zoom(Some(appear_zoom(card, travel, Anchor::Top)));
    self::card(frame, theme, card, scale, 1.0);

    if let Some(offer) = &panel.offer {
        for (text, rect) in &offer.lines {
            frame.text(ui(
                state,
                text.clone(),
                *rect,
                theme.ink,
                Align::Left,
                scale,
            ));
        }
        let radius = 5.0 * scale;
        for (index, (rect, button)) in offer.buttons.iter().enumerate() {
            // The first is the one the offer is for, and looks it.
            let primary = index == 0;
            if primary {
                frame.fill_rounded(*rect, theme.primary, radius);
            } else {
                frame.fill_rounded(
                    *rect,
                    if pointer_in(state, *rect) {
                        theme.row_active
                    } else {
                        theme.bg_alt
                    },
                    radius,
                );
                frame.outline_rounded(*rect, theme.rule, radius, border::BORDER * scale);
            }
            let text = elide(
                state,
                Role::Ui,
                button.label,
                rect.width - 8.0 * scale,
                scale,
            );
            frame.text(ui(
                state,
                text,
                *rect,
                if primary { theme.bg } else { theme.ink },
                Align::Center,
                scale,
            ));
        }
        frame.fill(
            Rect::new(
                offer.rect.x,
                offer.rect.bottom(),
                offer.rect.width,
                border::BORDER * scale,
            ),
            theme.rule_soft,
        );
    }

    frame.text(label(
        state,
        "notifications",
        panel.heading,
        theme.ink_faint,
        Align::Left,
        scale,
    ));
    let shown: Vec<&String> = state
        .chrome
        .notifications
        .iter()
        .rev()
        .take(NOTIFICATIONS_SHOWN)
        .collect();
    if shown.is_empty() {
        frame.text(ui(
            state,
            "Nothing yet. Messages the editor shows land here.",
            panel.rows[0],
            theme.ink_dim,
            Align::Left,
            scale,
        ));
        return;
    }
    for (index, (message, row)) in shown.iter().zip(&panel.rows).enumerate() {
        frame.text(ui(
            state,
            elide(state, Role::Ui, message, row.width, scale),
            *row,
            if index == 0 { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));
    }
}

/// The row above the code: navigation, the open files, and the pane controls.
fn draw_tabs(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    frame.fill_backdrop(rect, theme.bg_alt);
    frame.rule_bottom(rect, theme.rule_soft, border::BORDER);

    let names = state.tab_names();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let active_tab = state.active_tab();
    let bar = crate::layout::tab_bar(rect, &borrowed, active_tab, &state.typography, scale);

    icon_button(frame, state, bar.home, Icon::Home, theme.ink_dim, scale);
    icon_button(
        frame,
        state,
        bar.back,
        Icon::ChevronLeft,
        theme.ink_dim,
        scale,
    );
    icon_button(
        frame,
        state,
        bar.forward,
        Icon::ChevronRight,
        theme.ink_faint,
        scale,
    );
    icon_button(frame, state, bar.split, Icon::Split, theme.ink_dim, scale);
    icon_button(frame, state, bar.run, Icon::Play, theme.ink, scale);

    // The active tab's background glides from the tab that was active to the one that is, by way of
    // the tabs between. Placed from the tabs' own rectangles, so it always ends exactly on one.
    let now = state.now();
    let motion = &state.chrome.motion;
    let at = motion.movement(Key::TabIndicator, active_tab as f32, now);
    let rect_of = |index: usize| {
        bar.tabs
            .iter()
            .find(|tab| tab.index == index)
            .map(|tab| tab.rect)
    };
    let (low, high) = (at.floor().max(0.0) as usize, at.ceil().max(0.0) as usize);
    let glide = match (rect_of(low), rect_of(high)) {
        (Some(a), Some(b)) => {
            let t = at - low as f32;
            Some(Rect::new(
                a.x + (b.x - a.x) * t,
                a.y,
                a.width + (b.width - a.width) * t,
                a.height,
            ))
        }
        // One end is scrolled out of the strip: there is no path to draw, so it is simply there.
        _ => rect_of(active_tab),
    };
    if let Some(glide) = glide {
        frame.fill_rounded(glide, theme.bg, 6.0 * scale);
    }
    // A tab just opened grows in. Only the active one: opening a file is what makes a tab, and the
    // file opened is the one shown.
    let grow = (
        motion.fade(Key::TabGrow, 1.0, now),
        motion.movement(Key::TabGrow, 1.0, now),
    );

    // One tab per open file, from the same function the mouse hit-tests.
    let settings_tab = state.settings_tab();
    for tab in &bar.tabs {
        let name = &names[tab.index];
        let settings = settings_tab == Some(tab.index);
        // The settings tab is never dirty: what it changes is written on its own.
        let dirty = !settings
            && state
                .buffer_tabs
                .get(tab.index)
                .map(|(_, dirty)| *dirty)
                .unwrap_or(tab.index == state.active_buffer && state.document.is_dirty());
        let active = tab.index == active_tab;
        let over = hovered(state, tab.rect);

        let growing = active && grow.0 < 1.0;
        if growing {
            frame.set_opacity(grow.0);
            frame.set_zoom(Some(appear_zoom(tab.rect, grow.1, Anchor::Centre)));
        }
        if !active && over {
            frame.fill_rounded(tab.rect, theme.rule_soft, 6.0 * scale);
        }
        if settings {
            frame.icon(Icon::Gear, tab.icon, theme.ink_dim);
        } else {
            frame.icon(Icon::File, tab.icon, file_color(theme, name));
        }
        frame.text(ui(
            state,
            elide(state, Role::Ui, name, tab.label.width, scale),
            tab.label,
            if active { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));

        // The close button where the pointer can use it, and a dot where the file is unsaved —
        // the same trade both editors make, so the one glance answers "is this saved?".
        if active || over {
            let on_close = hovered(state, tab.close);
            if on_close {
                frame.fill_rounded(tab.close, theme.row_active, 4.0 * scale);
            }
            frame.icon(
                if dirty && !on_close {
                    Icon::Dot
                } else {
                    Icon::Close
                },
                centred(tab.close, 12.0 * scale),
                if on_close { theme.ink } else { theme.ink_dim },
            );
        } else if dirty {
            frame.icon(Icon::Dot, centred(tab.close, 12.0 * scale), theme.ink_dim);
        }
        if growing {
            frame.set_opacity(1.0);
            frame.set_zoom(None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_code(
    frame: &mut Frame,
    state: &EditorState,
    pane: &PaneView<'_>,
    blame: Option<Rect>,
    gutter: Rect,
    area: &crate::layout::CodeArea,
    row_height: f32,
    scale: f32,
) {
    // Everything below measures against the scrolling rows, never the whole pane: the split is the
    // caller's, so the mouse and the popups get the same rectangle this does.
    let code = area.code;
    let theme = &state.theme;
    // The mode belongs to the focused pane. An unfocused one is not in insert mode just because the
    // pane beside it is, and tinting it as though it were would be a lie about where typing goes.
    let accent = if pane.focused {
        theme.accent(state.mode)
    } else {
        theme.ink_faint
    };
    let text = pane.document.text();
    let cursor = pane.document.selections().primary().head;
    let cursor_line = text.char_to_point(cursor).line;

    // The gutter is the editor's own background — the backdrop under the whole editor area — so
    // line numbers sit on the code's surface, as they do in both editors this one is shaped after.

    if pane.focused && state.is_empty() {
        draw_empty_state(frame, state, code, scale);
        return;
    }

    if let Some(pinned) = area.pinned {
        frame.fill_backdrop(pinned, theme.bg_alt);
        frame.rule_bottom(pinned, theme.rule_soft, border::BORDER);

        for (index, head) in area.heads.iter().enumerate() {
            let y = pinned.y + index as f32 * row_height;
            let range = text.line_range(*head);
            let raw = text.slice_to_string(range.start..range.end);
            let content = expand_tabs(state, &raw);
            frame.text(run(
                state,
                Role::Code,
                content.trim_end().to_string(),
                Rect::new(
                    code.x + space::SM * scale,
                    y,
                    code.width - space::SM * scale,
                    row_height,
                ),
                theme.ink_dim,
                Align::Left,
                scale,
            ));
            // The line number, in the gutter beside it, so the pinned line is locatable.
            frame.text(run(
                state,
                Role::LineNumber,
                (head + 1).to_string(),
                Rect::new(gutter.x, y, gutter.width - space::LG * scale, row_height),
                theme.ink_faint,
                Align::Right,
                scale,
            ));
        }
    }

    let advance = column_advance(state, scale);
    // One grid, read by every x and y below and by the mouse. The horizontal scroll offset and the
    // touchpad's part of a row enter the arithmetic here and nowhere else — a second copy is a click
    // landing in a different column or row from the one under the pointer the instant the view moves.
    let grid = crate::layout::CodeGrid::new(
        code,
        advance,
        row_height,
        // Ignored while wrap is on: there is nothing to the right of the viewport then, and the
        // remembered column must not affect anything while it is invisible.
        if state.display.mode == WrapMode::None {
            pane.scroll.column
        } else {
            0
        },
        scale,
    )
    // Only the focused pane scrolls by pixels; the others sit on whole rows where they were left.
    .with_sub_row(if pane.focused {
        state.sub_row_px(row_height)
    } else {
        0.0
    });
    let height = grid.rows_drawn();
    let rows = state.display.rows_from(text, pane.scroll, height);

    // Everything from here is placed from the scroll position, and while a touchpad has the top row
    // part-scrolled that row starts above the code area: its band, its line number and its glyphs
    // would paint over the pinned strip or the tab bar. Gutter and code together, so the number
    // stays beside its line.
    let left = blame.map_or(gutter.x, |column| column.x);
    frame.set_clip(Some(Rect::new(
        left,
        code.y,
        (code.right() - left).max(0.0),
        code.height,
    )));
    // The relative dates are measured from one instant per frame, not one per row.
    let now = crate::blame::now();
    let text_left = grid.x_of(0);
    let em = state.typography.code_size * scale;

    // Diagnostics, search matches and inlay hints, narrowed to what is on screen — once per frame
    // rather than once per row.
    //
    // Each of the three loops below used to walk its whole list for every visible row, so the cost
    // was rows × N: searching a common substring in a large file made the row loop walk tens of
    // thousands of matches forty times a frame. That is per-frame work bounded by the *document*
    // instead of by the viewport, which is the one thing this editor is not allowed to do.
    //
    // A linear filter rather than a binary search, deliberately: the protocol does not promise
    // diagnostics arrive in document order, and a `partition_point` over an unsorted list drops
    // real errors silently. One pass over N is already the difference that matters.
    let span = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => first.range.start..last.range.end,
        _ => CharOffset(0)..CharOffset(0),
    };
    let overlaps =
        |range: &std::ops::Range<CharOffset>| range.start < span.end && range.end > span.start;

    let diagnostics: Vec<(std::ops::Range<CharOffset>, _)> = if pane.focused {
        state
            .diagnostics
            .items()
            .iter()
            .filter_map(|diagnostic| {
                // The conversion is a rope lookup, so doing it here also takes it off the row loop.
                let range = dacode_lsp::range_from_lsp(text, diagnostic.range);
                overlaps(&range).then_some((range, diagnostic.severity))
            })
            .collect()
    } else {
        Vec::new()
    };

    let matches: Vec<std::ops::Range<CharOffset>> = if pane.focused {
        state
            .search_matches
            .iter()
            .filter(|range| overlaps(range))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let hints: Vec<(CharOffset, String)> = if pane.focused && state.show_inlay_hints {
        state
            .inlay_hints
            .iter()
            .filter_map(|hint| {
                let at = dacode_lsp::from_lsp(text, hint.position);
                if at < span.start || at > span.end {
                    return None;
                }
                let content: String = match &hint.label {
                    dacode_lsp::lsp_types::InlayHintLabel::String(label) => label.clone(),
                    dacode_lsp::lsp_types::InlayHintLabel::LabelParts(parts) => {
                        parts.iter().map(|part| part.value.as_str()).collect()
                    }
                };
                (!content.is_empty()).then_some((at, content))
            })
            .collect()
    } else {
        Vec::new()
    };

    for (index, row) in rows.iter().enumerate() {
        let y = grid.y_of(index);
        let on_cursor_line = row.line == cursor_line;

        // A diff tab: each line's first character says what it is, as `git diff` prints it, and the
        // line takes the gutter's colour for that — a wash across the code and a bar in the gutter.
        // Read per visible row, from the row's own line, so a diff of any length costs the viewport.
        if pane.focused {
            if let Some(kind) = state
                .review
                .as_ref()
                .and_then(|_| diff_line_kind(text.line(row.line).chars().take(4)))
            {
                let color = match kind {
                    DiffLine::Added => theme.fresh(),
                    DiffLine::Removed => theme.over_budget(),
                    DiffLine::Hunk => theme.primary,
                };
                // Faint: blended in linear light, a few percent already reads as a clear wash, and
                // the text over it has to stay as legible as the file's own.
                frame.fill(
                    Rect::new(left, y, code.right() - left, row_height),
                    color.with_alpha(0.05),
                );
                frame.fill(
                    Rect::new(gutter.right() - 3.0 * scale, y, 2.0 * scale, row_height),
                    color,
                );
            }
        }

        if on_cursor_line && pane.focused {
            // Across the gutter and the code, translucent so it works on any surface — including
            // a see-through one. With modes on it takes the mode's hue, which is how the line
            // itself says what typing will do; with them off there is no mode to say, and a
            // coloured band would be a signal that means nothing.
            //
            // Mixed in OKLCH and drawn as part of the backdrop, not blended as a faint wash: five
            // percent of ink blended in linear light comes out several times lighter than five
            // percent looks, and a backdrop stays as see-through as the surface around it.
            let band = Rect::new(left, y, code.right() - left, row_height);
            if state.modal {
                frame.fill_backdrop(
                    band,
                    theme
                        .bg
                        .mix(theme.accent(state.mode), state.mode.line_tint_alpha()),
                );
                frame.fill(
                    Rect::new(code.x, y, border::ACCENT_BORDER * scale, row_height),
                    accent,
                );
            } else {
                frame.fill_backdrop(band, theme.bg.mix(theme.ink, 0.06));
            }
        }

        // Diagnostic underlines. Drawn as a 2px rule beneath the offending span rather than as a
        // fill, so the code stays readable — a wash over an error makes it *harder* to read the
        // thing you are being told about.
        for (range, severity) in &diagnostics {
            let start = range.start.max(row.range.start);
            let end = range.end.min(row.range.end);
            if start >= end {
                continue;
            }

            let from_column = columns_between(state, text, row.range.start, start);
            let to_column = columns_between(state, text, row.range.start, end);
            let severity = severity.unwrap_or(dacode_lsp::lsp_types::DiagnosticSeverity::WARNING);
            let color = if severity == dacode_lsp::lsp_types::DiagnosticSeverity::ERROR {
                theme.over_budget()
            } else {
                theme.stale()
            };

            frame.fill(
                Rect::new(
                    text_left + (from_column + row.indent) as f32 * advance,
                    y + row_height - border::ACCENT_BORDER * scale - 1.0,
                    ((to_column - from_column) as f32 * advance).max(advance),
                    border::ACCENT_BORDER * scale,
                )
                .intersect(code),
                color,
            );
        }

        // Search matches, under the selection fills: a match you have also selected should read as
        // selected.
        for range in &matches {
            let start = range.start.max(row.range.start);
            let end = range.end.min(row.range.end);
            if start >= end {
                continue;
            }
            let from_column = columns_between(state, text, row.range.start, start);
            let to_column = columns_between(state, text, row.range.start, end);
            frame.fill_linear(
                Rect::new(
                    text_left + (from_column + row.indent) as f32 * advance,
                    y,
                    ((to_column - from_column) as f32 * advance).max(advance * 0.4),
                    row_height,
                )
                .intersect(code),
                // The visual hue at low alpha: a found-text wash that is not the selection and not
                // the cursor line.
                theme
                    .accent(dacode_theme::Mode::Visual)
                    .with_alpha(0.20)
                    .over(theme.bg),
            );
        }

        // Selection fills, clipped to this row. In visual mode the *range* carries the signal,
        // so this is what communicates the mode.
        for selection in pane.document.selections().iter() {
            if selection.is_caret() {
                continue;
            }
            let start = selection.start().max(row.range.start);
            let end = selection.end().min(row.range.end);
            if start >= end {
                continue;
            }
            let from_column = columns_between(state, text, row.range.start, start);
            let to_column = columns_between(state, text, row.range.start, end);
            frame.fill_linear(
                Rect::new(
                    text_left + (from_column + row.indent) as f32 * advance,
                    y,
                    ((to_column - from_column) as f32 * advance).max(advance * 0.4),
                    row_height,
                )
                .intersect(code),
                theme.selection_fill(if pane.focused {
                    state.mode
                } else {
                    Mode::Normal
                }),
            );
        }

        // The git mark, on the gutter's inner edge. A 2px bar rather than a letter or an icon: it is
        // read peripherally while looking at the code, and shape at the edge of vision is all that
        // survives — which is also the whole reason the design is made of rectangles.
        // A diff tab draws its own bars, above; its marks are only there for `]c`.
        if pane.focused && row.is_first() && state.review.is_none() {
            if let Some(change) = state.diff.change_at(row.line) {
                let (color, height) = match change {
                    crate::git::Change::Added => (theme.fresh(), row_height),
                    crate::git::Change::Modified => (theme.stale(), row_height),
                    // A deletion has no line of its own, so it is drawn as a short mark straddling
                    // the boundary where the removed lines used to be.
                    crate::git::Change::Removed => (theme.over_budget(), row_height * 0.35),
                };
                frame.fill(
                    Rect::new(
                        gutter.right() - 3.0 * scale,
                        y + (row_height - height) / 2.0,
                        2.0 * scale,
                        height,
                    ),
                    color,
                );
            }
        }

        // Who changed it, once per run of lines from the same commit: a column repeating one name
        // forty times is a column nobody reads. The first row on screen always says, so scrolling
        // into the middle of a run does not leave the top of the view unattributed.
        if let (Some(column), true, true) = (blame, pane.focused, row.is_first()) {
            if let Some(attribution) = state.blame.at(row.line) {
                let previous = (index > 0 && row.line > 0)
                    .then(|| state.blame.at(row.line - 1))
                    .flatten();
                if previous != Some(attribution) {
                    draw_blame_entry(
                        frame,
                        state,
                        attribution,
                        Rect::new(column.x, y, column.width, row_height),
                        on_cursor_line,
                        now,
                        scale,
                    );
                }
            }
        }

        // A folded head says what it hides, at the end of its own text. Not a `…` glyph in the
        // gutter: the count is the useful part, and the gutter figure is already spoken for.
        if row.folded > 0 {
            let columns = columns_between(state, text, row.range.start, row.range.end);
            let x = text_left + (columns + row.indent) as f32 * advance + advance;
            frame.text(label(
                state,
                format!("{} lines", row.folded),
                Rect::new(x, y, (code.right() - x).max(0.0), row_height),
                theme.ink_faint,
                Align::Left,
                scale,
            ));
            frame.fill_linear(
                Rect::new(code.x, y, code.width, row_height),
                theme.ink_faint.with_alpha(0.06).over(theme.bg),
            );
        }

        // Gutter figure only on a line's first row: that blankness is how a wrap is told apart
        // from a new line at a glance.
        //
        // Relative numbering is Vim's, and it is there so `12j` can be read off the screen. Someone
        // who turned the grammar off has no count to type, and a gutter full of numbers that are not
        // the line's number is one more thing that felt wrong without being nameable.
        if row.is_first() && state.line_numbers != crate::config::LineNumbers::Off {
            let relative = state.line_numbers == crate::config::LineNumbers::Relative;
            let number = if on_cursor_line || !relative {
                (row.line + 1).to_string()
            } else {
                row.line.abs_diff(cursor_line).to_string()
            };
            frame.text(run(
                state,
                Role::LineNumber,
                number,
                Rect::new(gutter.x, y, gutter.width - space::LG * scale, row_height),
                match (on_cursor_line, state.modal) {
                    (true, true) => accent,
                    (true, false) => theme.ink,
                    (false, _) => theme.ink_faint,
                },
                Align::Right,
                scale,
            ));
        }

        // Inlay hints for this row, drawn in the gaps rather than shifting the text: the hint is
        // information *about* the code, and code that moves as hints arrive is code you cannot read.
        for (at, content) in &hints {
            if *at < row.range.start || *at > row.range.end {
                continue;
            }
            let column = columns_between(state, text, row.range.start, *at);
            let x = text_left + (column + row.indent) as f32 * advance;
            frame.text(run(
                state,
                Role::Value,
                content.clone(),
                Rect::new(x, y, (code.right() - x).max(0.0), row_height),
                theme.ink_faint,
                Align::Left,
                scale,
            ));
        }

        // One run per *highlighted segment*. Cost stays bounded by the viewport: a row yields at
        // most a handful of runs regardless of document size.
        let indent_px = row.indent as f32 * advance;

        // The running column is carried across segments rather than recomputed from the row start
        // each time: re-scanning the prefix per segment is quadratic in segments per row, and a
        // syntax-highlighted row has many segments.
        let mut column = 0usize;

        for segment in segments(pane.highlights, row.range.clone()) {
            let raw = text.slice_to_string(segment.range.clone());
            let content = expand_tabs(state, &raw);
            let at = row.indent + column;
            column += content.chars().count();

            // Sliced to the visible columns rather than drawn at a negative x and left to clip. A
            // run *is* clipped by glyphon — but to its own rect, which is the one starting at that
            // negative x, and glyphs are drawn after all quads within a layer. So a row scrolled
            // sideways would paint its code over the line numbers. Slicing also keeps shaping
            // bounded by the viewport, which is what a minified file needs.
            let Some((x, content)) = visible_slice(&grid, at, &content) else {
                continue;
            };
            let _ = indent_px;

            frame.text(run(
                state,
                if segment.highlight.map(Highlight::is_prose) == Some(true) {
                    // Comments are italic serif, larger than the code — the design's most
                    // distinctive typographic decision.
                    Role::Comment
                } else {
                    Role::Code
                },
                content,
                Rect::new(x, y, (code.right() - x).max(0.0), row_height),
                segment
                    .highlight
                    .map(|highlight| syntax_color(theme, highlight))
                    .unwrap_or(theme.ink),
                Align::Left,
                scale,
            ));
        }
    }

    // The cursor is drawn only where typing would go. A second blinking cursor in an unfocused pane
    // is the single most confusing thing a split view can do.
    //
    // The cursor comes from the display map, so it lands correctly on a wrapped row.
    if let Some(point) = pane
        .focused
        .then(|| {
            state
                .display
                .display_point(text, pane.scroll, height, cursor)
        })
        .flatten()
    {
        draw_cursor(
            frame,
            state,
            CursorGeometry {
                x: text_left + point.column as f32 * advance,
                y: grid.y_of(point.row),
                row_height,
                em,
                advance,
                scale,
            },
        );
    }
    frame.set_clip(None);
}

/// Where and how big the cursor is, in pixels.
struct CursorGeometry {
    x: f32,
    y: f32,
    row_height: f32,
    em: f32,
    advance: f32,
    scale: f32,
}

fn draw_cursor(frame: &mut Frame, state: &EditorState, geometry: CursorGeometry) {
    let CursorGeometry {
        x,
        y,
        row_height,
        em,
        advance,
        scale,
    } = geometry;
    let accent = state.theme.accent(state.mode);
    let block_width = advance.max(em * 0.5);

    match state.mode.cursor() {
        CursorShape::Block {
            fill_alpha,
            underline,
        } => {
            let height = em * 1.1;
            frame.fill(
                Rect::new(x, y + (row_height - height) / 2.0, block_width, height),
                accent.with_alpha(fill_alpha),
            );
            if underline {
                frame.fill(
                    Rect::new(
                        x,
                        y + (row_height + height) / 2.0 - border::ACCENT_BORDER * scale,
                        block_width,
                        border::ACCENT_BORDER * scale,
                    ),
                    accent,
                );
            }
        }
        CursorShape::Bar { width_px, .. } => {
            let height = em * 1.05;
            frame.fill(
                Rect::new(x, y + (row_height - height) / 2.0, width_px * scale, height),
                accent,
            );
        }
        CursorShape::Solid { fill_alpha } => {
            let height = em * 1.1;
            frame.fill(
                Rect::new(x, y + (row_height - height) / 2.0, block_width, height),
                accent.with_alpha(fill_alpha),
            );
        }
    }
}

/// A run of characters sharing one highlight (or none).
struct Segment {
    range: std::ops::Range<CharOffset>,
    highlight: Option<Highlight>,
}

/// Splits a row into highlighted segments, filling the gaps with unhighlighted text.
///
/// Spans arrive sorted and non-overlapping from the syntax layer, so this is a single pass. When
/// no highlights are available yet — before the first parse lands, or for a language without a
/// grammar — the row is one plain segment, which is why text is legible immediately rather than
/// waiting on the parser.
fn segments(highlights: &[HighlightSpan], row: std::ops::Range<CharOffset>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut at = row.start;

    for span in highlights {
        if span.range.end <= row.start {
            continue;
        }
        if span.range.start >= row.end {
            break;
        }

        let start = span.range.start.max(row.start);
        let end = span.range.end.min(row.end);
        if start > at {
            out.push(Segment {
                range: at..start,
                highlight: None,
            });
        }
        if end > start {
            out.push(Segment {
                range: start..end,
                highlight: Some(span.highlight),
            });
            at = end;
        }
    }

    if at < row.end {
        out.push(Segment {
            range: at..row.end,
            highlight: None,
        });
    }

    out
}

fn syntax_color(theme: &Theme, highlight: Highlight) -> Oklch {
    match highlight {
        Highlight::Keyword => theme.syntax.keyword,
        Highlight::Type => theme.syntax.type_,
        Highlight::Function => theme.syntax.function,
        Highlight::Attribute => theme.syntax.attribute,
        Highlight::Comment => theme.syntax.comment,
        Highlight::String => theme.syntax.string,
        Highlight::Number => theme.syntax.number,
        Highlight::Constant => theme.syntax.constant,
        Highlight::Text => theme.ink,
    }
}

/// Display columns spanned by a char range, without allocating: the rope is iterated directly.
fn columns_between(state: &EditorState, text: &Text, from: CharOffset, to: CharOffset) -> usize {
    let mut columns = 0;
    for c in text.rope().slice(from.get()..to.get()).chars() {
        columns += if c == '\t' {
            state.display.tab_width - (columns % state.display.tab_width)
        } else {
            1
        };
    }
    columns
}

/// Expands tabs to the next tab stop.
///
/// Returns the input untouched when it contains no tab, which is the overwhelmingly common case —
/// and this runs for every segment of every visible row, every frame.
fn expand_tabs<'a>(state: &EditorState, line: &'a str) -> std::borrow::Cow<'a, str> {
    if !line.contains('\t') {
        return std::borrow::Cow::Borrowed(line);
    }

    let mut out = String::with_capacity(line.len() + 8);
    let mut columns = 0usize;
    for c in line.chars() {
        if c == '\t' {
            let pad = state.display.tab_width - (columns % state.display.tab_width);
            out.extend(std::iter::repeat_n(' ', pad));
            columns += pad;
        } else {
            out.push(c);
            columns += 1;
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The sidebar: whichever view the activity bar has open.
fn draw_sidebar(frame: &mut Frame, state: &EditorState, slot: Rect, open_width: f32, scale: f32) {
    let theme = &state.theme;
    frame.fill_backdrop(slot, theme.bg_panel);
    frame.fill(
        Rect::new(
            slot.right() - border::BORDER * scale,
            slot.y,
            border::BORDER * scale,
            slot.height,
        ),
        theme.rule_soft,
    );

    // Sliding, the view is laid out at its open width with its right edge on the slot's, and only
    // the part that has arrived shows: it comes out from behind the activity bar rather than being
    // squeezed, and its rows do not reflow on every frame. The rows keep their heights, so the mouse
    // — which measures against the slot — lands on the row it is over. Closing has nothing to slide:
    // the view's state is already gone, so the empty panel folds away.
    let sliding = slot.width < open_width;
    let rect = if sliding {
        Rect::new(slot.right() - open_width, slot.y, open_width, slot.height)
    } else {
        slot
    };
    if sliding {
        frame.set_clip(Some(slot));
    }
    draw_sidebar_view(frame, state, rect, scale);
    if sliding {
        frame.set_clip(None);
    }
}

/// Whichever view the sidebar is showing, laid out in `rect`.
fn draw_sidebar_view(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    if let Some(tree) = &state.tree {
        draw_explorer(frame, state, tree, rect, scale);
    } else if state.show_diagnostics {
        draw_diagnostics_list(frame, state, rect, scale);
    } else {
        match state.chrome.activity {
            Some(crate::layout::Activity::Search) => {
                if let Some(panel) = &state.search {
                    draw_search_panel(frame, state, panel, rect, scale);
                }
            }
            Some(crate::layout::Activity::SourceControl) => {
                draw_source_control(frame, state, rect, scale)
            }
            _ => {}
        }
    }
}

fn draw_statusline(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill_backdrop(rect, theme.bg_alt);
    frame.rule_top(rect, theme.rule_soft, border::BORDER);

    // While a prompt is open it owns this line, as it does in Vim: `/` is used mid-thought, and an
    // overlay large enough for its own chrome would hide the text being searched.
    if let Some(prompt) = &state.prompt {
        let inner = Rect::new(
            rect.x + space::XL * scale,
            rect.y,
            rect.width - space::XL * 2.0 * scale,
            rect.height,
        );
        frame.text(run(
            state,
            Role::Value,
            prompt.display(),
            inner,
            if prompt.message.is_some() {
                theme.over_budget()
            } else {
                theme.ink
            },
            Align::Left,
            scale,
        ));

        // A caret after the input, so it reads as a field rather than a label.
        let advance = advance_for(state.typography.spec(Role::Value).size_px * scale);
        let caret_x = inner.x + prompt.display().chars().count() as f32 * advance + advance * 0.2;
        frame.fill(
            Rect::new(
                caret_x.min(inner.right() - 2.0 * scale),
                inner.y + inner.height * 0.25,
                2.0 * scale,
                inner.height * 0.5,
            ),
            accent,
        );

        // Where you are and how to move, on the right.
        //
        // This said "17 matches" and nothing else, which answers the least useful question: it told
        // you the haystack had needles without saying which one you were standing on or how to reach
        // the next. Both keys are named here because a search box that hides its own controls is how
        // people conclude the editor cannot do something it has always done.
        if prompt.is_search() && !prompt.input.is_empty() {
            // Words, not `↵`: the bundled font has no glyph for it, so it drew a tofu box — and the
            // whole point of this line is to be read by someone who does not yet know the keys.
            //
            // A replace prompt teaches its own keys instead of the search ones, and which of them
            // depends on where the caret is: naming "enter replaces" while the caret is in the field
            // you find with would be teaching a key that does nothing yet.
            let keys = if prompt.is_replace() {
                match prompt.focus {
                    crate::prompt::Field::Query => "tab to what replaces it",
                    crate::prompt::Field::Replacement => {
                        "enter replaces one · ctrl-enter replaces all · tab back"
                    }
                }
            } else {
                "enter next · shift-enter back · ctrl-shift-f every file"
            };
            let (text, color) = match prompt.matches.len() {
                0 => ("no matches".to_string(), theme.stale()),
                total => match dacode_buffer::index_of(&prompt.matches, state.cursor()) {
                    // On a match: say which one. Not on one yet — the caret sits between them — say
                    // how many there are, because "0 of 32" reads like a failure and is not one.
                    Some(index) => (format!("{} of {total}   {keys}", index + 1), theme.ink_dim),
                    None => (format!("{total} matches   {keys}"), theme.ink_dim),
                },
            };
            frame.text(run(
                state,
                Role::Value,
                text,
                inner,
                color,
                Align::Right,
                scale,
            ));
        }
        return;
    }

    let inner = Rect::new(
        rect.x + space::MD * scale,
        rect.y,
        rect.width - space::MD * 2.0 * scale,
        rect.height,
    );
    let point = state.cursor_point();
    let (left, right) = inner.split_left(inner.width * 0.5);

    // Problems first, with their icons, as both editors open the bar: the count that decides
    // whether to stop and look is the one the eye should land on. This file's, like the panel: the
    // larger of the store's count and the live diagnostics', since either may have been answered
    // first.
    let (project_errors, project_warnings) = state.problems.counts();
    let (file_errors, file_warnings) = state.diagnostics.counts();
    let (errors, warnings) = (
        project_errors.max(file_errors),
        project_warnings.max(file_warnings),
    );
    let notes = state
        .diagnostics
        .items()
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.severity,
                Some(dacode_lsp::lsp_types::DiagnosticSeverity::INFORMATION)
                    | Some(dacode_lsp::lsp_types::DiagnosticSeverity::HINT)
            )
        })
        .count();
    let mut x = left.x;
    // The branch, first: where you are, before what is wrong there. A button — it opens the picker.
    let branch_name = state.scm.status().map(|status| status.head_name());
    if let Some(cell) = branch_name
        .as_deref()
        .and_then(|name| crate::layout::status_branch(rect, name, &state.typography, scale))
    {
        if hovered(state, cell) {
            frame.fill(cell, theme.row_active);
        }
        let side = 13.0 * scale;
        frame.icon(
            Icon::Branch,
            Rect::new(
                cell.x + 4.0 * scale,
                cell.y + (cell.height - side) / 2.0,
                side,
                side,
            ),
            theme.ink_dim,
        );
        let name_rect = Rect::new(
            cell.x + side + 8.0 * scale,
            cell.y,
            (cell.width - side - 10.0 * scale).max(0.0),
            cell.height,
        );
        frame.text(run(
            state,
            Role::Value,
            elide(
                state,
                Role::Value,
                branch_name.as_deref().unwrap_or_default(),
                name_rect.width,
                scale,
            ),
            name_rect,
            theme.ink_dim,
            Align::Left,
            scale,
        ));
        x = x.max(cell.right() + 10.0 * scale);
    }
    for (icon, count, lit) in [
        (Icon::ErrorCircle, errors, theme.over_budget()),
        (Icon::Warning, warnings, theme.stale()),
        (Icon::Info, notes, theme.ink_dim),
    ] {
        let color = if count > 0 { lit } else { theme.ink_faint };
        frame.icon(
            icon,
            centred(
                Rect::new(x, left.y, 15.0 * scale, left.height),
                15.0 * scale,
            ),
            color,
        );
        let number = Rect::new(x + 19.0 * scale, left.y, 30.0 * scale, left.height);
        frame.text(ui(
            state,
            count.to_string(),
            number,
            color,
            Align::Left,
            scale,
        ));
        x += 19.0 * scale + (count.to_string().len() as f32 * 8.0 + 14.0) * scale;
    }
    let left = Rect::new(
        x + 8.0 * scale,
        left.y,
        (left.right() - x - 8.0 * scale).max(0.0),
        left.height,
    );

    let (mode_cell, after_mode) = left.split_left(96.0 * scale);
    frame.text(label(
        state,
        // Recording replaces the mode word, in the mode's own accent. An editor that is silently
        // recording is an editor that surprises you ten minutes later. With modal editing off there
        // is no mode to report — "NORMAL" would be a lie, and a word that never changes is noise.
        match (state.recording, state.modal) {
            (Some(register), _) => format!("rec @{register}"),
            (None, true) => state.mode.label().to_string(),
            (None, false) => String::new(),
        },
        mode_cell,
        accent,
        Align::Left,
        scale,
    ));

    let (selections, pending) = after_mode.split_left(200.0 * scale);
    let count = state.document.selections().len();
    let primary = state.document.selections().primary();
    let selection_text = if count == 1 {
        if primary.is_caret() {
            "1 selection".to_string()
        } else {
            format!("1 selection · {} chars", primary.len())
        }
    } else {
        format!("{count} selections · 1 primary")
    };
    // Both are measured against their boxes: at a narrow width the left half has little left after
    // the counts and the mode, and the run is clipped to its rectangle, so "1 selection" stopped at
    // "1 select" with nothing to say it was cut.
    frame.text(run(
        state,
        Role::Value,
        elide(state, Role::Value, &selection_text, selections.width, scale),
        selections,
        theme.ink_dim,
        Align::Left,
        scale,
    ));

    frame.text(run(
        state,
        Role::Value,
        elide(state, Role::Value, &state.pending, pending.width, scale),
        pending,
        accent,
        Align::Left,
        scale,
    ));

    let over = state.frame_ms > dacode_theme::FRAME_BUDGET_MS;
    let cells: [(String, Oklch); 4] = [
        (
            format!("{}:{}", point.line + 1, point.column + 1),
            theme.ink_dim,
        ),
        (
            format!(
                "{} · utf-8 · lf{}",
                // The detected language, not a hardcoded one: a Python file reading "rust" is the
                // kind of detail that makes an editor feel unfinished.
                state
                    .file_path
                    .as_deref()
                    .and_then(dacode_syntax::Language::from_path)
                    .map(|language| language.name())
                    .unwrap_or("text"),
                if state.document.is_dirty() {
                    " · mod"
                } else {
                    ""
                }
            ),
            theme.ink_dim,
        ),
        // The counts are on the left now, with their icons; this cell says who is producing them.
        (state.server_status.clone(), theme.ink_faint),
        (
            format!("{:.2} ms", state.frame_ms),
            if over {
                theme.over_budget()
            } else {
                theme.ink_faint
            },
        ),
    ];

    let cell_width = right.width / cells.len() as f32;
    for (index, (content, color)) in cells.into_iter().enumerate() {
        let cell = Rect::new(
            right.x + cell_width * index as f32,
            right.y,
            cell_width - space::MD * scale,
            right.height,
        );
        // These are right-aligned, so an over-long cell was clipped at its *left* edge and
        // `no language server` came out as `o language server` — a missing first letter, which
        // reads as a rendering fault rather than as text that did not fit.
        frame.text(run(
            state,
            Role::Value,
            elide(state, Role::Value, &content, cell.width, scale),
            cell,
            color,
            Align::Right,
            scale,
        ));
    }
}

#[cfg(test)]
mod appear_tests {
    use super::*;

    #[test]
    fn an_overlay_grows_from_its_anchor_to_exactly_its_own_rectangle() {
        let card = Rect::new(100.0, 200.0, 400.0, 300.0);
        assert_eq!(appear(card, 1.0, Anchor::Top), card, "arrived is untouched");

        let top = appear(card, 0.0, Anchor::Top);
        assert_eq!(top.y, card.y, "hangs from its top edge");
        assert!((top.width - card.width * 0.96).abs() < 1e-3);
        assert!((top.x + top.width / 2.0 - (card.x + card.width / 2.0)).abs() < 1e-3);

        let bottom = appear(card, 0.0, Anchor::Bottom);
        assert!(
            (bottom.bottom() - card.bottom()).abs() < 1e-3,
            "rises from its foot"
        );

        let centre = appear(card, 0.5, Anchor::Centre);
        assert!((centre.y + centre.height / 2.0 - (card.y + card.height / 2.0)).abs() < 1e-3);
        assert!(centre.width > top.width && centre.width < card.width);
    }
}

#[cfg(test)]
mod card_tests {
    use super::*;

    fn drawn(theme: &Theme, alpha: f32) -> Vec<dacode_render::Quad> {
        let mut frame = Frame::default();
        card(
            &mut frame,
            theme,
            Rect::new(100.0, 80.0, 400.0, 200.0),
            1.0,
            alpha,
        );
        frame.quads_on(Layer::Base).cloned().collect()
    }

    #[test]
    fn a_fading_card_fades_its_shadow_surface_and_outline_together() {
        // Faded one part at a time, a closing card leaves its shadow behind for a frame or its
        // outline floating over nothing — the kind of thing that reads as a rendering glitch.
        for theme in [Theme::walnut(), Theme::linen()] {
            let whole = drawn(&theme, 1.0);
            let half = drawn(&theme, 0.5);
            assert_eq!(whole.len(), 4, "two shadows, a surface, an outline");
            assert_eq!(whole.len(), half.len());
            for (whole, half) in whole.iter().zip(&half) {
                assert!((half.color[3] - whole.color[3] * 0.5).abs() < 1e-4);
                assert_eq!(whole.rect, half.rect);
            }
            assert!(drawn(&theme, 0.0).is_empty(), "nothing at all at zero");
        }
    }

    #[test]
    fn the_shadow_is_fainter_than_the_theme_shadow_and_the_surface_is_the_card() {
        // Subtle in both themes: a fraction of `shadow`, never the full scrim-dark of it.
        for theme in [Theme::walnut(), Theme::linen(), Theme::true_black()] {
            let quads = drawn(&theme, 1.0);
            let full = theme.shadow.to_linear()[3];
            for shadow in &quads[..2] {
                assert!(
                    shadow.color[3] < full * 0.5,
                    "a shadow at {}",
                    shadow.color[3]
                );
                assert_eq!(shadow.border, 0.0);
            }
            let rect = Rect::new(100.0, 80.0, 400.0, 200.0);
            assert_eq!(quads[2].rect, rect, "the surface is exactly the card");
            assert_eq!(quads[3].rect, rect, "and so is its outline");
            assert!(quads[3].border > 0.0);
        }
    }
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    /// Wrapping reads exactly one thing — the type scale — so that is all it is given. A function
    /// that takes less is a function a test can drive.
    fn state() -> Typography {
        Typography::default()
    }

    #[test]
    fn a_sentence_longer_than_the_box_wraps_instead_of_stopping() {
        // The defect: the box split only on hard newlines, so a one-paragraph message was one line,
        // and `run` sets `wrap: false` — the renderer clipped it at the rectangle with no ellipsis.
        // "…separate from the Python you have i" and nothing to say more had been said.
        let state = state();
        let message = "no Python language server. A language server is separate from the Python \
                       you have installed — dacode can format with ruff or black";
        let lines = wrap_to(&state, Role::Value, message, 300.0, 1.0);

        assert!(lines.len() > 1, "it did not wrap: {lines:?}");
        // Nothing is lost and nothing is invented: the words come back in order.
        let rejoined = lines.join(" ");
        assert_eq!(
            rejoined.split_whitespace().collect::<Vec<_>>(),
            message.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_wrapped_line_fits_the_width_it_was_given() {
        let state = state();
        let advance = state.advance(Role::Value);
        for width in [80.0f32, 160.0, 300.0, 620.0] {
            let per_line = (width / advance).floor() as usize;
            let lines = wrap_to(
                &state,
                Role::Value,
                "the quick brown fox jumps over the lazy dog and keeps going for a while yet",
                width,
                1.0,
            );
            for line in &lines {
                assert!(
                    line.chars().count() <= per_line,
                    "`{line}` is {} characters, past the {per_line} that fit {width}px",
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_word_longer_than_the_line_is_broken_rather_than_dropped() {
        // A path or a URL is exactly what an error message contains, and it is the one thing that
        // cannot be wrapped at a space. Dropping it would lose the part that identifies the problem.
        let state = state();
        let path = r"C:\Users\someone\projects\deeply\nested\directory\with\a\long\name.rs";
        let lines = wrap_to(&state, Role::Value, path, 90.0, 1.0);

        assert!(lines.len() > 1);
        assert_eq!(
            lines.concat(),
            path,
            "the path came back changed: {lines:?}"
        );
    }

    #[test]
    fn a_paragraph_kept_to_two_lines_ends_in_an_ellipsis_and_never_in_a_lone_one() {
        // The problems list's old version showed a bare "…" on its second row when a message's
        // first line fitted the first.
        let advance = 8.0;
        let short = wrap_lines("unused variable\nhelp: prefix it", 400.0, advance, 2);
        assert_eq!(short, vec!["unused variable help: prefix it".to_string()]);

        let long = "the quick brown fox jumps over the lazy dog and keeps going for a while yet";
        let lines = wrap_lines(long, 120.0, advance, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| !line.trim_end_matches('…').trim().is_empty())
        );
        assert!(
            lines
                .iter()
                .all(|line| line.chars().count() as f32 * advance <= 120.0)
        );
    }

    #[test]
    fn a_short_message_stays_one_line() {
        let state = state();
        let lines = wrap_to(&state, Role::Value, "saved", 620.0, 1.0);
        assert_eq!(lines, vec!["saved".to_string()]);
    }
}
