//! Syntax benchmarks.
//!
//! These exist to make the 60fps contract measurable rather than aspirational. The numbers that
//! matter are the *per-keystroke* ones: an incremental reparse plus a viewport-sized query is
//! what happens between pressing a key and seeing the result.

use criterion::{Criterion, criterion_group, criterion_main};
use dacode_syntax::{Highlighter, Language};
use dacode_text::{CharOffset, Edit, Text, Transaction};
use std::hint::black_box;
use std::time::Duration;

/// A realistic Rust file of roughly `lines` lines.
fn source(lines: usize) -> String {
    let mut out = String::new();
    for i in 0..lines / 6 {
        out.push_str(&format!(
            "/// Item {i}.\n\
             #[derive(Debug, Clone)]\n\
             pub struct Item{i} {{\n\
             \x20   pub name: String,\n\
             \x20   pub count: usize,\n\
             }}\n"
        ));
    }
    out
}

fn parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));

    for lines in [1_000usize, 20_000] {
        let text = Text::from_str(&source(lines));

        // Cold parse: the cost paid once when a file is opened.
        group.bench_function(format!("cold/{lines}_lines"), |b| {
            b.iter(|| {
                let mut highlighter = Highlighter::new(Language::Rust).unwrap();
                highlighter.parse(black_box(&text));
            });
        });

        // Incremental reparse: the cost paid on every keystroke. This is the one that has to stay
        // far under the frame budget.
        group.bench_function(format!("keystroke/{lines}_lines"), |b| {
            let mut highlighter = Highlighter::new(Language::Rust).unwrap();
            highlighter.parse(&text);

            let mut current = text.clone();
            b.iter(|| {
                let before = current.clone();
                let at = CharOffset(current.len_chars().get() / 2);
                let edits = vec![Edit::insert(at, "x")];
                Transaction::new(edits.clone()).apply(&mut current).unwrap();

                highlighter.edit(&before, &current, &edits);
                highlighter.parse(black_box(&current));
            });
        });
    }

    group.finish();
}

/// Attributes keystroke cost without measuring destructor time.
///
/// `iter_batched` includes dropping the input in its measurement, and dropping a `Highlighter`
/// frees a whole syntax tree plus a compiled query — which swamps the operation under test. So a
/// single long-lived highlighter is reused, and each iteration does an insert *and* the matching
/// delete, returning the document to its starting state.
fn keystroke_parts(c: &mut Criterion) {
    let mut group = c.benchmark_group("keystroke_parts");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));

    let lines = 20_000usize;
    let base = Text::from_str(&source(lines));
    let at = CharOffset(base.len_chars().get() / 2);

    // Tree bookkeeping alone: two `Tree::edit` calls, no parsing.
    group.bench_function("tree_edit_x2", |b| {
        let mut highlighter = Highlighter::new(Language::Rust).unwrap();
        highlighter.parse(&base);

        let mut typed = base.clone();
        let insert = vec![Edit::insert(at, "x")];
        Transaction::new(insert.clone()).apply(&mut typed).unwrap();
        let remove = vec![Edit::delete(at..CharOffset(at.get() + 1))];

        b.iter(|| {
            highlighter.edit(&base, &typed, &insert);
            highlighter.edit(&typed, &base, &remove);
            black_box(&highlighter);
        });
    });

    // A full round trip: type a character and delete it again, tree edits and reparses included.
    // Two keystrokes per iteration.
    group.bench_function("typing_cycle_x2", |b| {
        let mut highlighter = Highlighter::new(Language::Rust).unwrap();
        highlighter.parse(&base);

        let mut typed = base.clone();
        let insert = vec![Edit::insert(at, "x")];
        Transaction::new(insert.clone()).apply(&mut typed).unwrap();
        let remove = vec![Edit::delete(at..CharOffset(at.get() + 1))];

        b.iter(|| {
            highlighter.edit(&base, &typed, &insert);
            highlighter.parse(&typed);
            highlighter.edit(&typed, &base, &remove);
            highlighter.parse(black_box(&base));
        });
    });

    group.finish();
}

fn querying(c: &mut Criterion) {
    let mut group = c.benchmark_group("highlight");
    group
        .sample_size(30)
        .measurement_time(Duration::from_secs(5));

    for lines in [1_000usize, 20_000] {
        let text = Text::from_str(&source(lines));
        let mut highlighter = Highlighter::new(Language::Rust).unwrap();
        highlighter.parse(&text);

        // A viewport-sized query: what the editor actually asks for each frame's worth of work.
        let window_end = CharOffset(text.len_chars().get().min(3_000));
        group.bench_function(format!("viewport/{lines}_lines"), |b| {
            b.iter(|| {
                black_box(highlighter.highlights(&text, CharOffset(0)..window_end));
            });
        });

        // Whole document: not something the editor does, but it exposes any per-call cost that
        // scales with file size rather than with the requested range.
        group.bench_function(format!("whole_file/{lines}_lines"), |b| {
            b.iter(|| {
                black_box(highlighter.highlights(&text, CharOffset(0)..text.len_chars()));
            });
        });
    }

    group.finish();
}

criterion_group!(benches, parsing, keystroke_parts, querying);
criterion_main!(benches);
