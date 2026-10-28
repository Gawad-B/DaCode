//! Who last changed each line, and when: `git blame`, for the active file.
//!
//! Shells out to `git` for the same reason [`crate::git`] does — one question, one command, and the
//! binary already agrees with itself about worktrees and sparse checkouts. The blame is taken
//! against the **buffer**, not the file on disk: the snapshot goes in on stdin through
//! `git blame --contents -`, so a line typed since the last commit comes back attributed to nobody
//! (the all-zero hash) rather than to whichever commit last held that line number.
//!
//! # How it follows edits
//!
//! A blame is per line, and a line number held outside the document goes stale at the next
//! keystroke that adds or removes one — the same defect that once scattered diagnostics and highlight
//! spans across the wrong code. Running `git blame` again per keystroke would fix it the expensive
//! way: unlike `git show`, blame walks history, and on a long-lived file that is hundreds of
//! milliseconds of another process's time for every character typed.
//!
//! So the blame is **neither moved nor cleared** by `EditorState::mutate`. Instead `mutate` appends
//! each change's *line arithmetic* to an [`EditLog`] — where each edit started and ended, and how many
//! lines it left — and a line is looked up by walking that log backwards to the line it was when the
//! blame was taken. A line that an edit touched has no such line, and reads as
//! [`Attribution::Uncommitted`], which is exactly what `git blame` itself would say about it.
//!
//! Both halves are bounded. Recording is constant work per edit and never looks at the blame, so a
//! keystroke costs the same in a hundred-thousand-line file as in a small one (`latency.rs` holds
//! that). Looking up walks the log, which is capped at [`EditLog::CAP`] steps — and typing along one
//! line adds no steps at all, since a line already marked as touched cannot become more so. Past the
//! cap the log gives up, every line reads as unknown rather than guessed, and the editor asks for a
//! fresh blame. Drawing looks up only the rows on screen, so the cost is viewport × cap, never the
//! document.
//!
//! The honest limitation: the log knows lines, not their text. Whole-line edits are recognised —
//! deleting a line, inserting lines above one, pressing Enter at the end of one leave the
//! neighbours attributed — but splitting a line in the middle marks both halves, and a line typed
//! back to exactly what was committed stays "not committed yet". The next blame, on the next save,
//! puts both right.

use crate::wake::Waker;
use dacode_text::{Edit, Text, Version};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// One commit, as `git blame --porcelain` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub mail: String,
    /// Seconds since the epoch.
    pub time: i64,
    pub summary: String,
}

impl Commit {
    /// Whether this is git's stand-in for "not committed": the all-zero hash `--contents` gives
    /// every line that differs from `HEAD`.
    pub fn is_uncommitted(&self) -> bool {
        self.hash.bytes().all(|b| b == b'0')
    }

    /// The first seven characters, the length `git log --oneline` shows.
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }
}

/// Who a line belongs to, as far as the blame and the edits since it can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution<'a> {
    Commit(&'a Commit),
    /// Changed since `HEAD`, either before the blame was taken or by an edit after it.
    Uncommitted,
}

/// What `git blame --porcelain` said: every commit it named, and which one owns each line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub commits: Vec<Commit>,
    /// Per line of the blamed text, an index into `commits`.
    pub lines: Vec<u32>,
}

/// Reads porcelain output.
///
/// Each line of the file is a header — `<hash> <original line> <final line> [<group size>]` — then,
/// the first time a commit appears, its `author`, `author-time`, `summary` and the rest, then the
/// line's own text behind a tab. Anything not recognised is skipped rather than refused: git adds
/// keys between versions (`previous`, `boundary`), and a reader that rejected the unknown would stop
/// working on the next upgrade. Returns `None` only when nothing at all could be read.
pub fn parse_porcelain(output: &str) -> Option<Parsed> {
    let mut parsed = Parsed::default();
    let mut by_hash: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    let mut current: Option<u32> = None;

    for line in output.lines() {
        if line.starts_with('\t') {
            continue;
        }
        let mut words = line.splitn(2, ' ');
        let first = words.next().unwrap_or_default();
        let rest = words.next().unwrap_or_default();

        if is_hash(first) {
            let mut numbers = rest.split(' ').map(str::parse::<usize>);
            let (Some(Ok(_original)), Some(Ok(final_line))) = (numbers.next(), numbers.next())
            else {
                continue;
            };
            let index = *by_hash.entry(first).or_insert_with(|| {
                parsed.commits.push(Commit {
                    hash: first.to_string(),
                    author: String::new(),
                    mail: String::new(),
                    time: 0,
                    summary: String::new(),
                });
                (parsed.commits.len() - 1) as u32
            });
            current = Some(index);
            // One-based, and in the order of the *file* rather than of the output — git groups by
            // commit, not by line.
            let at = final_line.saturating_sub(1);
            if parsed.lines.len() <= at {
                parsed.lines.resize(at + 1, index);
            }
            parsed.lines[at] = index;
            continue;
        }

        let Some(index) = current else {
            continue;
        };
        let commit = &mut parsed.commits[index as usize];
        match first {
            "author" => commit.author = rest.to_string(),
            "author-mail" => {
                commit.mail = rest
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            }
            "author-time" => commit.time = rest.parse().unwrap_or(0),
            "summary" => commit.summary = rest.to_string(),
            _ => {}
        }
    }

    (!parsed.lines.is_empty()).then_some(parsed)
}

