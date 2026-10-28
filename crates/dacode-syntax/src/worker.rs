//! The background highlighting thread.
//!
//! This is where the 60fps contract is enforced rather than merely stated:
//!
//! - The UI thread hands over a **snapshot** (a cheap structural clone of the rope) plus its
//!   version, and never waits for a reply.
//! - The worker parses and queries on its own thread, then sends back a result **tagged with the
//!   version it saw**.
//! - The UI thread applies a result only if that version still matches the buffer. Anything
//!   behind is discarded, never applied.
//!
//! Sustained typing is handled by splitting the work in two. The **parse** runs for every
//! version, because tree-sitter's incremental reparse needs each edit in the coordinates it was
//! expressed in — skipping one would leave the tree describing text that never existed. The
//! **query** runs only for the newest queued version, because intermediate highlight results
//! would be discarded as stale the moment they arrived. Parsing is the cheap half; querying and
//! building spans is not.

use crate::highlight::{HighlightSpan, Highlighter, Language};
use crate::structure::Structure;
use dacode_text::{CharOffset, Edit, Text, Version};
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

pub struct SyntaxRequest {
    pub text: Text,
    pub version: Version,
    /// Edits since the last request, so the tree can be reused.
    pub edits: Vec<Edit>,
    /// The visible char range: highlighting is bounded by the viewport, not the document.
    pub range: Range<CharOffset>,
    /// Discard the existing tree and parse from scratch. Set when the editor could not supply a
    /// complete edit sequence, which would otherwise corrupt the incremental reparse.
    pub resync: bool,
}

#[derive(Debug, Clone)]
pub struct SyntaxResult {
    pub version: Version,
    pub spans: Vec<HighlightSpan>,
    pub node_path: Vec<String>,
    pub has_error: bool,
    /// The tree this result was read from, for the UI thread's indentation and text objects.
    /// Tagged with the same version as everything else here, and as useless as the spans once
    /// that version is not the buffer's.
    pub structure: Option<Structure>,
}

impl SyntaxResult {
    /// Whether this result still describes the current buffer.
    pub fn is_fresh_for(&self, buffer: Version) -> bool {
        self.version == buffer
    }
}

/// A handle to the highlighting thread.
pub struct SyntaxWorker {
    requests: Sender<Message>,
    results: Receiver<SyntaxResult>,
    handle: Option<JoinHandle<()>>,
    /// The newest result received, whether or not it is still fresh.
    latest: Option<SyntaxResult>,
    /// The newest version asked for, so [`SyntaxWorker::pending`] can say whether an answer is owed.
    requested: Option<Version>,
}

/// Called from the worker thread each time a result is ready. Must be cheap and must not block.
///
/// This crate sits below the binary and cannot name `dacode::wake::Waker`, so it keeps its own copy
/// of the alias — **it must stay identical to that one**, as `dacode-lsp`'s process helper does.
type Waker = Arc<dyn Fn() + Send + Sync>;

enum Message {
    Work(Box<SyntaxRequest>),
    Stop,
}

fn build(highlighter: &Highlighter, request: &SyntaxRequest) -> SyntaxResult {
    SyntaxResult {
        version: request.version,
        spans: highlighter.highlights(&request.text, request.range.clone()),
        node_path: highlighter.node_path(&request.text, request.range.start),
        has_error: highlighter.has_error(),
        structure: highlighter.structure(request.version),
    }
}

fn wake(waker: &Option<Waker>) {
    if let Some(wake) = waker {
        wake();
    }
}

impl SyntaxWorker {
    pub fn spawn(language: Language) -> Result<Self, String> {
        Self::spawn_with(language, None)
    }

