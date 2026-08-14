//! Parsing and highlight-query evaluation.

use std::ops::Range;
use tree_sitter::{
    InputEdit, Node, Parser, Point as TsPoint, Query, QueryCursor, StreamingIterator, TextProvider,
    Tree,
};
use vex_text::{ByteOffset, CharOffset, Edit, Text};

/// Feeds the parser directly from the rope's chunks.
///
/// The obvious implementation hands tree-sitter `text.to_string()`, which allocates and copies the
/// whole document on **every keystroke** — cost that scales with file size for work that should be
/// proportional to the edit. Streaming from chunks removes that copy entirely.
fn chunk_at(text: &Text, byte: usize) -> &[u8] {
    let rope = text.rope();
    if byte >= rope.len_bytes() {
        return &[];
    }
    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
    &chunk.as_bytes()[byte - chunk_start..]
}

/// Supplies node text to the query cursor from the rope, for the same reason.
struct RopeText<'a> {
    text: &'a Text,
}

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = std::iter::Map<ropey::iter::Chunks<'a>, fn(&str) -> &[u8]>;

    fn text(&mut self, node: Node<'_>) -> Self::I {
        let rope = self.text.rope();
        let start = node.start_byte().min(rope.len_bytes());
        let end = node.end_byte().min(rope.len_bytes());
        rope.byte_slice(start..end).chunks().map(str::as_bytes)
    }
}

/// How a language writes a comment.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Comment {
    /// The token that comments out the rest of the line.
    Line(&'static str),
    /// Opening and closing tokens, for languages with no line comment.
    Wrap(&'static str, &'static str),
}

/// A language the editor knows how to parse.
///
/// The set from the plan's locked decisions, minus Dart: no maintained `tree-sitter-dart` crate is
/// published, so its grammar would have to be vendored and built here — deferred rather than
/// half-done, since the Dart language server still works over LSP without it.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    C,
    Cpp,
    Html,
    Css,
}

impl Language {
    pub const ALL: [Language; 9] = [
        Language::Rust,
        Language::Python,
        Language::JavaScript,
        Language::TypeScript,
        Language::Tsx,
        Language::C,
        Language::Cpp,
        Language::Html,
        Language::Css,
    ];

    /// Guesses the language from a file extension. Content sniffing and modelines can come later;
    /// the extension is right almost always.
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit('.').next()?;
        Some(match extension {
            "rs" => Language::Rust,
            "py" | "pyi" => Language::Python,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            // Headers are ambiguous. `.h` is treated as C, because a C++ header that uses C++-only
            // syntax still parses error-tolerantly, while parsing C as C++ misreads nothing.
            "c" | "h" => Language::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
            "html" | "htm" => Language::Html,
            "css" => Language::Css,
            _ => return None,
        })
    }

    /// How this language spells a comment, for the toggle every conventional editor puts on
    /// `ctrl-/`.
    ///
    /// Line comments where the language has them. CSS and HTML have none, so their block form is
    /// wrapped around each line individually rather than around the selection: wrapping the block
    /// would nest on a second press and uncommenting would have to parse, while one wrapper per
    /// line is trivially reversible — which is what a *toggle* has to be.
    pub fn comment(self) -> Comment {
        match self {
            Language::Rust
            | Language::JavaScript
            | Language::TypeScript
            | Language::Tsx
            | Language::C
            | Language::Cpp => Comment::Line("//"),
            Language::Python => Comment::Line("#"),
            Language::Css => Comment::Wrap("/*", "*/"),
            Language::Html => Comment::Wrap("<!--", "-->"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Html => "html",
            Language::Css => "css",
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Language::C => tree_sitter_c::LANGUAGE.into(),
            Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Language::Html => tree_sitter_html::LANGUAGE.into(),
            Language::Css => tree_sitter_css::LANGUAGE.into(),
        }
    }

    /// The highlight query source for this language.
    ///
    /// Returns an owned string because TypeScript's query is not self-contained: by convention it
    /// carries only the TypeScript-specific captures and is *combined with* the JavaScript query.
    /// Using it alone highlights types and nothing else — which is exactly what happened before a
    /// test caught it.
    fn highlights_query(self) -> String {
        match self {
            Language::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            Language::Python => tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
            Language::JavaScript => tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
            // JavaScript first: later patterns win in tree-sitter, so the TypeScript-specific
            // captures must come last to take precedence.
            Language::TypeScript | Language::Tsx => format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            Language::Html => tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
            Language::Css => tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
            // The C and C++ crates ship query files but do not expose them, and one crate cannot
            // `include_str!` another's source. Rather than reach into a vendored path that may move,
            // these are written here — small, and they cover the roles the theme actually
            // distinguishes.
            Language::C | Language::Cpp => C_FAMILY_HIGHLIGHTS.to_string(),
        }
    }
}

