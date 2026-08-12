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

/// A language the editor knows how to parse.
///
/// One variant per grammar. The plan targets Rust, TS/JS/HTML/CSS, Python, C/C++ and Dart; Rust
/// is wired first because the editor is written in it and must highlight itself.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
}

impl Language {
    /// Guesses the language from a file extension. Content sniffing and modelines can come later;
    /// the extension is right almost always.
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit('.').next()?;
        match extension {
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    fn highlights_query(self) -> &'static str {
        match self {
            Language::Rust => tree_sitter_rust::HIGHLIGHTS_QUERY,
        }
    }
}

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
            "type" | "constructor" => Highlight::Type,
            "function" | "method" => Highlight::Function,
            "attribute" | "label" => Highlight::Attribute,
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
        let query = Query::new(&ts_language, language.highlights_query())
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