    /// Like [`spawn`](Self::spawn), but `waker` is called from the worker thread whenever a result
    /// has been sent. An event loop that sleeps until input would otherwise not look at the channel
    /// until the next keypress, and the colours would arrive a key late.
    pub fn spawn_with(language: Language, waker: Option<Waker>) -> Result<Self, String> {
        // Construct the highlighter on this thread so a bad grammar or query is a startup error
        // with a message, not a silently dead worker.
        let mut highlighter = Highlighter::new(language)?;

        let (request_tx, request_rx) = channel::<Message>();
        let (result_tx, result_rx) = channel::<SyntaxResult>();

        let handle = std::thread::Builder::new()
            .name("dacode-syntax".into())
            .spawn(move || {
                let mut previous: Option<Text> = None;

                while let Ok(message) = request_rx.recv() {
                    let request = match message {
                        Message::Work(request) => request,
                        Message::Stop => break,
                    };

                    // Parse this version: every version must be parsed, in order, or the tree's
                    // idea of the text diverges from the text.
                    if request.resync {
                        previous = None;
                        highlighter.reset();
                    }
                    if let Some(before) = &previous {
                        highlighter.edit(before, &request.text, &request.edits);
                    }
                    highlighter.parse(&request.text);
                    previous = Some(request.text.clone());

                    // If newer versions are already queued, skip the query: this result would be
                    // stale before it arrived. Drop through to parse the next one instead.
                    let mut superseded = false;
                    let mut stop = false;
                    let mut queued: Vec<Box<SyntaxRequest>> = Vec::new();
                    loop {
                        match request_rx.try_recv() {
                            Ok(Message::Work(newer)) => {
                                superseded = true;
                                queued.push(newer);
                            }
                            Ok(Message::Stop) => {
                                stop = true;
                                break;
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                stop = true;
                                break;
                            }
                        }
                    }

                    if superseded {
                        // Re-parse each queued version in order, then query only the last.
                        let last = queued.len().saturating_sub(1);
                        for (index, newer) in queued.into_iter().enumerate() {
                            if newer.resync {
                                previous = None;
                                highlighter.reset();
                            }
                            if let Some(before) = &previous {
                                highlighter.edit(before, &newer.text, &newer.edits);
                            }
                            highlighter.parse(&newer.text);
                            previous = Some(newer.text.clone());

                            if index == last {
                                let result = build(&highlighter, &newer);
                                if result_tx.send(result).is_err() {
                                    return;
                                }
                                wake(&waker);
                            }
                        }
                    } else {
                        let result = build(&highlighter, &request);
                        // A closed channel means the editor is shutting down.
                        if result_tx.send(result).is_err() {
                            break;
                        }
                        wake(&waker);
                    }

                    if stop {
                        break;
                    }
                }
            })
            .map_err(|err| format!("spawning the syntax thread: {err}"))?;

        Ok(Self {
            requests: request_tx,
            results: result_rx,
            handle: Some(handle),
            latest: None,
            requested: None,
        })
    }

    /// Queues work. Never blocks, so this is safe to call from the UI thread on every edit.
    pub fn request(&mut self, request: SyntaxRequest) {
        self.requested = Some(request.version);
        // A dead worker degrades highlighting; it must not take the editor down with it.
        let _ = self.requests.send(Message::Work(Box::new(request)));
    }

    /// Drains completed results, keeping the newest. Non-blocking by construction.
    pub fn poll(&mut self) -> Option<&SyntaxResult> {
        while let Ok(result) = self.results.try_recv() {
            self.latest = Some(result);
        }
        self.latest.as_ref()
    }

    /// Whether the newest request's version is different from the version of the newest result
    /// collected so far.
    ///
    /// Compares versions rather than counting messages, because the worker coalesces: forty queued
    /// edits produce one result, for the last of them. The consequence is that a request made
    /// again at a version that has already been answered reports *not* pending, though a second
    /// answer for it is still on its way; callers must not read this as "the worker is idle".
    pub fn pending(&self) -> bool {
        self.requested
            .is_some_and(|wanted| self.latest.as_ref().map(|result| result.version) != Some(wanted))
    }

    /// The newest result, or `None` if it is stale for this buffer version.
    ///
    /// The single place staleness is decided, so no caller can accidentally paint an out-of-date
    /// highlight over text that has moved.
    pub fn fresh_for(&self, buffer: Version) -> Option<&SyntaxResult> {
        self.latest.as_ref().filter(|r| r.is_fresh_for(buffer))
    }

    /// The newest result whether fresh or not, for the async ledger to report on.
    pub fn latest(&self) -> Option<&SyntaxResult> {
        self.latest.as_ref()
    }

    pub fn version(&self) -> Option<Version> {
        self.latest.as_ref().map(|result| result.version)
    }
}

impl Drop for SyntaxWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(Message::Stop);
        if let Some(handle) = self.handle.take() {
            // Joining keeps the thread from outliving the editor and writing into a dropped
            // channel; the worker checks for Stop between requests, so this is bounded.
            let _ = handle.join();
        }
    }
}
