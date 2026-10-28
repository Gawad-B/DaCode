//! The parse tree on the UI thread: indentation and structural text objects.
//!
//! # How the tree gets here
//!
//! The tree is built on the syntax worker, and two things have to be answered *on the keystroke*
//! from it: how far to indent the line Enter opens, and what `daf` deletes. Neither can wait for a
//! round trip to the worker. So every highlight result carries a [`Structure`] — a clone of the
//! tree, which tree-sitter makes by bumping a reference count rather than copying nodes — tagged
//! with the [`Version`] it was parsed from, plus the compiled queries, shared by `Arc` so the UI
//! thread never compiles anything.
//!
//! **A structure is used only while its version is the buffer's.** The first keystroke after a
//! parse makes it stale, and until the worker answers again everything here is skipped: Enter
//! falls back to the lexical rules in `dacode-buffer`, and a text object says it has no tree
//! rather than selecting from one that describes text that has since moved. That is the same rule
//! the highlight spans live by, applied to a second consumer.
//!
//! # Why every query here is bounded
//!
//! A query cursor run over the root walks the document. Each function below instead walks the
//! *ancestors* of one node — tens of nodes, however long the file — and runs the query on each of
//! them with a maximum start depth of zero, so a pattern can only match rooted at that node. A
//! pattern with a child step (`(parameters (_) @parameter.inside)`) still looks at that node's own
//! children, which is the argument list itself and nothing else. `latency.rs` holds the result
//! flat between a thousand-line file and a hundred-thousand-line one.

use crate::highlight::{Language, RopeText};
use dacode_text::{ByteOffset, CharOffset, Text, Version};
use std::ops::Range;
use std::sync::Arc;
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

/// How far back Enter looks for the last non-blank character before the cursor.
///
/// A run of blank lines is the only thing that can make that walk long, and past this many
/// characters of whitespace the lexical rules answer instead — they only read the cursor's line.
const BLANK_SCAN_LIMIT: usize = 4096;

/// How many neighbouring comments `ac` gathers in each direction. A file that is one enormous run
/// of line comments must not make one keystroke walk all of them.
const COMMENT_RUN_LIMIT: usize = 256;

/// The compiled indent and text-object queries for one language.
///
/// Compiled once, on the thread that builds the [`crate::Highlighter`], and shared by every
/// [`Structure`] after it.
pub struct Queries {
    indents: Query,
    objects: Query,
}

impl Queries {
    pub fn new(language: Language) -> Result<Self, String> {
        let grammar = language.ts_language();
        let indents = Query::new(&grammar, &indents_source(language))
            .map_err(|err| format!("compiling the {language:?} indent query: {err}"))?;
        let objects = Query::new(&grammar, &objects_source(language))
            .map_err(|err| format!("compiling the {language:?} text-object query: {err}"))?;
        Ok(Self { indents, objects })
    }
}

/// The queries are Helix's capture names over grammars that are not always Helix's versions, so
/// they are written here for exactly the grammar crates this workspace pins. A test compiles all
/// of them; a node name a grammar does not have is a compile error, not a silent miss.
fn indents_source(language: Language) -> String {
    const RUST: &str = include_str!("../queries/rust/indents.scm");
    const PYTHON: &str = include_str!("../queries/python/indents.scm");
    const ECMA: &str = include_str!("../queries/ecma/indents.scm");
    const JSX: &str = include_str!("../queries/jsx/indents.scm");
    const TYPESCRIPT: &str = include_str!("../queries/typescript/indents.scm");
    const C: &str = include_str!("../queries/c/indents.scm");
    const CPP: &str = include_str!("../queries/cpp/indents.scm");
    const HTML: &str = include_str!("../queries/html/indents.scm");
    const CSS: &str = include_str!("../queries/css/indents.scm");
    match language {
        Language::Rust => RUST.to_string(),
        Language::Python => PYTHON.to_string(),
        Language::JavaScript => format!("{ECMA}\n{JSX}"),
        Language::TypeScript => format!("{ECMA}\n{TYPESCRIPT}"),
        Language::Tsx => format!("{ECMA}\n{JSX}\n{TYPESCRIPT}"),
        Language::C => C.to_string(),
        Language::Cpp => format!("{C}\n{CPP}"),
        Language::Html => HTML.to_string(),
        Language::Css => CSS.to_string(),
    }
}

fn objects_source(language: Language) -> String {
    const RUST: &str = include_str!("../queries/rust/textobjects.scm");
    const PYTHON: &str = include_str!("../queries/python/textobjects.scm");
    const ECMA: &str = include_str!("../queries/ecma/textobjects.scm");
    const TYPESCRIPT: &str = include_str!("../queries/typescript/textobjects.scm");
    const C: &str = include_str!("../queries/c/textobjects.scm");
    const CPP: &str = include_str!("../queries/cpp/textobjects.scm");
    const HTML: &str = include_str!("../queries/html/textobjects.scm");
    const CSS: &str = include_str!("../queries/css/textobjects.scm");
    match language {
        Language::Rust => RUST.to_string(),
        Language::Python => PYTHON.to_string(),
        Language::JavaScript => ECMA.to_string(),
        Language::TypeScript | Language::Tsx => format!("{ECMA}\n{TYPESCRIPT}"),
        Language::C => C.to_string(),
        Language::Cpp => format!("{C}\n{CPP}"),
        Language::Html => HTML.to_string(),
        Language::Css => CSS.to_string(),
    }
}

