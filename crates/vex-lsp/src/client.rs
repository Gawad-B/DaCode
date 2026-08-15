//! The client: a child process, two pipes, and a reader thread.
//!
//! No async runtime. A language server is one process and two pipes, which threads and channels
//! model directly — the same shape the syntax worker uses, and one fewer large dependency in an
//! editor whose premise is staying light.
//!
//! The rule from §1.1 applies unchanged: the UI thread **sends and polls, never waits**. Requests
//! return an id immediately; replies arrive later and are matched to what asked for them. Anything
//! computed against a superseded document version is discarded rather than applied.

use crate::position;
use crate::transport::{read_message, write_message};
use anyhow::{Context, Result};
use lsp_types::{Diagnostic, InlayHint, Location, Position, Range, TextEdit, WorkspaceEdit};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use vex_text::{Edit, Text, Version};

/// Spawns without a console window.
///
/// A language server is a console program; the editor's release build is a GUI application with no
/// console of its own, so Windows allocates the child a fresh one and shows it — a black window
/// appearing over the editor a second after it starts. The same rule as `vex::process::quiet`, which
/// is the copy to keep in step with this one: `vex-lsp` is below the binary in the dependency order
/// and cannot borrow it.
pub fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// What the client is waiting for.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum State {
    /// `initialize` sent, reply not yet received. No document notifications may be sent yet — a
    /// server is entitled to reject anything before it has answered.
    Starting,
    Ready,
    /// The process exited or the pipe closed.
    Stopped,
}

/// A request the editor is waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Initialize,
    Hover,
    Definition,
    Completion,
    /// A rename, with the name asked for — the reply carries edits but not what they were for, and
    /// the editor wants to say "renamed to `total`" rather than "renamed".
    Rename {
        new_name: String,
    },
    Formatting,
    CodeAction,
    References,
    InlayHints,
    /// Asking for the rest of one suggestion. Carries where it sits in the list, since the reply
    /// says nothing about which item it is for.
    ResolveCompletion {
        at: usize,
        /// The label of the item asked about. A reply carries no identity of its own, and a list
        /// can be replaced while one is in flight — rust-analyzer marks nearly every list
        /// incomplete, so typing one more character fetches a new one. Without this the reply
        /// lands on whatever now sits at that index, and grafts one suggestion's import onto
        /// another's name.
        label: String,
    },
    /// Everything named in one file, for an outline. Carries the file, because the hierarchical
    /// form of the reply names no URI at all — it is all one document, and the server assumes the
    /// client remembers which.
    DocumentSymbols {
        path: PathBuf,
    },
    /// Everything in the project matching a query. Carries the query, because replies to an
    /// out-of-date one arrive after the user has typed on and must be discarded — the same
    /// staleness rule as everywhere else here, keyed on the text instead of a version.
    WorkspaceSymbols {
        query: String,
    },
    Shutdown,
}

/// Something the server said.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// The handshake completed; document notifications may now be sent.
    Ready {
        server_name: String,
        /// Whether it answers `textDocument/formatting` at all.
        formats: bool,
    },
    /// Diagnostics for a file, with the document version they were computed against when the
    /// server supplied one.
    Diagnostics {
        path: PathBuf,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    Hover {
        text: String,
    },
    Definition {
        path: PathBuf,
        position: Position,
    },
    Completion {
        items: Vec<CompletionItem>,
        /// The server says this list is not the whole answer and it should be asked again as more
        /// is typed. rust-analyzer sets it whenever it truncated, and filtering a truncated list
        /// locally forever is how an editor comes to insist a name does not exist.
        incomplete: bool,
    },
    /// One suggestion, filled in. `at` is where it sat in the list that was offered.
    CompletionResolved {
        at: usize,
        /// What the item was labelled when it was asked about, so a reply for a list that has
        /// since been replaced can be told apart from one for the list on screen.
        label: String,
        item: CompletionItem,
    },
    /// Everything named, in one file or across the project.
    ///
    /// `query` is the text the project-wide search was for, and is `None` for a file outline. The
    /// editor refuses a reply whose query is not the one being typed now — replies arrive out of
    /// order, and a list that flickers back to an older answer is worse than one that waits.
    Symbols {
        query: Option<String>,
        symbols: Vec<Symbol>,
    },
    /// Edits for a rename, possibly across several files.
    Rename {
        new_name: String,
        edit: WorkspaceEdit,
    },
    /// Edits that reformat the open document.
    Formatting {
        edits: Vec<TextEdit>,
    },
    /// What the server offers to do about the code under the cursor.
    CodeActions {
        actions: Vec<CodeActionItem>,
    },
    /// Everywhere a symbol is used.
    References {
        locations: Vec<Location>,
    },
    /// Types and parameter names to show inline.
    InlayHints {
        hints: Vec<InlayHint>,
    },
    /// The server reported a problem, or logged one.
    Message {
        text: String,
    },
    /// A request came back as an error.
    ///
    /// Separate from `Message`, which carries the server's log chatter: a failed request is something
    /// the user asked for and did not get, so it belongs on screen. `retry` marks the errors the
    /// protocol defines as transient — `ContentModified` and `ServerCancelled` mean "ask again",
    /// which is worth saying rather than leaving the user to guess.
    RequestFailed {
        what: String,
        reason: String,
        retry: bool,
    },
    Stopped,
}

