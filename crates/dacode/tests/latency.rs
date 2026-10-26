//! The one rule, as a test that fails the build.
//!
//! `cargo bench` has measured the UI thread since M2 and nothing has ever *failed* because of what
//! it measured. Criterion compares against the last run on the same machine, which means a
//! regression is caught only if somebody happens to run it, notices the red number, and does not
//! assume it was their laptop. The one real performance defect this project has found — a sidebar
//! walking 512 display rows to compute one coordinate, 1.13 ms per frame — was found by reading
//! code, not by the benchmark that was already measuring it.
//!
//! **The contract is flatness, not a number.** An absolute millisecond budget is a property of the
//! machine: a loaded CI runner is three times slower than a quiet laptop at everything, so any
//! threshold tight enough to catch a real regression is also tight enough to fail at random on a
//! busy Tuesday. What the one rule actually says is that cost must not grow with the document — so
//! that is what is asserted. A ratio cancels the machine out.
//!
//! Absolute ceilings are here too, but set where only a catastrophe trips them: something that
//! turned a keystroke into whole-document work would blow past them on any hardware, and nothing
//! else will.
//!
//! Timing in a test is inherently noisy, so every measurement is a **median** of many runs. A
//! median ignores the scheduler stealing one iteration; a mean does not.

use dacode::chrome::{self, EditorState};
use dacode::execute::execute;
use dacode_buffer::Document;
use dacode_display::{DisplayMap, ScrollAnchor};
use dacode_input::{Grammar, Key, Registers, Resolution};
use dacode_render::{Frame, Rect};
use dacode_text::{CharOffset, Version};
use dacode_theme::{Mode, Theme, Typography};
use std::time::{Duration, Instant};

/// The two sizes every scaling assertion compares. A hundredfold difference in document size, so
/// anything accidentally linear in the document shows up as a hundredfold difference in time.
const SMALL: usize = 1_000;
const LARGE: usize = 100_000;

/// How much slower the large document may be. Three, not one: a bigger rope is genuinely a little
/// slower — deeper tree, worse cache locality — and the number being asserted is "not proportional
/// to the document", not "identical".
const SCALE_LIMIT: f64 = 3.0;

/// A screenful at a typical window size.
const WINDOW: (f32, f32) = (1600.0, 1000.0);

fn source(lines: usize) -> String {
    (0..lines / 6)
        .map(|i| {
            format!(
                "/// Item {i}.\n\
                 #[derive(Debug, Clone)]\n\
                 pub struct Item{i} {{\n\
                 \x20   pub name: String,\n\
                 \x20   pub count: usize,\n\
                 }}\n"
            )
        })
        .collect()
}

fn state(lines: usize) -> EditorState {
    let text = dacode_text::Text::from_str(&source(lines));
    let mut document = Document::from_text(text);
    document.set_primary_caret(CharOffset(400));

    EditorState {
        mode: Mode::Insert,
        theme: Theme::dark(),
        typography: Typography::default(),
        document,
        pairs: dacode_buffer::PairConfig::default(),
        format_on_save: false,
        display: DisplayMap::default(),
        scroll: ScrollAnchor::at_line(20),
        path: vec!["bench.rs".into()],
        file_path: None,
        pending: String::new(),
        syntax_version: Version(1),
        diagnostics_version: Version(1),
        highlights: Vec::new(),
        node_path: Vec::new(),
        syntax_error: false,
        frame_ms: 0.0,
        worst_ms: 0.0,
        diagnostics: dacode_lsp::Diagnostics::default(),
        server_status: "test".into(),
        palette: None,
        prompt: None,
        results: None,
        search_matches: Vec::new(),
        hover: None,
        completion: None,
        pending_request: None,
        project_root: std::path::PathBuf::from("."),
        buffer_count: 1,
        buffer_tabs: Vec::new(),
        active_buffer: 0,
        modal: true,
        sticky_scroll: true,
        recording: None,
        diff: dacode::git::Diff::default(),
        show_diagnostics: false,
        focus: Default::default(),
        problems: Default::default(),
        inlay_hints: Vec::new(),
        show_inlay_hints: false,
        welcome: None,
        settings: None,
        setting_sources: std::collections::HashMap::new(),
        tree: None,
        autosave: false,
        line_numbers: dacode::config::LineNumbers::Relative,
        pointer: (0.0, 0.0),
        help: None,
        menu: None,
        chrome: Default::default(),
    }
}

