//! Source control: what git says about the repository, and the one worker that asks it.
//!
//! Every git command the source-control view runs — status, stage, commit, push, a diff for the diff
//! tab, the branch list — goes through [`Worker`], one thread that runs them in order. In order
//! matters: "stage this, then tell me the status" must not answer with the status from before the
//! stage. Nothing here runs on the UI thread, because every one of these is a process spawn and some
//! of them (a push over a slow link, a status in a huge repository) take seconds.
//!
//! Like [`crate::git`], this shells out to the `git` binary rather than linking a git library: the
//! editor wants exactly what `git` itself would say, including about worktrees, sparse checkouts
//! and the user's own hooks and credential helpers, and the binary is the only thing guaranteed to
//! agree with the binary.
//!
//! **Nothing may wait for a person.** A push that needs a password would otherwise sit forever on
//! a prompt nobody can see, with the view saying "pushing…" until the editor is closed. So every
//! command runs with `GIT_TERMINAL_PROMPT=0`, no stdin, `GIT_EDITOR=true` and — unless the user
//! configured their own — ssh in batch mode. A command that would have asked fails instead, and
//! [`Failure::sign_in`] says so, so the view can point at the terminal where it *can* ask.

use crate::wake::Waker;
use dacode_text::Version;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{Receiver, Sender, channel};

/// How many changed files a status keeps.
///
/// A repository with a hundred thousand untracked files — a `node_modules` nobody ignored — would
/// otherwise hand the view a list it can only ever show forty rows of, at a cost per status of
/// allocating all of them. The rest are counted, and the view says how many it is not showing.
pub const MAX_ENTRIES: usize = 5_000;

/// How much of a diff the diff tab holds. Past this it is cut at a line and says so.
pub const MAX_SHOWN_BYTES: usize = 2 * 1024 * 1024;

/// How long a refresh caused by the repository changing on disk waits before asking.
///
/// A `git checkout` in the terminal writes the index, then `HEAD`, then the files, over several
/// milliseconds, and each write is an event. Asking at the first would describe a repository half
/// way through changing, and asking at every one would run a status per event.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(200);

/// The three lists the view shows, in VSCode's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// Unmerged paths: a merge, rebase or stash pop stopped on them.
    Merge,
    /// What the next commit will contain.
    Staged,
    /// What differs from the index, untracked files included.
    Changes,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Merge, Group::Staged, Group::Changes];

    pub fn title(self) -> &'static str {
        match self {
            Group::Merge => "Merge Changes",
            Group::Staged => "Staged Changes",
            Group::Changes => "Changes",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Group::Merge => 0,
            Group::Staged => 1,
            Group::Changes => 2,
        }
    }
}

/// One path git reports, with both of its halves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative to the repository's top level, with `/` separators, as git prints it.
    pub path: String,
    /// Where a rename or copy came from.
    pub original: Option<String>,
    /// The index against `HEAD`: `M`, `A`, `D`, `R`, `C`, `T`, or `.` for unchanged.
    pub index: char,
    /// The working tree against the index, in the same letters.
    pub worktree: char,
    /// Unmerged — in the middle of a merge, a rebase or a stash pop.
    pub conflict: bool,
    pub untracked: bool,
}

impl Entry {
    /// Whether this path is listed under `group`. A file staged and then edited again is in both
    /// [`Group::Staged`] and [`Group::Changes`], as it is in VSCode — each half is its own change.
    pub fn in_group(&self, group: Group) -> bool {
        match group {
            Group::Merge => self.conflict,
            Group::Staged => !self.conflict && !self.untracked && self.index != '.',
            Group::Changes => !self.conflict && (self.untracked || self.worktree != '.'),
        }
    }

    /// The letter shown at the end of the row: what happened, as far as that group is concerned.
    pub fn letter(&self, group: Group) -> char {
        match group {
            // VSCode's letter for a conflict.
            Group::Merge => 'C',
            Group::Staged => self.index,
            Group::Changes if self.untracked => 'U',
            Group::Changes => self.worktree,
        }
    }

    /// The file's name, without its directory.
    pub fn name(&self) -> &str {
        let trimmed = self.path.trim_end_matches('/');
        trimmed.rsplit('/').next().unwrap_or(trimmed)
    }

    /// The directory, relative to the repository, or empty at its top.
    pub fn directory(&self) -> &str {
        let trimmed = self.path.trim_end_matches('/');
        trimmed
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory)
    }

    /// Which diff opening this row from `group` shows.
    pub fn side(&self, group: Group) -> Side {
        match group {
            Group::Staged => Side::Staged,
            _ if self.untracked => Side::Untracked,
            _ => Side::Unstaged,
        }
    }
}

/// What `git status` said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The repository's top level. Every path in `entries` is relative to it.
    pub root: PathBuf,
    /// Where `HEAD` and the index live, and where the refs do — two places in a linked worktree.
    /// What [`dacode_project::FileWatcher::watch_repository`] is given.
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    /// The branch checked out, or `None` when `HEAD` is detached.
    pub branch: Option<String>,
    /// The commit `HEAD` names, or `None` before the first commit.
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub entries: Vec<Entry>,
    /// How many paths git reported, which is more than `entries` holds once it passes
    /// [`MAX_ENTRIES`].
    pub total: usize,
}

impl Status {
    /// What the header and the status line call where you are: the branch, or the commit a
    /// detached `HEAD` is at.
    pub fn head_name(&self) -> String {
        match (&self.branch, &self.oid) {
            (Some(branch), _) => branch.clone(),
            (None, Some(oid)) => format!("{} (detached)", &oid[..oid.len().min(7)]),
            (None, None) => "(detached)".to_string(),
        }
    }