/// A SHA-1 or SHA-256 object name.
fn is_hash(word: &str) -> bool {
    (word.len() == 40 || word.len() == 64) && word.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One edit, reduced to the lines it replaced and the lines it left.
///
/// Both ranges are half-open. `old` is the lines of the text before that the edit changed, and
/// `new` the lines after it that hold changed text — every line in `new` is uncommitted, and every
/// line past it is the line `old.end` was, moved by the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    new_end: usize,
}

impl Step {
    /// Whether the edit left the line count alone and stayed on one line — a keystroke, nearly
    /// always.
    fn within_one_line(&self) -> bool {
        self.old_end == self.old_start + 1 && self.new_end == self.new_start + 1
    }
}

/// The changes since a blame was taken, as line arithmetic. See the module documentation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditLog {
    /// One entry per document change, oldest first. A change's steps are ascending and all
    /// expressed against the text before it, which is how `Document::changes_since` hands them over.
    changes: Vec<Vec<Step>>,
    steps: usize,
    /// More happened than the log keeps. Every line is then unknown, not guessed.
    lost: bool,
}

impl EditLog {
    /// How many steps the log keeps before it gives up. Lookups walk all of them, so this is what
    /// bounds a frame: a few hundred integer comparisons per visible row.
    pub const CAP: usize = 256;

    /// The steps one change makes, computed once and handed to every log that wants them.
    ///
    /// A handful of rope lookups per edit and nothing per line of the document: this runs inside
    /// every keystroke.
    ///
    /// Whole lines are recognised, because the common line edits would otherwise mark a line they
    /// never touched. Deleting a line — from the start of one line to the start of the next — leaves
    /// the line below it exactly as it was, and so does inserting whole lines at the start of one.
    /// Pressing Enter at the end of a line is the same insertion seen from the line above.
    /// Anything else marks every line it reaches, which errs towards "not committed yet" rather
    /// than towards naming a commit that never wrote the line.
    fn steps(before: &Text, edits: &[Edit]) -> Vec<Step> {
        let mut shift: isize = 0;
        edits
            .iter()
            .map(|edit| {
                let start = before.char_to_point(edit.range.start);
                let end = before.char_to_point(edit.range.end);
                let added = edit.text.matches('\n').count();
                let at_line_end =
                    |point: dacode_text::Point| point.column == before.line_len(point.line);

                let (old_start, old_end, touched) = if start.column == 0
                    && end.column == 0
                    && (edit.text.is_empty() || edit.text.ends_with('\n'))
                {
                    // Whole lines out, whole lines in.
                    (start.line, end.line, added)
                } else if edit.range.start == edit.range.end
                    && at_line_end(start)
                    && edit.text.starts_with('\n')
                {
                    // New lines after the end of this one, which itself is untouched.
                    (start.line + 1, start.line + 1, added)
                } else {
                    (start.line, end.line + 1, added + 1)
                };

                let new_start = (old_start as isize + shift).max(0) as usize;
                let new_end = new_start + touched;
                shift = new_end as isize - old_end as isize;
                Step {
                    old_start,
                    old_end,
                    new_start,
                    new_end,
                }
            })
            .collect()
    }

    fn record(&mut self, steps: &[Step]) {
        if self.lost || steps.is_empty() {
            return;
        }
        // Typing along a line it has already touched changes nothing a lookup could see: no line
        // moves, and the line is already uncommitted. Skipping it is what keeps a paragraph typed on
        // one line from filling the log a character at a time.
        if steps
            .iter()
            .all(|step| step.within_one_line() && self.origin(step.new_start).is_none())
        {
            return;
        }
        if self.steps + steps.len() > Self::CAP {
            self.lose();
            return;
        }
        self.steps += steps.len();
        self.changes.push(steps.to_vec());
    }

