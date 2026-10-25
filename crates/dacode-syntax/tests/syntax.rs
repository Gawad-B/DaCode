//! Highlighting, incremental reparse, and the version-tagging contract.

use dacode_syntax::{Highlight, Highlighter, Language, SyntaxRequest, SyntaxWorker};
use dacode_text::{CharOffset, Edit, Text, Transaction, Version};

fn rust(source: &str) -> (Highlighter, Text) {
    let text = Text::from_str(source);
    let mut highlighter = Highlighter::new(Language::Rust).expect("the Rust grammar loads");
    assert!(highlighter.parse(&text), "parsing succeeds");
    (highlighter, text)
}

/// The highlight covering a given offset, if any.
fn highlight_at(highlighter: &Highlighter, text: &Text, offset: usize) -> Option<Highlight> {
    highlighter
        .highlights(text, CharOffset(0)..text.len_chars())
        .into_iter()
        .find(|span| span.range.start.get() <= offset && offset < span.range.end.get())
        .map(|span| span.highlight)
}

#[test]
fn language_is_detected_from_the_extension() {
    assert_eq!(Language::from_path("src/main.rs"), Some(Language::Rust));
    assert_eq!(Language::from_path("README.md"), None);
}

#[test]
fn keywords_types_and_functions_are_highlighted() {
    let source = "fn main() { let x: u32 = 1; }";
    let (highlighter, text) = rust(source);

    assert_eq!(
        highlight_at(&highlighter, &text, 0),
        Some(Highlight::Keyword),
        "fn"
    );
    assert_eq!(
        highlight_at(&highlighter, &text, 3),
        Some(Highlight::Function),
        "main"
    );
    assert_eq!(
        highlight_at(&highlighter, &text, 19),
        Some(Highlight::Type),
        "u32"
    );
}

#[test]
fn comments_and_strings_are_distinguished() {
    let source = "// note\nlet s = \"text\";\n";
    let (highlighter, text) = rust(source);

    assert_eq!(
        highlight_at(&highlighter, &text, 2),
        Some(Highlight::Comment)
    );
    assert_eq!(
        highlight_at(&highlighter, &text, 17),
        Some(Highlight::String)
    );
}

#[test]
fn comments_are_the_prose_role() {
    // The design sets comments in italic serif; this is the flag the chrome keys off.
    assert!(Highlight::Comment.is_prose());
    assert!(!Highlight::Keyword.is_prose());
}

#[test]
fn capture_names_collapse_onto_theme_roles() {
    // Queries produce far finer captures than the theme distinguishes. Prefix matching means a
    // grammar with more specific names still lands somewhere sensible.
    assert_eq!(
        Highlight::from_capture("function.method"),
        Highlight::Function
    );
    assert_eq!(
        Highlight::from_capture("keyword.control"),
        Highlight::Keyword
    );
    assert_eq!(Highlight::from_capture("type.builtin"), Highlight::Type);
    assert_eq!(Highlight::from_capture("string.special"), Highlight::String);
    assert_eq!(
        Highlight::from_capture("something.unknown"),
        Highlight::Text,
        "unrecognized captures fall back rather than panicking"
    );
}