/// The median time of `runs` iterations, after discarding a warm-up.
///
/// The warm-up matters more than it looks: the first frame builds a text-shaping cache, so timing
/// it would measure the cache being filled rather than the work that repeats.
fn median(runs: usize, mut body: impl FnMut()) -> Duration {
    for _ in 0..(runs / 4).max(4) {
        body();
    }
    let mut samples: Vec<Duration> = (0..runs)
        .map(|_| {
            let at = Instant::now();
            body();
            at.elapsed()
        })
        .collect();
    samples.sort();
    samples[samples.len() / 2]
}

/// Asserts that going from a thousand lines to a hundred thousand did not change the cost.
///
/// The message names both numbers, because "it regressed" is not something anyone can act on and
/// "84 µs became 1.13 ms" is.
fn assert_flat(what: &str, small: Duration, large: Duration) {
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(f64::EPSILON);
    assert!(
        ratio <= SCALE_LIMIT,
        "{what} is {ratio:.1}x slower on a {LARGE}-line file than on a {SMALL}-line one \
         ({:.0} µs vs {:.0} µs). The one rule this project has is that nothing unbounded runs on \
         the UI thread — something here is now proportional to the document rather than to the \
         viewport.",
        large.as_secs_f64() * 1e6,
        small.as_secs_f64() * 1e6,
    );
}

/// A ceiling only a catastrophe trips, so it does not go off on a loaded runner.
fn assert_under(what: &str, taken: Duration, budget: Duration) {
    assert!(
        taken <= budget,
        "{what} took {:.1} ms, past the {:.0} ms ceiling. That is far beyond anything a slow \
         machine explains — this is whole-document work on the thread that draws.",
        taken.as_secs_f64() * 1e3,
        budget.as_secs_f64() * 1e3,
    );
}

fn keystroke(lines: usize) -> Duration {
    let mut state = state(lines);
    let mut grammar = Grammar::new();
    let mut registers = Registers::new();

    median(2_000, || {
        if let Resolution::Action(action) = grammar.feed(state.mode, Key::char('x')) {
            execute(&mut state, &mut registers, &action, 40);
        }
    })
}

/// A keystroke with a whole document's worth of search matches live.
///
/// This exists to refuse an obvious-looking fix. The highlight spans drift across an edit unless
/// `mutate` moves them, and the search matches — plain char ranges in the same state — drift for
/// exactly the same reason, so moving them there too looks like one more line of the same repair.
/// It is not: the spans cover the *viewport*, while the matches cover the *file*, and a common word
/// in a hundred-thousand-line document is tens of thousands of ranges. Written that way this
/// measured 311 µs against 13 µs, 24.6x, and the matches are left drifting instead — stated in
/// `mutate` and in the known gaps — until they can be moved lazily, at the viewport for drawing and
/// once per `n` for stepping.
fn keystroke_while_searching(lines: usize) -> Duration {
    let mut state = state(lines);
    // Spans for a screenful, which is what a parse of the visible range returns.
    state.highlights = (0..240)
        .map(|i| dacode_syntax::HighlightSpan {
            range: CharOffset(i * 7)..CharOffset(i * 7 + 4),
            highlight: dacode_syntax::Highlight::Keyword,
        })
        .collect();
    let mut grammar = Grammar::new();
    let mut registers = Registers::new();

    // One match every sixty characters, which is denser than a real search of a real word.
    let len = state.document.text().len_chars().get();
    state.search_matches = (0..len.saturating_sub(8))
        .step_by(60)
        .map(|at| CharOffset(at)..CharOffset(at + 4))
        .collect();

    median(2_000, || {
        if let Resolution::Action(action) = grammar.feed(state.mode, Key::char('x')) {
            execute(&mut state, &mut registers, &action, 40);
        }
    })
}