/// Which structural object `af`, `at`, `aa` and `ac` mean.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Function,
    /// A class, struct, enum, union, trait, impl, interface or type alias — "the type around me".
    Type,
    /// One argument or parameter of the nearest list of them.
    Argument,
    Comment,
}

impl ObjectKind {
    /// The capture-name stem, as Helix spells it.
    fn capture(self) -> &'static str {
        match self {
            ObjectKind::Function => "function",
            ObjectKind::Type => "class",
            ObjectKind::Argument => "parameter",
            ObjectKind::Comment => "comment",
        }
    }

    /// What to call it in a sentence, for the message when there is none.
    pub fn noun(self) -> &'static str {
        match self {
            ObjectKind::Function => "function",
            ObjectKind::Type => "type",
            ObjectKind::Argument => "argument",
            ObjectKind::Comment => "comment",
        }
    }
}

/// A parse tree, the version of the text it describes, and the queries that read it.
#[derive(Clone)]
pub struct Structure {
    version: Version,
    tree: Tree,
    queries: Arc<Queries>,
}

impl std::fmt::Debug for Structure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Structure")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Structure {
    pub fn new(version: Version, tree: Tree, queries: Arc<Queries>) -> Self {
        Self {
            version,
            tree,
            queries,
        }
    }

    pub fn version(&self) -> Version {
        self.version
    }

    /// Whether this tree describes the buffer as it is. Nothing below may be asked of a tree that
    /// is not: every offset it holds would name text that has moved.
    pub fn is_fresh_for(&self, buffer: Version) -> bool {
        self.version == buffer
    }

    /// The indentation for the line Enter opens at `caret`, or `None` when the tree cannot be
    /// trusted to say and the lexical rules should.
    ///
    /// Helix's model, cut down to what these queries use. Walking out from the last non-blank
    /// character before the cursor, every `@indent` node that started before the cursor and
    /// still continues past it encloses the new line; an `@extend` node also encloses it when it
    /// ended at the cursor but the cursor's line sits deeper than the node's first line, which is
    /// how a Python block — with no closing token to end it — keeps the next line inside. A
    /// closing token captured `@outdent` that begins the text after the cursor takes its parent
    /// back out, since that token is about to start the new line.
    ///
    /// The answer is relative rather than counted: the line the innermost enclosing node starts
    /// on, plus one unit. Counting levels from column zero would reformat every file that does
    /// not indent the way the query author does; copying the opener's own indentation keeps
    /// whatever the file already does, tabs included.
    ///
    /// **Refuses inside a parse error.** An `ERROR` node, or an enclosing node with an error or a
    /// missing token somewhere beneath it, describes text tree-sitter had to guess at — typically
    /// the line being typed — and the lexical rules are better at half-written code.
    pub fn newline_indent(
        &self,
        text: &Text,
        caret: CharOffset,
        tab_width: usize,
    ) -> Option<String> {
        let query = &self.queries.indents;
        let caret = text.clamp(caret);
        let caret_byte = text.char_to_byte(caret).get();
        let caret_line = text.char_to_line(caret);
        let root = self.tree.root_node();

        let anchor = match last_non_blank_before(text, caret) {
            Scan::Found(at) => at,
            Scan::StartOfFile => return Some(String::new()),
            Scan::TooFar => return None,
        };
        let anchor_byte = text.char_to_byte(anchor).get();
        let deepest = root.descendant_for_byte_range(anchor_byte, anchor_byte + 1)?;
        let line_width = indent_width(text, caret_line, tab_width);

        let indent = query.capture_index_for_name("indent");
        let extend = query.capture_index_for_name("extend");
        let outdent = query.capture_index_for_name("outdent");

        let mut cursor = QueryCursor::new();
        let mut enclosing: Vec<Node> = Vec::new();
        let mut node = Some(deepest);
        while let Some(current) = node {
            if current.is_error() {
                return None;
            }
            if current.start_byte() < caret_byte {
                let captured = captures_of(&mut cursor, query, current, text);
                if indent.is_some_and(|index| captured.contains(&index)) {
                    if current.has_error() {
                        return None;
                    }
                    let continues = current.end_byte() > caret_byte;
                    // A head still waiting for its body — `if x:` and nothing yet — ends in an
                    // empty block, which tree-sitter gives no width; the cursor after the colon is
                    // where that body begins.
                    let awaiting_body = current
                        .child(current.child_count().saturating_sub(1) as u32)
                        .is_some_and(|last| last.start_byte() == last.end_byte());
                    let extends = extend.is_some_and(|index| captured.contains(&index))
                        && (awaiting_body
                            || line_width
                                > indent_width(text, current.start_position().row, tab_width));
                    if continues || extends {
                        enclosing.push(current);
                    }
                }
            }
            node = current.parent();
        }

        // The closer the new line will begin with, if there is one: walk up from its first token
        // while the node still starts there, because HTML's `</div>` is an `end_tag` wrapped round
        // a `</` token and only the wrapper is the thing captured.
        if let (Some(outdent), Some(next)) = (outdent, first_non_blank_after(text, caret)) {
            let byte = text.char_to_byte(next).get();
            let mut token = root.descendant_for_byte_range(byte, byte + 1);
            while let Some(current) = token {
                if current.start_byte() != byte {
                    break;
                }
                if captures_of(&mut cursor, query, current, text).contains(&outdent) {
                    if let Some(owner) = current.parent() {
                        enclosing.retain(|node| node.id() != owner.id());
                    }
                    break;
                }
                token = current.parent();
            }
        }

        let Some(innermost) = enclosing.first() else {
            return Some(String::new());
        };
        let base = leading_whitespace(text, innermost.start_position().row);
        let unit = if base.contains('\t') {
            "\t".to_string()
        } else {
            " ".repeat(tab_width.max(1))
        };
        Some(base + &unit)
    }

    /// The indentation a closer typed at `caret` should pull its line back to, or `None` when the
    /// tree does not know and the lexical scan should decide.
    ///
    /// The innermost node that opens with the matching delimiter on an earlier line and is still
    /// open at the cursor — either closed further down, or closed by a token tree-sitter had to
    /// invent because it is missing. The scan this replaces reads characters back to the opener and
    /// gives up after a fixed distance; this reads ancestors, so it has no distance to give up at.
    pub fn closer_indent(&self, text: &Text, caret: CharOffset, typed: char) -> Option<String> {
        let opener = match typed {
            '}' => "{",
            ')' => "(",
            ']' => "[",
            _ => return None,
        };
        let caret = text.clamp(caret);
        let caret_byte = text.char_to_byte(caret).get();
        let caret_line = text.char_to_line(caret);

        let Scan::Found(anchor) = last_non_blank_before(text, caret) else {
            return None;
        };
        let anchor_byte = text.char_to_byte(anchor).get();
        let mut node = self
            .tree
            .root_node()
            .descendant_for_byte_range(anchor_byte, anchor_byte + 1);

        while let Some(current) = node {
            if current.is_error() {
                return None;
            }
            let opens = current.child(0).is_some_and(|first| first.kind() == opener);
            let open_here = current.end_byte() > caret_byte
                || current
                    .child(current.child_count().saturating_sub(1) as u32)
                    .is_some_and(|last| last.is_missing());
            if opens
                && open_here
                && current.start_byte() < caret_byte
                && current.start_position().row < caret_line
            {
                return Some(leading_whitespace(text, current.start_position().row));
            }
            node = current.parent();
        }
        None
    }

    /// The range of a structural text object around `at`, or `None` when there is none there.
    ///
    /// The innermost one wins, as `i(` takes the innermost parentheses: `af` inside a closure
    /// inside a function is the closure.
    ///
    /// For an argument, the innermost *list* of them decides, and the cursor between two items —
    /// on a comma, or the space after it — means the one before. Walking on outwards instead would
    /// select the whole call the list belongs to, which is an argument of something else and
    /// almost never what was meant.
    pub fn object(
        &self,
        text: &Text,
        at: CharOffset,
        kind: ObjectKind,
        around: bool,
    ) -> Option<Range<CharOffset>> {
        self.object_nth(text, at, kind, around, 1)
    }

    /// The `count`-th object of `kind` around `at`, counting outwards: `1` is [`Self::object`],
    /// `2` the one enclosing that, and so on — `2af` inside a closure is the function holding it.
    ///
    /// For an argument the count climbs *lists*: `2aa` is the argument of the next list out that
    /// contains this one, the whole inner call when the cursor is in an argument of a call that is
    /// itself an argument. That is what targets.vim and nvim-treesitter-textobjects do with a count
    /// on an object, and it keeps every kind counting the same way. "This argument and the next
    /// one" is the other reading, and neither of them chose it. Comments do not nest, so a count
    /// above one finds nothing. A count beyond the outermost is `None` rather than the outermost:
    /// an operator told to take the third enclosing function should not quietly take the second.
    ///
    /// Bounded by the depth of the tree, like every walk here: it only ever goes up from the
    /// cursor's node.
    pub fn object_nth(
        &self,
        text: &Text,
        at: CharOffset,
        kind: ObjectKind,
        around: bool,
        count: usize,
    ) -> Option<Range<CharOffset>> {
        let mut remaining = count.max(1);
        let query = &self.queries.objects;
        let at = text.clamp(at);
        let byte = text.char_to_byte(at).get();
        let stem = kind.capture();
        let around_index = query.capture_index_for_name(&format!("{stem}.around"));
        let inside_index = query.capture_index_for_name(&format!("{stem}.inside"));

        let root = self.tree.root_node();
        let mut node = root.descendant_for_byte_range(byte, byte + 1);
        let mut cursor = QueryCursor::new();
        cursor.set_max_start_depth(Some(0));

        while let Some(current) = node {
            if kind == ObjectKind::Argument {
                if let Some(index) = inside_index {
                    let items = captured_nodes(&mut cursor, query, current, text, index);
                    if let Some(item) = pick_item(&items, byte) {
                        remaining -= 1;
                        if remaining > 0 {
                            node = current.parent();
                            continue;
                        }
                        return Some(if around {
                            argument_around(text, item)
                        } else {
                            node_range(text, item)
                        });
                    }
                }
            } else if let Some(around_capture) = around_index {
                let mut found = false;
                let mut body: Option<Node> = None;
                let mut matches = cursor.matches(query, current, RopeText { text });
                while let Some(matched) = matches.next() {
                    if !matched
                        .captures
                        .iter()
                        .any(|c| c.index == around_capture && c.node.id() == current.id())
                    {
                        continue;
                    }
                    found = true;
                    if let Some(inside) = inside_index {
                        if let Some(capture) = matched.captures.iter().find(|c| c.index == inside) {
                            body = Some(capture.node);
                        }
                    }
                }
                if found && kind == ObjectKind::Comment && remaining > 1 {
                    return None;
                }
                if found && remaining > 1 {
                    remaining -= 1;
                } else if found {
                    return Some(match (kind, around) {
                        (ObjectKind::Comment, around) => comment_range(text, current, around),
                        (_, true) => {
                            // A decorator belongs to what it decorates.
                            let whole = current
                                .parent()
                                .filter(|parent| parent.kind() == "decorated_definition")
                                .unwrap_or(current);
                            whole_lines(text, node_range(text, whole))
                        }
                        // Nothing inside to take — a signature, a unit struct — is `None`.
                        // Selecting the whole thing instead would be selecting something wrong.
                        (_, false) => body_inside(text, body?),
                    });
                }
            }
            node = current.parent();
        }
        None
    }

    /// The smallest node strictly larger than `range` — one step of "expand selection".
    pub fn expand(&self, text: &Text, range: Range<CharOffset>) -> Option<Range<CharOffset>> {
        let start = text.char_to_byte(text.clamp(range.start)).get();
        let end = text.char_to_byte(text.clamp(range.end)).get();
        let root = self.tree.root_node();
        // From a caret, the first step is the node under it, whatever its size.
        if start == end {
            let under = root.descendant_for_byte_range(start, start + 1)?;
            return Some(node_range(text, under));
        }
        let mut node = Some(root.descendant_for_byte_range(start, end)?);
        while let Some(current) = node {
            let grows = current.start_byte() < start || current.end_byte() > end;
            if grows && current.start_byte() <= start && current.end_byte() >= end {
                return Some(node_range(text, current));
            }
            node = current.parent();
        }
        None
    }

    /// One step of "shrink selection" with nothing to retrace: the first named node inside `range`
    /// that holds `caret`, and is strictly smaller than `range`.
    ///
    /// The editor undoes its own expansions from a history it keeps and asks this only when there
    /// is none, for a selection made some other way — a `vaf`, a drag. Named, because an anonymous
    /// node is a bracket or a keyword, and shrinking onto a lone `(` is never what was meant. A
    /// child the same size as its parent is gone through, not stopped at, or the step would
    /// change nothing visible: an expression statement and its call usually cover the same text.
    pub fn shrink(
        &self,
        text: &Text,
        range: Range<CharOffset>,
        caret: CharOffset,
    ) -> Option<Range<CharOffset>> {
        let start = text.char_to_byte(text.clamp(range.start)).get();
        let end = text.char_to_byte(text.clamp(range.end)).get();
        if start >= end {
            return None;
        }
        // The caret as a byte *inside* the range: a selection's head sits on its exclusive end
        // after an expansion, and the node that starts there is outside it.
        let caret = text
            .char_to_byte(text.clamp(caret))
            .get()
            .clamp(start, end - 1);
        let mut node = self
            .tree
            .root_node()
            .descendant_for_byte_range(start, end)?;
        loop {
            let mut walker = node.walk();
            let child = node
                .named_children(&mut walker)
                .find(|child| child.start_byte() <= caret && caret < child.end_byte())?;
            if child.start_byte() >= start
                && child.end_byte() <= end
                && (child.start_byte() > start || child.end_byte() < end)
            {
                return Some(node_range(text, child));
            }
            node = child;
        }
    }
}