#[test]
fn spans_never_overlap() {
    // Two spans claiming the same characters would render one glyph twice, in two colors.
    let source = "fn main() { let s: String = String::from(\"x\"); }";
    let (highlighter, text) = rust(source);
    let spans = highlighter.highlights(&text, CharOffset(0)..text.len_chars());

    for pair in spans.windows(2) {
        assert!(
            pair[0].range.end <= pair[1].range.start,
            "overlapping spans: {:?} and {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn highlighting_is_bounded_by_the_requested_range() {
    // The viewport bounds the work, not the file size.
    let source = (0..500)
        .map(|i| format!("fn f{i}() {{ let x: u32 = {i}; }}\n"))
        .collect::<String>();
    let (highlighter, text) = rust(&source);

    let all = highlighter.highlights(&text, CharOffset(0)..text.len_chars());
    let window = highlighter.highlights(&text, CharOffset(0)..CharOffset(60));

    assert!(!window.is_empty());
    assert!(
        window.len() < all.len() / 10,
        "a small range should produce far fewer spans: {} vs {}",
        window.len(),
        all.len()
    );
}

#[test]
fn an_incremental_edit_keeps_highlighting_correct() {
    // The core of M4: after edit + reparse, the highlights must describe the *new* text.
    let source = "fn main() {}\n";
    let (mut highlighter, before) = rust(source);

    let mut after = before.clone();
    let edits = vec![Edit::insert(CharOffset(11), "\n    let x: u32 = 1;\n")];
    Transaction::new(edits.clone()).apply(&mut after).unwrap();

    highlighter.edit(&before, &after, &edits);
    assert!(highlighter.parse(&after));

    let text = after.to_string();
    let let_offset = text.find("let").unwrap();
    let u32_offset = text.find("u32").unwrap();

    assert_eq!(
        highlight_at(&highlighter, &after, let_offset),
        Some(Highlight::Keyword),
        "the inserted `let` is highlighted"
    );
    assert_eq!(
        highlight_at(&highlighter, &after, u32_offset),
        Some(Highlight::Type),
        "and so is the inserted type"
    );
}

#[test]
fn incremental_and_from_scratch_parses_agree() {
    // If they diverge, the incremental path is silently wrong — the worst kind of bug here,
    // because the colors merely look plausible.
    let before_source = "fn main() { let a = 1; }\n";
    let (mut incremental, before) = rust(before_source);

    let mut after = before.clone();
    let edits = vec![Edit::replace(
        CharOffset(16)..CharOffset(17),
        "value: String",
    )];
    Transaction::new(edits.clone()).apply(&mut after).unwrap();

    incremental.edit(&before, &after, &edits);
    incremental.parse(&after);

    let (fresh, _) = rust(&after.to_string());

    let range = CharOffset(0)..after.len_chars();
    assert_eq!(
        incremental.highlights(&after, range.clone()),
        fresh.highlights(&after, range),
        "incremental reparse must match a full reparse"
    );
}

#[test]
fn multi_edit_transactions_reparse_correctly() {
    // Multi-cursor editing produces several edits in one transaction, each shifting the next.
    let (mut highlighter, before) = rust("let a = 1;\nlet b = 2;\n");
    let mut after = before.clone();
    let edits = vec![
        Edit::insert(CharOffset(0), "pub "),
        Edit::insert(CharOffset(11), "pub "),
    ];
    Transaction::new(edits.clone()).apply(&mut after).unwrap();

    highlighter.edit(&before, &after, &edits);
    highlighter.parse(&after);

    let (fresh, _) = rust(&after.to_string());
    let range = CharOffset(0)..after.len_chars();
    assert_eq!(
        highlighter.highlights(&after, range.clone()),
        fresh.highlights(&after, range)
    );
}

#[test]
fn broken_code_still_highlights() {
    // tree-sitter is error-tolerant on purpose: half-typed code is the normal state of a file
    // being edited, and losing all color while typing would be worse than a wrong color.
    let (highlighter, text) = rust("fn main() { let x: u3");
    assert!(highlighter.has_error(), "the parse should report an error");

    let spans = highlighter.highlights(&text, CharOffset(0)..text.len_chars());
    assert!(!spans.is_empty(), "highlighting must survive a parse error");
    assert_eq!(
        highlight_at(&highlighter, &text, 0),
        Some(Highlight::Keyword)
    );
}

#[test]
fn unicode_positions_survive_the_byte_round_trip() {
    // tree-sitter speaks bytes, the editor speaks chars. A mismatch shifts every highlight after
    // the first multi-byte character — exactly the bug typed coordinates exist to prevent.
    let source = "// héllo 👋\nfn main() {}\n";
    let (highlighter, text) = rust(source);

    let fn_offset = source.chars().position(|c| c == 'f').unwrap();
    assert_eq!(
        highlight_at(&highlighter, &text, fn_offset),
        Some(Highlight::Keyword),
        "`fn` after multi-byte characters must still be found"
    );

    for span in highlighter.highlights(&text, CharOffset(0)..text.len_chars()) {
        assert!(
            span.range.end <= text.len_chars(),
            "span past the end of the document: {span:?}"
        );
    }
}

#[test]
fn the_node_path_is_the_ancestor_chain() {
    // The structure sidebar shows this, and it is what structural text objects are built on.
    let (highlighter, text) = rust("fn main() { let x = 1; }");
    let path = highlighter.node_path(&text, CharOffset(16)); // inside `let`

    assert_eq!(path.first().map(String::as_str), Some("source_file"));
    assert!(
        path.iter().any(|kind| kind == "function_item"),
        "the chain should pass through the function: {path:?}"
    );
    assert!(path.len() >= 3, "outermost to innermost: {path:?}");
}

#[test]
fn enclosing_node_finds_structural_regions() {
    // `if`/`ic` — a function or class as a text object. Plain Vim cannot express this.
    let source = "fn outer() {\n    fn inner() { let x = 1; }\n}\n";
    let (highlighter, text) = rust(source);

    let inner_x = source.find("let").unwrap();
    let range = highlighter
        .enclosing_node(&text, CharOffset(inner_x), &["function_item"])
        .expect("a function encloses this position");

    let covered: String = source
        .chars()
        .skip(range.start.get())
        .take(range.end.get() - range.start.get())
        .collect();
    assert!(
        covered.starts_with("fn inner"),
        "should find the innermost function, got {covered:?}"
    );
}

#[test]
fn the_worker_returns_a_version_tagged_result() {
    let mut worker = SyntaxWorker::spawn(Language::Rust).expect("the worker starts");
    let text = Text::from_str("fn main() { let x: u32 = 1; }");

    worker.request(SyntaxRequest {
        text: text.clone(),
        version: Version(7),
        edits: Vec::new(),
        range: CharOffset(0)..text.len_chars(),
        resync: false,
    });

    let result = wait_for_result(&mut worker);
    assert_eq!(result.version, Version(7), "results carry their version");
    assert!(!result.spans.is_empty());
    assert!(!result.node_path.is_empty());
}

#[test]
fn a_stale_result_is_never_reported_as_fresh() {
    // The rule from §1.1: a result computed against an older version is discarded, not applied.
    let mut worker = SyntaxWorker::spawn(Language::Rust).expect("the worker starts");
    let text = Text::from_str("fn main() {}");

    worker.request(SyntaxRequest {
        text: text.clone(),
        version: Version(3),
        edits: Vec::new(),
        range: CharOffset(0)..text.len_chars(),
        resync: false,
    });
    wait_for_result(&mut worker);

    // The buffer has moved on since the request was made.
    assert!(
        worker.fresh_for(Version(4)).is_none(),
        "a v3 result must not be applied to a v4 buffer"
    );
    assert!(worker.fresh_for(Version(3)).is_some());
    // But it is still available for the ledger to report as stale.
    assert_eq!(worker.latest().map(|r| r.version), Some(Version(3)));
}

#[test]
fn requests_are_coalesced_so_typing_does_not_queue_up() {
    // Fast typing must not make the worker fall further behind: only the newest snapshot matters,
    // and the answer must describe *that* snapshot.
    let mut worker = SyntaxWorker::spawn(Language::Rust).expect("the worker starts");

    let mut text = Text::from_str("fn main() {}");
    let mut version = Version(1);
    let mut edits_so_far = Vec::new();

    for index in 0..40 {
        let insert = Edit::insert(CharOffset(11), format!("let x{index} = {index};"));
        let before = text.clone();
        Transaction::new(vec![insert.clone()])
            .apply(&mut text)
            .unwrap();
        let _ = before;
        version = version.next();
        edits_so_far.push(insert);

        worker.request(SyntaxRequest {
            text: text.clone(),
            version,
            edits: vec![edits_so_far.last().unwrap().clone()],
            range: CharOffset(0)..text.len_chars(),
            resync: false,
        });
    }

    // Eventually the newest version arrives; intermediate ones may be skipped entirely.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(result) = worker.poll() {
            if result.version == version {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker never caught up to {version:?}"
        );
        std::thread::yield_now();
    }

    assert!(worker.fresh_for(version).is_some());
}

#[test]
fn dropping_the_worker_stops_its_thread() {
    // A worker that outlives the editor writes into a dropped channel.
    let worker = SyntaxWorker::spawn(Language::Rust).expect("the worker starts");
    drop(worker); // must not hang or panic
}

fn wait_for_result(worker: &mut SyntaxWorker) -> dacode_syntax::SyntaxResult {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(result) = worker.poll() {
            return result.clone();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the worker produced no result"
        );
        std::thread::yield_now();
    }
}