/// A keystroke with `count` diagnostics live, in a file of `lines` lines.
///
/// Both knobs, because the two failures are different. Moving a diagnostic must not cost a rope
/// lookup — with one it does, and the *file* size starts driving the keystroke. And however many
/// arrive, the total must stay far under a frame: a server is entitled to report a diagnostic per
/// line of a file mid-refactor, and nothing in the protocol caps it.
fn keystroke_with_diagnostics(lines: usize, count: usize) -> Duration {
    use dacode_lsp::lsp_types::{Diagnostic, Position, Range};

    let mut state = state(lines);
    let mut grammar = Grammar::new();
    let mut registers = Registers::new();

    let step = (lines / count.max(1)).max(1);
    let items: Vec<Diagnostic> = (0..count)
        .map(|i| Diagnostic {
            range: Range {
                start: Position {
                    line: (i * step) as u32,
                    character: 4,
                },
                end: Position {
                    line: (i * step) as u32,
                    character: 8,
                },
            },
            message: "something".into(),
            ..Default::default()
        })
        .collect();
    state
        .diagnostics
        .apply(items, None, state.document.version());

    median(2_000, || {
        if let Resolution::Action(action) = grammar.feed(state.mode, Key::char('x')) {
            execute(&mut state, &mut registers, &action, 40);
        }
    })
}

fn frame_build(lines: usize) -> Duration {
    let state = state(lines);
    let mut frame = Frame::default();

    median(200, || {
        frame.clear();
        chrome::draw(
            &mut frame,
            &state,
            Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1),
            1.0,
        );
    })
}

#[test]
fn a_keystroke_costs_the_same_in_a_huge_file_as_in_a_small_one() {
    let small = keystroke(SMALL);
    let large = keystroke(LARGE);

    assert_flat("typing a character", small, large);
    // 16.6 ms is one frame at 60fps; a *keystroke* getting anywhere near a whole frame on its own
    // is already a failure, and this is thirty times the measured cost on ordinary hardware.
    assert_under("typing a character", large, Duration::from_micros(500));
}

/// Moving the diagnostics over an edit follows how many there are, never how large the file is.
///
/// The first version of `Diagnostics::transform` converted each range to an offset and back, which
/// is two rope lookups per diagnostic: a keystroke in a hundred-thousand-line file carrying ten
/// thousand of them cost **39.6 ms**, more than two frames, for one character. Doing the arithmetic
/// in the diagnostics' own line-and-column coordinates instead — one conversion per *edit*, integers
/// after that — took the same keystroke to about a tenth of a millisecond. Both halves of that are
/// asserted: the same pile of diagnostics costs the same in a large file as in a small one, and even
/// a diagnostic every ten lines of a huge file stays far inside a frame.
#[test]
fn moving_diagnostics_follows_their_count_and_not_the_file() {
    let small = keystroke_with_diagnostics(SMALL, 500);
    let large = keystroke_with_diagnostics(LARGE, 500);
    assert_flat("typing with 500 diagnostics live", small, large);

    // Ten thousand of them. The rope-lookup version measured 39.6 ms here, so this is set where a
    // return to it fails outright rather than where the current cost happens to sit.
    let pathological = keystroke_with_diagnostics(LARGE, LARGE / 10);
    assert_under(
        "typing with a diagnostic every ten lines",
        pathological,
        Duration::from_millis(1),
    );
}

/// Moving the highlight spans over an edit is bounded, and walking the match list would not be.
#[test]
fn a_keystroke_with_a_search_live_stays_bounded() {
    let small = keystroke_while_searching(SMALL);
    let large = keystroke_while_searching(LARGE);

    assert_flat("typing with a search live", small, large);
    assert_under(
        "typing with a search live",
        large,
        Duration::from_micros(500),
    );
}