    /// The entries listed under `group`, by index.
    pub fn group(&self, group: Group) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.in_group(group))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn has_staged(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.in_group(Group::Staged))
    }

    pub fn has_changes(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.in_group(Group::Changes))
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|entry| entry.conflict)
    }

    /// Whether git reported more than this holds.
    pub fn truncated(&self) -> bool {
        self.total > self.entries.len()
    }

    /// The entry for a path relative to the repository, if git listed it.
    pub fn find(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// `path` relative to the repository, or `None` when it is outside it.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let path = dacode_project::canonical(path);
        let root = dacode_project::canonical(&self.root);
        let relative = path.strip_prefix(&root).ok()?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }
}

/// Reads `git status --porcelain=v2 -z --branch`.
///
/// Pure, so the format's corners are tested without a repository: a rename's original path is the
/// *next* NUL-separated record rather than a field of its own, a path may contain spaces (so a line
/// is split into exactly as many fields as its kind has and the rest is the path), and `-z` means
/// nothing is quoted — a path is whatever bytes git had, read here as UTF-8 with replacement.
pub fn parse_status(bytes: &[u8]) -> Status {
    let mut status = Status::default();
    let mut records = bytes.split(|byte| *byte == 0);

    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(record);
        let entry = match record[0] {
            b'#' => {
                read_header(&mut status, &line);
                continue;
            }
            b'1' => {
                let fields: Vec<&str> = line.splitn(9, ' ').collect();
                let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) else {
                    continue;
                };
                ordinary(xy, path, None)
            }
            b'2' => {
                // The original path follows as a record of its own — taken even past the cap, or it
                // would be read as an entry.
                let original = records
                    .next()
                    .map(|record| String::from_utf8_lossy(record).into_owned());
                let fields: Vec<&str> = line.splitn(10, ' ').collect();
                let (Some(xy), Some(path)) = (fields.get(1), fields.get(9)) else {
                    continue;
                };
                ordinary(xy, path, original)
            }
            b'u' => {
                let fields: Vec<&str> = line.splitn(11, ' ').collect();
                let (Some(xy), Some(path)) = (fields.get(1), fields.get(10)) else {
                    continue;
                };
                let mut entry = ordinary(xy, path, None);
                entry.conflict = true;
                entry
            }
            b'?' => Entry {
                path: line.get(2..).unwrap_or_default().to_string(),
                original: None,
                index: '.',
                worktree: '?',
                conflict: false,
                untracked: true,
            },
            // Ignored files are only reported when asked for, and this never asks.
            _ => continue,
        };
        status.total += 1;
        if status.entries.len() < MAX_ENTRIES {
            status.entries.push(entry);
        }
    }
    status
}

fn ordinary(xy: &str, path: &str, original: Option<String>) -> Entry {
    let mut letters = xy.chars();
    Entry {
        path: path.to_string(),
        original,
        index: letters.next().unwrap_or('.'),
        worktree: letters.next().unwrap_or('.'),
        conflict: false,
        untracked: false,
    }
}

fn read_header(status: &mut Status, line: &str) {
    let Some(rest) = line.strip_prefix("# ") else {
        return;
    };
    let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
    match key {
        "branch.oid" if value != "(initial)" => status.oid = Some(value.to_string()),
        "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            for part in value.split(' ') {
                if let Some(ahead) = part.strip_prefix('+') {
                    status.ahead = ahead.parse().unwrap_or(0);
                } else if let Some(behind) = part.strip_prefix('-') {
                    status.behind = behind.parse().unwrap_or(0);
                }
            }
        }
        _ => {}
    }
}

/// A branch, for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: String,
    /// When its tip was committed, as git says it: `3 days ago`.
    pub when: String,
}

/// The `for-each-ref` format [`parse_branches`] reads: tab-separated, one ref a line.
const BRANCH_FORMAT: &str = "%(HEAD)%09%(refname)%09%(upstream:short)%09%(committerdate:relative)";

/// Reads `git for-each-ref` in [`BRANCH_FORMAT`]. A remote's `HEAD` is a pointer to one of its
/// branches rather than a branch, so it is left out — checking it out would be a detached head
/// nobody asked for.
pub fn parse_branches(text: &str) -> Vec<Branch> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let head = fields.next()?;
            let refname = fields.next()?;
            let upstream = fields.next().unwrap_or_default();
            let when = fields.next().unwrap_or_default();
            let (name, remote) = if let Some(name) = refname.strip_prefix("refs/heads/") {
                (name, false)
            } else {
                let name = refname.strip_prefix("refs/remotes/")?;
                if name.ends_with("/HEAD") {
                    return None;
                }
                (name, true)
            };
            Some(Branch {
                name: name.to_string(),
                remote,
                current: head.trim() == "*",
                upstream: upstream.to_string(),
                when: when.to_string(),
            })
        })
        .collect()
}

/// Something that changes the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Stage(Vec<String>),
    Unstage(Vec<String>),
    /// Throw away working-tree changes: tracked paths go back to the index, untracked ones are
    /// deleted.
    Discard {
        tracked: Vec<String>,
        untracked: Vec<String>,
    },
    /// Every change, including the ones past [`MAX_ENTRIES`] the view never listed.
    StageAll,
    UnstageAll,
    DiscardAll,
    Commit {
        message: String,
        amend: bool,
        /// Stage everything first — the smart commit, when nothing was staged.
        stage_all: bool,
    },
    Fetch,
    Pull,
    Push,
    /// Pull, then push.
    Sync,
    Stash,
    PopStash,
    /// A local branch, or a remote one to track.
    Checkout {
        name: String,
        remote: bool,
    },
    CreateBranch(String),
    /// `git init` in the project, for the view that found no repository.
    Init,
}

