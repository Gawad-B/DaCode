//! What changed since the last commit, per line.
//!
//! Shells out to `git` rather than linking a git library. The editor needs exactly one thing from
//! git — the committed contents of one file — and `git show HEAD:path` is that in one line. Linking
//! `gix` or `git2` to get it would add a large dependency and an object-database implementation to
//! an editor whose premise is staying light, and it would still have to agree with the `git` binary
//! about worktrees, submodules and sparse checkouts.
//!
//! The diff itself is ours: a longest-common-subsequence over lines, run on a background thread
//! against a snapshot, with the result tagged by document version — the same contract the syntax
//! worker and the language server already follow.

use crate::wake::Waker;
use dacode_text::Version;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

/// What happened to one line, relative to the committed version.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    /// Lines were deleted *above* this one. Drawn as a mark between rows rather than on one, since
    /// the deleted lines have no row of their own.
    Removed,
}

/// The per-line status of a file, and the version it describes.
#[derive(Debug, Default, Clone)]
pub struct Diff {
    lines: Vec<(usize, Change)>,
    version: Option<Version>,
}

impl Diff {
    /// Builds a diff from known marks. For the screenshot path, which needs a fixed set rather than
    /// whatever happens to be uncommitted.
    pub fn from_marks(lines: Vec<(usize, Change)>, version: Version) -> Self {
        Self {
            lines,
            version: Some(version),
        }
    }

    pub fn version(&self) -> Option<Version> {
        self.version
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn change_at(&self, line: usize) -> Option<Change> {
        self.lines
            .iter()
            .find(|(at, _)| *at == line)
            .map(|(_, change)| *change)
    }

    /// The changed regions, as ranges of buffer lines.
    ///
    /// Consecutive marked lines are **one** hunk, which is what a person means by "this change".
    /// Stepping a line at a time would make walking the diff useless on a block of twenty added
    /// lines — twenty presses to get past one edit.
    pub fn hunks(&self) -> Vec<std::ops::Range<usize>> {
        let mut hunks: Vec<std::ops::Range<usize>> = Vec::new();
        for (line, _) in &self.lines {
            match hunks.last_mut() {
                Some(last) if *line <= last.end => last.end = line + 1,
                _ => hunks.push(*line..line + 1),
            }
        }
        hunks
    }

    /// The next changed region after `line`, or the previous one, wrapping.
    ///
    /// Wrapping for the same reason search does: walking the changes in a file and stopping dead at
    /// the last one, with three more above the cursor, is an answer nobody wants.
    pub fn change_near(&self, line: usize, forward: bool) -> Option<usize> {
        let hunks = self.hunks();
        if hunks.is_empty() {
            return None;
        }
        if forward {
            hunks
                .iter()
                .find(|hunk| hunk.start > line)
                .or_else(|| hunks.first())
                .map(|hunk| hunk.start)
        } else {
            hunks
                .iter()
                .rev()
                .find(|hunk| hunk.end <= line)
                .or_else(|| hunks.last())
                .map(|hunk| hunk.start)
        }
    }

    /// How many lines were added, changed and removed, for the statusline.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for (_, change) in &self.lines {
            match change {
                Change::Added => counts.0 += 1,
                Change::Modified => counts.1 += 1,
                Change::Removed => counts.2 += 1,
            }
        }
        counts
    }
}

/// A request to diff a snapshot of a file.
pub struct DiffRequest {
    pub path: PathBuf,
    pub text: String,
    pub version: Version,
    /// The line whose hunk to undo, when this is a revert rather than a diff.
    ///
    /// Reverting needs the committed text, a diff against it, and the hunk containing one line —
    /// all three of which are exactly what this thread already does, so it does them here rather
    /// than handing a whole file's committed contents back across a channel per keystroke.
    pub revert: Option<usize>,
}

