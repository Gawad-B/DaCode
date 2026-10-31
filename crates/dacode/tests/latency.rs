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
        theme: Theme::walnut(),
        theme_toggled_from: None,
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
        structure: None,
        frame_ms: 0.0,
        worst_ms: 0.0,
        diagnostics: dacode_lsp::Diagnostics::default(),
        server_status: "test".into(),
        palette: None,
        prompt: None,
        results: None,
        search_matches: Vec::new(),
        hover: None,
        signature: None,
        completion: None,
        pending_request: None,
        project_root: std::path::PathBuf::from("."),
        buffer_count: 1,
        buffer_tabs: Vec::new(),
        active_buffer: 0,
        modal: true,
        sticky_scroll: true,
        page_rows: 30,
        bracket_colors: true,
        minimap: false,
        vertical_goal: None,
        expansions: Default::default(),
        search: None,
        scm: Default::default(),
        review: None,
        preview: None,
        recording: None,
        diff: dacode::git::Diff::default(),
        blame: Default::default(),
        show_diagnostics: false,
        focus: Default::default(),
        terminal: Default::default(),
        problems: Default::default(),
        inlay_hints: Vec::new(),
        show_inlay_hints: false,
        welcome: None,
        settings_page: None,
        settings_active: false,
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

/// A blame of every line of `state`'s document: three commits in runs, the shape a real file has.
fn blame_everything(state: &mut EditorState) {
    let lines = state.document.text().len_lines();
    let commit = |hash: char, author: &str| dacode::blame::Commit {
        hash: std::iter::repeat_n(hash, 40).collect(),
        author: author.into(),
        mail: String::new(),
        time: 1_700_000_000,
        summary: "a change".into(),
    };
    let parsed = dacode::blame::Parsed {
        commits: vec![commit('a', "Ann"), commit('b', "Bob"), commit('c', "Cy")],
        lines: (0..lines).map(|line| ((line / 7) % 3) as u32).collect(),
    };
    state.blame.inline = true;
    state.blame.column = true;
    state
        .blame
        .show(dacode::blame::Blame::new("big.rs".into(), parsed));
}