/// One suggestion, as the server described it.
///
/// The parse used to keep the label and throw the rest away, which is three separate defects
/// wearing one coat. The label is what you *read*: rust-analyzer labels a macro `println!` and
/// expects `println!($0)` to be inserted, so inserting the label gives you `println!` with no
/// parentheses and the cursor after it. The `detail` is the type, without which a list of bare
/// names cannot tell a field from a method. And `additional` is where every server puts the
/// **import** — completing `HashMap` is supposed to add `use std::collections::HashMap;` at the
/// top of the file, which is the single most valuable thing completion does and which vex was
/// silently discarding.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionItem {
    pub label: String,
    /// What to insert. Falls back to the label when the server named nothing else.
    pub insert: String,
    /// Whether `insert` carries `$1` / `${1:name}` / `$0` placeholders.
    pub snippet: bool,
    /// A type or signature, shown beside the label.
    pub detail: Option<String>,
    /// A short tag — `fn`, `var`, `mod` — rather than the protocol's number.
    pub kind: &'static str,
    /// The server's own ranking. rust-analyzer's is far better than anything an editor can invent
    /// from a string, so it is kept rather than re-derived.
    pub sort: Option<String>,
    /// What to match the typed prefix against, when that is not the label.
    pub filter: Option<String>,
    /// Edits elsewhere in the file that have to land with it — the import, nearly always.
    pub additional: Vec<TextEdit>,
    pub deprecated: bool,
    /// The item exactly as it came off the wire.
    ///
    /// Kept because `completionItem/resolve` must be handed the item back *as received*: servers
    /// put an opaque `data` blob on it that only they can read, and reconstructing something that
    /// looks similar is not the same request.
    pub raw: serde_json::Value,
}

/// The protocol's `CompletionItemKind`, as something short enough to sit in a popup.
///
/// A tag rather than an icon: the bundled fonts have no symbol set, and a missing glyph renders as
/// a tofu box — which is how the file tree's chevrons first shipped.
fn completion_kind(number: Option<u64>) -> &'static str {
    match number.unwrap_or(0) {
        2 => "method",
        3 => "fn",
        4 => "new",
        5 => "field",
        6 => "var",
        7 => "class",
        8 => "trait",
        9 => "mod",
        10 => "prop",
        11 => "unit",
        12 => "value",
        13 => "enum",
        14 => "kw",
        15 => "snip",
        16 => "color",
        17 => "file",
        18 => "ref",
        19 => "dir",
        20 => "variant",
        21 => "const",
        22 => "struct",
        23 => "event",
        24 => "op",
        25 => "type",
        // 1 is Text, and 0 is "the server did not say" — neither is worth a column.
        _ => "",
    }
}

/// A named thing in the code: a function, a type, a field.
///
/// One type for both `textDocument/documentSymbol` and `workspace/symbol`, because to the person
/// reading a list they are the same question asked at two scopes, and the palette that shows them
/// should not need to know which one it is looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// A short tag — `fn`, `struct`, `mod` — rather than the protocol's number.
    pub kind: &'static str,
    /// What encloses it: the `impl` block or module a server named, or the file it is in.
    pub container: String,
    pub path: PathBuf,
    pub position: Position,
    /// How deeply nested, so a file outline can be indented into the shape of the file.
    pub depth: usize,
}

/// The protocol's `SymbolKind`, as something short enough to sit in a list.
fn symbol_kind(number: Option<u64>) -> &'static str {
    match number.unwrap_or(0) {
        1 => "file",
        2 => "mod",
        3 => "ns",
        4 => "pkg",
        5 => "class",
        6 => "method",
        7 => "prop",
        8 => "field",
        9 => "new",
        10 => "enum",
        11 => "trait",
        12 => "fn",
        13 => "var",
        14 => "const",
        15 => "str",
        16 => "num",
        17 => "bool",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "variant",
        23 => "struct",
        24 => "event",
        25 => "op",
        26 => "type",
        _ => "",
    }
}

/// Flattens a symbol reply, whichever shape it arrived in.
///
/// `documentSymbol` answers with either a *hierarchy* (`DocumentSymbol`, carrying `children` and no
/// path, since it is all one file) or the older flat `SymbolInformation` with a `location`.
/// `workspace/symbol` answers with `SymbolInformation` or `WorkspaceSymbol`. All four are read here
/// rather than in the editor, so the palette sees one kind of thing.
///
/// `fallback` is the file the request was for, used by the hierarchical form, which names no URI at
/// all because the server assumes the client remembers which document it asked about.
pub fn symbols_from(value: &serde_json::Value, fallback: &Path) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    collect_symbols(value, fallback, "", 0, &mut symbols);
    symbols
}