/// What the worker sends back.
///
/// Every arm carries the **file** it is about, not just the document version it was computed
/// against. A version alone is not an identity: it counts from `Version(1)` per document, so two
/// buffers that have only been opened both hold `Version(1)` and a result for one passes a
/// version check against the other. For a diff that means the wrong gutter marks; for a revert it
/// means overwriting one file with another file's committed text.
pub enum GitResult {
    Diff {
        path: PathBuf,
        diff: Diff,
    },
    /// Lines `range` of the buffer should become `text` — the hunk as it was committed.
    ///
    /// `text` is empty when the change was purely an addition, which reverting means deleting.
    Revert {
        path: PathBuf,
        version: Version,
        range: std::ops::Range<usize>,
        text: String,
    },
    /// A revert that could not be worked out, with the reason in the user's terms.
    Refused(String),
    /// A request the worker skipped because a newer one had already arrived.
    ///
    /// Sent rather than silently dropped so that **one request always produces one result**. The
    /// editor keeps the loop awake while anything is outstanding, and a coalesced request that
    /// answered nothing left that count permanently one high — which does not merely delay a reply,
    /// it stops the editor ever going back to sleep. The editor ignores this arm; it exists to be
    /// counted.
    Superseded,
}

/// A background thread that diffs buffers against `HEAD`.
///
/// One thread rather than one per request: diffs are cheap but `git show` is a process spawn, and a
/// keystroke-rate fan-out of subprocesses is exactly the kind of unbounded work §1.1 forbids.
pub struct GitWorker {
    requests: Sender<DiffRequest>,
    results: Receiver<GitResult>,
    /// How many requests have been sent and not yet answered.
    ///
    /// The event loop sleeps until input unless something says it is still working. Without this
    /// the reply to a revert sat in the channel with nobody to read it: the key appeared to do
    /// nothing at all until some unrelated keypress happened to wake the loop — and if that
    /// keypress was a text edit, the revert was then refused as stale and lost. The format worker
    /// was given this and git was not.
    outstanding: std::cell::Cell<usize>,
}

impl GitWorker {
    pub fn spawn() -> Self {
        Self::spawn_with(None)
    }

    /// Like [`spawn`](Self::spawn), but `waker` is called from the worker thread after every result
    /// has been sent, so a loop that sleeps until input still collects it.
    pub fn spawn_with(waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<DiffRequest>();
        let (outgoing, results) = channel::<GitResult>();

        std::thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                // Only the newest *diff* matters: everything queued behind it describes text that
                // has already been superseded. A revert is not a stream sampled at keystroke rate —
                // it is something the user asked for once — so it is never dropped for a newer one.
                let mut request = request;
                let mut skipped = 0usize;
                while request.revert.is_none() {
                    match incoming.try_recv() {
                        Ok(newer) => {
                            request = newer;
                            skipped += 1;
                        }
                        Err(_) => break,
                    }
                }
                // One result per request, even for the ones whose work was skipped. See
                // `GitResult::Superseded`.
                for _ in 0..skipped {
                    if outgoing.send(GitResult::Superseded).is_err() {
                        return;
                    }
                    if let Some(wake) = &waker {
                        wake();
                    }
                }

                let base = committed(&request.path);
                let result = match request.revert {
                    Some(line) => revert(
                        base.as_deref(),
                        &request.text,
                        line,
                        request.version,
                        request.path.clone(),
                    ),
                    None => GitResult::Diff {
                        path: request.path.clone(),
                        diff: match base {
                            Some(base) => Diff {
                                lines: diff_lines(&base, &request.text),
                                version: Some(request.version),
                            },
                            // Not in git, or not committed yet: no marks rather than every line
                            // marked as added, which would make a scratch file look like a
                            // catastrophe.
                            None => Diff {
                                lines: Vec::new(),
                                version: Some(request.version),
                            },
                        },
                    },
                };

                if outgoing.send(result).is_err() {
                    break;
                }
                if let Some(wake) = &waker {
                    wake();
                }
            }
        });

        Self {
            requests,
            results,
            outstanding: std::cell::Cell::new(0),
        }
    }

    pub fn request(&self, request: DiffRequest) {
        if self.requests.send(request).is_ok() {
            self.outstanding.set(self.outstanding.get() + 1);
        }
    }

    /// The next result, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<GitResult> {
        let result = self.results.try_recv().ok();
        if result.is_some() {
            self.outstanding
                .set(self.outstanding.get().saturating_sub(1));
        }
        result
    }

    /// Whether a reply is still coming, so the event loop keeps waking to collect it.
    ///
    /// Exact, and it has to be: the loop stays awake while this is true, so a count that could not
    /// return to zero would mean an editor that never sleeps — a permanent 10Hz wake-up burning
    /// battery in an idle window. That is why the worker answers *every* request, coalesced ones
    /// included, with `GitResult::Superseded`.
    pub fn busy(&self) -> bool {
        self.outstanding.get() > 0
    }
}