impl Op {
    /// What the view says while it runs.
    pub fn doing(&self) -> &'static str {
        match self {
            Op::Stage(_) | Op::StageAll => "staging…",
            Op::Unstage(_) | Op::UnstageAll => "unstaging…",
            Op::Discard { .. } | Op::DiscardAll => "discarding…",
            Op::Commit { .. } => "committing…",
            Op::Fetch => "fetching…",
            Op::Pull => "pulling…",
            Op::Push => "pushing…",
            Op::Sync => "pulling, then pushing…",
            Op::Stash => "stashing…",
            Op::PopStash => "popping the stash…",
            Op::Checkout { .. } => "checking out…",
            Op::CreateBranch(_) => "creating the branch…",
            Op::Init => "creating a repository…",
        }
    }

    /// The git command a person would type for this, for a sentence that sends them to the
    /// terminal to do it themselves.
    pub fn command(&self) -> &'static str {
        match self {
            Op::Fetch => "git fetch",
            Op::Pull => "git pull",
            Op::Push | Op::Sync => "git push",
            _ => "git",
        }
    }
}

/// Which diff a row opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The index against the working tree: `git diff`.
    Unstaged,
    /// `HEAD` against the index: `git diff --cached`.
    Staged,
    /// Nothing against the file: every line added.
    Untracked,
}

impl Side {
    pub fn describe(self) -> &'static str {
        match self {
            Side::Unstaged => "working tree",
            Side::Staged => "staged",
            Side::Untracked => "untracked",
        }
    }
}

/// Text for a read-only tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Show {
    Changes { path: String, side: Side },
    History,
}

/// Why a command failed, as a sentence for the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub sentence: String,
    /// The remote wanted credentials, and nothing here could ask for them.
    pub sign_in: bool,
}

impl Failure {
    fn new(sentence: impl Into<String>) -> Self {
        Self {
            sentence: sentence.into(),
            sign_in: false,
        }
    }
}

pub enum Request {
    Status {
        root: PathBuf,
        version: Version,
        /// Wait for the repository to stop changing first — see [`SETTLE`].
        settle: bool,
    },
    Run {
        root: PathBuf,
        op: Op,
    },
    Branches {
        root: PathBuf,
    },
    Show {
        root: PathBuf,
        show: Show,
    },
}

/// What a status found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repo {
    Ready(Box<Status>),
    NotARepository,
    /// No `git` program to ask.
    NoGit,
    Failed(String),
}

pub enum Answer {
    Status {
        version: Version,
        repo: Repo,
    },
    Done {
        op: Op,
        result: Result<String, Failure>,
    },
    Branches(Result<Vec<Branch>, Failure>),
    Shown {
        show: Show,
        text: Result<String, Failure>,
    },
    /// A status skipped because a newer one was already queued. Sent so one request always has one
    /// answer, which is what lets [`Worker::busy`] return to zero — the same contract as
    /// [`crate::git::GitResult::Superseded`].
    Superseded,
}

/// The thread every source-control command runs on.
pub struct Worker {
    requests: Sender<Request>,
    answers: Receiver<Answer>,
    outstanding: std::cell::Cell<usize>,
}

impl Worker {
    pub fn spawn_with(waker: Option<Waker>) -> Self {
        let (requests, incoming) = channel::<Request>();
        let (outgoing, answers) = channel::<Answer>();

        std::thread::spawn(move || {
            let mut queue: VecDeque<Request> = VecDeque::new();
            loop {
                let request = match queue.pop_front() {
                    Some(request) => request,
                    None => match incoming.recv() {
                        Ok(request) => request,
                        Err(_) => return,
                    },
                };
                if let Request::Status { settle: true, .. } = request {
                    std::thread::sleep(SETTLE);
                }
                while let Ok(more) = incoming.try_recv() {
                    queue.push_back(more);
                }
                // A status with another queued behind it describes a moment nobody will see. Only
                // a status is ever skipped: a stage or a commit is something somebody asked for.
                let superseded = matches!(request, Request::Status { .. })
                    && queue
                        .iter()
                        .any(|later| matches!(later, Request::Status { .. }));
                let answer = if superseded {
                    Answer::Superseded
                } else {
                    answer(request)
                };
                if outgoing.send(answer).is_err() {
                    return;
                }
                if let Some(wake) = &waker {
                    wake();
                }
            }
        });

        Self {
            requests,
            answers,
            outstanding: std::cell::Cell::new(0),
        }
    }

    pub fn request(&self, request: Request) {
        if self.requests.send(request).is_ok() {
            self.outstanding.set(self.outstanding.get() + 1);
        }
    }

    /// The next answer, if one is ready. Never blocks.
    pub fn poll(&self) -> Option<Answer> {
        let answer = self.answers.try_recv().ok();
        if answer.is_some() {
            self.outstanding
                .set(self.outstanding.get().saturating_sub(1));
        }
        answer
    }

    /// Whether an answer is still coming.
    pub fn busy(&self) -> bool {
        self.outstanding.get() > 0
    }
}