#[test]
fn every_grammar_loads_and_its_query_compiles() {
    // The failure mode adding a language: the grammar links but the query references a node kind it
    // does not have, so `Query::new` fails at runtime and that language silently loses all color.
    for language in Language::ALL {
        let highlighter = Highlighter::new(language)
            .unwrap_or_else(|error| panic!("{}: {error}", language.name()));
        drop(highlighter);
    }
}

#[test]
fn extensions_map_to_the_right_language() {
    assert_eq!(Language::from_path("a.py"), Some(Language::Python));
    assert_eq!(Language::from_path("a.tsx"), Some(Language::Tsx));
    assert_eq!(Language::from_path("a.ts"), Some(Language::TypeScript));
    assert_eq!(Language::from_path("a.jsx"), Some(Language::JavaScript));
    assert_eq!(Language::from_path("a.cpp"), Some(Language::Cpp));
    assert_eq!(
        Language::from_path("a.h"),
        Some(Language::C),
        "a bare header is C"
    );
    assert_eq!(Language::from_path("a.css"), Some(Language::Css));
    assert_eq!(Language::from_path("a.txt"), None);
}

#[test]
fn each_language_highlights_its_own_keywords() {
    // One real sample per grammar: enough to prove the query matches the grammar rather than merely
    // compiling against it.
    let cases: &[(Language, &str, &str)] = &[
        (Language::Python, "def main():\n    return 1\n", "def"),
        (
            Language::JavaScript,
            "function main() { return 1; }\n",
            "function",
        ),
        (
            Language::TypeScript,
            "function f(x: number) { return x; }\n",
            "function",
        ),
        (Language::C, "int main(void) { return 0; }\n", "return"),
        (Language::Cpp, "int main() { return 0; }\n", "return"),
        (Language::Css, "body { color: red; }\n", "body"),
    ];

    for (language, source, needle) in cases {
        let text = Text::from_str(source);
        let mut highlighter = Highlighter::new(*language).unwrap();
        assert!(
            highlighter.parse(&text),
            "{} failed to parse",
            language.name()
        );

        let spans = highlighter.highlights(&text, CharOffset(0)..text.len_chars());
        assert!(
            !spans.is_empty(),
            "{} produced no highlights at all",
            language.name()
        );

        let offset = source.find(needle).unwrap();
        let covered = spans
            .iter()
            .any(|span| span.range.start.get() <= offset && offset < span.range.end.get());
        assert!(
            covered,
            "{}: {needle:?} was not highlighted; spans were {:?}",
            language.name(),
            spans
                .iter()
                .map(|s| (text.slice_to_string(s.range.clone()), s.highlight))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn html_highlights_tags() {
    let source = "<!-- note -->\n<div class=\"x\">text</div>\n";
    let text = Text::from_str(source);
    let mut highlighter = Highlighter::new(Language::Html).unwrap();
    assert!(highlighter.parse(&text));

    let spans = highlighter.highlights(&text, CharOffset(0)..text.len_chars());
    assert!(!spans.is_empty(), "html produced no highlights");
    assert!(
        spans
            .iter()
            .any(|span| span.highlight == Highlight::Comment),
        "the comment should be recognized: {spans:?}"
    );
}

#[test]
fn markup_capture_names_are_mapped() {
    // CSS and HTML name things differently from programming-language grammars. Without these, a CSS
    // file came back with no highlights at all — every capture fell through to `Text` and was
    // filtered out.
    assert_eq!(Highlight::from_capture("tag"), Highlight::Type);
    assert_eq!(Highlight::from_capture("property"), Highlight::Attribute);
    assert_eq!(Highlight::from_capture("media"), Highlight::Keyword);
    assert_eq!(Highlight::from_capture("import"), Highlight::Keyword);
}