fn collect_symbols(
    value: &serde_json::Value,
    fallback: &Path,
    container: &str,
    depth: usize,
    into: &mut Vec<Symbol>,
) {
    let Some(list) = value.as_array() else {
        return;
    };

    for item in list {
        let Some(name) = item.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let kind = symbol_kind(item.get("kind").and_then(|k| k.as_u64()));

        // A hierarchical symbol carries `selectionRange` — the name itself — where a flat one
        // carries a whole `location`. Preferring the selection range is what makes choosing a
        // symbol land on its name rather than on the first line of its body.
        let (path, position) = match item.get("location") {
            Some(location) => {
                let path = location
                    .get("uri")
                    .and_then(|uri| uri.as_str())
                    .and_then(position::uri_to_path)
                    .unwrap_or_else(|| fallback.to_path_buf());
                (path, range_start(location.get("range")))
            }
            None => (
                fallback.to_path_buf(),
                range_start(item.get("selectionRange").or_else(|| item.get("range"))),
            ),
        };

        let own = item
            .get("containerName")
            .and_then(|c| c.as_str())
            .unwrap_or(container);

        into.push(Symbol {
            name: name.to_string(),
            kind,
            container: own.to_string(),
            path,
            position,
            depth,
        });

        if let Some(children) = item.get("children") {
            // The enclosing name becomes the children's container, which is what makes a method
            // read as `new` under `Anchor` rather than as an unattached `new`.
            collect_symbols(children, fallback, name, depth + 1, into);
        }
    }
}

fn range_start(range: Option<&serde_json::Value>) -> Position {
    let read = |key: &str| {
        range?
            .get("start")?
            .get(key)?
            .as_u64()
            .map(|value| value as u32)
    };
    Position {
        line: read("line").unwrap_or(0),
        character: read("character").unwrap_or(0),
    }
}

/// One thing a server offers to do about the code under the cursor.
///
/// Only actions carrying edits survive the parse: an action whose work is a server-side `command` is
/// something vex cannot perform, and listing it would offer the user a menu entry that does nothing.
#[derive(Debug, Clone)]
pub struct CodeActionItem {
    pub title: String,
    /// `quickfix`, `refactor.extract`, and so on. Shown so a list of six is readable at a glance.
    pub kind: String,
    pub edit: WorkspaceEdit,
}

/// What a request was, in the user's terms rather than the protocol's.
fn describe_request(kind: &Pending) -> String {
    match kind {
        Pending::Initialize => "starting the language server".into(),
        Pending::Hover => "hover".into(),
        Pending::Definition => "go to definition".into(),
        Pending::Completion => "completion".into(),
        Pending::Rename { new_name } => format!("rename to `{new_name}`"),
        Pending::Formatting => "formatting".into(),
        Pending::CodeAction => "code actions".into(),
        Pending::References => "find references".into(),
        Pending::InlayHints => "inlay hints".into(),
        Pending::ResolveCompletion { .. } => "the rest of a suggestion".into(),
        Pending::DocumentSymbols { .. } => "this file's symbols".into(),
        Pending::WorkspaceSymbols { query } => format!("symbols matching `{query}`"),
        Pending::Shutdown => "shutdown".into(),
    }
}

/// A running language server.
pub struct LspClient {
    child: Child,
    /// Guarded because the reader thread never writes, but the UI thread and `Drop` both do.
    writer: Arc<Mutex<BufWriter<std::process::ChildStdin>>>,
    events: Receiver<ServerEvent>,
    pending: Arc<Mutex<HashMap<i64, Pending>>>,
    next_id: AtomicI64,
    stopping: Arc<AtomicBool>,
    state: State,
    root: PathBuf,
    server_name: String,
}

