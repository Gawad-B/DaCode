//! The editor chrome: title bar, tab strip, gutter, code area, structure sidebar, statusline.
//!
//! A pure function of state into a [`Frame`], so it is testable without a GPU and can move into
//! `crates/ui` unchanged. The design's geometry is guidance: proportions are honored, but pixel
//! values derive from the type scale so the whole chrome scales with font size.
//!
//! The code area draws **display rows**, not buffer lines — with soft wrap one line can occupy
//! several rows, and the gutter figure belongs only to the first of them.

use crate::palette::Palette;
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
    /// Index of the active buffer.
    pub active_buffer: usize,
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

    // Last, so it covers everything: the palette is a modal overlay.
    frame.set_layer(Layer::Modal);
    if let Some(palette) = &state.palette {
        draw_palette(frame, state, palette, bounds, scale);
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
    let card_height = header_height + footer_height + visible.len() as f32 * row_height;

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
    let (rows_area, footer) = rest.split_top(visible.len() as f32 * row_height);
    for (index, row) in visible.iter().enumerate() {
        let y = rows_area.y + index as f32 * row_height;
        let selected = index == cursor;
        let row_rect = Rect::new(rows_area.x, y, rows_area.width, row_height);

        if selected {
            frame.fill(row_rect, theme.row_active);
        }

        // Score, right-aligned in a narrow column.
        let score_rect = Rect::new(row_rect.x + pad, y, 30.0 * scale, row_height);
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

        // Directory, dimmed, after the name.
        if !row.context.is_empty() {
            let context_x = name_x + (row.name.chars().count() as f32 + 2.0) * advance;
            frame.text(run(
                state,
                Role::Value,
                row.context.clone(),
                Rect::new(
                    context_x,
                    y,
                    (rows_area.right() - context_x - pad).max(0.0),
                    row_height,
                ),
                theme.ink_faint,
                Align::Left,
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
        "↵ open · ↑↓ move · esc dismiss",
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
}

/// Advance of one code column.
///
/// Derived from the type scale for now. At the point where a proportional fallback glyph can
/// appear in code, this must come from the shaper's measured advance, or the gutter and the text
/// will drift out of registration.
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
        format!("{} mode", state.mode.label()),
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

    let inner = Rect::new(
        rect.x + space::XL * scale,
        rect.y,
        rect.width - space::XL * 2.0 * scale,
        rect.height,
    );
    let name = state.path.last().cloned().unwrap_or_default();

    let tab_width = 130.0 * scale;
    let (tab, rest) = inner.split_left(tab_width);
    frame.text(run(
        state,
        Role::Value,
        name,
        tab,
        theme.ink,
        Align::Left,
        scale,
    ));
    frame.fill(
        Rect::new(
            tab.x,
            tab.bottom() - border::ACCENT_BORDER * scale,
            tab_width * 0.72,
            border::ACCENT_BORDER * scale,
        ),
        accent,
    );

    let hint = if state.buffer_count > 1 {
        format!(
            "buffer {} of {} · ctrl-p files · ctrl-b buffers",
            state.active_buffer + 1,
            state.buffer_count
        )
    } else {
        "ctrl-p files · i / v / R / esc mode · u undo · z wrap".to_string()
    };
    frame.text(label(
        state,
        hint,
        rest,
        theme.ink_faint,
        Align::Right,
        scale,
    ));
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

    let height = state.visible_rows(code.height, row_height);
    let rows = state.display.rows_from(text, pane.scroll, height);
    let advance = column_advance(state, scale);
    let text_left = code.x + space::SM * scale;
    let em = state.typography.code_size * scale;

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
        for diagnostic in state.diagnostics.items().iter().filter(|_| pane.focused) {
            let range = vex_lsp::range_from_lsp(text, diagnostic.range);
            let start = range.start.max(row.range.start);
            let end = range.end.min(row.range.end);
            if start >= end {
                continue;
            }

            let from_column = columns_between(state, text, row.range.start, start);
            let to_column = columns_between(state, text, row.range.start, end);
            let severity = diagnostic
                .severity
                .unwrap_or(vex_lsp::lsp_types::DiagnosticSeverity::WARNING);
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
                ),
                color,
            );
        }

        // Search matches, under the selection fills: a match you have also selected should read as
        // selected.
        for range in state.search_matches.iter().filter(|_| pane.focused) {
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
                ),
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
                ),
                theme.selection_fill(if pane.focused {
                    state.mode
                } else {
                    Mode::Normal
                }),
            );
        }

        // Gutter figure only on a line's first row: that blankness is how a wrap is told apart
        // from a new line at a glance.
        if row.is_first() {
            let number = if on_cursor_line {
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

        // One run per *highlighted segment*. Cost stays bounded by the viewport: a row yields at
        // most a handful of runs regardless of document size.
        let indent_px = row.indent as f32 * advance;

        // The running column is carried across segments rather than recomputed from the row start
        // each time: re-scanning the prefix per segment is quadratic in segments per row, and a
        // syntax-highlighted row has many segments.
        let mut column = 0usize;

        for segment in segments(pane.highlights, row.range.clone()) {
            let x = text_left + indent_px + column as f32 * advance;
            let raw = text.slice_to_string(segment.range.clone());
            let content = expand_tabs(state, &raw);
            column += content.chars().count();

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

    let section = |frame: &mut Frame, content: &str, y: &mut f32| {
        frame.text(label(
            state,
            content,
            Rect::new(rect.x + pad, *y, width, line),
            theme.ink_faint,
            Align::Left,
            scale,
        ));
        *y += line * 1.2;
    };

    let pair = |frame: &mut Frame, key: &str, value: &str, y: &mut f32, color: Oklch| {
        let row = Rect::new(rect.x + pad, *y, width, line);
        frame.text(run(
            state,
            Role::Value,
            key,
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

    let footer = Rect::new(rect.x + pad, rect.bottom() - line * 4.0, width, line * 3.0);
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

        // Match count on the right, so `/` tells you whether it found anything before you commit.
        if prompt.is_search() && !prompt.input.is_empty() {
            frame.text(run(
                state,
                Role::Value,
                if prompt.matches.is_empty() {
                    "no matches".to_string()
                } else {
                    format!("{} matches", prompt.matches.len())
                },
                inner,
                if prompt.matches.is_empty() {
                    theme.stale()
                } else {
                    theme.ink_dim
                },
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
        state.mode.label(),
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
            if errors + warnings == 0 {
                (state.server_status.clone(), theme.ink_faint)
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
        frame.text(run(
            state,
            Role::Value,
            content,
            cell,
            color,
            Align::Right,
            scale,
        ));
    }
}