/// The capture indices of every pattern that matches rooted at `node` — and only there.
fn captures_of(cursor: &mut QueryCursor, query: &Query, node: Node<'_>, text: &Text) -> Vec<u32> {
    cursor.set_max_start_depth(Some(0));
    let mut found = Vec::new();
    let mut matches = cursor.matches(query, node, RopeText { text });
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            if capture.node.id() == node.id() && !found.contains(&capture.index) {
                found.push(capture.index);
            }
        }
    }
    found
}

/// The nodes captured as `index` by patterns rooted at `node`, in document order.
fn captured_nodes<'tree>(
    cursor: &mut QueryCursor,
    query: &Query,
    node: Node<'tree>,
    text: &Text,
    index: u32,
) -> Vec<Node<'tree>> {
    let mut found: Vec<Node<'tree>> = Vec::new();
    let mut matches = cursor.matches(query, node, RopeText { text });
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            if capture.index == index && !found.iter().any(|n| n.id() == capture.node.id()) {
                found.push(capture.node);
            }
        }
    }
    found.sort_by_key(|node| node.start_byte());
    found
}

/// The item of a list the cursor means: the one it is on, or failing that the one before it.
fn pick_item<'tree>(items: &[Node<'tree>], byte: usize) -> Option<Node<'tree>> {
    if let Some(on) = items
        .iter()
        .find(|item| item.start_byte() <= byte && byte < item.end_byte())
    {
        return Some(*on);
    }
    let first = items.first()?;
    let last = items.last()?;
    // Outside the items altogether — on the opening parenthesis or past the closing one — is not
    // between two of them, and the list's parent may have a better answer.
    if byte < first.start_byte() || byte >= last.end_byte() {
        return None;
    }
    items
        .iter()
        .rev()
        .find(|item| item.end_byte() <= byte)
        .copied()
}