/// A compact highlight query for C and C++.
///
/// Deliberately limited to the captures this editor's theme has colors for. A longer query would add
/// captures that all collapse to the same few roles anyway.
const C_FAMILY_HIGHLIGHTS: &str = r#"
(comment) @comment
(string_literal) @string
(char_literal) @string
(system_lib_string) @string
(number_literal) @number
(true) @constant
(false) @constant
(null) @constant

(primitive_type) @type
(sized_type_specifier) @type
(type_identifier) @type

(function_declarator declarator: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @function))
(preproc_function_def name: (identifier) @function)

"break" @keyword
"case" @keyword
"const" @keyword
"continue" @keyword
"default" @keyword
"do" @keyword
"else" @keyword
"enum" @keyword
"extern" @keyword
"for" @keyword
"if" @keyword
"inline" @keyword
"return" @keyword
"sizeof" @keyword
"static" @keyword
"struct" @keyword
"switch" @keyword
"typedef" @keyword
"union" @keyword
"volatile" @keyword
"while" @keyword

(preproc_include) @attribute
(preproc_def) @attribute
(preproc_ifdef) @attribute
"#;

/// The theme-facing highlight roles.
///
/// Deliberately few. Highlight queries produce dozens of capture names; they are collapsed here
/// so the theme stays a palette a person can hold in their head, and so adding a language does
/// not add theme entries.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Type,
    Function,
    Attribute,
    Comment,
    String,
    Number,
    Constant,
    /// Anything the query captured but the theme does not distinguish.
    Text,
}

impl Highlight {
    /// Maps a tree-sitter capture name onto a role.
    ///
    /// Matching is by prefix on the dotted name (`function.method` → `function`), which is how
    /// the convention works, so a grammar with finer captures than the theme still lands
    /// somewhere sensible instead of losing its color.
    pub fn from_capture(name: &str) -> Self {
        let base = name.split('.').next().unwrap_or(name);
        match base {
            "keyword" => Highlight::Keyword,
            // Markup vocabularies: CSS and HTML name things differently from programming-language
            // grammars, and without these every CSS file came back with no highlights at all. A tag
            // is the nearest thing to a type; a CSS property is the nearest thing to an attribute.
            "tag" => Highlight::Type,
            "property" => Highlight::Attribute,
            // CSS at-rules (`@media`, `@import`, …) are keywords in every other language's terms.
            "media" | "import" | "charset" | "namespace" | "keyframes" | "supports" => {
                Highlight::Keyword
            }
            "type" | "constructor" => Highlight::Type,
            "function" | "method" => Highlight::Function,
            "attribute" | "label" | "decorator" => Highlight::Attribute,
            "comment" => Highlight::Comment,
            "string" | "character" => Highlight::String,
            "number" | "float" | "integer" => Highlight::Number,
            "constant" | "boolean" => Highlight::Constant,
            _ => Highlight::Text,
        }
    }

