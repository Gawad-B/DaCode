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

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use vex_text::Version;

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
}

/// A background thread that diffs buffers against `HEAD`.
///
/// One thread rather than one per request: diffs are cheap but `git show` is a process spawn, and a
/// keystroke-rate fan-out of subprocesses is exactly the kind of unbounded work §1.1 forbids.
pub struct GitWorker {
    requests: Sender<DiffRequest>,
    results: Receiver<Diff>,
}

impl GitWorker {
    pub fn spawn() -> Self {
        let (requests, incoming) = channel::<DiffRequest>();
        let (outgoing, results) = channel::<Diff>();

        std::thread::spawn(move || {
            while let Ok(request) = incoming.recv() {
                // Only the newest request matters: everything queued behind it describes text that
                // has already been superseded.
                let mut request = request;
                while let Ok(newer) = incoming.try_recv() {
                    request = newer;
                }

                let diff = match committed(&request.path) {
                    Some(base) => Diff {
                        lines: diff_lines(&base, &request.text),
                        version: Some(request.version),
                    },
                    // Not in git, or not committed yet: no marks rather than every line marked as
                    // added, which would make a scratch file look like a catastrophe.
                    None => Diff {
                        lines: Vec::new(),
                        version: Some(request.version),
                    },
                };

                if outgoing.send(diff).is_err() {
                    break;
                }
            }
        });

        Self { requests, results }
    }

    pub fn request(&self, request: DiffRequest) {
        let _ = self.requests.send(request);
    }

    /// The newest diff, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<Diff> {
        let mut latest = None;
        while let Ok(diff) = self.results.try_recv() {
            latest = Some(diff);
        }
        latest
    }
}

/// The committed contents of a file, via `git show HEAD:<path>`.
fn committed(path: &Path) -> Option<String> {
    let absolute = path.canonicalize().ok()?;
    let directory = absolute.parent()?;

    // The path has to be relative to the repository root, which is what `--show-prefix` gives.
    let prefix = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let prefix = String::from_utf8_lossy(&prefix.stdout).trim().to_string();
    let name = absolute.file_name()?.to_string_lossy().to_string();

    let output = std::process::Command::new("git")
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
}