impl LspClient {
    /// Spawns a server and sends `initialize`. Returns as soon as the process is running — the
    /// handshake completes asynchronously, announced by [`ServerEvent::Ready`].
    ///
    /// `executable` is a **resolved path**, not a bare name, and `name` is what to call it in the
    /// interface. Passing the bare name is what broke every npm-installed server on Windows: see
    /// [`crate::resolve_in`].
    pub fn start(executable: &Path, name: &str, args: &[&str], root: &Path) -> Result<Self> {
        let mut command = Command::new(executable);
        no_window(&mut command);
        let mut child = command
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Kept, not inherited: a chatty server must not scribble over the editor's own output.
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "could not start {name} from {} — it resolved on PATH but would not run",
                    executable.display()
                )
            })?;

        let stdin = child.stdin.take().context("no stdin pipe")?;
        let stdout = child.stdout.take().context("no stdout pipe")?;
        let stderr = child.stderr.take();

        let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let pending: Arc<Mutex<HashMap<i64, Pending>>> = Arc::new(Mutex::new(HashMap::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let (sender, events) = channel();

        // Reader thread: parses framed messages and turns them into events.
        {
            let pending = pending.clone();
            let sender = sender.clone();
            let stopping = stopping.clone();
            std::thread::Builder::new()
                .name("vex-lsp-reader".into())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    loop {
                        match read_message(&mut reader) {
                            Ok(Some(message)) => {
                                if let Some(event) = interpret(&message, &pending) {
                                    if sender.send(event).is_err() {
                                        return;
                                    }
                                }
                            }
                            // Clean end of stream, or a framing error we cannot recover from: the
                            // stream is desynchronized, so stopping beats guessing.
                            Ok(None) => break,
                            Err(error) => {
                                if !stopping.load(Ordering::Relaxed) {
                                    let _ = sender.send(ServerEvent::Message {
                                        text: format!("protocol error: {error}"),
                                    });
                                }
                                break;
                            }
                        }
                    }
                    let _ = sender.send(ServerEvent::Stopped);
                })
                .context("spawning the LSP reader thread")?;
        }

        // Stderr thread: a server's diagnostics about *itself* are how "it silently did nothing"
        // becomes a readable message.
        if let Some(stderr) = stderr {
            let sender = sender.clone();
            let stopping = stopping.clone();
            std::thread::Builder::new()
                .name("vex-lsp-stderr".into())
                .spawn(move || {
                    use std::io::BufRead;
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        if stopping.load(Ordering::Relaxed) {
                            return;
                        }
                        if sender
                            .send(ServerEvent::Message {
                                text: format!("server: {line}"),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .context("spawning the LSP stderr thread")?;
        }

        let mut client = Self {
            child,
            writer,
            events,
            pending,
            next_id: AtomicI64::new(1),
            stopping,
            state: State::Starting,
            root: root.to_path_buf(),
            server_name: name.to_string(),
        };

        client.send_initialize()?;
        Ok(client)
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    fn next_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn request(&self, method: &str, params: serde_json::Value, kind: Pending) -> Result<i64> {
        let id = self.next_id();
        self.pending.lock().unwrap().insert(id, kind);

        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        write_message(&mut *self.writer.lock().unwrap(), &message)?;
        Ok(id)
    }

    fn notify(&self, method: &str, params: serde_json::Value) -> Result<()> {
        if trace_enabled() {
            eprintln!("lsp -> {method}");
        }
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_message(&mut *self.writer.lock().unwrap(), &message)
    }

    fn send_initialize(&mut self) -> Result<()> {
        let root_uri = position::path_to_uri(&self.root);

        // Only capabilities vex actually implements are declared. Claiming more invites a server to
        // send messages the editor will drop, which looks like a server bug from the outside.
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "definition": { "dynamicRegistration": false },
                    // `snippetSupport` is honest now: an accepted suggestion goes through the same
                    // expander `tab` uses, so `println!($0)` lands with the cursor between the
                    // parentheses rather than as a literal `$0` on screen.
                    //
                    // `resolveSupport` is not politeness — it is the switch that turns auto-import
                    // on. rust-analyzer will not offer an out-of-scope name at all unless the
                    // client can ask for its import separately, because computing one for each of
                    // a hundred candidates up front is too slow to be worth doing. Without this,
                    // completing `HashMap` in a file that does not import it simply offered
                    // nothing, and the reason was invisible from the editor's side.
                    "completion": {
                    // `labelDetailsSupport` keeps the label a name. Without it rust-analyzer folds
                    // the detail into the label itself for compatibility, and an auto-import
                    // candidate arrives labelled `HashMap(use std::collections::HashMap)` — which
                    // is the whole import path sitting in the column meant for the identifier.
                        "completionItem": {
                            "snippetSupport": true,
                            "deprecatedSupport": true,
                            "labelDetailsSupport": true,
                            "resolveSupport": {
                                "properties": ["additionalTextEdits", "detail", "documentation"]
                            }
                        },
                        "dynamicRegistration": false
                    },
                    // The hierarchical form, so a file outline can be indented into the shape of the
                    // file rather than being a flat list of every name in it.
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "rename": { "dynamicRegistration": false, "prepareSupport": false },
                    "formatting": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "inlayHint": { "dynamicRegistration": false },
                    "codeAction": {
                        "dynamicRegistration": false,
                        // No `codeActionLiteralSupport`: vex takes the plain command-or-edit form.
                        // Claiming literal support invites kinds the editor would then drop.
                        "resolveSupport": { "properties": [] }
                    }
                },
                "workspace": {
                    "workspaceFolders": true,
                    "symbol": { "dynamicRegistration": false },
                    // `documentChanges: false` is deliberate: it asks servers for the simpler
                    // `changes` map. vex applies text edits and refuses file creation and deletion,
                    // so promising to understand resource operations would be a lie.
                    "workspaceEdit": { "documentChanges": false, "failureHandling": "abort" }
                }
            },
            "clientInfo": { "name": "vex", "version": env!("CARGO_PKG_VERSION") },
        });

        self.request("initialize", params, Pending::Initialize)?;
        Ok(())
    }

    /// Completes the handshake. Called when the `initialize` reply arrives.
    fn finish_initialize(&mut self) -> Result<()> {
        self.notify("initialized", serde_json::json!({}))?;
        self.state = State::Ready;
        Ok(())
    }

    // ---- document synchronization ------------------------------------------------------

    pub fn did_open(&self, path: &Path, language_id: &str, version: i32, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": position::path_to_uri(path),
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        )
    }

    /// Sends the whole document.
    ///
    /// Incremental sync would send only the changed range, but it requires the client and server to
    /// agree exactly on every position — and a single disagreement corrupts the server's copy
    /// silently, producing diagnostics for text that does not exist. Full sync is more bytes and
    /// obviously correct; incremental is an optimization to make once there is a reason to.
    pub fn did_change(&self, path: &Path, version: i32, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path), "version": version },
                "contentChanges": [{ "text": text }],
            }),
        )
    }

    pub fn did_save(&self, path: &Path, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didSave",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path) },
                "text": text,
            }),
        )
    }

    pub fn did_close(&self, path: &Path) -> Result<()> {
        self.notify(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path) }
            }),
        )
    }

    // ---- requests ----------------------------------------------------------------------

    pub fn hover(&self, path: &Path, position: Position) -> Result<i64> {
        self.request(
            "textDocument/hover",
            document_position(path, position),
            Pending::Hover,
        )
    }

    pub fn definition(&self, path: &Path, position: Position) -> Result<i64> {
        self.request(
            "textDocument/definition",
            document_position(path, position),
            Pending::Definition,
        )
    }

    pub fn completion(&self, path: &Path, position: Position) -> Result<i64> {
        self.request(
            "textDocument/completion",
            document_position(path, position),
            Pending::Completion,
        )
    }

    /// Asks for a symbol to be renamed everywhere.
    /// Asks for the rest of one suggestion — its import, most usefully.
    ///
    /// The item goes back **exactly as it arrived**: the server put an opaque `data` blob on it
    /// that only it can read, and an item rebuilt from the parsed fields is a different request.
    pub fn resolve_completion(
        &self,
        at: usize,
        label: &str,
        item: &serde_json::Value,
    ) -> Result<i64> {
        self.request(
            "completionItem/resolve",
            item.clone(),
            Pending::ResolveCompletion {
                at,
                label: label.to_string(),
            },
        )
    }

    /// Everything named in one file.
    pub fn document_symbols(&self, path: &Path) -> Result<i64> {
        self.request(
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": position::path_to_uri(path) } }),
            Pending::DocumentSymbols {
                path: path.to_path_buf(),
            },
        )
    }

    /// Everything in the project matching a query.
    pub fn workspace_symbols(&self, query: &str) -> Result<i64> {
        self.request(
            "workspace/symbol",
            serde_json::json!({ "query": query }),
            Pending::WorkspaceSymbols {
                query: query.to_string(),
            },
        )
    }

    pub fn rename(&self, path: &Path, position: Position, new_name: &str) -> Result<i64> {
        let mut params = document_position(path, position);
        params["newName"] = serde_json::json!(new_name);
        self.request(
            "textDocument/rename",
            params,
            Pending::Rename {
                new_name: new_name.to_string(),
            },
        )
    }

    /// Asks for the whole document to be reformatted.
    ///
    /// Whole-document only. Range formatting exists in the protocol, but a formatter run on part of a
    /// file gives results that depend on where you stopped selecting, which is not a feature.
    pub fn formatting(&self, path: &Path, tab_width: usize) -> Result<i64> {
        self.request(
            "textDocument/formatting",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path) },
                "options": {
                    "tabSize": tab_width,
                    "insertSpaces": true,
                    "trimTrailingWhitespace": true,
                    "insertFinalNewline": true,
                },
            }),
            Pending::Formatting,
        )
    }

    /// Asks what can be done about the code in `range` — quick fixes, refactors, imports.
    pub fn code_action(
        &self,
        path: &Path,
        range: Range,
        diagnostics: &[Diagnostic],
    ) -> Result<i64> {
        self.request(
            "textDocument/codeAction",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path) },
                "range": range,
                // The diagnostics under the cursor are the context: without them a server has no
                // reason to offer the fix for the error you are sitting on.
                "context": { "diagnostics": diagnostics },
            }),
            Pending::CodeAction,
        )
    }

    /// Asks where a symbol is used.
    pub fn references(&self, path: &Path, position: Position) -> Result<i64> {
        let mut params = document_position(path, position);
        params["context"] = serde_json::json!({ "includeDeclaration": true });
        self.request("textDocument/references", params, Pending::References)
    }

    /// Asks for inlay hints over a range of the document.
    pub fn inlay_hints(&self, path: &Path, range: Range) -> Result<i64> {
        self.request(
            "textDocument/inlayHint",
            serde_json::json!({
                "textDocument": { "uri": position::path_to_uri(path) },
                "range": range,
            }),
            Pending::InlayHints,
        )
    }

    /// Collects everything the server has said. Never blocks.
    pub fn poll(&mut self) -> Vec<ServerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match &event {
                ServerEvent::Ready { .. } => {
                    if let Err(error) = self.finish_initialize() {
                        events.push(ServerEvent::Message {
                            text: format!("could not complete the handshake: {error}"),
                        });
                    }
                }
                ServerEvent::Stopped => self.state = State::Stopped,
                _ => {}
            }
            events.push(event);
        }
        events
    }

    /// Asks the server to exit, as the protocol requires: `shutdown`, then `exit`.
    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        let _ = self.request("shutdown", serde_json::Value::Null, Pending::Shutdown);
        let _ = self.notify("exit", serde_json::Value::Null);

        // A server that ignores `exit` must not outlive the editor.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => std::thread::yield_now(),
                _ => {
                    let _ = self.child.kill();
                    break;
                }
            }
        }
        self.state = State::Stopped;
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if self.state != State::Stopped {
            self.stop();
        }
    }
}

