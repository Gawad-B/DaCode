//! What a keystroke costs the **UI thread**.
//!
//! This is the number the 60fps contract is about. Background work (parsing, LSP, search) is
//! explicitly allowed to be slower than a frame, because it runs off-thread and its results are
//! version-tagged. What may never exceed the budget is the path from key press to painted frame.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;
use vex::chrome::{self, EditorState};
use vex::execute::execute;
use vex_buffer::Document;
use vex_display::{DisplayMap, ScrollAnchor};
use vex_input::{Grammar, Key, Registers, Resolution};
use vex_render::{Frame, Rect};
use vex_text::{CharOffset, Version};
use vex_theme::{Mode, Theme, Typography};

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
    let text = vex_text::Text::from_str(&source(lines));
    let mut document = Document::from_text(text);
    document.set_primary_caret(CharOffset(400));

    EditorState {
        mode: Mode::Insert,
        theme: Theme::dark(),
        typography: Typography::default(),
        document,
        pairs: vex_buffer::PairConfig::default(),
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
        diagnostics: vex_lsp::Diagnostics::default(),
        server_status: "bench".into(),
        palette: None,
        prompt: None,
        results: None,
        search_matches: Vec::new(),
        hover: None,
        completion: None,
        pending_request: None,
        project_root: std::path::PathBuf::from("."),
        buffer_count: 1,
        active_buffer: 0,
        tree: None,
    }
}

/// A screenful at a typical window size.
const WINDOW: (f32, f32) = (1600.0, 1000.0);

fn keystroke(c: &mut Criterion) {
    let mut group = c.benchmark_group("ui_thread");
    group
        .sample_size(50)
        .measurement_time(Duration::from_secs(5));

    for lines in [1_000usize, 100_000] {
        // Typing: resolve the key through the grammar, then apply it to the document.
        group.bench_function(format!("keystroke/{lines}_lines"), |b| {
            let mut state = state(lines);
            let mut grammar = Grammar::new();
            let mut registers = Registers::new();

            b.iter(|| {
                let resolution = grammar.feed(state.mode, Key::char('x'));
                if let Resolution::Action(action) = resolution {
                    execute(&mut state, &mut registers, &action, 40);
                }
                black_box(state.document.version());
            });
        });

        // Building the frame: layout plus text-run construction for every visible row. No GPU.
        group.bench_function(format!("frame_build/{lines}_lines"), |b| {
            let state = state(lines);
            let mut frame = Frame::default();

            b.iter(|| {
                frame.clear();
                chrome::draw(
                    &mut frame,
                    &state,
                    Rect::new(0.0, 0.0, WINDOW.0, WINDOW.1),
                    1.0,
                );
                black_box(frame.quads.len() + frame.texts.len());
            });
        });

        // Scrolling: the viewport walk, which must be bounded by rows on screen, not file size.
        group.bench_function(format!("scroll/{lines}_lines"), |b| {
            let state = state(lines);
            let text = state.document.text();

            b.iter(|| {
                let rows = state.display.rows_from(text, state.scroll, 40);
                black_box(rows.len());
            });
        });
    }

    group.finish();
}

criterion_group!(benches, keystroke);
criterion_main!(benches);