/// An argument with its separator: the comma and space after it, or for the last one, the comma
/// and space before it — so `daa` leaves a list that still reads as one.
fn argument_around(text: &Text, item: Node<'_>) -> Range<CharOffset> {
    let own = node_range(text, item);
    if let Some(comma) = item.next_sibling().filter(|next| next.kind() == ",") {
        let end = match comma.next_named_sibling() {
            Some(next) => text.byte_to_char(ByteOffset(next.start_byte())),
            None => text.byte_to_char(ByteOffset(comma.end_byte())),
        };
        return own.start..end;
    }
    if let Some(comma) = item.prev_sibling().filter(|prev| prev.kind() == ",") {
        let start = match comma.prev_named_sibling() {
            Some(prev) => text.byte_to_char(ByteOffset(prev.end_byte())),
            None => text.byte_to_char(ByteOffset(comma.start_byte())),
        };
        return start..own.end;
    }
    own
}

/// A comment, gathered with the comments on the lines directly above and below it — a paragraph of
/// `//` lines is one comment to the person who wrote it, and several nodes to the grammar.
fn comment_range(text: &Text, node: Node<'_>, around: bool) -> Range<CharOffset> {
    let mut first = node;
    let mut last = node;
    for _ in 0..COMMENT_RUN_LIMIT {
        match first.prev_sibling() {
            Some(prev)
                if prev.kind() == node.kind()
                    && first.start_position().row <= prev.end_position().row + 1 =>
            {
                first = prev
            }
            _ => break,
        }
    }
    for _ in 0..COMMENT_RUN_LIMIT {
        match last.next_sibling() {
            Some(next)
                if next.kind() == node.kind()
                    && next.start_position().row <= last.end_position().row + 1 =>
            {
                last = next
            }
            _ => break,
        }
    }
    let whole = node_range(text, first).start..node_range(text, last).end;
    if around {
        return whole_lines(text, whole);
    }
    // Inside: the words, without the markers that make them a comment.
    let first_range = node_range(text, first);
    let opening = text.slice_to_string(first_range.clone());
    let skip = opening
        .chars()
        .take_while(|c| matches!(c, '/' | '*' | '#' | '!' | '<' | '-'))
        .count();
    let skip = skip
        + opening
            .chars()
            .skip(skip)
            .take_while(|c| c.is_whitespace())
            .count();
    let last_range = node_range(text, last);
    let closing = text.slice_to_string(last_range.clone());
    let trimmed = closing
        .strip_suffix("*/")
        .or_else(|| closing.strip_suffix("-->"))
        .unwrap_or(&closing);
    let drop = closing.chars().count() - trimmed.trim_end().chars().count();
    let start = CharOffset(first_range.start.get() + skip);
    let end = CharOffset(last_range.end.get().saturating_sub(drop)).max(start);
    start..end
}