fn answer(request: Request) -> Answer {
    match request {
        Request::Status { root, version, .. } => Answer::Status {
            version,
            repo: status(&root),
        },
        Request::Run { root, op } => {
            let result = run(&root, &op);
            Answer::Done { op, result }
        }
        Request::Branches { root } => Answer::Branches(
            git_output(
                &root,
                &[
                    "for-each-ref",
                    "--sort=-committerdate",
                    &format!("--format={BRANCH_FORMAT}"),
                    "refs/heads",
                    "refs/remotes",
                ],
            )
            .map(|text| parse_branches(&text)),
        ),
        Request::Show { root, show } => {
            let text = shown(&root, &show);
            Answer::Shown { show, text }
        }
    }
}

/// A `git` command in `root`, set up so that it can never wait for a person.
pub fn git(root: &Path) -> std::process::Command {
    let mut command = crate::process::command("git");
    command
        .arg("-C")
        .arg(root)
        // Colour, a pager or a translated `diff` driver would all change what is parsed.
        .args(["-c", "color.ui=false", "-c", "core.quotepath=false"])
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_EDITOR", "true")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_PAGER", "cat");
    command
}

/// The stdout of a command that succeeded, or why it did not.
fn git_output(root: &Path, args: &[&str]) -> Result<String, Failure> {
    finish(git(root).args(args).output())
}

fn finish(output: std::io::Result<std::process::Output>) -> Result<String, Failure> {
    let output = output.map_err(|error| {
        Failure::new(if error.kind() == std::io::ErrorKind::NotFound {
            "git is not installed, or not on PATH".to_string()
        } else {
            format!("could not run git: {error}")
        })
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Failure {
            sentence: sentence(&stderr, &stdout),
            sign_in: wants_credentials(&stderr),
        })
    }
}

