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
use lsp_types::{Diagnostic, Position};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use vex_text::Version;

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
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Pending {
    Initialize,
    Hover,
    Definition,
    Completion,
    Shutdown,
}

/// Something the server said.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// The handshake completed; document notifications may now be sent.
    Ready {
        server_name: String,
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
        items: Vec<String>,
    },
    /// The server reported a problem, or logged one.
    Message {
        text: String,
    },
    Stopped,
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
    pub fn start(program: &str, args: &[&str], root: &Path) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Kept, not inherited: a chatty server must not scribble over the editor's own output.
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {program} — is it on PATH?"))?;

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
            server_name: program.to_string(),
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
                    "completion": {
                        "completionItem": { "snippetSupport": false },
                        "dynamicRegistration": false
                    }
                },
                "workspace": { "workspaceFolders": true }
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
        let text = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Some(ServerEvent::Message {
            text: format!("{kind:?} failed: {text}"),
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
        Pending::Completion => {
            // Either a CompletionList or a bare array.
            let items = result
                .get("items")
                .and_then(|i| i.as_array())
                .or_else(|| result.as_array())?;
            Some(ServerEvent::Completion {
                items: items
                    .iter()
                    .filter_map(|item| item.get("label")?.as_str().map(str::to_string))
                    .collect(),
            })
        }
        Pending::Shutdown => None,
    }
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
