//! Project-wide search, on ripgrep's engine.
//!
//! `grep-searcher` and `grep-regex` are the same components ripgrep is built from. Writing this by
//! hand would mean reimplementing binary-file detection, encoding handling, multi-line matching and
//! a fast literal prefilter — all of it subtly, and slower.
//!
//! Search runs off the UI thread and streams results, so a match in the first file appears
//! immediately rather than after the whole project has been scanned.

use anyhow::{Context, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    /// 1-based, as every tool reports it.
    pub line_number: u64,
    pub line: String,
    /// Byte range of the match within `line`, for highlighting.
    pub range: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub pattern: String,
    /// Treat the pattern as a literal string rather than a regex.
    pub literal: bool,
    /// `None` means smart case: case-insensitive unless the pattern contains an uppercase letter.
    pub case_sensitive: Option<bool>,
    pub whole_word: bool,
    /// Stop after this many hits, so a match on `e` cannot flood the UI.
    pub limit: usize,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            literal: false,
            case_sensitive: None,
            whole_word: false,
            limit: 1_000,
        }
    }
}

impl SearchQuery {
    pub fn literal(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            literal: true,
            ..Default::default()
        }
    }

    pub fn regex(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            ..Default::default()
        }
    }
}

/// Searches one file, appending hits. Exposed because it is the unit the tests exercise and the
/// background search is built from.
pub fn search_file(path: &Path, query: &SearchQuery, hits: &mut Vec<SearchHit>) -> Result<()> {
    let pattern = if query.literal {
        regex_syntax_escape(&query.pattern)
    } else {
        query.pattern.clone()
    };

    let case_insensitive = match query.case_sensitive {
        Some(sensitive) => !sensitive,
        // Smart case: an uppercase letter in the pattern means the user meant it.
        None => !query.pattern.chars().any(char::is_uppercase),
    };

    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .word(query.whole_word)
        .build(&pattern)
        .with_context(|| format!("{} is not a valid pattern", query.pattern))?;

    let mut searcher = SearcherBuilder::new()
        // Binary files are skipped rather than dumped as mojibake into the results.
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();

    let limit = query.limit;
    searcher.search_path(
        &matcher,
        path,
        UTF8(|line_number, line| {
            if hits.len() >= limit {
                // Returning false stops the search for this file.
                return Ok(false);
            }
            // The matched span within the line, for highlighting.
            let trimmed = line.trim_end_matches(['\n', '\r']);
            let range = find_match_range(&matcher, trimmed).unwrap_or(0..trimmed.len());

            hits.push(SearchHit {
                path: path.to_path_buf(),
                line_number,
                line: trimmed.to_string(),
                range,
            });
            Ok(true)
        }),
    )?;

    Ok(())
}

fn find_match_range(
    matcher: &grep_regex::RegexMatcher,
    line: &str,
) -> Option<std::ops::Range<usize>> {
    // `Matcher` lives in grep-matcher, which grep-regex re-exports through its trait bound.
    use grep_matcher::Matcher;
    matcher
        .find(line.as_bytes())
        .ok()
        .flatten()
        .map(|m| m.start()..m.end())
}

/// Escapes regex metacharacters so a literal search means what it says.
fn regex_syntax_escape(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() * 2);
    for c in pattern.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A project search running on a background thread, streaming hits as it finds them.
pub struct ProjectSearch {
    hits: Receiver<Vec<SearchHit>>,
    cancelled: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl ProjectSearch {
    pub fn start(root: impl Into<PathBuf>, query: SearchQuery) -> Self {
        let root = root.into();
        let (sender, hits) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (flag, finished) = (cancelled.clone(), done.clone());

        std::thread::Builder::new()
            .name("vex-search".into())
            .spawn(move || {
                run(&root, &query, &sender, &flag);
                finished.store(true, Ordering::Release);
            })
            .expect("spawning the search thread");

        Self {
            hits,
            cancelled,
            done,
        }
    }

    /// Collects whatever has been found so far. Never blocks.
    pub fn poll(&self) -> Vec<SearchHit> {
        let mut all = Vec::new();
        while let Ok(batch) = self.hits.try_recv() {
            all.extend(batch);
        }
        all
    }

    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Stops the search. Called when the query changes — the old search's results are worthless the
    /// moment the user types another character.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for ProjectSearch {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(root: &Path, query: &SearchQuery, sender: &Sender<Vec<SearchHit>>, cancelled: &AtomicBool) {
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .parents(true)
        // As in `files`: ignore files apply whether or not this is a git repository.
        .require_git(false)
        .build();

    let mut total = 0usize;

    for entry in walker {
        if cancelled.load(Ordering::Relaxed) || total >= query.limit {
            return;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }

        let mut hits = Vec::new();
        if search_file(entry.path(), query, &mut hits).is_err() {
            // An unreadable or non-UTF-8 file is skipped, not fatal: one bad file must not end the
            // search.
            continue;
        }
        if hits.is_empty() {
            continue;
        }

        total += hits.len();
        // Sent per file, so results appear while the walk continues.
        if sender.send(hits).is_err() {
            return;
        }
    }
}