/// The inside of a body: between its braces when it has them, and whole lines when the content
/// starts and ends its own lines, so `dif` leaves `{` and `}` where they were with nothing between.
fn body_inside(text: &Text, body: Node<'_>) -> Range<CharOffset> {
    let count = body.child_count();
    let pairs = [("{", "}"), ("(", ")"), ("[", "]")];
    let first = body.child(0);
    let last = body.child(count.saturating_sub(1) as u32);
    let braced = match (first, last) {
        (Some(first), Some(last))
            if count >= 2
                && !last.is_missing()
                && pairs
                    .iter()
                    .any(|(open, close)| first.kind() == *open && last.kind() == *close) =>
        {
            Some(
                text.byte_to_char(ByteOffset(first.end_byte()))
                    ..text.byte_to_char(ByteOffset(last.start_byte())),
            )
        }
        _ => None,
    };
    // Without braces — a Python block — the widening may take the indentation in front of the
    // first statement, which belongs to no node; with them it must stop at the braces.
    let inner = braced.clone().unwrap_or_else(|| node_range(text, body));
    let bound = braced.unwrap_or(CharOffset::ZERO..text.len_chars());

    // Trim to the content, then widen to whole lines only where that takes in nothing but
    // indentation and the line break.
    let Some((start, end)) = trim(text, inner.clone()) else {
        return inner;
    };
    let start_line = text.char_to_line(start);
    let line_start = text.line_to_char(start_line);
    let start = if line_start >= bound.start && blank(text, line_start..start) {
        line_start
    } else {
        start
    };
    let end_line = text.char_to_line(CharOffset(end.get().saturating_sub(1)).max(start));
    let next_line = if end_line < text.last_line() {
        text.line_to_char(end_line + 1)
    } else {
        text.len_chars()
    };
    let end = if next_line <= bound.end && blank(text, end..next_line) {
        next_line
    } else {
        end
    };
    start..end
}

/// A range widened to whole lines, newline included, when it already starts and ends its lines
/// apart from indentation — so `daf` takes the function and not the blank line's worth of
/// indentation it leaves behind. Anything sharing a line with other code is left exact.
fn whole_lines(text: &Text, range: Range<CharOffset>) -> Range<CharOffset> {
    let first = text.char_to_line(range.start);
    let last = text.char_to_line(CharOffset(range.end.get().saturating_sub(1)).max(range.start));
    let line_start = text.line_to_char(first);
    let line_end = text.line_range(last).end;
    if !blank(text, line_start..range.start) || !blank(text, range.end..line_end) {
        return range;
    }
    let end = if last < text.last_line() {
        text.line_to_char(last + 1)
    } else {
        line_end
    };
    line_start..end
}

fn trim(text: &Text, range: Range<CharOffset>) -> Option<(CharOffset, CharOffset)> {
    let slice = text.slice(range.clone());
    let leading = slice.chars().take_while(|c| c.is_whitespace()).count();
    let length = slice.len_chars();
    if leading == length {
        return None;
    }
    let mut trailing = 0;
    let mut chars = slice.chars_at(length).reversed();
    while chars.next().is_some_and(|c| c.is_whitespace()) {
        trailing += 1;
    }
    Some((
        CharOffset(range.start.get() + leading),
        CharOffset(range.end.get() - trailing),
    ))
}