fn document_position(path: &Path, position: Position) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": position::path_to_uri(path) },
        "position": { "line": position.line, "character": position.character },
    })
}

/// Turns one incoming message into an event, matching replies to what asked for them.
fn trace_enabled() -> bool {
    std::env::var_os("VEX_LSP_TRACE").is_some()
}

fn interpret(
    message: &serde_json::Value,
    pending: &Arc<Mutex<HashMap<i64, Pending>>>,
) -> Option<ServerEvent> {
    if trace_enabled() {
        let label = message
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("reply to id {:?}", message.get("id")));
        eprintln!("lsp <- {label}");
    }
    // A notification: no id, has a method.
    if let Some(method) = message.get("method").and_then(|m| m.as_str()) {
        let params = message.get("params");
        return match method {
            "textDocument/publishDiagnostics" => {
                let params = params?;
                let uri = params.get("uri").and_then(|u| u.as_str());
                if trace_enabled() {
                    eprintln!("lsp    publishDiagnostics uri={uri:?}");
                }
                let path = position::uri_to_path(uri?)?;
                if trace_enabled() {
                    eprintln!(
                        "lsp    -> path {} version={:?} count={}",
                        path.display(),
                        params.get("version"),
                        params
                            .get("diagnostics")
                            .and_then(|d| d.as_array())
                            .map(|d| d.len())
                            .unwrap_or(0)
                    );
                }
                let diagnostics = params
                    .get("diagnostics")
                    .and_then(|d| serde_json::from_value(d.clone()).ok())
                    .unwrap_or_default();
                Some(ServerEvent::Diagnostics {
                    path,
                    // Servers that support it tell us which version they analyzed. Without it, the
                    // editor cannot know whether the result is current — see `Diagnostics::apply`.
                    version: params
                        .get("version")
                        .and_then(|v| v.as_i64())
                        .map(|v| v as i32),
                    diagnostics,
                })
            }
            "window/showMessage" | "window/logMessage" => Some(ServerEvent::Message {
                text: params?.get("message")?.as_str()?.to_string(),
            }),
            // Progress, registration and the rest are ignored on purpose: vex declared it does not
            // support them, so there is nothing to do with them.
            _ => None,
        };
    }

    // A reply: has an id.
    let id = message.get("id")?.as_i64()?;
    let kind = pending.lock().unwrap().remove(&id)?;

    if let Some(error) = message.get("error") {
        let reason = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        // -32801 ContentModified, -32802 ServerCancelled: both mean the request raced with something
        // the server was doing, and both are worth retrying.
        let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
        return Some(ServerEvent::RequestFailed {
            what: describe_request(&kind),
            reason,
            retry: matches!(code, -32801 | -32802),
        });
    }

    let result = message.get("result")?;

    match kind {
        Pending::Initialize => Some(ServerEvent::Ready {
            server_name: result
                .get("serverInfo")
                .and_then(|info| info.get("name"))
                .and_then(|name| name.as_str())
                .unwrap_or("language server")
                .to_string(),
            // Whether asking it to format is worth doing at all. pyright — the most common Python
            // server — says no, and an editor that asks anyway gets silence and has nothing to
            // tell the user. Knowing up front is what lets it reach for an external formatter
            // instead. Either `true` or an options object means yes.
            formats: matches!(
                result
                    .get("capabilities")
                    .and_then(|c| c.get("documentFormattingProvider")),
                Some(serde_json::Value::Bool(true)) | Some(serde_json::Value::Object(_))
            ),
        }),
        Pending::Hover => {
            let text = hover_text(result)?;
            Some(ServerEvent::Hover { text })
        }
        Pending::Definition => {
            // The reply is a Location, a Location[], or a LocationLink[] — servers pick freely.
            let location = if result.is_array() {
                result.get(0)?
            } else {
                result
            };
            let uri = location
                .get("uri")
                .or_else(|| location.get("targetUri"))?
                .as_str()?;
            let range = location
                .get("range")
                .or_else(|| location.get("targetSelectionRange"))
                .or_else(|| location.get("targetRange"))?;
            let start = range.get("start")?;

            Some(ServerEvent::Definition {
                path: position::uri_to_path(uri)?,
                position: Position {
                    line: start.get("line")?.as_u64()? as u32,
                    character: start.get("character")?.as_u64()? as u32,
                },
            })
        }
        Pending::Rename { new_name } => {
            // Deserialized rather than walked by hand: a workspace edit has two interchangeable
            // shapes and an optional annotation wrapper, and hand-parsing that is how an editor ends
            // up silently dropping half a rename.
            //
            // A reply that will not parse is *reported*. Returning `None` here would make a failed
            // rename indistinguishable from a key that does nothing, which is the hardest class of
            // bug to find from the outside.
            match serde_json::from_value::<WorkspaceEdit>(result.clone()) {
                Ok(edit) => Some(ServerEvent::Rename { new_name, edit }),
                Err(error) => Some(ServerEvent::Message {
                    text: format!("rename to `{new_name}` was refused: {error}"),
                }),
            }
        }
        Pending::CodeAction => {
            // The reply mixes `Command` and `CodeAction` objects freely, and a `CodeAction` may
            // carry edits, a command, or neither until resolved. Only the ones vex can actually
            // carry out are kept — an offer the editor would silently drop is worse than no offer.
            let items = result.as_array()?;
            let actions = items
                .iter()
                .filter_map(|item| {
                    let title = item.get("title")?.as_str()?.to_string();
                    let edit = item.get("edit").and_then(|edit| {
                        serde_json::from_value::<WorkspaceEdit>(edit.clone()).ok()
                    });
                    edit.map(|edit| CodeActionItem {
                        title,
                        kind: item
                            .get("kind")
                            .and_then(|kind| kind.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        edit,
                    })
                })
                .collect();
            Some(ServerEvent::CodeActions { actions })
        }
        Pending::References => {
            let locations: Vec<Location> = serde_json::from_value(result.clone()).ok()?;
            Some(ServerEvent::References { locations })
        }
        Pending::InlayHints => {
            let hints: Vec<InlayHint> = serde_json::from_value(result.clone()).unwrap_or_default();
            Some(ServerEvent::InlayHints { hints })
        }
        Pending::Formatting => match serde_json::from_value::<Vec<TextEdit>>(result.clone()) {
            Ok(edits) => Some(ServerEvent::Formatting { edits }),
            Err(error) => Some(ServerEvent::Message {
                text: format!("formatting was refused: {error}"),
            }),
        },
        Pending::Completion => {
            // Either a CompletionList or a bare array.
            let items = result
                .get("items")
                .and_then(|i| i.as_array())
                .or_else(|| result.as_array())?;
            Some(ServerEvent::Completion {
                items: items.iter().filter_map(completion_item).collect(),
                incomplete: result
                    .get("isIncomplete")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            })
        }
        Pending::ResolveCompletion { at, label } => {
            completion_item(result).map(|item| ServerEvent::CompletionResolved { at, label, item })
        }
        Pending::DocumentSymbols { path } => Some(ServerEvent::Symbols {
            query: None,
            symbols: symbols_from(result, &path),
        }),
        Pending::WorkspaceSymbols { query } => Some(ServerEvent::Symbols {
            symbols: symbols_from(result, Path::new("")),
            query: Some(query),
        }),
        Pending::Shutdown => None,
    }
}