/// Works out what reverting the hunk containing `line` would put back.
///
/// Runs on the worker, not the UI thread, because it needs a second diff — and the version it was
/// computed against goes back with it, so the editor can refuse a result for text that has since
/// moved. The same contract every producer here follows.
fn revert(
    base: Option<&str>,
    current: &str,
    line: usize,
    version: Version,
    path: PathBuf,
) -> GitResult {
    let Some(base) = base else {
        return GitResult::Refused(
            "this file is not in git, so there is nothing to go back to".into(),
        );
    };

    let marks = diff_lines(base, current);
    let diff = Diff {
        lines: marks,
        version: Some(version),
    };
    let Some(hunk) = diff.hunks().into_iter().find(|hunk| hunk.contains(&line)) else {
        return GitResult::Refused("no change under the cursor to undo".into());
    };

    let changed: std::collections::HashSet<usize> = diff.lines.iter().map(|(at, _)| *at).collect();
    let old: Vec<&str> = base.lines().collect();
    let new: Vec<&str> = current.lines().collect();

    // Where the hunk's committed counterpart begins. Found by walking both files together rather
    // than by counting: an earlier hunk may have changed the line count, so buffer line N and
    // committed line N are not the same place. The unchanged buffer lines *are* the diff's common
    // subsequence, so matching each of them forward through the committed file reconstructs the
    // alignment in one pass. Counting instead put a two-hunk revert a line early, which is the
    // difference between undoing your change and eating the line above it.
    let mut from = 0usize;
    for (line, text) in new.iter().enumerate().take(hunk.start) {
        if changed.contains(&line) {
            continue;
        }
        while from < old.len() && old[from] != *text {
            from += 1;
        }
        from = (from + 1).min(old.len());
    }
    let after: Vec<&str> = new.iter().skip(hunk.end).copied().collect();
    let to = old
        .iter()
        .enumerate()
        .skip(from)
        .find(|(at, _)| tails_match(&old[*at..], &after))
        .map(|(at, _)| at)
        .unwrap_or(old.len());

    // The buffer's own line ending, not `\n`. `lines()` strips the `\r` from both sides, so
    // rejoining with `\n` splices LF lines into a CRLF file — and because the diff strips `\r`
    // too, the gutter then reports the file as unchanged while git sees every touched line as
    // modified. Taken from the *buffer* rather than the commit, which is the side the text is
    // being spliced into and is right even when `git show` hands back an LF blob for a CRLF
    // worktree.
    let eol = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };

    let text = if to > from {
        let mut joined = old[from..to].join(eol);
        // Terminated only when a terminator belongs there. Reverting the last hunk of a file whose
        // committed form has no final newline must not add one — `apply_revert` extends the range
        // to the end of the buffer for a final hunk, so this restores the missing newline in both
        // directions rather than always adding it.
        if to < old.len() || base.ends_with('\n') {
            joined.push_str(eol);
        }
        joined
    } else {
        String::new()
    };

    GitResult::Revert {
        path,
        version,
        range: hunk,
        text,
    }
}

/// Whether two line runs agree for as far as it is worth checking.
///
/// Bounded rather than exact: a full comparison is O(n) per candidate and this is called once per
/// committed line, which on a large file is the quadratic cost the diff itself takes care to avoid.
/// Eight lines is enough to tell one alignment from another in real source.
fn tails_match(old: &[&str], new: &[&str]) -> bool {
    const ENOUGH: usize = 8;
    let take = ENOUGH.min(old.len()).min(new.len());
    if take == 0 {
        return old.is_empty() && new.is_empty();
    }
    old[..take] == new[..take]
}