fn blank(text: &Text, range: Range<CharOffset>) -> bool {
    range.start >= range.end
        || text
            .slice(range)
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

fn node_range(text: &Text, node: Node<'_>) -> Range<CharOffset> {
    text.byte_to_char(ByteOffset(node.start_byte()))..text.byte_to_char(ByteOffset(node.end_byte()))
}

enum Scan {
    Found(CharOffset),
    StartOfFile,
    TooFar,
}

/// The last character before `caret` that is not whitespace, within [`BLANK_SCAN_LIMIT`].
fn last_non_blank_before(text: &Text, caret: CharOffset) -> Scan {
    let mut index = caret.get();
    let mut backwards = text.rope().chars_at(index).reversed();
    let floor = index.saturating_sub(BLANK_SCAN_LIMIT);
    while index > floor {
        index -= 1;
        match backwards.next() {
            Some(c) if c.is_whitespace() => {}
            Some(_) => return Scan::Found(CharOffset(index)),
            None => break,
        }
    }
    if index == 0 {
        Scan::StartOfFile
    } else {
        Scan::TooFar
    }
}

/// The first non-blank character after `caret` on its own line, looking no further than an
/// indentation's worth of whitespace.
fn first_non_blank_after(text: &Text, caret: CharOffset) -> Option<CharOffset> {
    let mut index = caret.get();
    for c in text.rope().chars_at(index).take(256) {
        match c {
            ' ' | '\t' => index += 1,
            '\n' | '\r' => return None,
            _ => return Some(CharOffset(index)),
        }
    }
    None
}

/// A line's leading whitespace. Reads only the indentation, never the rest of the line — one line
/// can be a whole minified file.
fn leading_whitespace(text: &Text, line: usize) -> String {
    text.rope()
        .line(line)
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn indent_width(text: &Text, line: usize, tab_width: usize) -> usize {
    text.rope()
        .line(line)
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { tab_width.max(1) } else { 1 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Highlighter;

    fn structure(language: Language, source: &str) -> (Text, Structure) {
        let text = Text::from_str(source);
        let mut highlighter = Highlighter::new(language).expect("grammar loads");
        highlighter.parse(&text);
        let structure = highlighter.structure(Version(1)).expect("parsed");
        (text, structure)
    }

    /// The offset of the `n`th `needle`, plus `shift`.
    fn at(text: &Text, needle: &str, shift: usize) -> CharOffset {
        let source = text.to_string();
        let byte = source.find(needle).expect("needle in source");
        CharOffset(source[..byte].chars().count() + shift)
    }

    fn selected(text: &Text, range: Range<CharOffset>) -> String {
        text.slice_to_string(range)
    }

    #[test]
    fn every_language_compiles_its_indent_and_object_queries() {
        // A node name a grammar does not have fails `Query::new`, and `Highlighter::new` refuses to
        // build — so a query written against the wrong grammar version would take a language's
        // highlighting down with it. This is where that is found, rather than on someone's file.
        for language in Language::ALL {
            if let Err(err) = Queries::new(language) {
                panic!("{err}");
            }
        }
    }

    #[test]
    fn a_new_line_after_a_finished_statement_drops_back_to_its_block() {
        // The case the lexical rule gets wrong: it copies the line it is on, so after a call whose
        // arguments were aligned under the parenthesis the next line starts sixteen columns in.
        let source = "fn main() {\n    let x = foo(a,\n                b);\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "b);", 3);
        assert_eq!(
            tree.newline_indent(&text, caret, 4).as_deref(),
            Some("    ")
        );
    }

    #[test]
    fn a_line_opened_inside_an_unfinished_call_continues_it() {
        let source = "fn main() {\n    foo(a, b)\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, " b)", 0);
        assert_eq!(
            tree.newline_indent(&text, caret, 4).as_deref(),
            Some("        ")
        );
    }

    #[test]
    fn a_closer_after_the_cursor_takes_its_level_with_it() {
        // `x|}` and Enter: the `}` lands on the new line, and belongs with `fn`, not inside it.
        let source = "fn main() {\n    x}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "}\n", 0);
        assert_eq!(tree.newline_indent(&text, caret, 4).as_deref(), Some(""));
    }

    #[test]
    fn the_indentation_copies_the_files_own_style() {
        // Relative, not counted: a file indented with tabs gets a tab, and one indented by two
        // gets its own two plus a level, rather than being reformatted to what the query assumes.
        let source = "fn main() {\n\tif x {\n\t\ty();\n\t}\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "y();", 4);
        assert_eq!(
            tree.newline_indent(&text, caret, 4).as_deref(),
            Some("\t\t")
        );
    }

    #[test]
    fn python_stays_in_its_block_after_the_last_statement() {
        // A Python block ends at its last statement, so the cursor at the end of `return x` is
        // *after* every node — without `@extend` the next line would fall to column zero.
        let source = "class A:\n    def f(self):\n        return x\n";
        let (text, tree) = structure(Language::Python, source);
        let caret = at(&text, "return x", 8);
        assert_eq!(
            tree.newline_indent(&text, caret, 4).as_deref(),
            Some("        ")
        );
    }

    #[test]
    fn python_leaves_a_closed_bracket_at_the_statements_level() {
        let source = "x = foo(a,\n        b)\n";
        let (text, tree) = structure(Language::Python, source);
        let caret = at(&text, "b)", 2);
        assert_eq!(tree.newline_indent(&text, caret, 4).as_deref(), Some(""));
    }

    #[test]
    fn a_python_head_with_no_body_yet_opens_one() {
        // tree-sitter reads `if x:` at the end of a file as an if with an empty block rather than
        // an error, so the tree has to know that an empty block is where the next line goes.
        let source = "def f():\n    if x:\n";
        let (text, tree) = structure(Language::Python, source);
        let caret = at(&text, "x:", 2);
        assert_eq!(
            tree.newline_indent(&text, caret, 4).as_deref(),
            Some("        ")
        );
    }

    #[test]
    fn a_parse_error_hands_the_answer_back_to_the_lexical_rules() {
        // A missing expression is text tree-sitter had to guess around; the tree has no honest
        // answer inside it and must say so rather than guess.
        let source = "fn main() {\n    let x = ;\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "= ;", 3);
        assert_eq!(tree.newline_indent(&text, caret, 4), None);
    }

    #[test]
    fn a_closer_finds_its_opener_however_far_up_it_is() {
        // The lexical scan gives up after sixteen thousand characters; the tree has no such
        // distance, because it walks ancestors rather than characters.
        let body = "    let a = 1;\n".repeat(2000);
        let source = format!("fn main() {{\n    if x {{\n{body}        \n    }}\n}}\n");
        let (text, tree) = structure(Language::Rust, &source);
        let line = text.len_lines() - 4;
        let caret = CharOffset(text.line_to_char(line).get() + 8);
        assert_eq!(
            tree.closer_indent(&text, caret, '}').as_deref(),
            Some("    ")
        );
    }

    #[test]
    fn af_and_if_take_a_rust_function_and_its_body() {
        let source = "fn one() {\n    let a = 1;\n    a\n}\n\nfn two() {}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "let a", 0);
        let around = tree
            .object(&text, caret, ObjectKind::Function, true)
            .unwrap();
        assert_eq!(
            selected(&text, around),
            "fn one() {\n    let a = 1;\n    a\n}\n"
        );
        let inside = tree
            .object(&text, caret, ObjectKind::Function, false)
            .unwrap();
        assert_eq!(selected(&text, inside), "    let a = 1;\n    a\n");
    }

    #[test]
    fn at_takes_the_type_around_the_cursor() {
        let source = "struct Point {\n    x: i32,\n}\n\nimpl Point {\n    fn x(&self) {}\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "x: i32", 0);
        let around = tree.object(&text, caret, ObjectKind::Type, true).unwrap();
        assert_eq!(selected(&text, around), "struct Point {\n    x: i32,\n}\n");
    }

    #[test]
    fn aa_takes_an_argument_and_one_comma() {
        let source = "fn f() { g(first, second, third); }";
        let (text, tree) = structure(Language::Rust, source);
        let middle = at(&text, "second", 2);
        let inside = tree
            .object(&text, middle, ObjectKind::Argument, false)
            .unwrap();
        assert_eq!(selected(&text, inside), "second");
        let around = tree
            .object(&text, middle, ObjectKind::Argument, true)
            .unwrap();
        assert_eq!(selected(&text, around), "second, ");
        // The last has no comma after it, so it takes the one before.
        let last = at(&text, "third", 0);
        let around = tree
            .object(&text, last, ObjectKind::Argument, true)
            .unwrap();
        assert_eq!(selected(&text, around), ", third");
    }

    #[test]
    fn an_argument_is_the_innermost_lists() {
        // On the comma inside `h(b, c)`: the argument before it, not the whole `h(...)` that the
        // outer list would offer.
        let source = "fn f() { g(a, h(b, c)); }";
        let (text, tree) = structure(Language::Rust, source);
        let comma = at(&text, ", c", 0);
        let inside = tree
            .object(&text, comma, ObjectKind::Argument, false)
            .unwrap();
        assert_eq!(selected(&text, inside), "b");
    }

    #[test]
    fn ac_gathers_a_paragraph_of_line_comments() {
        let source = "// one\n// two\nfn f() {}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "two", 0);
        let around = tree
            .object(&text, caret, ObjectKind::Comment, true)
            .unwrap();
        assert_eq!(selected(&text, around), "// one\n// two\n");
        let inside = tree
            .object(&text, caret, ObjectKind::Comment, false)
            .unwrap();
        assert_eq!(selected(&text, inside), "one\n// two");
    }

    #[test]
    fn a_python_function_inside_is_its_statements() {
        let source = "def f(a, b):\n    x = a\n    return x\n\ny = 1\n";
        let (text, tree) = structure(Language::Python, source);
        let caret = at(&text, "x = a", 0);
        let inside = tree
            .object(&text, caret, ObjectKind::Function, false)
            .unwrap();
        assert_eq!(selected(&text, inside), "    x = a\n    return x\n");
        let around = tree
            .object(&text, caret, ObjectKind::Function, true)
            .unwrap();
        assert_eq!(
            selected(&text, around),
            "def f(a, b):\n    x = a\n    return x\n"
        );
    }

    #[test]
    fn nothing_there_is_none_rather_than_something_near() {
        // Outside every function, `af` must not reach for the nearest one: an operator applied to
        // the wrong function is worse than a key that does nothing and says so.
        let source = "use std::fmt;\n\nfn f() {}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "fmt", 0);
        assert_eq!(tree.object(&text, caret, ObjectKind::Function, true), None);
        // And a function with no body has no inside to take.
        let source = "trait T {\n    fn f(&self);\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "f(&self)", 0);
        assert_eq!(tree.object(&text, caret, ObjectKind::Function, false), None);
    }

    #[test]
    fn expanding_grows_to_the_next_node_out() {
        let source = "fn f() { g(a + b); }";
        let (text, tree) = structure(Language::Rust, source);
        let a = at(&text, "a + b", 0);
        let word = tree.expand(&text, a..a).unwrap();
        assert_eq!(selected(&text, word.clone()), "a");
        let sum = tree.expand(&text, word).unwrap();
        assert_eq!(selected(&text, sum.clone()), "a + b");
        let call = tree.expand(&text, sum).unwrap();
        assert_eq!(selected(&text, call), "(a + b)");
    }

    #[test]
    fn a_count_takes_the_function_around_the_one_you_are_in() {
        // `2af` inside a closure is the function holding it: the count climbs outwards, as `2i(`
        // does in Vim. Counting the same way for every kind is what lets the count be learned once.
        let source = "fn outer() {\n    let f = |x| {\n        x + 1\n    };\n}\n";
        let (text, tree) = structure(Language::Rust, source);
        let caret = at(&text, "x + 1", 0);
        let first = tree
            .object_nth(&text, caret, ObjectKind::Function, true, 1)
            .unwrap();
        assert_eq!(selected(&text, first), "|x| {\n        x + 1\n    }");
        let second = tree
            .object_nth(&text, caret, ObjectKind::Function, true, 2)
            .unwrap();
        assert_eq!(selected(&text, second), source);
        // Past the outermost there is nothing, and saying so beats quietly taking the outermost:
        // `d3af` that deleted the second function out would delete code nobody asked about.
        assert_eq!(
            tree.object_nth(&text, caret, ObjectKind::Function, true, 3),
            None
        );
        assert_eq!(
            tree.object_nth(&text, caret, ObjectKind::Function, true, 1),
            tree.object(&text, caret, ObjectKind::Function, true),
            "a count of one is the plain object"
        );
    }

    #[test]
    fn a_count_on_an_argument_climbs_to_the_list_outside() {
        // targets.vim's and nvim-treesitter-textobjects' reading: `2aa` on `b` inside `h(b, c)`,
        // itself an argument of `g`, is `h(b, c)` — the argument one list out — and not "b and c".
        let source = "fn f() { g(a, h(b, c)); }";
        let (text, tree) = structure(Language::Rust, source);
        let b = at(&text, "b, c", 0);
        let inner = tree
            .object_nth(&text, b, ObjectKind::Argument, false, 1)
            .unwrap();
        assert_eq!(selected(&text, inner), "b");
        let outer = tree
            .object_nth(&text, b, ObjectKind::Argument, false, 2)
            .unwrap();
        assert_eq!(selected(&text, outer), "h(b, c)");
        let outer_around = tree
            .object_nth(&text, b, ObjectKind::Argument, true, 2)
            .unwrap();
        assert_eq!(selected(&text, outer_around), ", h(b, c)");
        assert_eq!(
            tree.object_nth(&text, b, ObjectKind::Argument, false, 3),
            None
        );
    }

    #[test]
    fn a_count_on_a_type_and_on_a_comment() {
        // Types nest (a struct in a module's impl, a class in a class); comments never do, so a
        // count above one on a comment finds nothing rather than something unrelated.
        let source = "# note\nclass A:\n    class B:\n        x = 1\n";
        let (text, tree) = structure(Language::Python, source);
        let caret = at(&text, "x = 1", 0);
        let outer = tree
            .object_nth(&text, caret, ObjectKind::Type, true, 2)
            .unwrap();
        assert_eq!(
            selected(&text, outer),
            "class A:\n    class B:\n        x = 1\n"
        );
        let note = at(&text, "note", 0);
        assert!(
            tree.object_nth(&text, note, ObjectKind::Comment, true, 1)
                .is_some()
        );
        assert_eq!(
            tree.object_nth(&text, note, ObjectKind::Comment, true, 2),
            None
        );
    }

    #[test]
    fn shrinking_steps_to_the_named_node_under_the_caret() {
        // With no history of how the selection grew — a `vaf`, a drag — shrinking is the tree's
        // to answer: the named child holding the caret, and never one the same size as the
        // selection, which would make the key look broken.
        let source = "fn f() { g(a + b); }";
        let (text, tree) = structure(Language::Rust, source);
        let start = at(&text, "g(", 0);
        let end = at(&text, "; }", 0);
        let caret = at(&text, "a + b", 0);
        let args = tree.shrink(&text, start..end, caret).unwrap();
        assert_eq!(selected(&text, args.clone()), "(a + b)");
        let sum = tree.shrink(&text, args, caret).unwrap();
        assert_eq!(selected(&text, sum.clone()), "a + b");
        let a = tree.shrink(&text, sum, caret).unwrap();
        assert_eq!(selected(&text, a.clone()), "a");
        assert_eq!(
            tree.shrink(&text, a, caret),
            None,
            "a leaf has nothing inside"
        );
        // A caret on the selection's exclusive end — where an expansion leaves it — still means
        // the text inside: the last character, not the node that starts after it.
        let sum_start = at(&text, "a + b", 0);
        let sum_end = at(&text, "); }", 0);
        let b = tree.shrink(&text, sum_start..sum_end, sum_end).unwrap();
        assert_eq!(selected(&text, b), "b");
    }
}