    /// Whether this role is set in italic serif — the design's most distinctive typographic move.
    pub fn is_prose(self) -> bool {
        self == Highlight::Comment
    }
}

/// A highlighted span, in char offsets so it can be intersected with display rows directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<CharOffset>,
    pub highlight: Highlight,
}

/// Owns a parser, its tree and the compiled query for one document.
pub struct Highlighter {
    language: Language,
    parser: Parser,
    query: Query,
    tree: Option<Tree>,
}

impl Highlighter {
    pub fn new(language: Language) -> Result<Self, String> {
        let ts_language = language.ts_language();
        let mut parser = Parser::new();
        parser
            .set_language(&ts_language)
            .map_err(|err| format!("loading the {language:?} grammar: {err}"))?;
        let query = Query::new(&ts_language, &language.highlights_query())
            .map_err(|err| format!("compiling the {language:?} highlight query: {err}"))?;

        Ok(Self {
            language,
            parser,
            query,
            tree: None,
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn has_tree(&self) -> bool {
        self.tree.is_some()
    }

    /// Drops the tree so the next parse starts from scratch. Used when the editor cannot supply a
    /// complete edit sequence — a full reparse is slower but correct, and silently reusing a tree
    /// whose edits are missing produces highlights for text that never existed.
    pub fn reset(&mut self) {
        self.tree = None;
    }

    /// Reparses, reusing the previous tree when one exists.
    ///
    /// Call [`Self::edit`] with the document's edits *before* this, or the reuse is unsound and
    /// tree-sitter will reparse from scratch at best.
    pub fn parse(&mut self, text: &Text) -> bool {
        let Self { parser, tree, .. } = self;
        // Streamed from the rope: no full-document allocation, so the cost tracks the edit rather
        // than the file size.
        let mut callback = |byte: usize, _: TsPoint| chunk_at(text, byte);

        match parser.parse_with_options(&mut callback, tree.as_ref(), None) {
            Some(parsed) => {
                *tree = Some(parsed);
                true
            }
            // Only happens on cancellation or timeout; keep the old tree so highlighting degrades
            // to stale rather than vanishing.
            None => false,
        }
    }

    /// Tells the tree where the text changed, so the next parse is incremental.
    ///
    /// `before` must be the text as it was *before* the edits, since the old byte and point
    /// positions have to be measured against it.
    pub fn edit(&mut self, before: &Text, after: &Text, edits: &[Edit]) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };

        // Edits are in the pre-edit coordinate space and applied in order, so the running delta
        // converts each one into the coordinates the tree currently holds.
        let mut delta: isize = 0;

        for edit in edits {
            let start_char = edit.range.start.shift(delta);
            let old_end_char = edit.range.end.shift(delta);
            let new_end_char = CharOffset(start_char.get() + edit.new_len());

            let start_byte = before.char_to_byte(edit.range.start);
            let old_end_byte = before.char_to_byte(edit.range.end);
            let new_end_byte = after.char_to_byte(new_end_char);

            tree.edit(&InputEdit {
                start_byte: byte(start_byte),
                old_end_byte: byte(old_end_byte),
                new_end_byte: byte(new_end_byte),
                start_position: ts_point(before, edit.range.start),
                old_end_position: ts_point(before, edit.range.end),
                new_end_position: ts_point(after, new_end_char),
            });

            let _ = start_char;
            let _ = old_end_char;
            delta += edit.delta();
        }
    }

    /// Highlight spans overlapping a char range — the visible rows, never the whole document.
    ///
    /// Bounded by the viewport rather than by file size, which is what keeps this affordable
    /// even though it runs off the UI thread.
    pub fn highlights(&self, text: &Text, range: Range<CharOffset>) -> Vec<HighlightSpan> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };

        let start_byte = text.char_to_byte(range.start).get();
        let end_byte = text.char_to_byte(range.end).get();

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);

        let mut spans: Vec<HighlightSpan> = Vec::new();
        let mut captures = cursor.captures(&self.query, tree.root_node(), RopeText { text });

        while let Some((matched, index)) = captures.next() {
            let capture = matched.captures[*index];
            let name = &self.query.capture_names()[capture.index as usize];
            let highlight = Highlight::from_capture(name);
            if highlight == Highlight::Text {
                continue;
            }

            let node_range = node_char_range(text, capture.node);
            if node_range.start >= node_range.end {
                continue;
            }
            spans.push(HighlightSpan {
                range: node_range,
                highlight,
            });
        }

        // Later, more specific captures win — tree-sitter's convention — so resolving overlaps
        // by keeping the *last* match at each position is what makes queries behave as authored.
        spans.sort_by_key(|span| (span.range.start, span.range.end));
        resolve_overlaps(spans)
    }

    /// The tree-sitter node path at a position: the ancestor chain of the smallest node
    /// containing it, outermost first. The design shows this in the structure sidebar.
    pub fn node_path(&self, text: &Text, at: CharOffset) -> Vec<String> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };

        let byte = text.char_to_byte(at).get();
        let mut node = tree
            .root_node()
            .descendant_for_byte_range(byte, byte)
            .unwrap_or(tree.root_node());

        let mut path = vec![node.kind().to_string()];
        while let Some(parent) = node.parent() {
            path.push(parent.kind().to_string());
            node = parent;
        }
        path.reverse();
        path
    }

    /// The smallest node of one of `kinds` containing `at` — the basis for structural text
    /// objects (`if` for a function, `ic` for a class), which plain Vim cannot express.
    pub fn enclosing_node(
        &self,
        text: &Text,
        at: CharOffset,
        kinds: &[&str],
    ) -> Option<Range<CharOffset>> {
        let tree = self.tree.as_ref()?;
        let byte = text.char_to_byte(at).get();
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte)?;

        loop {
            if kinds.contains(&node.kind()) {
                return Some(node_char_range(text, node));
            }
            node = node.parent()?;
        }
    }

    /// Whether the tree contains a parse error. Useful as a signal, not as a gate: tree-sitter
    /// is error-tolerant precisely so half-typed code still highlights.
    pub fn has_error(&self) -> bool {
        self.tree
            .as_ref()
            .map(|tree| tree.root_node().has_error())
            .unwrap_or(false)
    }
}

/// Keeps the last span at each position, dropping earlier spans it covers.
fn resolve_overlaps(spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
    let mut out: Vec<HighlightSpan> = Vec::with_capacity(spans.len());

    for span in spans {
        while let Some(last) = out.last() {
            // A later span starting at or before the previous one's start supersedes it.
            if last.range.start >= span.range.start && last.range.end <= span.range.end {
                out.pop();
            } else {
                break;
            }
        }

        match out.last_mut() {
            // Trim a partial overlap rather than letting two spans claim the same characters.
            Some(last) if span.range.start < last.range.end => {
                if span.range.end > last.range.end {
                    let trimmed = last.range.end..span.range.end;
                    out.push(HighlightSpan {
                        range: trimmed,
                        highlight: span.highlight,
                    });
                }
            }
            _ => out.push(span),
        }
    }

    out
}

fn node_char_range(text: &Text, node: Node<'_>) -> Range<CharOffset> {
    text.byte_to_char(ByteOffset(node.start_byte()))..text.byte_to_char(ByteOffset(node.end_byte()))
}

fn byte(offset: ByteOffset) -> usize {
    offset.get()
}

fn ts_point(text: &Text, at: CharOffset) -> TsPoint {
    let point = text.char_to_point(at);
    // tree-sitter points are (row, byte column), not character columns.
    let line_start = text.line_to_char(point.line);
    let column_bytes = text.char_to_byte(at).get() - text.char_to_byte(line_start).get();
    TsPoint::new(point.line, column_bytes)
}