/// One item of a completion reply, or nothing if it has no label to show.
///
/// `textEdit` is read only for the **text** it carries, not its range. The range is in the
/// coordinates of the document version the request was made against, and by the time a suggestion
/// is accepted the user has typed the rest of the word — so the range is stale by construction.
/// The editor replaces the word it anchored the session at instead, which is the same span for
/// every item that survives its own prefix filter. `additionalTextEdits` *are* used with their
/// ranges, and can be: they are line-and-column, they sit on other lines, and an edit within one
/// line does not move another line's coordinates.
fn completion_item(item: &serde_json::Value) -> Option<CompletionItem> {
    let string = |key: &str| item.get(key)?.as_str().map(str::to_string);
    let label = string("label")?;

    // In preference order: the edit's text, `insertText`, then the label. A server that sends a
    // `textEdit` means it; `insertText` is the older spelling of the same intent.
    let insert = item
        .get("textEdit")
        .and_then(|edit| edit.get("newText"))
        .and_then(|text| text.as_str())
        .map(str::to_string)
        .or_else(|| string("insertText"))
        .unwrap_or_else(|| label.clone());

    Some(CompletionItem {
        label,
        insert,
        // `insertTextFormat: 2` is the protocol's word for "this has placeholders in it".
        snippet: item.get("insertTextFormat").and_then(|f| f.as_u64()) == Some(2),
        // `labelDetails.description` is where a server puts the thing that makes two identical
        // names different — for an auto-import candidate, the module it would come from, which is
        // the one fact that decides which `HashMap` you meant.
        detail: string("detail").or_else(|| {
            let details = item.get("labelDetails")?;
            let part = |key: &str| details.get(key)?.as_str().filter(|text| !text.is_empty());
            match (part("detail"), part("description")) {
                (Some(detail), Some(description)) => Some(format!("{detail}  {description}")),
                (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
                (None, None) => None,
            }
        }),
        kind: completion_kind(item.get("kind").and_then(|k| k.as_u64())),
        sort: string("sortText"),
        filter: string("filterText"),
        additional: item
            .get("additionalTextEdits")
            .and_then(|edits| serde_json::from_value::<Vec<TextEdit>>(edits.clone()).ok())
            .unwrap_or_default(),
        // Both spellings: `deprecated` is the old boolean, `tags: [1]` the one that replaced it.
        deprecated: item.get("deprecated").and_then(|d| d.as_bool()) == Some(true)
            || item
                .get("tags")
                .and_then(|tags| tags.as_array())
                .is_some_and(|tags| tags.iter().any(|tag| tag.as_u64() == Some(1))),
        raw: item.clone(),
    })
}

/// Hover contents come in three shapes across protocol versions.
fn hover_text(result: &serde_json::Value) -> Option<String> {
    let contents = result.get("contents")?;

    if let Some(text) = contents.as_str() {
        return Some(text.to_string());
    }
    if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
        return Some(value.to_string());
    }
    if let Some(list) = contents.as_array() {
        let joined: Vec<String> = list
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .map(str::to_string)
                    .or_else(|| part.get("value")?.as_str().map(str::to_string))
            })
            .collect();
        if !joined.is_empty() {
            return Some(joined.join("\n"));
        }
    }
    None
}