    fn lose(&mut self) {
        self.lost = true;
        self.changes.clear();
        self.steps = 0;
    }

    pub fn is_lost(&self) -> bool {
        self.lost
    }

    /// Where `line` of the current text was when the log began, or `None` if an edit touched it.
    ///
    /// Walks the changes newest first, undoing each one's line shift. Within a change the steps are
    /// ascending against the text before it, so the shift accumulates the way it did when it was
    /// applied.
    fn origin(&self, line: usize) -> Option<usize> {
        let mut line = line;
        for change in self.changes.iter().rev() {
            let mut shift: isize = 0;
            let mut found = None;
            for step in change {
                if line < step.new_start {
                    found = Some(line as isize - shift);
                    break;
                }
                if line < step.new_end {
                    return None;
                }
                shift = step.new_end as isize - step.old_end as isize;
            }
            line = found.unwrap_or(line as isize - shift).max(0) as usize;
        }
        Some(line)
    }
}

/// A blame of one snapshot of a file, with the edits made since it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blame {
    path: PathBuf,
    commits: Vec<Commit>,
    lines: Vec<u32>,
    log: EditLog,
}

impl Blame {
    pub fn new(path: PathBuf, parsed: Parsed) -> Self {
        Self {
            path,
            commits: parsed.commits,
            lines: parsed.lines,
            log: EditLog::default(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the edit log gave up, so every line reads as unknown until the next blame.
    pub fn is_lost(&self) -> bool {
        self.log.is_lost()
    }

    /// Who owns `line` of the current text, or `None` when that cannot be known: the log gave up,
    /// or the line is past the end of what was blamed.
    pub fn at(&self, line: usize) -> Option<Attribution<'_>> {
        if self.log.is_lost() {
            return None;
        }
        let Some(origin) = self.log.origin(line) else {
            return Some(Attribution::Uncommitted);
        };
        let commit = &self.commits[*self.lines.get(origin)? as usize];
        Some(if commit.is_uncommitted() {
            Attribution::Uncommitted
        } else {
            Attribution::Commit(commit)
        })
    }

    /// The commit `line` belongs to, when it belongs to one, for "show commit".
    pub fn commit_at(&self, line: usize) -> Option<&Commit> {
        match self.at(line)? {
            Attribution::Commit(commit) => Some(commit),
            Attribution::Uncommitted => None,
        }
    }
}

/// A request sent and not yet answered, and the edits made since its snapshot was taken.
///
/// Kept apart from the blame on screen because the two logs start at different texts. The answer
/// lands with this log attached, so a blame taken half a second ago is still right about every line
/// typed since — and it lands even while typing continues, where refusing every answer that was
/// overtaken by a keystroke would leave a fast typist with no fresh blame at all.
#[derive(Debug, Clone)]
struct Pending {
    path: PathBuf,
    version: Version,
    log: EditLog,
}

/// The blame the editor holds, and what it shows of it.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    shown: Option<Blame>,
    pending: Option<Pending>,
    /// An answer arrived after its own log had given up, so another must be asked for.
    retry: bool,
    /// Whether the caret's line carries its blame after its end. The `inline_blame` setting.
    pub inline: bool,
    /// Whether the gutter has a blame column. The `toggle_blame` action.
    pub column: bool,
}

impl Tracker {
    /// Nothing blamed yet, with the inline note on or off as the setting says.
    pub fn new(inline: bool) -> Self {
        Self {
            inline,
            ..Self::default()
        }
    }

    /// Whether anything here holds line numbers, so `mutate` has to tell it about an edit.
    pub fn is_tracking(&self) -> bool {
        self.shown.is_some() || self.pending.is_some()
    }

    /// Whether either view wants a blame at all. With both off nothing is asked of git.
    pub fn wanted(&self) -> bool {
        self.inline || self.column
    }

    /// Records one document change. Called by `EditorState::mutate` for every change it replays.
    pub fn record(&mut self, before: &Text, edits: &[Edit]) {
        if !self.is_tracking() {
            return;
        }
        let steps = EditLog::steps(before, edits);
        if let Some(blame) = &mut self.shown {
            blame.log.record(&steps);
        }
        if let Some(pending) = &mut self.pending {
            pending.log.record(&steps);
        }
    }