/// The committed contents of a file, via `git show HEAD:<path>`.
fn committed(path: &Path) -> Option<String> {
    // Through `canonical`: `git -C \\?\C:\x` is not a directory git will accept.
    let absolute = dacode_project::canonical(path);
    if !absolute.exists() {
        return None;
    }
    let directory = absolute.parent()?;

    // The path has to be relative to the repository root, which is what `--show-prefix` gives.
    // `git` is a console program and the release build is a GUI one, so an ordinary spawn flashes a
    // console window — once per reload, which on a busy file is once a keystroke.
    let prefix = crate::process::command("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let prefix = String::from_utf8_lossy(&prefix.stdout).trim().to_string();
    let name = absolute.file_name()?.to_string_lossy().to_string();

    let output = crate::process::command("git")
        .arg("-C")
        .arg(directory)
        .arg("show")
        .arg(format!("HEAD:{prefix}{name}"))
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// Per-line changes between two texts.
///
/// A longest-common-subsequence over lines, which is what `diff` is. Bounded rather than exact for
/// very large files: the table is O(n·m), so beyond a size where an editor could not draw the marks
/// anyway, the comparison degrades to "everything after the first difference is modified" instead of
/// allocating gigabytes.
pub fn diff_lines(before: &str, after: &str) -> Vec<(usize, Change)> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();

    // 4000×4000 is 16M cells of u32 — 64MB, already generous. Past that, fall back.
    const MAX_CELLS: usize = 16_000_000;
    if old.len() * new.len() > MAX_CELLS {
        return coarse(&old, &new);
    }

    // Standard LCS table.
    let mut table = vec![0u32; (old.len() + 1) * (new.len() + 1)];
    let width = new.len() + 1;
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + (j + 1)] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + (j + 1)])
            };
        }
    }

    // Walk the table, turning runs of deletions and insertions into marks. A deletion immediately
    // followed by an insertion is a *modification*, which is what a person sees when they change a
    // line rather than two separate events.
    let mut marks = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            i += 1;
            j += 1;
            continue;
        }

        let deleted_start = i;
        let inserted_start = j;
        while i < old.len()
            && j < new.len()
            && old[i] != new[j]
            && table[(i + 1) * width + j] >= table[i * width + (j + 1)]
        {
            i += 1;
        }
        while j < new.len()
            && i < old.len()
            && old[i] != new[j]
            && table[(i + 1) * width + j] < table[i * width + (j + 1)]
        {
            j += 1;
        }

        // Nothing advanced — the table is flat here, so take one of each and keep going.
        if i == deleted_start && j == inserted_start {
            i += 1;
            j += 1;
            if inserted_start < new.len() {
                marks.push((inserted_start, Change::Modified));
            }
            continue;
        }

        let deleted = i - deleted_start;
        let inserted = j - inserted_start;
        for offset in 0..inserted {
            let line = inserted_start + offset;
            marks.push((
                line,
                if offset < deleted {
                    Change::Modified
                } else {
                    Change::Added
                },
            ));
        }
        if deleted > inserted {
            marks.push((j.min(new.len().saturating_sub(1)), Change::Removed));
        }
    }

    for line in j..new.len() {
        marks.push((line, Change::Added));
    }
    if i < old.len() && !new.is_empty() {
        marks.push((new.len() - 1, Change::Removed));
    }

    marks.sort_by_key(|(line, _)| *line);
    marks.dedup_by_key(|(line, _)| *line);
    marks
}