/// One edit, described where the diagnostics live: in LSP line-and-column pairs.
///
/// Built once per edit so that moving a diagnostic costs no rope access at all. `end` is where the
/// replaced text finished before the edit, and `new_end` is where that same point sits after it —
/// the difference between the two is what every position past the edit moves by.
struct Move {
    start: Position,
    end: Position,
    new_end: Position,
}

impl Move {
    fn of(before: &Text, edit: &Edit) -> Self {
        let start = position::to_lsp(before, edit.range.start);
        let end = position::to_lsp(before, edit.range.end);

        // UTF-16 code units, because that is the unit an LSP character is counted in. A line of
        // emoji is half as many characters as it is code units, and using the wrong one puts the
        // squiggle in the middle of one.
        let utf16 = |s: &str| s.chars().map(char::len_utf16).sum::<usize>() as u32;
        let lines = edit.text.matches('\n').count() as u32;
        let new_end = if lines == 0 {
            Position {
                line: start.line,
                character: start.character + utf16(&edit.text),
            }
        } else {
            Position {
                line: start.line + lines,
                character: utf16(edit.text.rsplit('\n').next().unwrap_or_default()),
            }
        };

        Self {
            start,
            end,
            new_end,
        }
    }

    /// Where `at` — a position in the text before these edits — lands after them.
    ///
    /// The edits of one change are all expressed against that same text and are ascending, so the
    /// deltas accumulate: the line shift of every edit above the position, and the column shift of
    /// only those that end on its own line.
    fn apply(moves: &[Move], at: Position) -> Position {
        let mut lines: i64 = 0;
        let mut characters: i64 = 0;

        for step in moves {
            if before_position(at, step.start) {
                break;
            }
            if before_position(at, step.end) {
                // Inside what was replaced. There is no honest answer, so it collapses to the front
                // of the edit — the same choice `transform_offset` makes for an offset.
                return step.start;
            }
            lines += i64::from(step.new_end.line) - i64::from(step.end.line);
            if at.line == step.end.line {
                characters += i64::from(step.new_end.character) - i64::from(step.end.character);
            }
        }

        Position {
            line: (i64::from(at.line) + lines).max(0) as u32,
            character: (i64::from(at.character) + characters).max(0) as u32,
        }
    }
}

