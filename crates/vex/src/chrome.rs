//! The editor chrome: title bar, tab strip, gutter, code area, structure sidebar, statusline.
//!
//! A pure function of state into a [`Frame`], so it is testable without a GPU and can move into
//! `crates/ui` unchanged. The design's geometry is guidance: proportions are honored, but pixel
//! values derive from the type scale so the whole chrome scales with font size.
//!
//! The code area draws **display rows**, not buffer lines — with soft wrap one line can occupy
//! several rows, and the gutter figure belongs only to the first of them.

use crate::palette::{Palette, PaletteKind};
use crate::prompt::Prompt;
use crate::results::Results;
use vex_buffer::Document;
use vex_display::{DisplayMap, ScrollAnchor, WrapMode};
use vex_render::{Align, Frame, Layer, Rect, TextRun};
use vex_syntax::{Highlight, HighlightSpan};
use vex_text::{CharOffset, Point, Text, Version};
use vex_theme::{CursorShape, Mode, Oklch, Role, Theme, Typography, frame as border, space};

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

pub struct EditorState {
    pub mode: Mode,
    pub theme: Theme,
    pub typography: Typography,

    pub document: Document,
    /// Bracket pairing and indentation rules for the active buffer's language.
    pub pairs: vex_buffer::PairConfig,
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
    pub pending_request: Option<vex_input::Action>,
    /// The project root, for showing paths relatively.
    pub project_root: std::path::PathBuf,
    /// Diagnostics for the active buffer, and the version they describe.
    pub diagnostics: vex_lsp::Diagnostics,
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
    /// The macro register being recorded into, if any.
    pub recording: Option<char>,
    /// What changed since the last commit, per line.
    pub diff: crate::git::Diff,
    /// Whether the sidebar shows the diagnostics list instead of the structure panel.
    pub show_diagnostics: bool,
    /// Every problem the language server has reported, across the project.
    pub problems: crate::problems::Store,
    /// Inlay hints for the visible range, when they are on.
    pub inlay_hints: Vec<vex_lsp::lsp_types::InlayHint>,
    /// Whether to ask for and draw them. Off by default: hints move the code sideways as you type,
    /// and that is a preference, not a default worth imposing.
    pub show_inlay_hints: bool,
    /// First-run setup, when it is showing. It owns the whole window while it does.
    pub welcome: Option<crate::welcome::Welcome>,
    /// The settings panel, when open. Shares the sidebar with the tree and the diagnostics list.
    pub settings: Option<crate::settings_ui::Settings>,
    /// Where each setting's value came from, for the panel's third column.
    pub setting_sources: std::collections::HashMap<String, crate::settings_ui::Source>,
    /// The file tree, when open. It takes the sidebar over rather than adding a third column: the
    /// window is finite, and a tree squeezed beside the structure panel would be too narrow to read
    /// a path in.
    pub tree: Option<crate::tree::FileTree>,
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
}

impl EditorState {
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

    /// How many display rows fit in a viewport of this pixel height.
    pub fn visible_rows(&self, height_px: f32, row_height: f32) -> usize {
        ((height_px / row_height).floor() as usize).max(1)
    }

    /// Height of the code area, given the whole window — the chrome's vertical stack in reverse.
    pub fn code_height(&self, window_height: f32, scale: f32) -> f32 {
        (window_height - (72.0 + 34.0 + 34.0) * scale).max(0.0)
    }
}

/// The first-run setup screen.
///
/// A function of the wizard's state and a theme, like the splash — it runs before there is anything
/// else worth drawing, and it must be legible in whichever theme is being previewed.
pub fn draw_welcome(
    frame: &mut Frame,
    welcome: &crate::welcome::Welcome,
    typography: &Typography,
    bounds: Rect,
    scale: f32,
) {
    use crate::welcome::Step;

    let theme = welcome.preview_theme();
    frame.fill(bounds, theme.bg);

    let px = |v: f32| v * scale;
    let content_width = px(1000.0).min(bounds.width - px(96.0));
    let left = (bounds.x + (bounds.width - content_width) / 2.0).round();

    // Centred vertically, because a wizard pinned to the top of a tall window reads as a page that
    // failed to load. The content's height is known per step, so this is arithmetic rather than a
    // guess.
    let content_height = match welcome.step {
        Step::Keys => px(200.0),
        Step::Theme => px(200.0),
        Step::Import => px(230.0),
        Step::Done => 0.0,
    };
    let header = px(34.0) + px(48.0) + px(26.0) + px(52.0);

    let mark = px(34.0);
    let mut y =
        (bounds.y + ((bounds.height - header - content_height) / 2.0).max(px(56.0))).round();
    frame.mark(Rect::new(left, y, mark, mark), theme.ink, theme.bg);

    let mut wordmark = typography.spec(Role::Display);
    wordmark.size_px = px(30.0);
    frame.text(TextRun {
        text: "welcome to vex".into(),
        spec: wordmark,
        color: theme.ink,
        rect: Rect::new(left + mark + px(18.0), y, content_width, mark),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
    });

    y += mark + px(48.0);

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

    let geometry = crate::welcome::geometry(welcome, bounds, scale);
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
        });

        let mut body = typography.spec(Role::Value);
        body.size_px = px(13.0);
        frame.text(TextRun {
            text: (*description).into(),
            spec: body,
            color: theme.ink_faint,
            rect: Rect::new(
                card.x + px(24.0),
                card.y + px(46.0),
                card.width - px(48.0),
                px(34.0),
            ),
            align: Align::Left,
            center_vertically: false,
            wrap: true,
            layer: Layer::default(),
        });
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

    for (index, (name, build)) in Theme::BUILT_IN.iter().enumerate() {
        let theme = build();
        let Some(card) = context.options.get(index).copied() else {
            continue;
        };
        // Under the pointer counts as selected, so the theme previews as you move over it — the
        // same "choosing is looking" the arrow keys give.
        let selected = index == welcome.selected
            || crate::layout::contains(card, welcome.pointer.0, welcome.pointer.1);

        // The card is painted in *its own* theme, which is what makes it a preview.
        frame.fill(card, theme.bg);
        frame.fill(Rect::new(card.x, card.y, card.width, px(1.0)), theme.rule);

        let line = px(21.0);
        let mut code = typography.spec(Role::Code);
        code.size_px = px(11.5);
        for (row, (text, color)) in sample.iter().enumerate() {
            frame.text(TextRun {
                text: (*text).into(),
                spec: code,
                color: color(&theme),
                rect: Rect::new(
                    card.x + px(12.0),
                    card.y + px(14.0) + row as f32 * line,
                    card.width - px(16.0),
                    line,
                ),
                align: Align::Left,
                center_vertically: true,
                wrap: false,
                layer: Layer::default(),
            });
        }

        // The name goes on the card, in the card's own ink.
        frame.text(welcome_label(
            typography,
            *name,
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
            for edge in [
                Rect::new(card.x, card.y, card.width, px(2.0)),
                Rect::new(card.x, card.bottom() - px(2.0), card.width, px(2.0)),
                Rect::new(card.x, card.y, px(2.0), card.height),
                Rect::new(card.right() - px(2.0), card.y, px(2.0), card.height),
            ] {
                frame.fill(edge, accent);
            }
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
            frame.fill(row, theme.row_active);
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
        });

    }

    // The finishing row, which is where the last option lands — and the one thing on this screen
    // that has to look like a button, because it is the only way out that is not a keyboard key.
    let row = context.next.unwrap_or(Rect::new(left, top, width, line));
    let selected = welcome.selected >= crate::settings_ui::Importable::ALL.len()
        || crate::layout::contains(row, welcome.pointer.0, welcome.pointer.1);
    if selected {
        frame.fill(row, theme.row_active);
    }
    frame.rule_top(row, theme.rule, border::BORDER);
    frame.rule_bottom(row, theme.rule, border::BORDER);
    frame.rule_left(row, theme.rule, border::BORDER);
    let mut body = typography.spec(Role::Value);
    body.size_px = px(14.0);
    frame.text(TextRun {
        text: if welcome.answers.import.is_empty() {
            "start without importing".into()
        } else {
            format!("import {} and start", welcome.answers.import.len())
        },
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
    });
}