/// The fallback for files too large to diff exactly.
fn coarse(old: &[&str], new: &[&str]) -> Vec<(usize, Change)> {
    let common = old
        .iter()
        .zip(new.iter())
        .take_while(|(a, b)| a == b)
        .count();
    (common..new.len())
        .map(|line| {
            (
                line,
                if line < old.len() {
                    Change::Modified
                } else {
                    Change::Added
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_file_has_no_marks() {
        let text = "one\ntwo\nthree\n";
        assert!(diff_lines(text, text).is_empty());
    }

    #[test]
    fn an_added_line_is_marked_added() {
        let marks = diff_lines("one\ntwo\n", "one\ninserted\ntwo\n");
        assert_eq!(marks, vec![(1, Change::Added)]);
    }

    #[test]
    fn a_changed_line_is_modified_not_added_and_removed() {
        // What a person sees when they edit a line is one changed line, not a deletion beside an
        // insertion — and a gutter that says both is a gutter nobody trusts.
        let marks = diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\n");
        assert_eq!(marks, vec![(1, Change::Modified)]);
    }

    #[test]
    fn a_deleted_line_marks_the_line_that_took_its_place() {
        let marks = diff_lines("one\ntwo\nthree\n", "one\nthree\n");
        assert!(
            marks.iter().any(|(_, change)| *change == Change::Removed),
            "a deletion has to show somewhere: {marks:?}"
        );
    }

    #[test]
    fn appending_to_the_end_marks_only_the_new_lines() {
        let marks = diff_lines("one\n", "one\ntwo\nthree\n");
        assert_eq!(marks, vec![(1, Change::Added), (2, Change::Added)]);
    }

    #[test]
    fn an_empty_base_marks_everything_added() {
        let marks = diff_lines("", "one\ntwo\n");
        assert_eq!(marks, vec![(0, Change::Added), (1, Change::Added)]);
    }

    #[test]
    fn counts_are_reported_by_kind() {
        let diff = Diff {
            lines: diff_lines("one\ntwo\nthree\n", "one\nTWO\nthree\nfour\n"),
            version: None,
        };
        let (added, modified, _) = diff.counts();
        assert_eq!((added, modified), (1, 1));
    }

    #[test]
    fn a_file_too_large_to_diff_exactly_still_reports_something() {
        // The point is that it terminates and says something useful, not that it is precise.
        let old: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let mut new: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        new.push_str("appended\n");

        let marks = diff_lines(&old, &new);
        assert!(!marks.is_empty());
        assert!(
            marks.iter().all(|(line, _)| *line < 5001),
            "no mark may point past the end of the file"
        );
    }

    #[test]
    fn every_mark_is_inside_the_new_text() {
        // A mark on a line that does not exist would be drawn nowhere, or panic a stricter caller.
        let cases = [
            ("a\nb\nc\n", ""),
            ("", "a\n"),
            ("a\nb\nc\nd\n", "a\nd\n"),
            ("a\n", "b\nc\nd\n"),
        ];
        for (before, after) in cases {
            let lines = after.lines().count();
            for (line, _) in diff_lines(before, after) {
                assert!(
                    line < lines.max(1),
                    "mark at {line} but {after:?} has {lines} lines"
                );
            }
        }
    }

    #[test]
    fn consecutive_marks_are_one_hunk() {
        // What a person means by "this change". Stepping a line at a time would make `]c` useless
        // on a block of twenty added lines — twenty presses to get past one edit.
        let diff = Diff::from_marks(
            vec![
                (3, Change::Added),
                (4, Change::Added),
                (5, Change::Modified),
                (20, Change::Modified),
            ],
            Version(1),
        );
        assert_eq!(diff.hunks(), vec![3..6, 20..21]);
    }

    #[test]
    fn walking_the_changes_wraps_at_both_ends() {
        // The same rule search follows: stopping dead at the last change with three more above the
        // cursor is an answer nobody wants.
        let diff = Diff::from_marks(
            vec![
                (3, Change::Added),
                (4, Change::Added),
                (20, Change::Modified),
            ],
            Version(1),
        );

        assert_eq!(diff.change_near(0, true), Some(3), "the first one below");
        assert_eq!(
            diff.change_near(3, true),
            Some(20),
            "and not the one you are on"
        );
        assert_eq!(
            diff.change_near(50, true),
            Some(3),
            "past the last, back to the first"
        );

        assert_eq!(diff.change_near(21, false), Some(20));
        assert_eq!(
            diff.change_near(20, false),
            Some(3),
            "the hunk above, not its own start"
        );
        assert_eq!(
            diff.change_near(0, false),
            Some(20),
            "above the first, round to the last"
        );

        assert_eq!(
            Diff::default().change_near(0, true),
            None,
            "and nothing when nothing changed"
        );
    }

    #[test]
    fn a_result_says_which_file_it_is_about() {
        // A `Version` is not an identity — it counts from 1 per document, so two files that have
        // only been opened both hold Version(1). Reverting a hunk in one and switching to the other
        // before `git show` returned would have passed a version check and overwritten the second
        // file with the first one's committed text, silently, as one undo group.
        let result = revert(
            Some("one\ntwo\n"),
            "one\nTWO\n",
            1,
            Version(1),
            PathBuf::from("a.rs"),
        );
        let GitResult::Revert { path, .. } = result else {
            panic!("a modified line has a hunk");
        };
        assert_eq!(path, PathBuf::from("a.rs"));
    }

    #[test]
    fn a_crlf_file_is_put_back_with_crlf() {
        // `lines()` strips the `\r` from both sides, so rejoining with `\n` spliced LF lines into a
        // CRLF file — and because the diff strips `\r` too, the gutter then reported the file as
        // unchanged while git saw every touched line as modified.
        let GitResult::Revert { text, .. } = revert(
            Some("one\r\ntwo\r\nthree\r\n"),
            "one\r\nTWO\r\nthree\r\n",
            1,
            Version(1),
            PathBuf::from("x.rs"),
        ) else {
            panic!("a modified line has a hunk");
        };
        assert_eq!(text, "two\r\n", "the buffer's own line ending, not `\\n`");
    }

    #[test]
    fn reverting_the_last_line_does_not_invent_a_trailing_newline() {
        // The committed file has none, so putting the line back must not add one — git would then
        // report the file as modified for a change nobody made.
        let GitResult::Revert { text, .. } = revert(
            Some("one\ntwo"),
            "one\nTWO",
            1,
            Version(1),
            PathBuf::from("x.rs"),
        ) else {
            panic!("a modified line has a hunk");
        };
        assert_eq!(text, "two");

        // And a file that *does* end with one keeps it.
        let GitResult::Revert { text, .. } = revert(
            Some("one\ntwo\n"),
            "one\nTWO\n",
            1,
            Version(1),
            PathBuf::from("x.rs"),
        ) else {
            panic!("a modified line has a hunk");
        };
        assert_eq!(text, "two\n");
    }

    #[test]
    fn the_worker_says_when_it_still_owes_an_answer() {
        // Without this the event loop slept with a revert's reply sitting unread in the channel:
        // the key appeared to do nothing until some unrelated keypress woke it, and if that
        // keypress was an edit the revert was then refused as stale and lost.
        let worker = GitWorker::spawn();
        assert!(!worker.busy(), "nothing asked for yet");

        worker.request(DiffRequest {
            path: std::env::temp_dir().join("dacode-not-a-repo-file.rs"),
            text: "x\n".into(),
            version: Version(1),
            revert: None,
        });
        assert!(worker.busy(), "and now the loop must keep waking");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while worker.poll().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!worker.busy(), "until the answer has been collected");
    }

    #[test]
    fn reverting_a_modified_line_puts_the_committed_one_back() {
        let base = "one\ntwo\nthree\n";
        let current = "one\nCHANGED\nthree\n";

        let GitResult::Revert { range, text, .. } =
            revert(Some(base), current, 1, Version(1), PathBuf::from("x.rs"))
        else {
            panic!("a modified line has a hunk");
        };
        assert_eq!(range, 1..2);
        assert_eq!(text, "two\n");

        // And applying it reproduces the committed file, which is the only thing that makes this
        // safe — a revert that got the range right and the text wrong would corrupt the buffer.
        let lines: Vec<&str> = current.lines().collect();
        let mut rebuilt = lines[..range.start].join("\n");
        if !rebuilt.is_empty() {
            rebuilt.push('\n');
        }
        rebuilt.push_str(&text);
        rebuilt.push_str(&lines[range.end..].join("\n"));
        rebuilt.push('\n');
        assert_eq!(rebuilt, base);
    }

    #[test]
    fn reverting_an_addition_deletes_it() {
        // Nothing was committed there, so putting it back means removing the lines — and an empty
        // replacement is how that is spelled rather than a special case.
        let base = "one\ntwo\n";
        let current = "one\nnew\nalso new\ntwo\n";

        let GitResult::Revert { range, text, .. } =
            revert(Some(base), current, 1, Version(1), PathBuf::from("x.rs"))
        else {
            panic!("added lines are a hunk");
        };
        assert_eq!(range, 1..3);
        assert!(text.is_empty(), "got {text:?}");
    }

    #[test]
    fn a_revert_with_nothing_under_the_cursor_is_refused_rather_than_guessed() {
        let base = "one\ntwo\nthree\n";
        let current = "one\nCHANGED\nthree\n";

        assert!(
            matches!(
                revert(Some(base), current, 2, Version(1), PathBuf::from("x.rs")),
                GitResult::Refused(_)
            ),
            "line 2 is unchanged, so there is no hunk to undo"
        );
        assert!(
            matches!(
                revert(None, current, 0, Version(1), PathBuf::from("x.rs")),
                GitResult::Refused(_)
            ),
            "and a file not in git has nothing to go back to"
        );
    }

    #[test]
    fn reverting_the_second_of_two_hunks_does_not_take_the_first() {
        // The alignment case: the committed lines a hunk replaced are found by counting the
        // unchanged lines before it, so a file with an earlier edit must not shift the answer.
        let base = "a\nb\nc\nd\ne\nf\n";
        let current = "a\nB!\nc\nd\nE!\nf\n";

        let GitResult::Revert { range, text, .. } =
            revert(Some(base), current, 4, Version(1), PathBuf::from("x.rs"))
        else {
            panic!("the second hunk exists");
        };
        assert_eq!(range, 4..5);
        assert_eq!(text, "e\n", "the committed line at that position, not `b`");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// A real repository with one committed file. The revert path spawns `git show`, so the parts
    /// that matter — finding the repository root, the path prefix, and the committed text — are only
    /// exercised against a real one.
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

        let root = std::env::temp_dir().join(format!("dacode-git-{}-{label}", std::process::id()));
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
        // Committing needs an identity, and the machine's own may not be set — in CI it is not.
        git(&["config", "user.email", "dacode@example.invalid"])?;
        git(&["config", "user.name", "dacode"])?;

        std::fs::write(root.join("src/main.rs"), "one\ntwo\nthree\nfour\n").ok()?;
        git(&["add", "."])?;
        git(&["commit", "-q", "-m", "first"])?;
        Some(root)
    }

    #[test]
    fn a_finished_diff_wakes_whoever_is_waiting() {
        // The editor sleeps until something wakes it. A diff that lands in the channel silently is
        // gutter marks that wait for the next keypress.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(root) = repository("wake") else {
            return;
        };
        let woken = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = woken.clone();
        let worker = GitWorker::spawn_with(Some(std::sync::Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })));
        worker.request(DiffRequest {
            path: root.join("src/main.rs"),
            text: "one\nTWO!\nthree\nfour\n".into(),
            version: Version(1),
            revert: None,
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while woken.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            woken.load(Ordering::SeqCst) > 0,
            "the waker was never called"
        );
        assert!(worker.poll().is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_real_repository_reports_what_changed_and_puts_it_back() {
        let Some(root) = repository("revert") else {
            return;
        };
        let path = root.join("src/main.rs");
        let edited = "one\nTWO!\nthree\nfour\n";

        // The diff, through the same function the worker calls.
        let base = committed(&path).expect("the file is committed");
        assert_eq!(base, "one\ntwo\nthree\nfour\n");
        let diff = Diff {
            lines: diff_lines(&base, edited),
            version: Some(Version(1)),
        };
        assert_eq!(diff.hunks(), vec![1..2], "one line differs");

        // And undoing it reproduces the committed file exactly.
        let GitResult::Revert { range, text, .. } =
            revert(Some(&base), edited, 1, Version(1), PathBuf::from("x.rs"))
        else {
            panic!("there is a hunk to undo");
        };
        let lines: Vec<&str> = edited.lines().collect();
        let rebuilt = format!(
            "{}\n{text}{}\n",
            lines[..range.start].join("\n"),
            lines[range.end..].join("\n")
        );
        assert_eq!(rebuilt, base);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_file_git_has_never_seen_has_no_marks_and_cannot_be_reverted() {
        // A new file in a repository is not a catastrophe with every line marked as added — and it
        // has nothing committed to go back to, which is said rather than guessed at.
        let Some(root) = repository("untracked") else {
            return;
        };
        let fresh = root.join("src/new.rs");
        std::fs::write(&fresh, "fn main() {}\n").unwrap();

        assert_eq!(committed(&fresh), None);
        assert!(matches!(
            revert(None, "fn main() {}\n", 0, Version(1), PathBuf::from("x.rs")),
            GitResult::Refused(_)
        ));

        std::fs::remove_dir_all(&root).ok();
    }
}