    /// The edits can no longer be replayed — the document's change queue was truncated — so no line
    /// number held here can be trusted.
    pub fn lose(&mut self) {
        if let Some(blame) = &mut self.shown {
            blame.log.lose();
        }
        if let Some(pending) = &mut self.pending {
            pending.log.lose();
        }
    }

    /// A blame of `path` at `version` has been asked for. Its log starts now.
    pub fn requested(&mut self, path: PathBuf, version: Version) {
        self.pending = Some(Pending {
            path,
            version,
            log: EditLog::default(),
        });
        self.retry = false;
    }

    /// Takes an answer, if it is the one being waited for. Returns whether anything changed.
    ///
    /// Refused when it is about another file or another snapshot: a newer request has replaced the
    /// one this answers, and its log describes edits since a different text. `None` is git saying
    /// there is no blame to have — not a repository, a file it does not track, no `git` at all —
    /// which clears what was shown and is not an error anyone needs to hear about.
    pub fn accept(&mut self, path: &Path, version: Version, blame: Option<Blame>) -> bool {
        let matches = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.path == path && pending.version == version);
        if !matches {
            return false;
        }
        let Some(pending) = self.pending.take() else {
            return false;
        };
        if pending.log.is_lost() {
            self.retry = true;
            return false;
        }
        self.shown = blame.map(|mut blame| {
            blame.log = pending.log;
            blame
        });
        true
    }

    /// Whether a fresh blame should be asked for because the one held can no longer answer.
    pub fn needs_refresh(&self) -> bool {
        self.pending.is_none()
            && (self.retry || self.shown.as_ref().is_some_and(|blame| blame.log.is_lost()))
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Forgets everything: the buffer it described has gone.
    pub fn clear(&mut self) {
        self.shown = None;
        self.pending = None;
        self.retry = false;
    }

    pub fn blame(&self) -> Option<&Blame> {
        self.shown.as_ref()
    }

    /// Sets what is shown directly. For the screenshot path and tests, which need a known blame
    /// rather than whatever the repository happens to say.
    pub fn show(&mut self, blame: Blame) {
        self.shown = Some(blame);
        self.pending = None;
    }

    pub fn at(&self, line: usize) -> Option<Attribution<'_>> {
        self.shown.as_ref()?.at(line)
    }
}

/// How long ago, in words: "3 days ago".
pub fn ago(then: i64, now: i64) -> String {
    let seconds = (now - then).max(0);
    let (count, unit) = match seconds {
        s if s < 60 => return "just now".into(),
        s if s < 3_600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3_600, "hour"),
        s if s < 7 * 86_400 => (s / 86_400, "day"),
        s if s < 30 * 86_400 => (s / (7 * 86_400), "week"),
        s if s < 365 * 86_400 => (s / (30 * 86_400), "month"),
        s => (s / (365 * 86_400), "year"),
    };
    format!("{count} {unit}{} ago", if count == 1 { "" } else { "s" })
}

/// How long ago, in as few characters as say it: "3d". For the gutter column, where every character
/// is taken from the code.
pub fn ago_short(then: i64, now: i64) -> String {
    let seconds = (now - then).max(0);
    match seconds {
        s if s < 60 => "now".into(),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s if s < 7 * 86_400 => format!("{}d", s / 86_400),
        s if s < 30 * 86_400 => format!("{}w", s / (7 * 86_400)),
        s if s < 365 * 86_400 => format!("{}mo", s / (30 * 86_400)),
        s => format!("{}y", s / (365 * 86_400)),
    }
}

/// What is written after the caret's line: "Ann Example, 3 days ago · Fix the parser".
pub fn inline_text(attribution: Attribution<'_>, now: i64) -> String {
    match attribution {
        Attribution::Uncommitted => "not committed yet".into(),
        Attribution::Commit(commit) => format!(
            "{}, {} · {}",
            commit.author,
            ago(commit.time, now),
            commit.summary
        ),
    }
}

/// Seconds since the epoch, for the relative dates. Drawing asks once per frame.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// What the worker is asked.
pub enum Request {
    /// Blame this snapshot of `path`.
    Blame {
        path: PathBuf,
        text: String,
        version: Version,
    },
    /// The whole of one commit: hash, author, date and full message.
    Show { path: PathBuf, hash: String },
}