/// A card background with a selection border.
fn draw_welcome_card(frame: &mut Frame, theme: &Theme, card: Rect, selected: bool, scale: f32) {
    frame.fill(
        card,
        if selected {
            theme.bg_panel
        } else {
            theme.bg_alt
        },
    );
    let accent = theme.accent(Mode::Normal);
    if selected {
        frame.fill(
            Rect::new(card.x, card.y, border::ACCENT_BORDER * scale, card.height),
            accent,
        );
    } else {
        frame.rule_top(card, theme.rule_soft, border::BORDER);
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
        theme.bg,
    );

    let mut spec = typography.spec(Role::Display);
    spec.size_px = wordmark_size;
    frame.text(TextRun {
        text: "vex".into(),
        spec,
        color: theme.ink,
        rect: Rect::new(
            left + mark_size * vex_render::MARK_INK_LEFT,
            top + mark_size + gap,
            bounds.width,
            // Tight: a taller rect centres the glyphs lower and the 8px gap becomes 40px of air.
            wordmark_size,
        ),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
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
    frame.fill(bounds, theme.bg);

    let px = |v: f32| v * scale;
    let row_height = px(state.typography.row_height());

    let (title, rest) = bounds.split_top(px(72.0));
    let (tabs, rest) = rest.split_top(px(34.0));
    let (status, body) = rest.split_bottom(px(34.0));

    draw_title(frame, state, title, scale);
    draw_tabs(frame, state, tabs, scale);

    // The results panel takes the bottom of the editor area, above the statusline: it is a list
    // beside the text, not an overlay, because you read the hit and the code together.
    let (body_area, results_area) = match &state.results {
        Some(results) => {
            let rows = results.visible().0.len().max(1);
            let height = (rows as f32 + 2.0) * 22.0 * scale;
            let (panel, rest) = body.split_bottom(height.min(body.height * 0.5));
            (rest, Some(panel))
        }
        None => (body, None),
    };

    let sidebar_width = px(292.0).min(body_area.width * 0.32);
    let (sidebar, editor) = body_area.split_right(sidebar_width);

    // Panes divide the editor area evenly along one axis. A flat row or column rather than a tree:
    // a tree buys arbitrary nesting that almost nobody arranges deliberately, at the cost of every
    // layout question becoming recursive.
    let count = panes.len().max(1);
    let mut focused_code = editor;
    for (index, pane) in panes.iter().enumerate() {
        let slot = match layout {
            PaneLayout::Vertical => {
                let width = (editor.width / count as f32).floor();
                Rect::new(
                    editor.x + width * index as f32,
                    editor.y,
                    if index + 1 == count {
                        editor.right() - (editor.x + width * index as f32)
                    } else {
                        width
                    },
                    editor.height,
                )
            }
            PaneLayout::Horizontal => {
                let height = (editor.height / count as f32).floor();
                Rect::new(
                    editor.x,
                    editor.y + height * index as f32,
                    editor.width,
                    if index + 1 == count {
                        editor.bottom() - (editor.y + height * index as f32)
                    } else {
                        height
                    },
                )
            }
        };

        // A hairline between panes, on the leading edge of every pane after the first. Without it
        // two files sitting side by side read as one document with a strange gap in it.
        let slot = if index == 0 {
            slot
        } else {
            match layout {
                PaneLayout::Vertical => {
                    frame.fill(
                        Rect::new(slot.x, slot.y, border::BORDER * scale, slot.height),
                        theme.rule,
                    );
                    Rect::new(
                        slot.x + border::BORDER * scale,
                        slot.y,
                        slot.width - border::BORDER * scale,
                        slot.height,
                    )
                }
                PaneLayout::Horizontal => {
                    frame.fill(
                        Rect::new(slot.x, slot.y, slot.width, border::BORDER * scale),
                        theme.rule,
                    );
                    Rect::new(
                        slot.x,
                        slot.y + border::BORDER * scale,
                        slot.width,
                        slot.height - border::BORDER * scale,
                    )
                }
            }
        };

        // With more than one pane, each takes a footer naming its file. A reserved strip rather than
        // a label floating over the text: an overlay would sit on top of the last row of code, which
        // is exactly where the eye goes when reading down.
        let (footer, slot) = if count > 1 {
            let (footer, rest) = slot.split_bottom(22.0 * scale);
            (Some(footer), rest)
        } else {
            (None, slot)
        };

        // The gutter narrows with the pane, so a three-way split does not spend most of its width
        // on line numbers.
        let gutter_width = px(78.0).min(slot.width * 0.22);
        let (gutter, code) = slot.split_left(gutter_width);
        draw_code(frame, state, pane, gutter, code, row_height, scale);

        if let Some(footer) = footer {
            frame.fill(footer, if pane.focused { theme.bg_alt } else { theme.bg });
            frame.rule_top(footer, theme.rule_soft, border::BORDER);
            frame.text(label(
                state,
                pane.path.last().cloned().unwrap_or_default(),
                Rect::new(
                    footer.x + space::MD * scale,
                    footer.y,
                    footer.width - space::MD * 2.0 * scale,
                    footer.height,
                ),
                // The focused pane's name takes the mode accent: with two files open it is the
                // fastest way to see where typing will land.
                if pane.focused {
                    theme.accent(state.mode)
                } else {
                    theme.ink_faint
                },
                Align::Left,
                scale,
            ));
        }

        if pane.focused {
            focused_code = code;
        }
    }

    let code = focused_code;
    let visible = state.visible_rows(code.height, row_height);
    draw_sidebar(frame, state, sidebar, visible, scale);
    if let (Some(results), Some(area)) = (&state.results, results_area) {
        draw_results(frame, state, results, area, scale);
    }
    draw_statusline(frame, state, status, scale);

    // Info boxes sit above the code but below the palette: a hover is contextual, a palette is
    // modal. The layer is what makes them opaque — within one layer, later text still shows through
    // an earlier panel, because all of a layer's rectangles are drawn before any of its glyphs.
    frame.set_layer(Layer::Panel);
    draw_info(frame, state, code, row_height, scale);
    draw_completion(frame, state, code, row_height, scale);

    // The which-key hint sits with the info boxes: it is contextual, it appears beside what you are
    // doing, and a scrim over the document would be absurd for something that lasts one keystroke.
    draw_which_key(frame, state, code, scale);

    // Last, so they cover everything: these are modal overlays.
    frame.set_layer(Layer::Modal);
    if let Some(palette) = &state.palette {
        draw_palette(frame, state, palette, bounds, scale);
    }
    if let Some(help) = &state.help {
        draw_help(frame, state, help, bounds, scale);
    }
    if let Some(menu) = &state.menu {
        draw_menu(frame, state, menu, bounds, scale);
    }
    frame.set_layer(Layer::Base);
}

/// The completion popup, anchored under the word being completed.
fn draw_completion(
    frame: &mut Frame,
    state: &EditorState,
    code: Rect,
    row_height: f32,
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

    let widest = visible
        .iter()
        .map(|item| item.label.chars().count())
        .max()
        .unwrap_or(12)
        .max(12);
    let width = ((widest as f32 + 2.0) * advance).min(code.width * 0.6);

    let x = (code.x + space::SM * scale + point.column as f32 * advance)
        .min(code.right() - width - space::SM * scale);
    let cursor_y = code.y + point.row as f32 * row_height;
    // Below the line normally, above it when there is no room — the same rule as the hover box.
    let below = cursor_y + row_height;
    let y = if below + height <= code.bottom() {
        below
    } else {
        (cursor_y - height).max(code.y)
    };

    let popup = Rect::new(x, y, width, height);
    frame.fill(popup, theme.bg_panel);
    frame.rule_top(popup, theme.rule, border::BORDER);
    frame.rule_bottom(popup, theme.rule, border::BORDER);
    frame.fill(
        Rect::new(popup.x, popup.y, border::BORDER, popup.height),
        theme.rule,
    );

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
        if selected {
            frame.fill(
                Rect::new(popup.x, item_y, popup.width, row),
                theme.row_active,
            );
            frame.fill(
                Rect::new(popup.x, item_y, border::ACCENT_BORDER * scale, row),
                accent,
            );
        }

        let text_x = popup.x + space::XS * scale;
        frame.text(run(
            state,
            Role::Value,
            item.label.clone(),
            Rect::new(text_x, item_y, popup.width - space::XS * 2.0 * scale, row),
            if selected { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));

        // The typed prefix, in the mode color, over the label's own glyphs.
        let prefix_advance = advance_for(state.typography.spec(Role::Value).size_px * scale);
        for position in &item.highlights {
            let Some(c) = item.label.chars().nth(*position as usize) else {
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

/// A hover reply, or the message of the diagnostic under the cursor.
///
/// Hover wins when both apply: it was asked for explicitly, while the diagnostic message appears
/// merely because the cursor is on the line.
fn draw_info(frame: &mut Frame, state: &EditorState, code: Rect, row_height: f32, scale: f32) {
    let theme = &state.theme;

    let (text, accent) = match (&state.hover, diagnostic_at_cursor(state)) {
        (Some(hover), _) => (hover.clone(), theme.accent(state.mode)),
        (None, Some((message, is_error))) => (
            message,
            if is_error {
                theme.over_budget()
            } else {
                theme.stale()
            },
        ),
        (None, None) => return,
    };

    // Lines are trimmed to keep the box a box: hover replies can be pages of markdown, and a panel
    // that grows to fit one would cover the code it describes.
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && line.trim() != "```")
        .take(6)
        .collect();
    if lines.is_empty() {
        return;
    }

    let line_height = 20.0 * scale;
    let width = (620.0 * scale).min(code.width - space::XL * scale);
    let height = lines.len() as f32 * line_height + space::MD * scale;

    // Below the cursor line where there is room, above it otherwise, so the box never hides the
    // thing it is describing.
    let cursor_y = state
        .display
        .display_point(state.document.text(), state.scroll, 512, state.cursor())
        .map(|point| code.y + point.row as f32 * row_height)
        .unwrap_or(code.y);
    let below = cursor_y + row_height;
    let y = if below + height <= code.bottom() {
        below
    } else {
        (cursor_y - height).max(code.y)
    };

    let box_rect = Rect::new(code.x + space::SM * scale, y, width, height);
    frame.fill(box_rect, theme.bg_panel);
    frame.rule_top(box_rect, theme.rule, border::BORDER);
    frame.rule_bottom(box_rect, theme.rule, border::BORDER);
    frame.fill(
        Rect::new(
            box_rect.x,
            box_rect.y,
            border::ACCENT_BORDER * scale,
            box_rect.height,
        ),
        accent,
    );

    for (index, line) in lines.iter().enumerate() {
        frame.text(run(
            state,
            Role::Value,
            line.trim_end().to_string(),
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

/// The message of the diagnostic covering the cursor, and whether it is an error.
pub fn diagnostic_at_cursor(state: &EditorState) -> Option<(String, bool)> {
    let text = state.document.text();
    let cursor = state.cursor();

    state
        .diagnostics
        .items()
        .iter()
        .find(|diagnostic| {
            let range = vex_lsp::range_from_lsp(text, diagnostic.range);
            // `<=` on the end so a diagnostic about a single position still matches when the cursor
            // sits on it.
            range.start <= cursor && cursor <= range.end
        })
        .map(|diagnostic| {
            (
                diagnostic.message.clone(),
                diagnostic.severity == Some(vex_lsp::lsp_types::DiagnosticSeverity::ERROR),
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
    let row_height = 22.0 * scale;
    let (header, rows_area) = rect.split_top(row_height);

    frame.text(label(
        state,
        format!("search: {}", results.query),
        Rect::new(
            header.x + pad,
            header.y,
            header.width - pad * 2.0,
            header.height,
        ),
        theme.ink_dim,
        Align::Left,
        scale,
    ));
    frame.text(label(
        state,
        results.summary(),
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

    let (visible, cursor) = results.visible();
    for (index, hit) in visible.iter().enumerate() {
        let y = rows_area.y + index as f32 * row_height;
        let row = Rect::new(rows_area.x, y, rows_area.width, row_height);
        let selected = index == cursor;

        if selected {
            frame.fill(row, theme.row_active);
        }

        // `path:line` first, then the matching line — the order that lets you scan the list.
        let location = format!(
            "{}:{}",
            results.relative(hit, &state.project_root),
            hit.line_number
        );
        let location_width = 320.0 * scale;
        frame.text(run(
            state,
            Role::Value,
            location,
            Rect::new(row.x + pad, y, location_width, row_height),
            if selected { accent } else { theme.ink_dim },
            Align::Left,
            scale,
        ));

        // The matched line, trimmed: leading indentation wastes the width that shows context.
        frame.text(run(
            state,
            Role::Value,
            hit.line.trim_start().to_string(),
            Rect::new(
                row.x + pad + location_width + space::MD * scale,
                y,
                (row.right() - row.x - pad * 2.0 - location_width).max(0.0),
                row_height,
            ),
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

/// The command palette: a scrim over the window and a centered card.
///
/// The design specifies this precisely — full-bleed scrim, a card with a serif-italic query and a
/// blinking caret, rows of `score · name · directory`, and a footer of hints. Geometry is derived
/// from the type scale so it tracks font size.
fn draw_palette(
    frame: &mut Frame,
    state: &EditorState,
    palette: &Palette,
    bounds: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);

    // Scrim: dims the editor without hiding it, so you keep your place.
    frame.fill(bounds, theme.scrim);

    let width = (600.0 * scale).min(bounds.width - space::XXL * 2.0 * scale);
    let row_height = 26.0 * scale;
    let (visible, cursor) = palette.visible();

    let header_height = 52.0 * scale;
    let footer_height = 34.0 * scale;

    // An empty result still occupies a row: the "no matches" line needs somewhere to go. Sizing the
    // card by `visible.len()` alone collapsed it to header plus footer, and the empty-state line was
    // then drawn at the footer's own y — two strings on one baseline, which is what a real run
    // showed for a query that matched nothing.
    let row_count = visible.len().max(1);
    let card_height = header_height + footer_height + row_count as f32 * row_height;

    let card = Rect::new(
        bounds.x + (bounds.width - width) / 2.0,
        bounds.y + 110.0 * scale,
        width,
        card_height,
    );

    // One shadow in the whole design, and this is it. Approximated by a slightly larger dark rect
    // behind the card, since the renderer draws only rectangles.
    frame.fill(
        Rect::new(
            card.x - 2.0 * scale,
            card.y + 6.0 * scale,
            card.width + 4.0 * scale,
            card.height,
        ),
        theme.shadow,
    );
    frame.fill(card, theme.bg_panel);
    frame.rule_top(card, theme.rule, border::BORDER);
    frame.rule_bottom(card, theme.rule, border::BORDER);
    frame.rule_left(card, theme.rule, border::BORDER);
    // The renderer has no right-edge rule helper; the card needs all four sides.
    frame.fill(
        Rect::new(
            card.right() - border::BORDER,
            card.y,
            border::BORDER,
            card.height,
        ),
        theme.rule,
    );

    // Header: the query, set in italic serif, with a caret in the mode color.
    let (header, rest) = card.split_top(header_height);
    let pad = space::LG * scale;
    let query_rect = Rect::new(
        header.x + pad,
        header.y,
        header.width - pad * 2.0,
        header.height,
    );

    let query_text = if palette.query.is_empty() {
        palette.kind.prompt().to_string()
    } else {
        palette.query.clone()
    };
    let mut query_spec = state.typography.spec(Role::Display);
    query_spec.size_px = 22.0 * scale;
    frame.text(TextRun {
        text: query_text.clone(),
        spec: query_spec,
        color: if palette.query.is_empty() {
            theme.ink_faint
        } else {
            theme.ink
        },
        rect: query_rect,
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
    });

    // Caret after the query. Its width is an estimate of the serif advance — the palette is not on
    // the code grid, so exactness is not required here.
    let caret_x = query_rect.x + query_text.chars().count() as f32 * 10.0 * scale + 4.0 * scale;
    frame.fill(
        Rect::new(
            caret_x.min(query_rect.right() - 2.0 * scale),
            query_rect.y + (query_rect.height - 20.0 * scale) / 2.0,
            2.0 * scale,
            20.0 * scale,
        ),
        accent,
    );

    frame.text(label(
        state,
        format!("{} of {}", palette.rows.len(), palette.total),
        query_rect,
        theme.ink_faint,
        Align::Right,
        scale,
    ));
    frame.rule_bottom(header, theme.rule_soft, border::BORDER);

    // Rows.
    let (rows_area, footer) = rest.split_top(row_count as f32 * row_height);
    for (index, row) in visible.iter().enumerate() {
        let y = rows_area.y + index as f32 * row_height;
        let selected = index == cursor;
        let row_rect = Rect::new(rows_area.x, y, rows_area.width, row_height);

        if selected {
            frame.fill(row_rect, theme.row_active);
        }

        // Score, right-aligned in a narrow column. Not for commands: a match score beside "save this
        // file" is the editor showing its workings to someone who asked what it can do.
        let score_rect = Rect::new(row_rect.x + pad, y, 30.0 * scale, row_height);
        if palette.kind != PaletteKind::Commands {
            frame.text(run(
                state,
                Role::Value,
                row.score.to_string(),
                score_rect,
                if selected {
                    theme.ink_dim
                } else {
                    theme.ink_faint
                },
                Align::Right,
                scale,
            ));
        }

        // Name, with matched characters in the mode color. Drawn as per-character runs only where
        // a highlight falls, so the common case stays one run.
        let name_x = score_rect.right() + space::MD * scale;
        // The advance must match the *row's* font, not the code font.
        let advance = advance_for(state.typography.spec(Role::Value).size_px * scale);
        frame.text(run(
            state,
            Role::Value,
            row.name.clone(),
            Rect::new(name_x, y, rows_area.right() - name_x, row_height),
            if selected { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));
        for position in &row.highlights {
            let Some(c) = row.name.chars().nth(*position as usize) else {
                continue;
            };
            frame.text(run(
                state,
                Role::Value,
                c.to_string(),
                Rect::new(
                    name_x + *position as f32 * advance,
                    y,
                    advance * 2.0,
                    row_height,
                ),
                accent,
                Align::Left,
                scale,
            ));
        }

        // Directory, dimmed, after the name — except in the command palette, where the context is
        // the *key*, and a key column has to be a column. Ragged keys trailing each description
        // would be unreadable as the table it needs to be, and reading it as a table is how the
        // palette teaches shortcuts instead of merely running things.
        if !row.context.is_empty() {
            let (context_rect, align, color) = if palette.kind == PaletteKind::Commands {
                (
                    Rect::new(rows_area.x, y, rows_area.width - pad, row_height),
                    Align::Right,
                    if selected { accent } else { theme.ink_faint },
                )
            } else {
                let context_x = name_x + (row.name.chars().count() as f32 + 2.0) * advance;
                (
                    Rect::new(
                        context_x,
                        y,
                        (rows_area.right() - context_x - pad).max(0.0),
                        row_height,
                    ),
                    Align::Left,
                    theme.ink_faint,
                )
            };
            frame.text(run(
                state,
                Role::Value,
                row.context.clone(),
                context_rect,
                color,
                align,
                scale,
            ));
        }
    }

    if visible.is_empty() {
        frame.text(run(
            state,
            Role::Value,
            "no matches",
            Rect::new(
                rows_area.x + pad,
                rows_area.y,
                rows_area.width,
                row_height.max(1.0),
            ),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
    }

    // Footer hints.
    frame.rule_top(footer, theme.rule_soft, border::BORDER);
    frame.text(label(
        state,
        // `↵` was a tofu box here too — the bundled font has the arrows but not the return symbol.
        "enter open · ↑↓ move · esc dismiss",
        Rect::new(
            footer.x + pad,
            footer.y,
            footer.width - pad * 2.0,
            footer.height,
        ),
        theme.ink_faint,
        Align::Left,
        scale,
    ));
}

/// The which-key hint: what can follow the keys typed so far.
///
/// The statusline already showed the prefix — `<c-w>` sat there on its own, which tells you the
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
    let entries = vex_input::commands::continuations(&state.pending, state.modal);
    if entries.is_empty() {
        return;
    }

    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let row_height = 22.0 * scale;
    let pad = space::MD * scale;

    // Wide enough for the longest description, so nothing is clipped — the popup is small and the
    // measurement is over a handful of static strings.
    let longest = entries
        .iter()
        .map(|(key, what)| key.chars().count() + what.chars().count())
        .max()
        .unwrap_or(20) as f32;
    let keys_width = entries
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(4) as f32;
    let width = (longest * 8.0 * scale + pad * 6.0).min(code.width - pad * 2.0);
    let height = entries.len() as f32 * row_height + pad * 2.0;

    // Kept inside the code area: the popup must not fall off the bottom of the window, which is
    // exactly where the cursor is when a file is longer than the screen.
    let cursor_y = code.y + row_height;
    let y = if cursor_y + height > code.bottom() {
        (code.bottom() - height).max(code.y)
    } else {
        cursor_y
    };
    let card = Rect::new(code.x + pad, y, width, height);

    frame.fill(
        Rect::new(card.x - 2.0 * scale, card.y + 4.0 * scale, card.width + 4.0 * scale, card.height),
        theme.shadow,
    );
    frame.fill(card, theme.bg_panel);
    frame.rule_top(card, theme.rule, border::BORDER);
    frame.rule_bottom(card, theme.rule, border::BORDER);
    frame.rule_left(card, theme.rule, border::BORDER);

    for (index, (key, what)) in entries.iter().enumerate() {
        let y = card.y + pad + index as f32 * row_height;
        // The key column is only as wide as the longest key: `v` and `<c-o>` next to a description
        // half a window away read as two unrelated lists.
        let key_column = (keys_width + 2.0) * 8.0 * scale;
        frame.text(run(
            state,
            Role::Value,
            *key,
            Rect::new(card.x + pad, y, key_column, row_height),
            accent,
            Align::Left,
            scale,
        ));
        frame.text(run(
            state,
            Role::Value,
            *what,
            Rect::new(
                card.x + pad + key_column,
                y,
                (card.width - pad * 2.0 - key_column).max(0.0),
                row_height,
            ),
            theme.ink_dim,
            Align::Left,
            scale,
        ));
    }
}

/// The help panel: every key that works right now, grouped and filterable.
fn draw_help(
    frame: &mut Frame,
    state: &EditorState,
    help: &crate::help::Help,
    bounds: Rect,
    scale: f32,
) {
    use crate::help::Line;

    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(bounds, theme.scrim);

    let pad = space::LG * scale;
    let row_height = 24.0 * scale;
    let width = (720.0 * scale).min(bounds.width - space::XXL * 2.0 * scale);
    let card = Rect::new(
        bounds.x + (bounds.width - width) / 2.0,
        bounds.y + 80.0 * scale,
        width,
        (bounds.height - 160.0 * scale).max(row_height * 6.0),
    );

    frame.fill(card, theme.bg_panel);
    frame.rule_top(card, theme.rule, border::BORDER);
    frame.rule_bottom(card, theme.rule, border::BORDER);
    frame.rule_left(card, theme.rule, border::BORDER);

    let (header, rest) = card.split_top(52.0 * scale);
    // `split_bottom` returns the *bottom* piece first, unlike `split_top`. Taking them the other way
    // round drew the whole list into a thirty-pixel strip at the foot of the card and the footer
    // across the middle of it — which is what the first render of this panel actually looked like.
    let (footer, rows_area) = rest.split_bottom(30.0 * scale);

    let mut title_spec = state.typography.spec(Role::Display);
    title_spec.size_px = 22.0 * scale;
    frame.text(TextRun {
        // Says which grammar these are, because that is the question underneath the question: a
        // non-modal user needs to know they are not being shown Vim's keys.
        text: if help.query.is_empty() {
            match state.modal {
                true => "keys — vim grammar".to_string(),
                false => "keys".to_string(),
            }
        } else {
            help.query.clone()
        },
        spec: title_spec,
        color: if help.query.is_empty() {
            theme.ink
        } else {
            accent
        },
        rect: Rect::new(header.x + pad, header.y, header.width - pad * 2.0, header.height),
        align: Align::Left,
        center_vertically: true,
        wrap: false,
        layer: Layer::default(),
    });
    frame.text(label(
        state,
        "type to filter",
        Rect::new(header.x + pad, header.y, header.width - pad * 2.0, header.height),
        theme.ink_faint,
        Align::Right,
        scale,
    ));
    frame.rule_bottom(header, theme.rule_soft, border::BORDER);

    let visible = ((rows_area.height / row_height).floor() as usize).max(1);
    let lines = help.lines();
    for (index, line) in lines.iter().skip(help.scroll).take(visible).enumerate() {
        let y = rows_area.y + index as f32 * row_height;
        match line {
            Line::Heading(title) => {
                frame.text(label(
                    state,
                    *title,
                    Rect::new(rows_area.x + pad, y, rows_area.width - pad * 2.0, row_height),
                    theme.ink_faint,
                    Align::Left,
                    scale,
                ));
            }
            Line::Binding { keys, what } => {
                // Keys in the mode colour on the left, what they do beside them. The key column is
                // fixed so the list reads as a table rather than as prose.
                frame.text(run(
                    state,
                    Role::Value,
                    *keys,
                    Rect::new(rows_area.x + pad * 2.0, y, 150.0 * scale, row_height),
                    accent,
                    Align::Left,
                    scale,
                ));
                frame.text(run(
                    state,
                    Role::Value,
                    *what,
                    Rect::new(
                        rows_area.x + pad * 2.0 + 160.0 * scale,
                        y,
                        (rows_area.width - pad * 4.0 - 160.0 * scale).max(0.0),
                        row_height,
                    ),
                    theme.ink_dim,
                    Align::Left,
                    scale,
                ));
            }
        }
    }

    frame.rule_top(footer, theme.rule_soft, border::BORDER);
    frame.text(label(
        state,
        "↑↓ scroll · esc close · ctrl-shift-p every command",
        Rect::new(footer.x + pad, footer.y, footer.width - pad * 2.0, footer.height),
        theme.ink_faint,
        Align::Left,
        scale,
    ));
}

/// The menu bar, or a context menu at the pointer.
fn draw_menu(
    frame: &mut Frame,
    state: &EditorState,
    menu: &crate::menu::Menu,
    bounds: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);

    // Geometry comes from the menu, not from here: the mouse reads the same function, and a second
    // copy of this arithmetic is how clicks start landing a row off.
    let geometry = menu.geometry(bounds, scale);
    let card = geometry.card;
    let pad = geometry.pad;
    let label_width = geometry.label_width;

    // The bar's titles are drawn here too, since the bar only exists while it is open — a permanent
    // menu bar would cost a row of every window for something most people use twice.
    for (index, (title, rect)) in menu.titles.iter().zip(&geometry.titles).enumerate() {
        if index == menu.open {
            frame.fill(*rect, theme.row_active);
        }
        frame.text(run(
            state,
            Role::Value,
            *title,
            Rect::new(rect.x + 4.0 * scale, rect.y, rect.width, rect.height),
            if index == menu.open {
                accent
            } else {
                theme.ink_dim
            },
            Align::Left,
            scale,
        ));
    }

    frame.fill(
        Rect::new(card.x - 2.0 * scale, card.y + 4.0 * scale, card.width + 4.0 * scale, card.height),
        theme.shadow,
    );
    frame.fill(card, theme.bg_panel);
    frame.rule_top(card, theme.rule, border::BORDER);
    frame.rule_bottom(card, theme.rule, border::BORDER);
    frame.rule_left(card, theme.rule, border::BORDER);

    for (index, (item, row)) in menu.items.iter().zip(&geometry.rows).enumerate() {
        if index == menu.selected {
            frame.fill(*row, theme.row_active);
        }
        frame.text(run(
            state,
            Role::Value,
            item.label.clone(),
            Rect::new(row.x + pad, row.y, label_width + pad, row.height),
            if index == menu.selected {
                theme.ink
            } else {
                theme.ink_dim
            },
            Align::Left,
            scale,
        ));
        // The key, right-aligned in its own column: the menu's whole purpose is that you read this
        // and stop needing the menu.
        if !item.keys.is_empty() {
            frame.text(run(
                state,
                Role::Value,
                item.keys.clone(),
                Rect::new(row.x + pad, row.y, row.width - pad * 2.0, row.height),
                theme.ink_faint,
                Align::Right,
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
    let advance = state.typography.advance(role) * scale;
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

/// What the viewport shows when no file is open: the mark, and one line telling you how to open one.
///
/// Nothing else. A recent-files list or a tips carousel here would be the first place the editor
/// started competing with the work instead of holding it.
fn draw_empty_state(frame: &mut Frame, state: &EditorState, code: Rect, scale: f32) {
    let theme = &state.theme;
    let size = 64.0 * scale;
    let line_height = 28.0 * scale;
    let block = size + line_height;

    let top = (code.y + (code.height - block) / 2.0).round();
    let left = (code.x + (code.width - size) / 2.0).round();

    // 40% opacity, over the canvas it sits on: the mark is present, not insistent.
    frame.mark(
        Rect::new(left, top, size, size),
        theme.ink_faint.with_alpha(0.4),
        theme.bg,
    );

    frame.text(label(
        state,
        "ctrl-p  open a file",
        Rect::new(code.x, top + size, code.width, line_height),
        theme.ink_faint,
        Align::Center,
        scale,
    ));

    // The one line of teaching in the whole editor, and it goes here because this is the screen
    // someone is looking at when they have no idea what to do next. Not a tips carousel, not a
    // tour: one key, and it answers everything else.
    frame.text(label(
        state,
        "f1  every key that works",
        Rect::new(
            code.x,
            top + size + line_height * 1.4,
            code.width,
            line_height,
        ),
        theme.ink_faint.with_alpha(0.7),
        Align::Center,
        scale,
    ));
}

/// The settings panel.
fn draw_settings(
    frame: &mut Frame,
    state: &EditorState,
    settings: &crate::settings_ui::Settings,
    rect: Rect,
    scale: f32,
) {
    use crate::settings_ui::Source;

    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let pad = space::LG * scale;
    let line = 20.0 * scale;
    let width = rect.width - pad * 2.0;
    let title = Rect::new(rect.x + pad, rect.y + pad, width, line);

    frame.text(run(
        state,
        Role::Prose,
        "Settings",
        title,
        theme.ink,
        Align::Left,
        scale,
    ));

    let values = crate::settings_ui::Values {
        modal: state.modal,
        font_size: state.typography.code_size,
        tab_width: state.display.tab_width,
        soft_wrap: state.display.mode == WrapMode::Word,
        format_on_save: state.format_on_save,
        theme: state.theme.name(),
        line_numbers: state.line_numbers,
        sticky_scroll: state.sticky_scroll,
        inlay_hints: state.show_inlay_hints,
        autosave: state.autosave,
    };

    // Every rectangle comes from `settings_ui::panel`, which the mouse reads too. The panel had two
    // copies of this walk, and the moment the list started scrolling they disagreed: a click
    // selected whichever row used to be under the pointer.
    let panel = crate::settings_ui::panel(rect, scale, settings.selected);

    if let Some(heading) = panel.heading {
        frame.text(label(
            state,
            "import from vscode",
            heading,
            theme.ink_dim,
            Align::Left,
            scale,
        ));
    }

    for placement in &panel.rows {
        use crate::settings_ui::Line;

        let (index, entry, y, block) = (
            placement.index,
            &placement.line,
            placement.rect.y,
            placement.rect.height,
        );
        let selected = index == settings.selected;

        if selected {
            frame.fill(
                Rect::new(rect.x, y - 3.0 * scale, rect.width, block),
                theme.row_active,
            );
            frame.fill(
                Rect::new(
                    rect.x,
                    y - 3.0 * scale,
                    border::ACCENT_BORDER * scale,
                    block,
                ),
                accent,
            );
        }

        match entry {
            Line::Option(field) => {
                // The value is the thing you came to read, so it keeps its width and the name gives
                // way: in a narrow panel a truncated name is still recognisable, a truncated value
                // is useless.
                let value = values.get(*field);
                let value_width = ((value.chars().count() as f32 + 1.0)
                    * state.typography.advance(Role::Value)
                    * scale)
                    .min(width);

                frame.text(run(
                    state,
                    Role::Value,
                    elide(
                        state,
                        Role::Value,
                        field.title(state.modal),
                        (width - value_width).min(width * 0.62),
                        scale,
                    ),
                    Rect::new(rect.x + pad, y, width * 0.62, line),
                    if selected { theme.ink } else { theme.ink_dim },
                    Align::Left,
                    scale,
                ));

                // The value in the mode accent when selected: it is what `h`/`l` changes.
                frame.text(run(
                    state,
                    Role::Value,
                    value,
                    Rect::new(rect.x + pad, y, width, line),
                    if selected { accent } else { theme.ink },
                    Align::Right,
                    scale,
                ));

                // Where it came from, or what it does. The source is the more useful of the two once
                // you are looking for why a setting is not what you expected.
                let source = state
                    .setting_sources
                    .get(field.name())
                    .copied()
                    .unwrap_or(Source::Default);
                frame.text(label(
                    state,
                    elide(
                        state,
                        Role::Label,
                        if selected {
                            field.help(state.modal)
                        } else {
                            source.label()
                        },
                        width,
                        scale,
                    ),
                    Rect::new(rect.x + pad, y + line * 0.85, width, line),
                    theme.ink_faint,
                    Align::Left,
                    scale,
                ));
            }
            Line::Import(item) => {
                // `[x]` rather than a check mark: the bundled fonts have none, and a missing glyph
                // renders as a tofu box — which is how the file tree's chevrons first shipped.
                frame.text(run(
                    state,
                    Role::Value,
                    elide(
                        state,
                        Role::Value,
                        &format!(
                            "[{}] {}",
                            if settings.is_ticked(*item) { "x" } else { " " },
                            item.name()
                        ),
                        width,
                        scale,
                    ),
                    Rect::new(rect.x + pad, y, width, line),
                    if settings.is_ticked(*item) {
                        theme.ink
                    } else {
                        theme.ink_dim
                    },
                    Align::Left,
                    scale,
                ));
            }
            Line::LoadTheme => {
                frame.text(run(
                    state,
                    Role::Value,
                    "load a theme file…",
                    Rect::new(rect.x + pad, y, width, line),
                    if selected { accent } else { theme.ink_dim },
                    Align::Left,
                    scale,
                ));
            }
            Line::RunImport => {
                let available = crate::import_cli::vscode_is_present();
                frame.text(run(
                    state,
                    Role::Value,
                    elide(
                        state,
                        Role::Value,
                        if available {
                            "import now"
                        } else {
                            "no vscode installation found"
                        },
                        width,
                        scale,
                    ),
                    Rect::new(rect.x + pad, y, width, line),
                    match (available, selected) {
                        (false, _) => theme.ink_faint,
                        (true, true) => accent,
                        (true, false) => theme.ink_dim,
                    },
                    Align::Left,
                    scale,
                ));
            }
        }
    }

    // What the last import said, if one ran. Under the last visible row, if there is room.
    if let Some(report) = &settings.import_report {
        let after = panel
            .rows
            .last()
            .map(|row| row.rect.bottom() + line * 0.4)
            .unwrap_or(panel.footer.y);
        if after + line <= panel.footer.y - line * 0.6 {
            frame.text(label(
                state,
                elide(state, Role::Label, report, width, scale),
                Rect::new(rect.x + pad, after, width, line),
                theme.fresh(),
                Align::Left,
                scale,
            ));
        }
    }

    // The scroll cue rides on the title, which has the whole width of the panel spare, rather than
    // competing with the footer, which does not. Without a cue at all a list that scrolls looks
    // like a list that is missing settings — the complaint this panel already answered once.
    //
    // `↓` rather than a solid triangle: the tree's chevrons shipped as tofu boxes once, and the
    // arrows are glyphs this font is known to have.
    if panel.scrolls {
        frame.text(label(
            state,
            "more ↓",
            title,
            theme.ink_faint,
            Align::Right,
            scale,
        ));
    }

    // The footer says how to change and how to keep. Nothing is written without being asked: this is
    // a file the user owns.
    //
    // Short, because the panel is 292px wide and the old hint was half again too long for it — so
    // it was hard-clipped to `CLICK TO CHANGE · H L ADJUST · W WI`, which is how the defect was
    // first reported. `h l` and `w` are also Vim's letters, and this panel is reached from a toolbar
    // button by people who turned Vim off; the keys underneath take arrows and ctrl-s either way.
    let hint = match (settings.dirty, state.modal) {
        (true, true) => "w writes init.lua",
        (true, false) => "ctrl-s writes init.lua",
        (false, true) => "click or h l · w saves",
        (false, false) => "click or ←→ · ctrl-s saves",
    };

    frame.text(label(
        state,
        // Says click first: the panel is reachable from the toolbar by people who have not been
        // told any keys, and a footer that only names keys is a footer that excludes them.
        elide(state, Role::Label, hint, width, scale),
        panel.footer,
        if settings.dirty {
            theme.stale()
        } else {
            theme.ink_faint
        },
        Align::Left,
        scale,
    ));
}

/// Every problem in the **project**, as a list.
///
/// The underlines say *where* in this file; this says what is wrong anywhere. Both are needed: `]d`
/// walks one file's problems one at a time, which cannot answer "did my change break anything?" —
/// and vex has no terminal to ask that from either.
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

    let here = state.file_path.as_deref().map(std::path::Path::new);
    let problems = state.problems.ordered(here);

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

    for problem in &problems {
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

        let on_cursor_line = here == Some(problem.path.as_path())
            && problem.line == state.cursor_point().line;
        if on_cursor_line {
            frame.fill(
                Rect::new(rect.x, y - 2.0 * scale, rect.width, line * 2.0),
                theme.row_active,
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

        // The first line of the message, wrapped across the two rows this entry has. Server messages
        // run to paragraphs; the box that appears when the cursor reaches the diagnostic is where the
        // whole text belongs, and a list is for deciding where to go.
        let message = problem.message.clone();
        let mut spec = state.typography.spec(Role::Value);
        spec.size_px *= scale;
        frame.text(TextRun {
            text: message,
            spec,
            color: if on_cursor_line {
                theme.ink
            } else {
                theme.ink_dim
            },
            rect: Rect::new(
                rect.x + pad + 40.0 * scale,
                y,
                width - 40.0 * scale,
                line * 2.0,
            ),
            align: Align::Left,
            center_vertically: false,
            // Wrapped, unlike everything else in the chrome: a clipped message is unreadable exactly
            // when it matters, and there is a second row of space here already.
            wrap: true,
            layer: Layer::default(),
        });

        y += line * 2.0;
    }
}

/// The file tree, in the sidebar.
fn draw_tree(
    frame: &mut Frame,
    state: &EditorState,
    tree: &crate::tree::FileTree,
    rect: Rect,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    let pad = space::LG * scale;
    let line = 22.0 * scale;
    let mut y = rect.y + pad;

    frame.text(run(
        state,
        Role::Prose,
        "Files",
        Rect::new(rect.x + pad, y, rect.width - pad * 2.0, line),
        theme.ink,
        Align::Left,
        scale,
    ));
    y += line * 1.6;

    if tree.is_empty() {
        frame.text(label(
            state,
            "indexing",
            Rect::new(rect.x + pad, y, rect.width - pad * 2.0, line),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        return;
    }

    let height = (((rect.bottom() - pad) - y) / line).floor().max(1.0) as usize;
    let (rows, cursor) = tree.visible(height);

    for (index, row) in rows.iter().enumerate() {
        let row_y = y + index as f32 * line;
        let selected = index == cursor;

        if selected {
            frame.fill(Rect::new(rect.x, row_y, rect.width, line), theme.row_active);
            frame.fill(
                Rect::new(rect.x, row_y, border::ACCENT_BORDER * scale, line),
                accent,
            );
        }

        // One indent step per level, and an ASCII marker. Not a chevron or a folder glyph: the
        // bundled fonts have neither, and a missing glyph renders as a tofu box — which is how this
        // first shipped, in a capture where every directory wore a small empty rectangle.
        // A trailing slash names the directory, `+`/`-` says whether it is open.
        let indent = pad + row.depth as f32 * 12.0 * scale;
        let text = if row.is_directory {
            format!("{} {}/", if row.expanded { "-" } else { "+" }, row.name)
        } else {
            format!("  {}", row.name)
        };

        frame.text(run(
            state,
            Role::Value,
            text,
            Rect::new(
                rect.x + indent,
                row_y,
                (rect.right() - pad - (rect.x + indent)).max(0.0),
                line,
            ),
            match (row.is_directory, selected) {
                (_, true) => theme.ink,
                (true, false) => theme.ink_dim,
                (false, false) => theme.ink_faint,
            },
            Align::Left,
            scale,
        ));
    }
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
fn visible_slice(grid: &crate::layout::CodeGrid, start_column: usize, content: &str) -> Option<(f32, String)> {
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

fn draw_title(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(rect, theme.bg_alt);
    frame.rule_bottom(rect, theme.rule, border::BORDER);

    let inner = Rect::new(
        rect.x + space::XL * scale,
        rect.y,
        rect.width - space::XL * 2.0 * scale,
        rect.height,
    );

    // The mark, then the wordmark. The mark is dimmer on purpose — it is a maker's stamp, and
    // nothing in the title bar should compete with the mode indicator on the right. It is also the
    // one element in the chrome immune to the mode hue, which is what makes it read as identity
    // rather than state.
    let mark_size = 26.0 * scale;
    let (mark_box, after) = inner.split_left(mark_size);
    frame.mark(
        Rect::new(
            mark_box.x,
            mark_box.y + (mark_box.height - mark_size) / 2.0,
            mark_size,
            mark_size,
        ),
        theme.ink_faint,
        theme.bg_alt,
    );

    // Wordmark: lowercase italic serif, always.
    let (wordmark, after) = after.split_left(20.0 * scale).1.split_left(78.0 * scale);
    frame.text(run(
        state,
        Role::Display,
        "vex",
        wordmark,
        theme.ink,
        Align::Left,
        scale,
    ));

    let mut breadcrumb = state.path.join(" / ");
    if state.document.is_dirty() {
        breadcrumb.push_str(" •");
    }
    let (crumbs, right) = after.split_left(after.width * 0.55);
    frame.text(run(
        state,
        Role::Prose,
        breadcrumb,
        crumbs,
        theme.ink_dim,
        Align::Left,
        scale,
    ));

    let (bar, name) = right.split_right(34.0 * scale);
    frame.text(label(
        state,
        // No mode word when there are no modes. The statusline already knew this; the title bar did
        // not, so someone who turned Vim off was told "INSERT MODE" by the largest label on screen
        // while the editor underneath had no modes at all. It says what the file is instead, which
        // is the thing that is actually true of a non-modal editor.
        match state.modal {
            true => format!("{} mode", state.mode.label()),
            false => state
                .file_path
                .as_deref()
                .and_then(vex_syntax::Language::from_path)
                .map(|language| language.name().to_string())
                .unwrap_or_default(),
        },
        name,
        theme.ink_faint,
        Align::Right,
        scale,
    ));
    frame.fill(
        Rect::new(
            bar.x + space::SM * scale,
            bar.y + (bar.height - 3.0 * scale) / 2.0,
            34.0 * scale,
            3.0 * scale,
        ),
        accent,
    );
}

fn draw_tabs(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(rect, theme.bg_alt);
    frame.rule_bottom(rect, theme.rule_soft, border::BORDER);

    // One tab per open file, from the same function the mouse hit-tests. The strip used to draw a
    // single tab — the active file's name — so every other open buffer was invisible, and there was
    // no way to close one at all.
    for tab in crate::layout::tabs(rect, state.buffer_count, state.active_buffer, scale) {
        let (name, dirty) = state
            .buffer_tabs
            .get(tab.index)
            .cloned()
            .unwrap_or_else(|| (String::from("untitled"), false));
        let active = tab.index == state.active_buffer;
        let hovered = crate::layout::contains(tab.rect, state.pointer.0, state.pointer.1);

        if active {
            frame.fill(tab.rect, theme.bg);
        } else if hovered {
            frame.fill(tab.rect, theme.row_active);
        }

        // A dot rather than a second word: the tab is narrow, and "modified" spelled out would
        // take the room the name needs.
        let label_text = if dirty { format!("{name} ·") } else { name };
        let label_area = Rect::new(
            tab.rect.x + space::XS * scale,
            tab.rect.y,
            (tab.close.x - tab.rect.x - space::XS * 2.0 * scale).max(0.0),
            tab.rect.height,
        );
        frame.text(run(
            state,
            Role::Value,
            elide(state, Role::Value, &label_text, label_area.width, scale),
            label_area,
            if active { theme.ink } else { theme.ink_dim },
            Align::Left,
            scale,
        ));

        // `x` rather than a multiplication sign or a cross glyph: the bundled font has the letter,
        // and the tree's chevrons shipped as tofu boxes once already.
        if active || hovered {
            frame.text(run(
                state,
                Role::Value,
                "x",
                tab.close,
                if crate::layout::contains(tab.close, state.pointer.0, state.pointer.1) {
                    theme.ink
                } else {
                    theme.ink_faint
                },
                Align::Center,
                scale,
            ));
        }

        if active {
            frame.fill(
                Rect::new(
                    tab.rect.x,
                    tab.rect.bottom() - border::ACCENT_BORDER * scale,
                    tab.rect.width,
                    border::ACCENT_BORDER * scale,
                ),
                accent,
            );
        }
    }

    // Buttons rather than a line of advice. This row used to advertise `i / v / R / esc mode` to
    // people who had turned modes off — keys that do nothing, shown to exactly the person least able
    // to tell — and even at its best it was a list of shortcuts for someone who had just told the
    // wizard they did not want to memorise shortcuts.
    //
    // Each one prints its own key beside it in the menu and the palette, so using the button twice
    // teaches the shortcut and the third time it is not needed. That is the same bargain the menus
    // make: the mouse path exists to make itself redundant, not to replace the keyboard.
    let buttons = crate::layout::toolbar_buttons(rect, scale);
    for (button, box_) in &buttons {
        let hovered = crate::layout::contains(*box_, state.pointer.0, state.pointer.1);
        if hovered {
            frame.fill(*box_, theme.row_active);
        }
        frame.text(label(
            state,
            button.label,
            *box_,
            if hovered { accent } else { theme.ink_dim },
            Align::Center,
            scale,
        ));
    }

    // `buffer 3 of 7` used to live here, which was the strip apologising for not showing the tabs.
    // It shows them now.
}

fn draw_code(
    frame: &mut Frame,
    state: &EditorState,
    pane: &PaneView<'_>,
    gutter: Rect,
    code: Rect,
    row_height: f32,
    scale: f32,
) {
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

    let (from, to) = theme.gutter_tint(state.mode);
    if pane.focused {
        frame.fill_gradient_x(gutter, from, to, 24);
    } else {
        frame.fill(gutter, theme.bg_alt);
    }

    if pane.focused && state.is_empty() {
        draw_empty_state(frame, state, code, scale);
        return;
    }

    // Sticky scroll: the block heads enclosing the top of the viewport, pinned above it. Only when
    // the head has actually scrolled off — pinning a line that is already visible would show it
    // twice, which is the thing that makes this feature annoying elsewhere.
    let sticky: Vec<usize> = if pane.focused && state.sticky_scroll {
        vex_display::enclosing_heads(text, pane.scroll.line, state.display.tab_width, 3)
            .into_iter()
            .filter(|head| *head < pane.scroll.line)
            .collect()
    } else {
        Vec::new()
    };

    let (code, sticky_area) = if sticky.is_empty() {
        (code, None)
    } else {
        let strip = row_height * sticky.len() as f32;
        let (top, rest) = code.split_top(strip);
        (rest, Some(top))
    };

    if let Some(area) = sticky_area {
        frame.fill(area, theme.bg_alt);
        frame.rule_bottom(area, theme.rule_soft, border::BORDER);

        for (index, head) in sticky.iter().enumerate() {
            let y = area.y + index as f32 * row_height;
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

    let height = state.visible_rows(code.height, row_height);
    let rows = state.display.rows_from(text, pane.scroll, height);
    let advance = column_advance(state, scale);
    // One grid, read by every x below and by the mouse. The horizontal scroll offset enters the
    // arithmetic here and nowhere else — a second copy is a click landing in a different column
    // from the one under the pointer the instant the view moves sideways.
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
    );
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
    let overlaps = |range: &std::ops::Range<CharOffset>| range.start < span.end && range.end > span.start;

    let diagnostics: Vec<(std::ops::Range<CharOffset>, _)> = if pane.focused {
        state
            .diagnostics
            .items()
            .iter()
            .filter_map(|diagnostic| {
                // The conversion is a rope lookup, so doing it here also takes it off the row loop.
                let range = vex_lsp::range_from_lsp(text, diagnostic.range);
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
                let at = vex_lsp::from_lsp(text, hint.position);
                if at < span.start || at > span.end {
                    return None;
                }
                let content: String = match &hint.label {
                    vex_lsp::lsp_types::InlayHintLabel::String(label) => label.clone(),
                    vex_lsp::lsp_types::InlayHintLabel::LabelParts(parts) => {
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
        let y = code.y + index as f32 * row_height;
        let on_cursor_line = row.line == cursor_line;

        if on_cursor_line && pane.focused {
            frame.fill_linear(
                Rect::new(code.x, y, code.width, row_height),
                theme.line_tint(state.mode),
            );
            frame.fill(
                Rect::new(code.x, y, border::ACCENT_BORDER * scale, row_height),
                accent,
            );
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
            let severity =
                severity.unwrap_or(vex_lsp::lsp_types::DiagnosticSeverity::WARNING);
            let color = if severity == vex_lsp::lsp_types::DiagnosticSeverity::ERROR {
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
                ).intersect(code),
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
                ).intersect(code),
                // The visual hue at low alpha: a found-text wash that is not the selection and not
                // the cursor line.
                theme
                    .accent(vex_theme::Mode::Visual)
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
                ).intersect(code),
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
        if pane.focused && row.is_first() {
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
                if on_cursor_line {
                    accent
                } else {
                    theme.ink_faint
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
    if !pane.focused {
        return;
    }

    // The cursor comes from the display map, so it lands correctly on a wrapped row.
    if let Some(point) = state
        .display
        .display_point(text, pane.scroll, height, cursor)
    {
        draw_cursor(
            frame,
            state,
            CursorGeometry {
                x: text_left + point.column as f32 * advance,
                y: code.y + point.row as f32 * row_height,
                row_height,
                em,
                advance,
                scale,
            },
        );
    }
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

fn draw_sidebar(
    frame: &mut Frame,
    state: &EditorState,
    rect: Rect,
    visible_rows: usize,
    scale: f32,
) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(rect, theme.bg_panel);
    frame.rule_left(rect, theme.rule, border::BORDER);

    if let Some(tree) = &state.tree {
        draw_tree(frame, state, tree, rect, scale);
        return;
    }
    if state.show_diagnostics {
        draw_diagnostics_list(frame, state, rect, scale);
        return;
    }
    if let Some(settings) = &state.settings {
        draw_settings(frame, state, settings, rect, scale);
        return;
    }

    let pad = space::LG * scale;
    let line = 20.0 * scale;
    let width = rect.width - pad * 2.0;
    let mut y = rect.y + pad;

    let text = state.document.text();
    let cursor = state.cursor();
    let point = state.cursor_point();

    // Heading.
    frame.text(run(
        state,
        Role::Prose,
        "Structure",
        Rect::new(rect.x + pad, y, width, line * 1.4),
        theme.ink,
        Align::Left,
        scale,
    ));
    y += line * 1.6;

    // The panel is a fixed height and this list is not. In a short window the rows ran straight
    // into the footer below and the two were painted over each other — the same defect the settings
    // panel beside it had, and invisible for the same reason: the capture harness only ever
    // rendered at 1600x1000, where everything fits.
    //
    // `y` still advances for a row that is not drawn, so the arithmetic is unchanged and only the
    // painting is suppressed. The rows get the whole panel: the tagline below is decoration and
    // gives way to them, rather than the other way round.
    let limit = rect.bottom() - pad;

    let section = |frame: &mut Frame, content: &str, y: &mut f32| {
        if *y + line <= limit {
            frame.text(label(
                state,
                content,
                Rect::new(rect.x + pad, *y, width, line),
                theme.ink_faint,
                Align::Left,
                scale,
            ));
        }
        *y += line * 1.2;
    };

    let pair = |frame: &mut Frame, key: &str, value: &str, y: &mut f32, color: Oklch| {
        if *y + line <= limit {
            let row = Rect::new(rect.x + pad, *y, width, line);
            frame.text(run(
                state,
                Role::Value,
                elide(state, Role::Value, key, width * 0.6, scale),
                row,
                theme.ink_dim,
                Align::Left,
                scale,
            ));
            frame.text(run(
                state,
                Role::Value,
                value,
                row,
                color,
                Align::Right,
                scale,
            ));
        }
        *y += line;
    };

    // All four coordinate spaces for the primary cursor. Not a debug panel: this is the
    // typed-coordinates decision made visible, and it is how conversion bugs get caught early.
    section(frame, "coordinate spaces", &mut y);
    let byte = text.char_to_byte(cursor);
    let (lsp_line, lsp_column) = text.point_to_lsp(point);
    let display_point = state
        .display
        // Bounded by the viewport, not by a generous guess: walking rows that are not on screen
        // is the difference between a 0.2ms frame and a 1.1ms one.
        .display_point(text, state.scroll, visible_rows, cursor)
        .map(|p| format!("{} : {}", p.row, p.column))
        .unwrap_or_else(|| "off screen".into());

    pair(
        frame,
        "Point",
        &format!("{} : {}", point.line, point.column),
        &mut y,
        theme.ink,
    );
    pair(frame, "DisplayPoint", &display_point, &mut y, theme.ink);
    pair(
        frame,
        "CharOffset",
        &cursor.get().to_string(),
        &mut y,
        theme.ink,
    );
    pair(
        frame,
        "ByteOffset",
        &format!("0x{:x}", byte.get()),
        &mut y,
        theme.ink,
    );
    pair(
        frame,
        "UTF-16 (lsp)",
        &format!("{lsp_line} : {}", lsp_column.get()),
        &mut y,
        theme.ink,
    );
    y += line * 0.8;

    // The async ledger: every background producer reports the version it computed against.
    // Behind the buffer means the result is discarded, never applied. This is the 60fps
    // contract made visible, and it is how that contract gets debugged.
    section(frame, "async ledger", &mut y);
    let buffer = state.document.version();
    pair(frame, "buffer", &buffer.to_string(), &mut y, theme.ink);
    for (name, version) in [
        ("syntax tree", state.syntax_version),
        ("diagnostics", state.diagnostics_version),
    ] {
        let fresh = version.is_fresh_for(buffer);
        pair(
            frame,
            name,
            &format!("{version} {}", if fresh { "fresh" } else { "stale" }),
            &mut y,
            if fresh { theme.fresh() } else { theme.stale() },
        );
    }
    y += line * 0.8;

    // The tree-sitter ancestor chain at the cursor: the deepest node is the one a structural
    // text object would act on.
    if !state.node_path.is_empty() {
        section(frame, "node path", &mut y);

        // Outermost first, deepest last: the chain reads top-down the way the tree nests, and the
        // deepest node — the one a structural text object would act on — is the accented leaf.
        let depth = state.node_path.len();
        let shown: Vec<&String> = state.node_path.iter().rev().take(4).rev().collect();

        if depth > shown.len() {
            frame.text(label(
                state,
                format!("+{} above", depth - shown.len()),
                Rect::new(rect.x + pad, y, width, line),
                theme.ink_faint,
                Align::Left,
                scale,
            ));
            y += line;
        }

        let last = shown.len().saturating_sub(1);
        for (index, kind) in shown.into_iter().enumerate() {
            let deepest = index == last;
            let indent = index as f32 * 10.0 * scale;
            frame.text(run(
                state,
                Role::Value,
                if index == 0 {
                    kind.clone()
                } else {
                    format!("└ {kind}")
                },
                Rect::new(rect.x + pad + indent, y, width - indent, line),
                if deepest { accent } else { theme.ink_dim },
                Align::Left,
                scale,
            ));
            y += line;
        }
        y += line * 0.8;
    }

    section(frame, "document", &mut y);
    pair(
        frame,
        "lines",
        &(text.last_line() + 1).to_string(),
        &mut y,
        theme.ink_dim,
    );
    pair(
        frame,
        "undo depth",
        &state.document.history().depth().to_string(),
        &mut y,
        theme.ink_dim,
    );
    pair(
        frame,
        "parse",
        if state.syntax_error {
            "has errors"
        } else {
            "clean"
        },
        &mut y,
        if state.syntax_error {
            theme.stale()
        } else {
            theme.ink_dim
        },
    );
    let wrap = if state.display.mode == WrapMode::Word {
        format!("{} cols", state.display.columns)
    } else {
        "off".to_string()
    };
    pair(frame, "soft wrap", &wrap, &mut y, theme.ink_dim);
    y += line * 0.8;

    section(frame, "frame", &mut y);
    let over = state.frame_ms > vex_theme::FRAME_BUDGET_MS;
    pair(
        frame,
        "this frame",
        &format!(
            "{:.2} of {:.1} ms",
            state.frame_ms,
            vex_theme::FRAME_BUDGET_MS
        ),
        &mut y,
        if over {
            theme.over_budget()
        } else {
            theme.fresh()
        },
    );
    pair(
        frame,
        "worst recent",
        &format!("{:.2} ms", state.worst_ms),
        &mut y,
        theme.ink_dim,
    );

    // The rule this editor is built around, in the panel that shows it being kept. Decorative, so
    // it is the first thing to go when the panel is too short to hold both — in a small window the
    // coordinate readouts are what someone is actually looking at.
    let footer = Rect::new(rect.x + pad, rect.bottom() - line * 4.0, width, line * 3.0);
    if y + line <= footer.y - line * 0.5 {
        let mut spec = state.typography.spec(Role::Prose);
        spec.size_px *= scale;
        frame.text(TextRun {
            text: "Nothing on the UI thread may ever be unbounded in cost.".into(),
            spec,
            color: theme.ink_dim,
            rect: footer,
            align: Align::Left,
            center_vertically: false,
            wrap: true,
            layer: Layer::default(),
        });
        frame.fill(
            Rect::new(
                rect.x + pad,
                footer.y - line * 0.5,
                26.0 * scale,
                2.0 * scale,
            ),
            accent,
        );
    }
}

fn draw_statusline(frame: &mut Frame, state: &EditorState, rect: Rect, scale: f32) {
    let theme = &state.theme;
    let accent = theme.accent(state.mode);
    frame.fill(rect, theme.bg_alt);
    frame.rule_top(rect, theme.rule, border::BORDER);

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
                total => match vex_buffer::index_of(&prompt.matches, state.cursor()) {
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
        rect.x + space::XL * scale,
        rect.y,
        rect.width - space::XL * 2.0 * scale,
        rect.height,
    );
    let point = state.cursor_point();
    let (left, right) = inner.split_left(inner.width * 0.5);

    let (mode_cell, after_mode) = left.split_left(110.0 * scale);
    frame.text(label(
        state,
        // Recording replaces the mode word, in the mode's own accent. An editor that is silently
        // recording is an editor that surprises you ten minutes later.
        match (state.recording, state.modal) {
            (Some(register), _) => format!("rec @{register}"),
            (None, true) => state.mode.label().to_string(),
            // With modal editing off there is no mode to report, and "NORMAL" would be a lie.
            (None, false) => "editing".to_string(),
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
    frame.text(run(
        state,
        Role::Value,
        selection_text,
        selections,
        theme.ink_dim,
        Align::Left,
        scale,
    ));

    frame.text(run(
        state,
        Role::Value,
        if state.pending.is_empty() {
            "—".to_string()
        } else {
            state.pending.clone()
        },
        pending,
        accent,
        Align::Left,
        scale,
    ));

    let over = state.frame_ms > vex_theme::FRAME_BUDGET_MS;
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
                    .and_then(vex_syntax::Language::from_path)
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
        {
            let (errors, warnings) = state.diagnostics.counts();
            let (project_errors, project_warnings) = state.problems.counts();
            let elsewhere = (project_errors + project_warnings)
                .saturating_sub(errors + warnings);

            if errors + warnings == 0 {
                // Nothing wrong here. If something is wrong *elsewhere*, say so — that is the whole
                // reason the project store exists, and a clean statusline over a broken project is
                // the misleading answer.
                if elsewhere > 0 {
                    (format!("{elsewhere} elsewhere"), theme.stale())
                } else {
                    (state.server_status.clone(), theme.ink_faint)
                }
            } else {
                let plural = |count: usize, word: &str| {
                    if count == 1 {
                        format!("{count} {word}")
                    } else {
                        format!("{count} {word}s")
                    }
                };
                (
                    format!(
                        "{} · {}",
                        plural(errors, "error"),
                        plural(warnings, "warning")
                    ),
                    if errors > 0 {
                        theme.over_budget()
                    } else {
                        theme.stale()
                    },
                )
            }
        },
        (
            format!("{:.2} ms", state.frame_ms),
            if over { theme.over_budget() } else { theme.ink },
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