#[test]
fn building_a_frame_is_bounded_by_the_viewport_and_not_by_the_file() {
    // The defect this exists for, exactly: a sidebar walked 512 display rows to compute one
    // coordinate and cost 1.13 ms a frame. Bounding it to the ~40 rows on screen took it to 84 µs.
    // Nothing failed at the time — the benchmark measured it and said nothing, because nothing was
    // comparing the two sizes to each other.
    let small = frame_build(SMALL);
    let large = frame_build(LARGE);

    assert_flat("building a frame", small, large);
    // Half a frame at 60fps. A frame build past this leaves nothing for the GPU.
    assert_under("building a frame", large, Duration::from_millis(8));
}

#[test]
fn scrolling_walks_the_viewport_and_not_the_document() {
    let rows = |lines: usize| {
        let state = state(lines);
        let text = state.document.text().clone();
        median(5_000, || {
            let visible = state.display.rows_from(&text, state.scroll, 40);
            std::hint::black_box(visible.len());
        })
    };

    assert_flat("taking a screenful of rows", rows(SMALL), rows(LARGE));
}

#[test]
fn every_frame_a_panel_adds_is_still_bounded_by_the_viewport() {
    // Each of these was a separate opportunity to walk the whole document once per frame, and the
    // panels added this session — the settings report, the problems list — are the newest. Measured
    // one at a time so a failure names which one, rather than "the frame got slower".
    let with = |lines: usize, arrange: fn(&mut EditorState)| {
        let mut state = state(lines);
        arrange(&mut state);
        let mut frame = Frame::default();
        median(200, || {
            frame.clear();
            chrome::draw(
                &mut frame,
                &state,
                Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1),
                1.0,
            );
        })
    };

    /// A named way of putting the editor into a state that draws one more thing.
    type Panel = (&'static str, fn(&mut EditorState));

    let panels: [Panel; 3] = [
        ("the settings panel", |state| {
            state.settings = Some(dacode::settings_ui::Settings::new());
        }),
        ("the problems list", |state| {
            state.show_diagnostics = true;
        }),
        ("a search with matches on screen", |state| {
            let text = state.document.text().clone();
            state.search_matches =
                dacode_buffer::find_all(&text, "pub", &dacode_buffer::SearchOptions::default());
        }),
    ];

    for (what, arrange) in panels {
        assert_flat(
            &format!("building a frame with {what}"),
            with(SMALL, arrange),
            with(LARGE, arrange),
        );
    }
}

/// Electric dedent: what `}` on a blank line costs, measured against the *end* of the document.
///
/// Measured on the function rather than through `execute`, deliberately. Routing it through a
/// keystroke looks more realistic and cannot fail: `median` reuses one `EditorState`, so the first
/// iteration inserts the `}` and every one after it finds a non-blank line before the caret and
/// returns three lines in. The scan under test would never run again, and restoring the
/// whole-document walk would leave the test green — a guard that checks nothing, which is the exact
/// failure `conventions.rs` records.
fn dedent(lines: usize) -> Duration {
    let text = dacode_text::Text::from_str(&source(lines));
    let config = dacode_buffer::PairConfig::default();
    // A blank line at the very end: the worst case, and the one a walk back to the start of the
    // document pays in full.
    let caret = text.len_chars();

    median(2_000, || {
        std::hint::black_box(dacode_buffer::pairs::dedent_for_closer(
            &text, caret, '}', &config,
        ));
    })
}

#[test]
fn typing_a_closing_brace_costs_the_same_in_a_huge_file_as_in_a_small_one() {
    // It did not. Finding the `{` a `}` belongs to walked back to the start of the document, on the
    // UI thread, from a keystroke — and paid a fresh rope descent per character on the way, so it
    // was worse than linear. Nothing here caught it, because the keystroke test types `x`, which is
    // not a closer and returns before the scan.
    let small = dedent(SMALL);
    let large = dedent(LARGE);

    assert_flat("typing a closing brace", small, large);
    assert_under("typing a closing brace", large, Duration::from_micros(500));
}