/// What the worker answers. Every arm names the file, for the reason [`crate::git::GitResult`] does:
/// a `Version` counts from 1 per document and is not an identity.
pub enum BlameResult {
    Blame {
        path: PathBuf,
        version: Version,
        blame: Option<Blame>,
    },
    Commit {
        path: PathBuf,
        text: String,
    },
    /// A blame request skipped for a newer one. Sent so that one request always gets one answer,
    /// which is what lets [`BlameWorker::busy`] return to zero.
    Superseded,
}

/// A background thread that runs `git blame` and `git show`.
///
/// One thread, and blame requests coalesce: a blame walks history, and two of them queued for the
/// same file only ever need the newer.
pub struct BlameWorker {
    requests: Sender<Request>,
    results: Receiver<BlameResult>,
    outstanding: std::cell::Cell<usize>,
}

impl BlameWorker {
    pub fn spawn() -> Self {
        Self::spawn_with(None)
    }

    /// Like [`spawn`](Self::spawn), with `waker` called after every answer so a sleeping loop
    /// collects it.
    pub fn spawn_with(waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<Request>();
        let (outgoing, results) = channel::<BlameResult>();

        std::thread::spawn(move || {
            let send = |result: BlameResult| {
                let sent = outgoing.send(result).is_ok();
                if let Some(wake) = &waker {
                    wake();
                }
                sent
            };
            while let Ok(first) = incoming.recv() {
                // Everything queued, sorted: every `Show` is answered, and of the blames only the
                // newest is run.
                let mut blame = None;
                let mut shows = Vec::new();
                let mut skipped = 0usize;
                let mut next = Some(first);
                while let Some(request) = next.take() {
                    match request {
                        Request::Blame { .. } => {
                            if blame.replace(request).is_some() {
                                skipped += 1;
                            }
                        }
                        Request::Show { path, hash } => shows.push((path, hash)),
                    }
                    next = incoming.try_recv().ok();
                }

                for _ in 0..skipped {
                    if !send(BlameResult::Superseded) {
                        return;
                    }
                }
                for (path, hash) in shows {
                    let text = show(&path, &hash).unwrap_or_else(|| {
                        format!("commit {hash}\n\ngit could not describe this commit")
                    });
                    if !send(BlameResult::Commit { path, text }) {
                        return;
                    }
                }
                if let Some(Request::Blame {
                    path,
                    text,
                    version,
                }) = blame
                {
                    let blame =
                        run_blame(&path, &text).map(|parsed| Blame::new(path.clone(), parsed));
                    if !send(BlameResult::Blame {
                        path,
                        version,
                        blame,
                    }) {
                        return;
                    }
                }
            }
        });

        Self {
            requests,
            results,
            outstanding: std::cell::Cell::new(0),
        }
    }

    pub fn request(&self, request: Request) {
        if self.requests.send(request).is_ok() {
            self.outstanding.set(self.outstanding.get() + 1);
        }
    }

    /// The next answer, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<BlameResult> {
        let result = self.results.try_recv().ok();
        if result.is_some() {
            self.outstanding
                .set(self.outstanding.get().saturating_sub(1));
        }
        result
    }

    /// Whether an answer is still owed, for a loop with no waker to stay awake for it.
    pub fn busy(&self) -> bool {
        self.outstanding.get() > 0
    }
}

/// The directory to run git in and the name to give it, or `None` for a path with no parent.
fn locate(path: &Path) -> Option<(PathBuf, std::ffi::OsString)> {
    // Through `canonical`: `git -C \\?\C:\x` is not a directory git will accept.
    let absolute = dacode_project::canonical(path);
    let directory = absolute.parent()?.to_path_buf();
    let name = absolute.file_name()?.to_os_string();
    Some((directory, name))
}