/// A keystroke that adds a line and one that takes a character away, with a whole file's blame
/// held.
///
/// The blame is per line of the file, and the obvious way to keep it right across an edit is to
/// move every entry below the edit — a walk of the document per Enter. It is instead left where it
/// is and the edit's line arithmetic appended to a log, which costs the same whatever the file. Few
/// enough iterations that the log never reaches its cap: past it, recording returns at once, and a
/// guard measuring that early return would pass whatever recording cost.
fn keystroke_with_blame(lines: usize) -> Duration {
    use dacode_input::KeyCode;

    let mut state = state(lines);
    blame_everything(&mut state);
    let mut grammar = Grammar::new();
    let mut registers = Registers::new();

    let taken = median(100, || {
        for key in [Key::named(KeyCode::Enter), Key::named(KeyCode::Backspace)] {
            if let Resolution::Action(action) = grammar.feed(state.mode, key) {
                execute(&mut state, &mut registers, &action, 40);
            }
        }
    });
    assert!(
        state.blame.blame().is_some_and(|blame| !blame.is_lost()),
        "the log reached its cap, so this measured the early return and not the recording"
    );
    taken
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

/// Keeping a blame right across edits follows the edit, never the file.
#[test]
fn a_keystroke_with_a_blame_held_costs_the_same_in_a_huge_file_as_in_a_small_one() {
    let small = keystroke_with_blame(SMALL);
    let large = keystroke_with_blame(LARGE);
    assert_flat("typing with a blame held", small, large);
    assert_under(
        "typing with a blame held",
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

fn minimap_frame_build(lines: usize) -> Duration {
    let mut state = state(lines);
    state.minimap = true;
    // Half way down, so the strip is showing the middle of the file and not its first page — the
    // place an implementation that walked from the top to find its lines would be slowest.
    state.scroll.line = lines / 2;
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
fn the_minimap_draws_what_fits_in_its_strip_and_not_the_file() {
    // A minimap is the one thing on screen that is *about* the whole file, which is exactly why it
    // must not read the whole file: it draws the lines that fit in its height, a line only as far
    // as it is wide, and finds them by arithmetic on the scroll position.
    let small = minimap_frame_build(SMALL);
    let large = minimap_frame_build(LARGE);
    assert_flat("building a frame with the minimap", small, large);
    assert_under(
        "building a frame with the minimap",
        large,
        Duration::from_millis(8),
    );
}

/// A table of `lines` rows, drawn as `data.csv`, so every row on screen is split into columns.
fn csv_frame_build(lines: usize) -> Duration {
    let rows: String = (0..lines)
        .map(|i| format!("{i},\"Name, {i}\",city {i},{},\"say \"\"hi\"\"\"\n", i * 7))
        .collect();
    let mut state = state(SMALL);
    state.document = Document::from_text(dacode_text::Text::from_str(&rows));
    state.document.set_primary_caret(CharOffset(400));
    state.path = vec!["data.csv".into()];
    state.file_path = Some("data.csv".into());
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
fn colouring_a_tables_columns_is_bounded_by_the_viewport_and_not_by_the_file() {
    // The columns are worked out per row as it is drawn and never stored, which is only sound
    // while that work is the viewport's: a parse of the whole table per frame would be the
    // sidebar defect again, and the status line's column name reads two lines, not the file.
    let small = csv_frame_build(SMALL);
    let large = csv_frame_build(LARGE);
    assert_flat("building a frame of a table", small, large);
    assert_under(
        "building a frame of a table",
        large,
        Duration::from_millis(8),
    );
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

    let panels: [Panel; 5] = [
        ("the settings tab", |state| {
            state.settings_page = Some(dacode::settings_ui::Page::new());
            state.settings_active = true;
        }),
        ("the problems list", |state| {
            state.show_diagnostics = true;
        }),
        ("the top row part-scrolled", |state| {
            // One more row drawn and everything clipped: still a screenful, whatever the file.
            let row = state.typography.row_height();
            state.scroll = dacode::scroll::by_pixels(
                &state.display,
                state.document.text(),
                state.scroll,
                &mut state.chrome.scroll_input,
                row * 0.5,
                row,
            );
        }),
        ("the blame column and the caret line's blame", |state| {
            blame_everything(state);
            // A log near its cap, so every row's lookup walks the most it ever will.
            let text = state.document.text().clone();
            for _ in 0..dacode::blame::EditLog::CAP - 8 {
                state
                    .blame
                    .record(&text, &[dacode_text::Edit::insert(CharOffset(0), "\n")]);
            }
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

/// What the syntax tree costs the UI thread: the indentation for Enter, the dedent for a `}`, the
/// type around the cursor, and one step of expand-selection — each from a fresh tree, at the
/// *end* of the document, where anything that walked from the start would pay in full.
///
/// Measured on the functions for the reason `dedent` is: through `execute`, the first iteration's
/// edit makes the tree stale, and every iteration after it would time the version check and the
/// lexical fallback instead of the query.
fn tree_queries(lines: usize) -> [Duration; 4] {
    let text = dacode_text::Text::from_str(&source(lines));
    let mut highlighter =
        dacode_syntax::Highlighter::new(dacode_syntax::Language::Rust).expect("grammar loads");
    highlighter.parse(&text);
    let structure = highlighter.structure(Version(1)).expect("parsed");

    // Inside the last struct, at the end of its first field.
    let last_field = text.len_lines() - 4;
    let caret = text.line_range(last_field).end;
    let closing = text.line_to_char(text.len_lines() - 2);

    let enter = median(2_000, || {
        std::hint::black_box(structure.newline_indent(&text, caret, 4));
    });
    let closer = median(2_000, || {
        std::hint::black_box(structure.closer_indent(&text, closing, '}'));
    });
    let object = median(2_000, || {
        std::hint::black_box(structure.object(&text, caret, dacode_syntax::ObjectKind::Type, true));
    });
    let expand = median(2_000, || {
        std::hint::black_box(structure.expand(&text, caret..caret));
    });
    [enter, closer, object, expand]
}

#[test]
fn asking_the_syntax_tree_is_bounded_by_its_depth_and_not_by_the_file() {
    // Enter, `}`, `daf` and expand-selection all read the tree on the keystroke. Each walks the
    // ancestors of one node and runs its query rooted at each with a start depth of zero; a query
    // cursor over the root instead would visit every node in the file.
    let small = tree_queries(SMALL);
    let large = tree_queries(LARGE);
    let names = [
        "indenting a new line from the tree",
        "dedenting a closer from the tree",
        "finding the type around the cursor",
        "expanding the selection",
    ];
    for ((what, small), large) in names.iter().zip(small).zip(large) {
        assert_flat(what, small, large);
        assert_under(what, large, Duration::from_millis(2));
    }
}

/// A touchpad burst: two hundred three-pixel deltas, through the same function the wheel handler
/// calls, from the same place in each document.
///
/// From a fresh anchor every time rather than carrying on from where the last burst stopped: carried
/// on, the small document would reach its last line within a few iterations and every one after that
/// would time the clamp at the bottom instead of the scrolling.
fn touchpad_burst(lines: usize) -> Duration {
    let state = state(lines);
    let text = state.document.text().clone();
    let row = state.typography.row_height();

    median(400, || {
        let mut input = dacode::scroll::ScrollInput::default();
        let mut anchor = ScrollAnchor::at_line(20);
        for _ in 0..200 {
            anchor = dacode::scroll::by_pixels(&state.display, &text, anchor, &mut input, 3.0, row);
        }
        std::hint::black_box((anchor, input.remainder()));
    })
}

#[test]
fn a_touchpad_burst_costs_the_same_in_a_huge_file_as_in_a_small_one() {
    // A touchpad sends a delta every few milliseconds for as long as the fingers move, and each one
    // now does real work — whole rows, the remainder, the clamp at either end. Any of that reaching
    // for the size of the document (counting its rows to clamp the bottom, say) would be paid dozens
    // of times a second.
    let small = touchpad_burst(SMALL);
    let large = touchpad_burst(LARGE);

    assert_flat("a burst of touchpad deltas", small, large);
    // Two hundred deltas is a long swipe, measured at about 0.35 ms for all of them. Five is far past
    // anything a slow machine explains, and far short of a document walk repeated two hundred times.
    assert_under(
        "a burst of touchpad deltas",
        large,
        Duration::from_millis(5),
    );
}

/// Where the palette's card and visible rows are, for a list of `rows` symbols with the selection
/// half-way down. Drawing asks for this every frame and the pointer twice per move.
fn palette_geometry(rows: usize) -> Duration {
    use dacode::palette::{Palette, PaletteKind, Row, Target};
    let mut palette = Palette::new(PaletteKind::Symbols);
    palette.rows = (0..rows)
        .map(|index| Row {
            name: format!("symbol_{index}"),
            context: "fn".into(),
            score: 0,
            highlights: Vec::new(),
            target: Target::Symbol(index),
            heading: None,
        })
        .collect();
    palette.selected = rows / 2;
    let bounds = Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1);
    median(2_000, || {
        let geometry = palette.geometry(bounds, None, 1.0);
        std::hint::black_box(geometry.rows.len());
    })
}

/// A file's outline can run to thousands of symbols, and the palette's geometry once laid out every
/// one of them to find the dozen on screen — per frame, and twice per pointer move.
#[test]
fn the_palette_lays_out_the_rows_on_screen_and_not_the_whole_list() {
    assert_flat(
        "laying out the palette",
        palette_geometry(SMALL),
        palette_geometry(LARGE),
    );
}

/// The settings page's geometry for a Languages page carrying `rows` reports, scrolled half-way.
/// Drawing asks for it every frame, and every frame of a scroll spring.
fn settings_geometry(rows: usize) -> Duration {
    use dacode::settings_ui::{Category, Page, ServerRow, Values};
    let report = |index: usize| ServerRow {
        language: format!("language {index}"),
        program: "server".into(),
        status: "/usr/bin/server".into(),
        found: true,
        configured: false,
    };
    let mut page = Page::new();
    page.set_reports((0..rows).map(report).collect(), Vec::new(), true);
    page.set_category(Category::Languages, true);
    let bounds = Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1);
    let typography = Typography::default();
    let values = Values::defaults(true);
    let half = page.max_scroll(bounds, 1.0, true, &typography) / 2.0;
    median(2_000, || {
        let geometry = page.geometry_at(half, bounds, 1.0, &values, true, &typography);
        std::hint::black_box(geometry.rows.len());
    })
}

/// The page's list, heights and match flags are built when the page, query, grammar or width
/// changes, and a frame lays out only the rows on screen. A list of a hundred reports and one of a
/// hundred thousand must cost a frame the same — which they could not if the list were rebuilt per
/// call, as the first version of the page did.
#[test]
fn the_settings_page_lays_out_the_rows_on_screen_and_not_the_whole_list() {
    assert_flat(
        "laying out the settings page",
        settings_geometry(SMALL / 10),
        settings_geometry(LARGE),
    );
}

/// A frame with the terminal open over `history` rows of scrollback, scrolled half-way back: the
/// copy of the visible rows the event loop makes and the drawing of them, together.
fn terminal_frame(history: usize) -> Duration {
    use dacode_terminal::{Emulator, GridSize};
    let mut emulator = Emulator::new(GridSize::new(120, 20), history);
    let mut bytes = Vec::new();
    for n in 0..history {
        bytes
            .extend_from_slice(format!("\x1b[32mline\x1b[0m {n} of the scrollback\r\n").as_bytes());
    }
    emulator.feed(&bytes);
    emulator.scroll((history / 2) as i32);

    let mut state = state(SMALL);
    state.terminal.open = true;
    let mut frame = Frame::default();
    let bounds = Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1);
    median(200, || {
        state.terminal.screen = Some(emulator.screen());
        frame.clear();
        chrome::draw(&mut frame, &state, bounds, 1.0);
    })
}

/// A terminal that has printed a hundred thousand lines costs a frame what one that has printed a
/// thousand does. The copy addresses rows from the display offset and never walks the history;
/// a walk over it, injected to check this guard, failed it at 9.7x (1.9 ms against 200 µs).
#[test]
fn the_terminal_copies_and_draws_the_rows_on_screen_and_not_its_history() {
    let small = terminal_frame(SMALL);
    let large = terminal_frame(LARGE);
    assert_flat("a frame with the terminal open", small, large);
    assert_under(
        "a frame with the terminal open",
        large,
        Duration::from_millis(50),
    );
}