/// What git said, as one line for the screen.
///
/// Its advice is left out (`hint:` lines tell you which command to type next, in a terminal this is
/// not), and so are the `fatal:` and `error:` prefixes, which say nothing a reader does not already
/// know from the fact that a message appeared. What is left is joined, because the status line is
/// one line, and cut short if it is still long — git's longest complaints list every file.
pub fn sentence(stderr: &str, stdout: &str) -> String {
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let lines: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .map(|line| {
            ["fatal: ", "error: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .unwrap_or(line)
        })
        .collect();
    if lines.is_empty() {
        return "git failed without saying why".to_string();
    }
    let joined = lines.join(" — ");
    const LONGEST: usize = 240;
    if joined.chars().count() > LONGEST {
        let cut: String = joined.chars().take(LONGEST - 1).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

/// Whether a remote refused for want of credentials it could not ask for.
pub fn wants_credentials(stderr: &str) -> bool {
    [
        "terminal prompts disabled",
        "could not read Username",
        "could not read Password",
        "Authentication failed",
        "Permission denied (publickey",
        "Host key verification failed",
        "invalid username or password",
    ]
    .iter()
    .any(|needle| stderr.contains(needle))
}

fn status(root: &Path) -> Repo {
    let located = git(root)
        .args([
            "rev-parse",
            "--show-toplevel",
            "--absolute-git-dir",
            "--git-common-dir",
        ])
        .output();
    let located = match located {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Repo::NoGit,
        Err(error) => return Repo::Failed(format!("could not run git: {error}")),
    };
    if !located.status.success() {
        let stderr = String::from_utf8_lossy(&located.stderr);
        if stderr.contains("not a git repository") {
            return Repo::NotARepository;
        }
        return Repo::Failed(sentence(&stderr, ""));
    }
    let text = String::from_utf8_lossy(&located.stdout).into_owned();
    let mut lines = text.lines();
    let top = PathBuf::from(lines.next().unwrap_or_default());
    let git_dir = PathBuf::from(lines.next().unwrap_or_default());
    // Relative to where git was run when it is the ordinary `.git`.
    let common = PathBuf::from(lines.next().unwrap_or_default());
    let common_dir = if common.is_absolute() {
        common
    } else {
        root.join(common)
    };

    // `GIT_OPTIONAL_LOCKS=0`: a status may otherwise rewrite the index to refresh its stat cache,
    // which the repository watch would report as a change, which would ask for a status — a loop
    // that only ends when the index stops being worth refreshing.
    let output = git(&top)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=normal",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let mut status = parse_status(&output.stdout);
            status.root = top;
            status.git_dir = git_dir;
            status.common_dir = dacode_project::canonical(&common_dir);
            Repo::Ready(Box::new(status))
        }
        Ok(output) => Repo::Failed(sentence(&String::from_utf8_lossy(&output.stderr), "")),
        Err(error) => Repo::Failed(format!("could not run git: {error}")),
    }
}

/// Paths are handed to git in batches, so discarding five thousand files does not build a command
/// line past what the platform allows — 32K characters on Windows.
const BATCH: usize = 200;

fn each_batch(
    root: &Path,
    head: &[&str],
    paths: &[String],
    remote: bool,
) -> Result<String, Failure> {
    let mut out = String::new();
    for batch in paths.chunks(BATCH) {
        let mut command = git(root);
        if remote {
            batch_ssh(&mut command, root);
        }
        command.args(head).arg("--").args(batch);
        out.push_str(&finish(command.output())?);
    }
    Ok(out)
}

/// ssh in batch mode, so a key with a passphrase fails rather than prompting on a terminal that
/// is not there — unless the user configured how git runs ssh, which is theirs to decide.
fn batch_ssh(command: &mut std::process::Command, root: &Path) {
    if std::env::var_os("GIT_SSH_COMMAND").is_some() || std::env::var_os("GIT_SSH").is_some() {
        return;
    }
    let configured = git(root)
        .args(["config", "--get", "core.sshCommand"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !configured {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
}

fn remote_command(root: &Path, args: &[&str]) -> Result<String, Failure> {
    let mut command = git(root);
    batch_ssh(&mut command, root);
    finish(command.args(args).output())
}

fn plural(count: usize, one: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {one}s")
    }
}

/// Runs one operation, and says what it did.
fn run(root: &Path, op: &Op) -> Result<String, Failure> {
    match op {
        Op::Stage(paths) => {
            each_batch(root, &["add", "-A"], paths, false)?;
            Ok(format!("staged {}", plural(paths.len(), "file")))
        }
        Op::Unstage(paths) => {
            // `reset` rather than `restore --staged`: it also works before the first commit, when
            // there is no `HEAD` for `restore` to take the index back to.
            each_batch(root, &["reset", "-q"], paths, false)?;
            Ok(format!("unstaged {}", plural(paths.len(), "file")))
        }
        Op::Discard { tracked, untracked } => {
            if !tracked.is_empty() {
                each_batch(root, &["checkout", "-q"], tracked, false)?;
            }
            if !untracked.is_empty() {
                // `-d` so an untracked directory, which status lists as `dir/`, goes too. Never
                // `-x`: ignored files are not changes, and are often somebody's secrets.
                each_batch(root, &["clean", "-f", "-d", "-q"], untracked, false)?;
            }
            Ok(format!(
                "discarded changes to {}",
                plural(tracked.len() + untracked.len(), "file")
            ))
        }
        Op::StageAll => {
            git_output(root, &["add", "-A", "--", ":/"])?;
            Ok("staged every change".to_string())
        }
        Op::UnstageAll => {
            git_output(root, &["reset", "-q"])?;
            Ok("unstaged everything".to_string())
        }
        Op::DiscardAll => {
            git_output(root, &["checkout", "-q", "--", ":/"])?;
            git_output(root, &["clean", "-f", "-d", "-q", "--", ":/"])?;
            Ok("discarded every change".to_string())
        }
        Op::Commit {
            message,
            amend,
            stage_all,
        } => {
            if *stage_all {
                git_output(root, &["add", "-A", "--", ":/"])?;
            }
            let mut args = vec!["commit", "--quiet"];
            if *amend {
                args.push("--amend");
            }
            if *amend && message.trim().is_empty() {
                args.push("--no-edit");
            } else {
                args.push("-m");
                args.push(message);
            }
            git_output(root, &args)?;
            let summary = git_output(root, &["log", "-1", "--format=%h %s"]).unwrap_or_default();
            Ok(format!(
                "{} {}",
                if *amend { "amended" } else { "committed" },
                summary.trim()
            ))
        }
        Op::Fetch => {
            remote_command(root, &["fetch", "--prune"])?;
            Ok("fetched".to_string())
        }
        Op::Pull => {
            let out = remote_command(root, &["pull", "--no-edit"])?;
            Ok(if out.contains("Already up to date") {
                "pulled — already up to date".to_string()
            } else {
                "pulled".to_string()
            })
        }
        Op::Push => push(root),
        Op::Sync => {
            remote_command(root, &["pull", "--no-edit"])?;
            push(root).map(|pushed| format!("pulled, then {pushed}"))
        }
        Op::Stash => {
            let out = git_output(root, &["stash", "push"])?;
            if out.contains("No local changes to save") {
                Err(Failure::new(
                    "nothing to stash — no tracked file has changed",
                ))
            } else {
                Ok("stashed your changes".to_string())
            }
        }
        Op::PopStash => {
            git_output(root, &["stash", "pop"])?;
            Ok("put the stash back".to_string())
        }
        Op::Checkout { name, remote } => {
            if *remote {
                git_output(root, &["checkout", "-q", "--track", name])?;
            } else {
                // `--` after the name, so a branch called like a file is still a branch.
                git_output(root, &["checkout", "-q", name, "--"])?;
            }
            Ok(format!("checked out {name}"))
        }
        Op::CreateBranch(name) => {
            git_output(root, &["checkout", "-q", "-b", name])?;
            Ok(format!("created and checked out {name}"))
        }
        Op::Init => {
            git_output(root, &["init", "-q"])?;
            Ok(format!("created a repository in {}", root.display()))
        }
    }
}

/// Pushes the branch, publishing it to the first remote when it has no upstream yet — which is
/// what VSCode's "Publish Branch" does, and what a person means by pushing a branch they just made.
fn push(root: &Path) -> Result<String, Failure> {
    let branch = git_output(root, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|name| name.trim().to_string())
        .map_err(|_| Failure::new("HEAD is detached, so there is no branch to push"))?;
    let upstream = git_output(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    if upstream.is_ok() {
        remote_command(root, &["push"])?;
        return Ok(format!("pushed {branch}"));
    }
    let remotes = git_output(root, &["remote"])?;
    let remotes: Vec<&str> = remotes.lines().map(str::trim).collect();
    let Some(remote) = remotes
        .iter()
        .find(|remote| **remote == "origin")
        .or_else(|| remotes.first())
        .filter(|remote| !remote.is_empty())
    else {
        return Err(Failure::new(
            "this repository has no remote to push to — add one with `git remote add` in the \
             terminal",
        ));
    };
    remote_command(root, &["push", "-u", remote, "HEAD"])?;
    Ok(format!("published {branch} to {remote}"))
}

/// The text of a read-only tab: a diff, or the history.
fn shown(root: &Path, show: &Show) -> Result<String, Failure> {
    let output = match show {
        Show::Changes { path, side } => {
            let mut command = git(root);
            command.args(["diff", "--no-color", "--no-ext-diff"]);
            match side {
                Side::Unstaged => {
                    command.arg("--").arg(path);
                }
                Side::Staged => {
                    command.arg("--cached").arg("--").arg(path);
                }
                Side::Untracked => {
                    command.args(["--no-index", "--", "/dev/null"]).arg(path);
                }
            }
            let output = command
                .output()
                .map_err(|error| Failure::new(format!("could not run git: {error}")))?;
            // `--no-index` reports "they differ" with status 1, which for an untracked file is the
            // whole point.
            let differs = *side == Side::Untracked && output.status.code() == Some(1);
            if !output.status.success() && !differs {
                return Err(Failure::new(sentence(
                    &String::from_utf8_lossy(&output.stderr),
                    "",
                )));
            }
            output.stdout
        }
        Show::History => git_output(
            root,
            &[
                "log",
                "-n",
                "500",
                "--date=short",
                "--format=%h  %ad  %<(18,trunc)%an  %s%d",
            ],
        )?
        .into_bytes(),
    };
    Ok(capped(&output))
}

/// At most [`MAX_SHOWN_BYTES`] of `bytes`, cut at a line, saying so when it was cut.
fn capped(bytes: &[u8]) -> String {
    if bytes.len() <= MAX_SHOWN_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let cut = bytes[..MAX_SHOWN_BYTES]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(MAX_SHOWN_BYTES, |at| at + 1);
    let mut text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
    text.push_str("\n… cut short at 2 MB — run git diff in the terminal for the rest\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(parts: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for part in parts {
            bytes.extend_from_slice(part.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn the_branch_headers_say_where_you_are_and_how_far_from_upstream() {
        let status = parse_status(&record(&[
            "# branch.oid 1234567890abcdef1234567890abcdef12345678",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
        ]));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 3));
        assert!(status.entries.is_empty());
        assert_eq!(status.head_name(), "main");
    }

    #[test]
    fn a_detached_head_is_named_by_its_commit_and_a_fresh_repository_has_none() {
        // Before the first commit there is no oid, and calling the branch by a hash that does not
        // exist would send someone looking for it.
        let detached = parse_status(&record(&[
            "# branch.oid abcdef0123456789abcdef0123456789abcdef01",
            "# branch.head (detached)",
        ]));
        assert_eq!(detached.branch, None);
        assert_eq!(detached.head_name(), "abcdef0 (detached)");

        let fresh = parse_status(&record(&["# branch.oid (initial)", "# branch.head main"]));
        assert_eq!(fresh.oid, None);
        assert_eq!(fresh.head_name(), "main");
    }

    #[test]
    fn a_file_staged_and_then_edited_again_is_listed_in_both_groups() {
        // Each half is its own change: the staged half is what will be committed, the other is what
        // will not. Listing it once would hide one of them.
        let status = parse_status(&record(&[
            "1 MM N... 100644 100644 100644 aaaa bbbb src/main.rs",
        ]));
        let entry = &status.entries[0];
        assert!(entry.in_group(Group::Staged) && entry.in_group(Group::Changes));
        assert_eq!(entry.letter(Group::Staged), 'M');
        assert_eq!(entry.letter(Group::Changes), 'M');
        assert_eq!(entry.side(Group::Staged), Side::Staged);
        assert_eq!(entry.side(Group::Changes), Side::Unstaged);
        assert_eq!((entry.name(), entry.directory()), ("main.rs", "src"));
    }

    #[test]
    fn a_path_with_spaces_is_kept_whole() {
        // The path is everything after the fixed fields. Splitting on every space would name the
        // file `world.txt` and stage a file that does not exist.
        let status = parse_status(&record(&[
            "1 .M N... 100644 100644 100644 aaaa bbbb docs/hello world.txt",
            "? notes/a b c.md",
        ]));
        assert_eq!(status.entries[0].path, "docs/hello world.txt");
        assert_eq!(status.entries[0].name(), "hello world.txt");
        assert_eq!(status.entries[1].path, "notes/a b c.md");
        assert!(status.entries[1].untracked);
        assert_eq!(status.entries[1].letter(Group::Changes), 'U');
    }

    #[test]
    fn a_rename_takes_its_original_path_from_the_next_record() {
        // `-z` puts the original path in a record of its own. Read as an entry it would list a file
        // that no longer exists, and the one after it would be misread too.
        let status = parse_status(&record(&[
            "2 R. N... 100644 100644 100644 aaaa aaaa R100 src/new name.rs",
            "src/old name.rs",
            "1 .M N... 100644 100644 100644 aaaa bbbb README.md",
        ]));
        assert_eq!(status.entries.len(), 2);
        let renamed = &status.entries[0];
        assert_eq!(renamed.path, "src/new name.rs");
        assert_eq!(renamed.original.as_deref(), Some("src/old name.rs"));
        assert_eq!(renamed.letter(Group::Staged), 'R');
        assert!(!renamed.in_group(Group::Changes));
        assert_eq!(status.entries[1].path, "README.md");
    }

    #[test]
    fn a_conflict_is_listed_only_under_merge_changes() {
        let status = parse_status(&record(&[
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc src/lib.rs",
        ]));
        let entry = &status.entries[0];
        assert!(entry.conflict);
        assert!(entry.in_group(Group::Merge));
        assert!(!entry.in_group(Group::Staged) && !entry.in_group(Group::Changes));
        assert_eq!(entry.letter(Group::Merge), 'C');
        assert!(status.has_conflicts());
    }

    #[test]
    fn unicode_paths_arrive_unquoted() {
        // `-z` turns git's octal quoting off, so a name in any script reads as itself.
        let status = parse_status(&record(&[
            "1 A. N... 000000 100644 100644 0000 aaaa docs/日本語 ファイル.md",
            "? café/naïve.txt",
        ]));
        assert_eq!(status.entries[0].name(), "日本語 ファイル.md");
        assert_eq!(status.entries[0].letter(Group::Staged), 'A');
        assert_eq!(status.entries[1].directory(), "café");
    }

    #[test]
    fn an_untracked_directory_is_one_row_named_for_the_directory() {
        let status = parse_status(&record(&["? build/out/"]));
        assert_eq!(status.entries[0].name(), "out");
        assert_eq!(status.entries[0].directory(), "build");
    }

    #[test]
    fn an_enormous_status_is_capped_and_counted() {
        // Viewport rows of a list of a hundred thousand: holding the rest costs memory per status
        // and shows nothing. They are counted so the view can say how many it left out.
        let lines: Vec<String> = (0..MAX_ENTRIES + 10)
            .map(|index| format!("? file{index}.txt"))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let status = parse_status(&record(&refs));
        assert_eq!(status.entries.len(), MAX_ENTRIES);
        assert_eq!(status.total, MAX_ENTRIES + 10);
        assert!(status.truncated());
    }

    #[test]
    fn a_rename_past_the_cap_does_not_leak_its_original_as_an_entry() {
        let mut lines: Vec<String> = (0..MAX_ENTRIES)
            .map(|index| format!("? file{index}.txt"))
            .collect();
        lines.push("2 R. N... 100644 100644 100644 aaaa aaaa R100 new.rs".into());
        lines.push("old.rs".into());
        lines.push("? last.txt".into());
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let status = parse_status(&record(&refs));
        assert_eq!(
            status.total,
            MAX_ENTRIES + 2,
            "old.rs is not a path of its own"
        );
    }

    #[test]
    fn branches_leave_out_a_remotes_head_and_mark_the_current_one() {
        let branches = parse_branches(
            "*\trefs/heads/main\torigin/main\t2 hours ago\n \
             \trefs/heads/feature/x\t\t3 days ago\n \
             \trefs/remotes/origin/HEAD\t\t2 hours ago\n \
             \trefs/remotes/origin/main\t\t2 hours ago\n",
        );
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["main", "feature/x", "origin/main"]);
        assert!(branches[0].current && !branches[1].current);
        assert!(branches[2].remote);
        assert_eq!(branches[0].upstream, "origin/main");
    }

    #[test]
    fn gits_complaint_becomes_one_line_without_its_advice() {
        let stderr = "error: Your local changes to the following files would be overwritten by \
                      checkout:\n\tsrc/main.rs\nPlease commit your changes or stash them before you \
                      switch branches.\nhint: use git stash\nAborting\n";
        let said = sentence(stderr, "");
        assert!(said.starts_with("Your local changes"), "{said}");
        assert!(said.contains("src/main.rs"));
        assert!(!said.contains("hint"));
        assert!(!said.contains('\n'));
    }

    #[test]
    fn a_remote_that_wanted_a_password_is_recognised() {
        // The sentence then points at the terminal, the one place in the editor that can ask.
        assert!(wants_credentials(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        assert!(wants_credentials(
            "git@github.com: Permission denied (publickey)."
        ));
        assert!(!wants_credentials("fatal: not a git repository"));
    }

    /// A repository of this test's own, or `None` where there is no git — a skip, not a failure.
    /// Signing and hooks are off in the repository, so a developer's global config cannot reach in.
    fn repository(label: &str, commit: bool) -> Option<PathBuf> {
        let dir =
            std::env::temp_dir().join(format!("dacode-scm-worker-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("hooks-none")).unwrap();
        let ok = |args: &[&str]| git_output(&dir, args).is_ok();
        if !(ok(&["init", "-q"])
            && ok(&["config", "user.email", "ann@example.invalid"])
            && ok(&["config", "user.name", "Ann Example"])
            && ok(&["config", "commit.gpgsign", "false"])
            && ok(&["config", "core.hooksPath", "hooks-none"]))
        {
            eprintln!("skipping: no git to build a repository with");
            return None;
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        if commit && !(ok(&["add", "a.txt"]) && ok(&["commit", "-q", "-m", "first"])) {
            return None;
        }
        Some(dir)
    }

    /// Sends one request and waits for its answer.
    fn ask(worker: &Worker, request: Request) -> Answer {
        worker.request(request);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Some(answer) = worker.poll() {
                return answer;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the worker never answered");
    }

    fn status_of(worker: &Worker, root: &Path) -> Status {
        match ask(
            worker,
            Request::Status {
                root: root.to_path_buf(),
                version: Version(1),
                settle: false,
            },
        ) {
            Answer::Status {
                repo: Repo::Ready(status),
                ..
            } => *status,
            _ => panic!("no status"),
        }
    }

    fn done(worker: &Worker, root: &Path, op: Op) -> Result<String, Failure> {
        match ask(
            worker,
            Request::Run {
                root: root.to_path_buf(),
                op,
            },
        ) {
            Answer::Done { result, .. } => result,
            _ => panic!("not an operation's answer"),
        }
    }

    #[test]
    fn before_the_first_commit_a_file_can_still_be_unstaged() {
        // There is no HEAD for `restore --staged` to go back to yet, which is the moment a person
        // most often stages the wrong file.
        let Some(dir) = repository("unborn", false) else {
            return;
        };
        let worker = Worker::spawn_with(None);
        done(&worker, &dir, Op::Stage(vec!["a.txt".into()])).expect("staged");
        let status = status_of(&worker, &dir);
        assert_eq!(status.oid, None);
        assert!(status.find("a.txt").unwrap().in_group(Group::Staged));
        done(&worker, &dir, Op::Unstage(vec!["a.txt".into()])).expect("unstaged");
        let status = status_of(&worker, &dir);
        assert!(status.find("a.txt").unwrap().untracked);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_branch_is_made_listed_and_left_and_a_dirty_checkout_is_refused_in_gits_words() {
        let Some(dir) = repository("branches", true) else {
            return;
        };
        let worker = Worker::spawn_with(None);
        let first = status_of(&worker, &dir).branch.expect("on a branch");
        done(&worker, &dir, Op::CreateBranch("topic/x".into())).expect("made");
        assert_eq!(status_of(&worker, &dir).branch.as_deref(), Some("topic/x"));
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        done(
            &worker,
            &dir,
            Op::Commit {
                message: "on the topic".into(),
                amend: false,
                stage_all: true,
            },
        )
        .expect("committed everything");

        let Answer::Branches(Ok(branches)) = ask(&worker, Request::Branches { root: dir.clone() })
        else {
            panic!("no branches");
        };
        assert!(branches.iter().any(|b| b.name == "topic/x" && b.current));
        assert!(branches.iter().any(|b| b.name == first && !b.current));

        // A change the other branch would overwrite: git refuses, and says which file.
        std::fs::write(dir.join("a.txt"), "three\n").unwrap();
        let refused = done(
            &worker,
            &dir,
            Op::Checkout {
                name: first.clone(),
                remote: false,
            },
        )
        .expect_err("git refuses to overwrite local changes");
        assert!(refused.sentence.contains("a.txt"), "{}", refused.sentence);
        assert!(!refused.sentence.contains('\n'));

        done(
            &worker,
            &dir,
            Op::Discard {
                tracked: vec!["a.txt".into()],
                untracked: Vec::new(),
            },
        )
        .expect("discarded");
        done(
            &worker,
            &dir,
            Op::Checkout {
                name: first.clone(),
                remote: false,
            },
        )
        .expect("clean now");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pushing_with_no_remote_says_so_instead_of_failing_obscurely() {
        let Some(dir) = repository("push", true) else {
            return;
        };
        let worker = Worker::spawn_with(None);
        let failure = done(&worker, &dir, Op::Push).expect_err("nowhere to push");
        assert!(
            failure.sentence.contains("no remote"),
            "{}",
            failure.sentence
        );
        assert!(!failure.sign_in);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_untracked_file_diffs_as_every_line_added_and_a_staged_one_against_head() {
        let Some(dir) = repository("show", true) else {
            return;
        };
        let worker = Worker::spawn_with(None);
        std::fs::write(dir.join("b.txt"), "fresh\n").unwrap();
        let show = |path: &str, side: Side| match ask(
            &worker,
            Request::Show {
                root: dir.clone(),
                show: Show::Changes {
                    path: path.into(),
                    side,
                },
            },
        ) {
            Answer::Shown { text, .. } => text.expect("a diff"),
            _ => panic!("not a diff"),
        };
        assert!(show("b.txt", Side::Untracked).contains("+fresh"));
        std::fs::write(dir.join("a.txt"), "one\nstaged\n").unwrap();
        done(&worker, &dir, Op::Stage(vec!["a.txt".into()])).unwrap();
        std::fs::write(dir.join("a.txt"), "one\nstaged\nunstaged\n").unwrap();
        let staged = show("a.txt", Side::Staged);
        assert!(staged.contains("+staged") && !staged.contains("+unstaged"));
        let unstaged = show("a.txt", Side::Unstaged);
        assert!(unstaged.contains("+unstaged") && !unstaged.contains("+staged"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_status_queued_behind_a_newer_one_is_answered_as_superseded() {
        // One answer per request, or `busy` never returns to zero and the editor never sleeps.
        let Some(dir) = repository("coalesce", true) else {
            return;
        };
        let worker = Worker::spawn_with(None);
        for version in 1..=5 {
            worker.request(Request::Status {
                root: dir.clone(),
                version: Version(version),
                settle: version == 1,
            });
        }
        let mut answers = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while worker.busy() && std::time::Instant::now() < deadline {
            if let Some(answer) = worker.poll() {
                answers.push(answer);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(answers.len(), 5);
        let newest = answers.iter().any(
            |answer| matches!(answer, Answer::Status { version, .. } if *version == Version(5)),
        );
        assert!(newest, "the newest question is always answered");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_diff_too_large_to_hold_is_cut_at_a_line_and_says_so() {
        let line = "+".repeat(99) + "\n";
        let big = line.repeat(MAX_SHOWN_BYTES / 100 + 50);
        let text = capped(big.as_bytes());
        assert!(text.len() < big.len());
        assert!(text.ends_with("for the rest\n"));
        let before_note = text.rsplit_once("\n…").unwrap().0;
        assert!(before_note.lines().all(|line| line.len() == 99));
    }
}