/// `git blame --porcelain --contents -` over `text`. `None` for anything git refuses.
fn run_blame(path: &Path, text: &str) -> Option<Parsed> {
    use std::io::Write;
    use std::process::Stdio;

    let (directory, name) = locate(path)?;
    let mut child = crate::process::command("git")
        .arg("-C")
        .arg(&directory)
        .args(["blame", "--porcelain", "--contents", "-", "--"])
        .arg(&name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // Written from a thread of its own: git starts answering before it has read everything, and a
    // file larger than the pipe's buffer would leave each side waiting for the other.
    let mut stdin = child.stdin.take()?;
    let input = text.to_string();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let output = child.wait_with_output().ok()?;
    let _ = writer.join();

    if !output.status.success() {
        return None;
    }
    parse_porcelain(&String::from_utf8_lossy(&output.stdout))
}

/// The whole of one commit, in the shape `git log` prints it.
fn show(path: &Path, hash: &str) -> Option<String> {
    if !is_hash(hash) {
        return None;
    }
    let (directory, _) = locate(path)?;
    let output = crate::process::command("git")
        .arg("-C")
        .arg(&directory)
        .args([
            "show",
            "-s",
            "--date=format:%Y-%m-%d %H:%M",
            "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n%B",
            hash,
        ])
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORCELAIN: &str = "\
0bbd9b9815f3458d69c6e1ed5895df69e70e894a 1 1 1
author Ann Example
author-mail <ann@example.invalid>
author-time 1700000000
author-tz +0300
committer Ann Example
committer-mail <ann@example.invalid>
committer-time 1700000000
committer-tz +0300
summary first commit
boundary
filename f.txt
\tone
ef2d0e71db26fca4b2e8ad52c7b6afc2548e34bf 2 2 1
author Bob Example
author-mail <bob@example.invalid>
author-time 1700086400
author-tz +0000
summary second
previous 0bbd9b9815f3458d69c6e1ed5895df69e70e894a f.txt
filename f.txt
\tTWO
0bbd9b9815f3458d69c6e1ed5895df69e70e894a 3 3 1
\tthree
0000000000000000000000000000000000000000 4 4 1
author External file (--contents)
author-mail <external.file>
author-time 1700090000
author-tz +0300
summary Version of f.txt from standard input
filename f.txt
\tfour
";

    fn parsed() -> Parsed {
        parse_porcelain(PORCELAIN).expect("the fixture is valid porcelain")
    }

    fn text(s: &str) -> Text {
        Text::from_str(s)
    }

    #[test]
    fn porcelain_names_each_commit_once_and_every_line_by_its_commit() {
        // Metadata comes only the first time a commit appears, so a reader that expected it on
        // every header would give line 3 an author of nobody.
        let parsed = parsed();
        assert_eq!(parsed.commits.len(), 3);
        assert_eq!(parsed.lines, vec![0, 1, 0, 2]);
        assert_eq!(parsed.commits[0].author, "Ann Example");
        assert_eq!(parsed.commits[0].mail, "ann@example.invalid");
        assert_eq!(parsed.commits[0].time, 1_700_000_000);
        assert_eq!(parsed.commits[1].summary, "second");
    }

    #[test]
    fn the_all_zero_hash_is_not_committed_yet() {
        let blame = Blame::new(PathBuf::from("f.txt"), parsed());
        assert_eq!(blame.at(3), Some(Attribution::Uncommitted));
        assert!(matches!(blame.at(1), Some(Attribution::Commit(c)) if c.author == "Bob Example"));
        assert_eq!(blame.at(4), None, "past the end is unknown, not anybody's");
    }

    #[test]
    fn porcelain_with_nothing_in_it_is_no_blame() {
        assert_eq!(parse_porcelain(""), None);
        assert_eq!(parse_porcelain("fatal: no such path 'x' in HEAD\n"), None);
    }

    #[test]
    fn a_line_inserted_above_moves_every_attribution_below_it() {
        // The defect this exists to prevent: a blame held by line number, after a line is added
        // above, names the commit of the line that *used* to be there.
        let mut blame = Blame::new(PathBuf::from("f.txt"), parsed());
        let before = text("one\nTWO\nthree\nfour\n");
        let steps = EditLog::steps(
            &before,
            &[Edit::insert(dacode_text::CharOffset(0), "zero\n")],
        );
        blame.log.record(&steps);

        assert_eq!(blame.at(0), Some(Attribution::Uncommitted), "the new line");
        assert!(
            matches!(blame.at(1), Some(Attribution::Commit(c)) if c.summary == "first commit"),
            "and the line it was inserted above is untouched"
        );
        assert!(matches!(blame.at(2), Some(Attribution::Commit(c)) if c.summary == "second"));
        assert!(matches!(blame.at(3), Some(Attribution::Commit(c)) if c.summary == "first commit"));
    }

    #[test]
    fn a_deleted_line_pulls_the_ones_below_it_up() {
        let mut blame = Blame::new(PathBuf::from("f.txt"), parsed());
        let before = text("one\nTWO\nthree\nfour\n");
        // Delete "one\n" entirely: lines 0..1 become line 0.
        let steps = EditLog::steps(
            &before,
            &[Edit::delete(
                dacode_text::CharOffset(0)..dacode_text::CharOffset(4),
            )],
        );
        blame.log.record(&steps);

        // Deleting a whole line touches nothing that is left: the line below moves up intact.
        assert!(matches!(blame.at(0), Some(Attribution::Commit(c)) if c.summary == "second"));
        assert!(matches!(blame.at(1), Some(Attribution::Commit(c)) if c.summary == "first commit"));
        assert_eq!(
            blame.at(2),
            Some(Attribution::Uncommitted),
            "four, still uncommitted"
        );
    }

    #[test]
    fn enter_at_the_end_of_a_line_leaves_that_line_its_author() {
        // The most common way a line is added. Marking the line above as well would make every new
        // line take its neighbour's history with it.
        let mut blame = Blame::new(PathBuf::from("f.txt"), parsed());
        let before = text("one\nTWO\nthree\nfour\n");
        blame.log.record(&EditLog::steps(
            &before,
            &[Edit::insert(dacode_text::CharOffset(7), "\n    ")],
        ));
        assert!(matches!(blame.at(1), Some(Attribution::Commit(c)) if c.summary == "second"));
        assert_eq!(blame.at(2), Some(Attribution::Uncommitted));
        assert!(matches!(blame.at(3), Some(Attribution::Commit(c)) if c.summary == "first commit"));

        // Splitting a line in the middle does change both halves.
        let mut blame = Blame::new(PathBuf::from("f.txt"), parsed());
        blame.log.record(&EditLog::steps(
            &before,
            &[Edit::insert(dacode_text::CharOffset(5), "\n")],
        ));
        assert_eq!(blame.at(1), Some(Attribution::Uncommitted));
        assert_eq!(blame.at(2), Some(Attribution::Uncommitted));
        assert!(matches!(blame.at(3), Some(Attribution::Commit(c)) if c.summary == "first commit"));
    }

    #[test]
    fn typing_along_one_line_does_not_fill_the_log() {
        // Each keystroke is a change. Were each one recorded, a paragraph typed on one line would
        // reach the cap and throw the blame away for nothing.
        let mut log = EditLog::default();
        let mut current = String::from("one\ntwo\n");
        for i in 0..EditLog::CAP * 2 {
            let before = text(&current);
            let at = dacode_text::CharOffset(5 + i);
            log.record(&EditLog::steps(&before, &[Edit::insert(at, "x")]));
            current.insert(5 + i, 'x');
        }
        assert!(!log.is_lost());
        assert_eq!(log.steps, 1);
        assert_eq!(log.origin(1), None);
        assert_eq!(log.origin(0), Some(0));
    }

    #[test]
    fn a_log_past_its_cap_knows_nothing_rather_than_guessing() {
        let mut blame = Blame::new(PathBuf::from("f.txt"), parsed());
        let mut current = String::from("one\nTWO\nthree\nfour\n");
        for _ in 0..=EditLog::CAP {
            let before = text(&current);
            blame.log.record(&EditLog::steps(
                &before,
                &[Edit::insert(dacode_text::CharOffset(0), "\n")],
            ));
            current.insert(0, '\n');
        }
        assert!(blame.log.is_lost());
        assert_eq!(blame.at(0), None);
    }

    #[test]
    fn a_stale_answer_is_refused() {
        // A request at version 3 was overtaken by one at version 5. The answer for 3 describes a
        // snapshot whose edits were never logged, so drawing it would attribute lines to the
        // commits of the lines that used to be there.
        let mut tracker = Tracker::default();
        let path = PathBuf::from("f.txt");
        tracker.requested(path.clone(), Version(3));
        tracker.requested(path.clone(), Version(5));

        let answer = Blame::new(path.clone(), parsed());
        assert!(!tracker.accept(&path, Version(3), Some(answer.clone())));
        assert!(tracker.blame().is_none());
        assert!(
            !tracker.accept(Path::new("other.txt"), Version(5), Some(answer.clone())),
            "nor one about another file with the same version"
        );
        assert!(tracker.accept(&path, Version(5), Some(answer)));
        assert!(tracker.blame().is_some());
    }

    #[test]
    fn edits_made_while_waiting_are_carried_onto_the_answer() {
        // The answer is half a second old when it lands. Without the pending log a line typed in
        // that half second would be attributed to whichever commit held its number.
        let mut tracker = Tracker::default();
        let path = PathBuf::from("f.txt");
        tracker.requested(path.clone(), Version(1));
        tracker.record(
            &text("one\nTWO\nthree\nfour\n"),
            &[Edit::insert(dacode_text::CharOffset(0), "new\n")],
        );
        assert!(tracker.accept(&path, Version(1), Some(Blame::new(path.clone(), parsed()))));
        assert_eq!(tracker.at(0), Some(Attribution::Uncommitted));
        assert!(
            matches!(tracker.at(1), Some(Attribution::Commit(c)) if c.summary == "first commit")
        );
    }

    #[test]
    fn relative_dates_read_as_a_person_would_say_them() {
        let day = 86_400;
        assert_eq!(ago(0, 30), "just now");
        assert_eq!(ago(0, 3 * day), "3 days ago");
        assert_eq!(ago(0, day), "1 day ago");
        assert_eq!(ago(0, 400 * day), "1 year ago");
        assert_eq!(ago_short(0, 3 * day), "3d");
        assert_eq!(ago_short(0, 60 * day), "2mo");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// A real repository with one file committed twice, by two authors.
    fn repository(label: &str) -> Option<PathBuf> {
        if crate::process::command("git")
            .arg("--version")
            .output()
            .map(|out| !out.status.success())
            .unwrap_or(true)
        {
            eprintln!("skipping: no git on this machine");
            return None;
        }
        let root =
            std::env::temp_dir().join(format!("dacode-blame-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("src")).ok()?;
        let git = |args: &[&str]| {
            crate::process::command("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        git(&["init", "-q"])?;
        git(&["config", "user.email", "ann@example.invalid"])?;
        git(&["config", "user.name", "Ann Example"])?;
        std::fs::write(root.join("src/main.rs"), "one\ntwo\nthree\n").ok()?;
        git(&["add", "."])?;
        git(&["commit", "-q", "-m", "first commit"])?;
        git(&["config", "user.name", "Bob Example"])?;
        std::fs::write(root.join("src/main.rs"), "one\nTWO\nthree\n").ok()?;
        git(&[
            "commit",
            "-q",
            "-am",
            "shout the second line\n\nBecause it matters.",
        ])?;
        Some(root)
    }

    fn answer(worker: &BlameWorker) -> Option<BlameResult> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(result) = worker.poll() {
                return Some(result);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn the_worker_blames_the_buffer_not_the_file_on_disk() {
        // The buffer has a line the disk does not. Blaming the disk would attribute it to the
        // commit of whatever line sits at that number there.
        let Some(root) = repository("buffer") else {
            return;
        };
        let worker = BlameWorker::spawn();
        let path = root.join("src/main.rs");
        worker.request(Request::Blame {
            path: path.clone(),
            text: "one\nTWO\nunsaved\nthree\n".into(),
            version: Version(4),
        });
        let Some(BlameResult::Blame {
            path: about,
            version,
            blame: Some(blame),
        }) = answer(&worker)
        else {
            panic!("a tracked file has a blame");
        };
        assert_eq!((about, version), (path.clone(), Version(4)));
        assert!(matches!(blame.at(0), Some(Attribution::Commit(c)) if c.author == "Ann Example"));
        assert!(matches!(blame.at(1), Some(Attribution::Commit(c)) if c.author == "Bob Example"));
        assert_eq!(blame.at(2), Some(Attribution::Uncommitted));
        assert!(matches!(blame.at(3), Some(Attribution::Commit(c)) if c.summary == "first commit"));

        // And the whole commit, message body included, for "show commit".
        let hash = blame.commit_at(1).expect("committed").hash.clone();
        worker.request(Request::Show {
            path: path.clone(),
            hash: hash.clone(),
        });
        let Some(BlameResult::Commit { text, .. }) = answer(&worker) else {
            panic!("a commit can be shown");
        };
        assert!(text.starts_with(&format!("commit {hash}")), "{text}");
        assert!(text.contains("Bob Example"), "{text}");
        assert!(text.contains("Because it matters."), "{text}");
        assert!(!worker.busy());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_git_does_not_track_has_no_blame_and_no_complaint() {
        let Some(root) = repository("untracked") else {
            return;
        };
        let fresh = root.join("src/new.rs");
        std::fs::write(&fresh, "fn main() {}\n").unwrap();
        let outside =
            std::env::temp_dir().join(format!("dacode-blame-none-{}.rs", std::process::id()));
        std::fs::write(&outside, "x\n").unwrap();

        let worker = BlameWorker::spawn();
        for path in [fresh, outside.clone()] {
            worker.request(Request::Blame {
                path,
                text: "fn main() {}\n".into(),
                version: Version(1),
            });
            assert!(matches!(
                answer(&worker),
                Some(BlameResult::Blame { blame: None, .. })
            ));
        }

        std::fs::remove_file(&outside).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