fn before_position(a: Position, b: Position) -> bool {
    (a.line, a.character) < (b.line, b.character)
}

/// Diagnostics for one document, with the staleness rule applied in one place.
#[derive(Debug, Default, Clone)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
    /// The document version these describe, when the server reported one.
    version: Option<i32>,
}

impl Diagnostics {
    /// Accepts diagnostics only if they are not known to be stale.
    ///
    /// A server that reports versions makes this exact: a result for v3 is discarded once the buffer
    /// is at v4, because its ranges refer to text that has moved. A server that reports no version
    /// gets the benefit of the doubt — dropping everything from such a server would mean no
    /// diagnostics at all, which is worse than diagnostics that may lag by a keystroke.
    pub fn apply(
        &mut self,
        incoming: Vec<Diagnostic>,
        version: Option<i32>,
        buffer: Version,
    ) -> bool {
        let buffer_version = buffer.0 as i32;
        if let Some(version) = version {
            if version < buffer_version {
                return false;
            }
        }
        self.items = incoming;
        self.version = version;
        true
    }

    /// Moves the ranges over an edit, so an underline stays under the code it is about.
    ///
    /// A diagnostic is the slowest of the three things the editor holds outside the document — a
    /// round trip to another process — so the window in which its ranges describe text that has
    /// already moved is not the milliseconds a parse takes but as long as the server needs. Until
    /// this existed, typing on one line left the previous answer's squiggles scattered across the
    /// lines below it.
    ///
    /// **The arithmetic is done in the diagnostics' own coordinates, and that is the whole design.**
    /// The obvious version converts each range to an offset against the old text, moves it, and
    /// converts back — two rope lookups per diagnostic, which measured **39.6 ms** for a keystroke
    /// in a file carrying ten thousand of them, a hundred times the cost of the same keystroke in a
    /// small one. Here each *edit* is converted once, into the line-and-column pair its end lands
    /// at, and every diagnostic is then moved by comparing and adding integers. The rope is touched
    /// twice per edit rather than twice per diagnostic, and the same keystroke costs microseconds.
    pub fn transform(&mut self, before: &Text, edits: &[Edit]) {
        let moves: Vec<Move> = edits.iter().map(|edit| Move::of(before, edit)).collect();

        self.items.retain_mut(|item| {
            let was_empty = item.range.start == item.range.end;
            let start = Move::apply(&moves, item.range.start);
            let end = Move::apply(&moves, item.range.end);

            // A diagnostic pointing at code that has been deleted collapses. Dropping it is right,
            // but only when it had something to point at: "expected a token here" arrives empty
            // already, and dropping those would silently lose every diagnostic that names a
            // position rather than a span.
            if !was_empty && start == end {
                return false;
            }
            item.range = Range { start, end };
            true
        });
    }

    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    pub fn version(&self) -> Option<i32> {
        self.version
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Counts by severity, for the statusline: `(errors, warnings)`.
    pub fn counts(&self) -> (usize, usize) {
        use lsp_types::DiagnosticSeverity;
        let mut errors = 0;
        let mut warnings = 0;
        for item in &self.items {
            match item.severity {
                Some(DiagnosticSeverity::ERROR) => errors += 1,
                Some(DiagnosticSeverity::WARNING) => warnings += 1,
                _ => {}
            }
        }
        (errors, warnings)
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.version = None;
    }
}
