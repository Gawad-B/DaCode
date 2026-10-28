//! The source-control view in the sidebar, shaped after VSCode's: where you are, a box for the
//! commit message, and the changes in three groups you stage, unstage and discard from.
//!
//! This is the model and its geometry; [`crate::scm`] is git, and `chrome::draw_source_control`
//! the drawing. **Drawing and the mouse both read [`View::geometry`]**, as every other surface here
//! does, so a click on a row's `+` stages the row whose `+` is drawn under the pointer.
//!
//! The rows are built once per status rather than per frame ([`View::rows`]): a status can hold
//! thousands of entries, and only the rows on screen are ever laid out — the per-frame cost is the
//! viewport's, not the repository's.

use crate::scm::{Entry, Group, Repo, Status};
use dacode_render::Rect;
use dacode_text::Version;

/// What the view knows about the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum State {
    /// Nothing asked yet, or the first answer still on its way.
    #[default]
    Unknown,
    Ready(Box<Status>),
    NotARepository,
    NoGit,
    Failed(String),
}

/// One line of the list: a group's heading, or a file under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Header(Group),
    /// `entry` indexes [`Status::entries`].
    File {
        group: Group,
        entry: usize,
    },
}

/// Which part of the view the keys go to while it has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    /// The commit message — where opening the view puts the caret, as VSCode does.
    #[default]
    Message,
    List,
}

/// The buttons on the heading's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Commit,
    Refresh,
    More,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Commit, Tool::Refresh, Tool::More];

    /// The command it runs, by the name in `dacode_input::commands`, so its tooltip names that
    /// command's key.
    pub fn command(self) -> &'static str {
        match self {
            Tool::Commit => "git_commit",
            Tool::Refresh => "git_refresh",
            Tool::More => "git_menu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Commit => "Commit",
            Tool::Refresh => "Refresh",
            Tool::More => "More actions",
        }
    }

    pub fn icon(self) -> dacode_render::Icon {
        match self {
            Tool::Commit => dacode_render::Icon::Check,
            Tool::Refresh => dacode_render::Icon::Refresh,
            Tool::More => dacode_render::Icon::More,
        }
    }
}

/// The buttons a row shows under the pointer or the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// The file itself, rather than its diff.
    Open,
    Stage,
    Unstage,
    Discard,
    StageAll,
    UnstageAll,
    DiscardAll,
}

impl RowAction {
    pub fn icon(self) -> dacode_render::Icon {
        use dacode_render::Icon;
        match self {
            RowAction::Open => Icon::File,
            RowAction::Stage | RowAction::StageAll => Icon::Plus,
            RowAction::Unstage | RowAction::UnstageAll => Icon::Minus,
            RowAction::Discard | RowAction::DiscardAll => Icon::Reset,
        }
    }

    /// Its tooltip: a name, and the command whose key it prints.
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            RowAction::Open => ("Open file", "git_open_file"),
            RowAction::Stage => ("Stage changes", "git_stage"),
            RowAction::Unstage => ("Unstage changes", "git_unstage"),
            RowAction::Discard => ("Discard changes", "git_discard"),
            RowAction::StageAll => ("Stage all changes", "git_stage_all"),
            RowAction::UnstageAll => ("Unstage all changes", "git_unstage_all"),
            RowAction::DiscardAll => ("Discard all changes", "git_discard_all"),
        }
    }
}

/// What a press asks to be pressed again for: each one throws work away, or does more than was
/// asked. The message under the commit button is the question, as replace-all's is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    /// Discard these paths' working-tree changes.
    Discard(Vec<String>),
    DiscardAll,
    /// Nothing is staged: commit every change instead.
    StageAllAndCommit,
}

/// Where everything in the view is.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub heading: Rect,
    pub tools: Vec<(Tool, Rect)>,
    /// The branch name — a click opens the branch picker.
    pub branch: Rect,
    /// The pull and push buttons, with how far behind and ahead.
    pub pull: Rect,
    pub push: Rect,
    pub message: Rect,
    pub commit: Rect,
    /// A line for what is happening, or the question being asked.
    pub summary: Rect,
    /// The visible rows, `(index into View::rows, rectangle)`.
    pub rows: Vec<(usize, Rect)>,
    pub capacity: usize,
    /// "Initialize Repository", when there is none.
    pub init: Option<Rect>,
    /// Where the explanation goes when there is no list.
    pub notice: Rect,
}

/// What a press at a point is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Tool(Tool),
    Branch,
    Pull,
    Push,
    Message,
    Commit,
    Init,
    Row(usize),
    Action(usize, RowAction),
}

/// The height of a list row, at scale 1.
pub const ROW: f32 = 22.0;
/// The height of a line of the commit message.
pub const MESSAGE_LINE: f32 = 20.0;
/// How many lines of message the box grows to before it scrolls.
pub const MESSAGE_LINES: usize = 6;
/// The side of a row's action button.
pub const ACTION: f32 = 20.0;

#[derive(Debug, Default)]
pub struct View {
    pub state: State,
    rows: Vec<Row>,
    /// How many files each group lists, counted once per status rather than once per frame.
    counts: [usize; 3],
    pub collapsed: [bool; 3],
    /// Index into `rows`.
    pub selected: Option<usize>,
    first: usize,
    pub message: String,
    /// The caret in `message`, in chars.
    pub caret: usize,
    pub field: Field,
    pub confirm: Option<Confirm>,
    /// What the worker is doing for the view, while it does.
    pub busy: Option<&'static str>,
    /// The newest status asked for. An answer for any other is a moment that has passed.
    asked: Version,
    /// Amend the last commit instead of making a new one.
    pub amend: bool,
}

impl View {
    /// Asks for a status: the version the answer must carry to be taken.
    pub fn ask(&mut self) -> Version {
        self.asked = self.asked.next();
        self.asked
    }

    /// Takes a status if it answers the newest question. Returns whether it was taken.
    pub fn accept(&mut self, version: Version, repo: Repo) -> bool {
        if version != self.asked {
            return false;
        }
        let keep = self.selected_key();
        self.state = match repo {
            Repo::Ready(status) => State::Ready(status),
            Repo::NotARepository => State::NotARepository,
            Repo::NoGit => State::NoGit,
            Repo::Failed(why) => State::Failed(why),
        };
        self.rebuild();
        self.reselect(keep);
        true
    }

    /// Forgets the repository, for a folder that may be another one. The message stays — it is
    /// the user's text — and so does the count of questions asked, so an answer about the old
    /// folder still on its way cannot pass for one about the new.
    pub fn forget(&mut self) {
        *self = View {
            message: std::mem::take(&mut self.message),
            caret: self.caret,
            asked: self.asked,
            ..View::default()
        };
    }

    pub fn status(&self) -> Option<&Status> {
        match &self.state {
            State::Ready(status) => Some(status),
            _ => None,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// How many files `group` lists — counted when the status arrived, never per frame.
    pub fn count(&self, group: Group) -> usize {
        self.counts[group.index()]
    }

    /// The entry a row is about, for a file row.
    pub fn entry(&self, row: usize) -> Option<(Group, &Entry)> {
        match self.rows.get(row)? {
            Row::File { group, entry } => Some((*group, self.status()?.entries.get(*entry)?)),
            Row::Header(_) => None,
        }
    }

    /// The paths a group lists, for its heading's buttons.
    pub fn paths(&self, group: Group) -> Vec<String> {
        let Some(status) = self.status() else {
            return Vec::new();
        };
        status
            .entries
            .iter()
            .filter(|entry| entry.in_group(group))
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Shows or hides a group's files.
    pub fn toggle(&mut self, group: Group) {
        let keep = self.selected_key();
        self.collapsed[group.index()] = !self.collapsed[group.index()];
        self.rebuild();
        self.reselect(keep.or(Some((group, None, 0))));
    }

    fn rebuild(&mut self) {
        self.rows.clear();
        self.counts = [0; 3];
        let Some(status) = self.status() else {
            self.selected = None;
            self.first = 0;
            return;
        };
        let mut rows = Vec::new();
        let mut counts = [0; 3];
        for group in Group::ALL {
            let members = status.group(group);
            counts[group.index()] = members.len();
            // Changes is always there, as in VSCode: an empty heading is how a clean tree says so.
            if members.is_empty() && group != Group::Changes {
                continue;
            }
            rows.push(Row::Header(group));
            if !self.collapsed[group.index()] {
                rows.extend(members.into_iter().map(|entry| Row::File { group, entry }));
            }
        }
        self.rows = rows;
        self.counts = counts;
    }

    /// The selected row as something that survives a rebuild: its group, its path, and how far
    /// down its group it was.
    fn selected_key(&self) -> Option<Key> {
        let at = self.selected?;
        let row = *self.rows.get(at)?;
        Some(match row {
            Row::Header(group) => (group, None, 0),
            Row::File { group, entry } => {
                let header = self.rows[..at]
                    .iter()
                    .rposition(|row| *row == Row::Header(group))
                    .unwrap_or(0);
                (
                    group,
                    self.status()
                        .and_then(|status| status.entries.get(entry))
                        .map(|entry| entry.path.clone()),
                    at - header - 1,
                )
            }
        })
    }

    fn reselect(&mut self, key: Option<Key>) {
        let keep_at = self.selected;
        let Some((group, path, ordinal)) = key else {
            self.selected = self
                .selected
                .map(|at| at.min(self.rows.len().saturating_sub(1)));
            return;
        };
        let found = self.rows.iter().position(|row| match row {
            Row::Header(at) => path.is_none() && *at == group,
            Row::File { group: at, entry } => {
                *at == group
                    && path.as_deref()
                        == self
                            .status()
                            .and_then(|status| status.entries.get(*entry))
                            .map(|entry| entry.path.as_str())
            }
        });
        // A file that left its group — staged, say — leaves the selection where it was in that
        // group, on what moved up into its place: pressing `s` five times stages five files.
        self.selected = found.or_else(|| {
            let header = self
                .rows
                .iter()
                .position(|row| *row == Row::Header(group))?;
            let files = self.rows[header + 1..]
                .iter()
                .take_while(|row| matches!(row, Row::File { group: at, .. } if *at == group))
                .count();
            Some(if files == 0 {
                header
            } else {
                header + 1 + ordinal.min(files - 1)
            })
        });
        // The whole group went — the last staged file unstaged: stay at about the same place.
        if self.selected.is_none() && !self.rows.is_empty() {
            self.selected = keep_at.map(|at| at.min(self.rows.len() - 1));
        }
        if self.rows.is_empty() {
            self.selected = None;
        }
    }

    pub fn move_selection(&mut self, delta: isize, capacity: usize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        let at = match self.selected {
            Some(at) => (at as isize + delta).clamp(0, last),
            None if delta < 0 => last,
            None => 0,
        };
        self.selected = Some(at as usize);
        self.reveal(capacity);
    }

    /// Scrolls so the selection is on screen.
    pub fn reveal(&mut self, capacity: usize) {
        let Some(at) = self.selected else {
            return;
        };
        let capacity = capacity.max(1);
        if at < self.first {
            self.first = at;
        } else if at >= self.first + capacity {
            self.first = at + 1 - capacity;
        }
    }

    pub fn scroll(&mut self, rows: isize, capacity: usize) {
        let most = self.rows.len().saturating_sub(capacity.max(1));
        self.first = (self.first as isize + rows).clamp(0, most as isize) as usize;
    }

    pub fn first(&self) -> usize {
        self.first
    }

    // ---- the message box ----------------------------------------------------------------------

    pub fn type_text(&mut self, text: &str) {
        let at = byte_at(&self.message, self.caret);
        self.message.insert_str(at, text);
        self.caret += text.chars().count();
        self.confirm = None;
    }

    pub fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        let start = byte_at(&self.message, self.caret - 1);
        let end = byte_at(&self.message, self.caret);
        self.message.replace_range(start..end, "");
        self.caret -= 1;
    }

    pub fn delete(&mut self) {
        let start = byte_at(&self.message, self.caret);
        let end = byte_at(&self.message, self.caret + 1);
        self.message.replace_range(start..end, "");
    }

    pub fn move_caret(&mut self, delta: isize) {
        let length = self.message.chars().count() as isize;
        self.caret = (self.caret as isize + delta).clamp(0, length) as usize;
    }

    /// To the start or end of the caret's line.
    pub fn caret_to_line_edge(&mut self, end: bool) {
        let (line, column) = self.caret_line_column();
        let length = self
            .message
            .split('\n')
            .nth(line)
            .map_or(0, |text| text.chars().count());
        if end {
            self.caret += length - column;
        } else {
            self.caret -= column;
        }
    }

    /// The caret's line and column in the message.
    pub fn caret_line_column(&self) -> (usize, usize) {
        let before: String = self.message.chars().take(self.caret).collect();
        let line = before.matches('\n').count();
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |text| text.chars().count());
        (line, column)
    }

    /// Takes the message for a commit, leaving the box empty.
    pub fn take_message(&mut self) -> String {
        self.caret = 0;
        std::mem::take(&mut self.message)
    }

    /// The first line of the message box shown, so the caret's line is always in view.
    pub fn message_top(&self) -> usize {
        let (line, _) = self.caret_line_column();
        line.saturating_sub(MESSAGE_LINES - 1)
    }

    // ---- where things are ---------------------------------------------------------------------

    /// Lays the view out in `rect`.
    pub fn geometry(
        &self,
        rect: Rect,
        scale: f32,
        typography: &dacode_theme::Typography,
    ) -> Geometry {
        let pad = 12.0 * scale;
        let left = rect.x + pad;
        let width = (rect.width - pad * 2.0).max(0.0);
        let gap = 6.0 * scale;
        let button = 22.0 * scale;

        let heading = Rect::new(left, rect.y + 10.0 * scale, width, 22.0 * scale);
        let ready = self.status().is_some();
        let mut tools = Vec::new();
        if ready {
            let mut x = heading.right();
            for tool in Tool::ALL.iter().rev() {
                x -= button;
                tools.push((*tool, Rect::new(x, heading.y, button, button)));
                x -= 2.0 * scale;
            }
            tools.reverse();
        }

        let line = Rect::new(left, heading.bottom() + 4.0 * scale, width, 24.0 * scale);
        let advance = crate::layout::measured_advance(typography, dacode_theme::Role::Ui, scale);
        let (behind, ahead) = self
            .status()
            .map_or((0, 0), |status| (status.behind, status.ahead));
        let chip = |count: usize| 22.0 * scale + count.to_string().len() as f32 * advance;
        let push = Rect::new(line.right() - chip(ahead), line.y, chip(ahead), line.height);
        let pull = Rect::new(
            push.x - 2.0 * scale - chip(behind),
            line.y,
            chip(behind),
            line.height,
        );
        let branch = Rect::new(
            line.x,
            line.y,
            (pull.x - 4.0 * scale - line.x).max(0.0),
            line.height,
        );

        let shown_lines = self.message.split('\n').count().clamp(1, MESSAGE_LINES);
        let message = Rect::new(
            left,
            line.bottom() + gap,
            width,
            shown_lines as f32 * MESSAGE_LINE * scale + 10.0 * scale,
        );
        let commit = Rect::new(left, message.bottom() + gap, width, 28.0 * scale);
        let summary = Rect::new(left, commit.bottom() + 4.0 * scale, width, 20.0 * scale);

        let row = ROW * scale;
        let top = summary.bottom() + 4.0 * scale;
        let capacity = if ready {
            (((rect.bottom() - pad) - top) / row).floor().max(0.0) as usize
        } else {
            0
        };
        let first = self.first.min(self.rows.len().saturating_sub(capacity));
        let rows = (first..self.rows.len().min(first + capacity))
            .enumerate()
            .map(|(slot, index)| {
                (
                    index,
                    Rect::new(rect.x, top + slot as f32 * row, rect.width, row),
                )
            })
            .collect();

        let init = matches!(self.state, State::NotARepository)
            .then(|| Rect::new(left, heading.bottom() + 10.0 * scale, width, 28.0 * scale));
        let notice_top = init.map_or(heading.bottom() + 10.0 * scale, |button| {
            button.bottom() + 12.0 * scale
        });
        let notice = Rect::new(
            left,
            notice_top,
            width,
            (rect.bottom() - pad - notice_top).max(0.0),
        );

        Geometry {
            heading,
            tools,
            branch,
            pull,
            push,
            message,
            commit,
            summary,
            rows,
            capacity,
            init,
            notice,
        }
    }

    /// The buttons a row offers, left to right.
    pub fn actions(&self, row: usize) -> &'static [RowAction] {
        use RowAction::*;
        match self.rows.get(row) {
            Some(Row::Header(Group::Merge)) => &[StageAll],
            Some(Row::Header(Group::Staged)) => &[UnstageAll],
            Some(Row::Header(Group::Changes)) => &[DiscardAll, StageAll],
            Some(Row::File {
                group: Group::Merge,
                ..
            }) => &[Open, Stage],
            Some(Row::File {
                group: Group::Staged,
                ..
            }) => &[Open, Unstage],
            Some(Row::File {
                group: Group::Changes,
                ..
            }) => &[Open, Discard, Stage],
            None => &[],
        }
    }

    /// Where a row's buttons are: right-aligned, before the column its letter or count takes.
    pub fn action_rects(&self, row: usize, rect: Rect, scale: f32) -> Vec<(RowAction, Rect)> {
        let actions = self.actions(row);
        let size = ACTION * scale;
        let right = rect.right() - 12.0 * scale - 22.0 * scale;
        let top = rect.y + (rect.height - size) / 2.0;
        let start = right - actions.len() as f32 * size;
        actions
            .iter()
            .enumerate()
            .map(|(slot, action)| {
                (
                    *action,
                    Rect::new(start + slot as f32 * size, top, size, size),
                )
            })
            .collect()
    }

    /// What a press at `(x, y)` lands on.
    pub fn hit(&self, geometry: &Geometry, x: f32, y: f32, scale: f32) -> Option<Hit> {
        use crate::layout::contains;
        if let Some(init) = geometry.init {
            return contains(init, x, y).then_some(Hit::Init);
        }
        self.status()?;
        if let Some((tool, _)) = geometry.tools.iter().find(|(_, r)| contains(*r, x, y)) {
            return Some(Hit::Tool(*tool));
        }
        for (rect, hit) in [
            (geometry.pull, Hit::Pull),
            (geometry.push, Hit::Push),
            (geometry.branch, Hit::Branch),
            (geometry.message, Hit::Message),
            (geometry.commit, Hit::Commit),
        ] {
            if contains(rect, x, y) {
                return Some(hit);
            }
        }
        let (row, rect) = geometry.rows.iter().find(|(_, r)| contains(*r, x, y))?;
        if let Some((action, _)) = self
            .action_rects(*row, *rect, scale)
            .into_iter()
            .find(|(_, r)| contains(*r, x, y))
        {
            return Some(Hit::Action(*row, action));
        }
        Some(Hit::Row(*row))
    }
}

/// A selection by what it is rather than where: group, path, and place in the group.
type Key = (Group, Option<String>, usize);

/// The byte index of char `chars` in `text`, or its end.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(at, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::parse_status;

    fn status(lines: &[&str]) -> Repo {
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(0);
        }
        let mut status = parse_status(&bytes);
        status.root = std::path::PathBuf::from("/repo");
        Repo::Ready(Box::new(status))
    }

    fn ready(lines: &[&str]) -> View {
        let mut view = View::default();
        let version = view.ask();
        assert!(view.accept(version, status(lines)));
        view
    }

    const SAMPLE: &[&str] = &[
        "# branch.head main",
        "1 M. N... 100644 100644 100644 aaaa bbbb src/staged.rs",
        "1 .M N... 100644 100644 100644 aaaa bbbb src/changed.rs",
        "? new.txt",
    ];

    #[test]
    fn a_status_for_a_question_already_superseded_is_refused() {
        // A stage answered by the status from before it would put the file back in the list it was
        // just taken out of, for as long as the newer answer took.
        let mut view = View::default();
        let old = view.ask();
        let new = view.ask();
        assert!(!view.accept(old, status(SAMPLE)));
        assert_eq!(view.state, State::Unknown);
        assert!(view.accept(new, status(SAMPLE)));
    }

    #[test]
    fn groups_are_listed_in_vscodes_order_and_an_empty_one_is_left_out() {
        let view = ready(SAMPLE);
        let rows = view.rows();
        assert_eq!(
            rows[0],
            Row::Header(Group::Staged),
            "no conflicts, no merge heading"
        );
        assert!(matches!(
            rows[1],
            Row::File {
                group: Group::Staged,
                ..
            }
        ));
        assert_eq!(rows[2], Row::Header(Group::Changes));
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn a_clean_tree_still_shows_the_changes_heading() {
        let view = ready(&["# branch.head main"]);
        assert_eq!(view.rows(), [Row::Header(Group::Changes)]);
    }

    #[test]
    fn collapsing_a_group_hides_its_files_and_keeps_its_heading_selected() {
        let mut view = ready(SAMPLE);
        view.selected = Some(3);
        view.toggle(Group::Changes);
        assert_eq!(view.rows().len(), 3);
        assert_eq!(
            view.selected,
            Some(2),
            "the heading of the group just folded"
        );
    }

    #[test]
    fn the_selection_follows_its_file_through_a_new_status() {
        let mut view = ready(SAMPLE);
        view.selected = Some(4); // new.txt
        let version = view.ask();
        // Something new arrives above it.
        view.accept(
            version,
            status(&[
                "# branch.head main",
                "1 M. N... 100644 100644 100644 aaaa bbbb a.rs",
                "1 M. N... 100644 100644 100644 aaaa bbbb src/staged.rs",
                "1 .M N... 100644 100644 100644 aaaa bbbb src/changed.rs",
                "? new.txt",
            ]),
        );
        assert_eq!(
            view.entry(view.selected.unwrap()).unwrap().1.path,
            "new.txt"
        );
    }

    #[test]
    fn staging_the_selected_file_leaves_the_selection_on_the_next_one() {
        // Pressing `s` down a list should stage the list, one file per press.
        let mut view = ready(&[
            "# branch.head main",
            "1 .M N... 100644 100644 100644 aaaa bbbb a.rs",
            "1 .M N... 100644 100644 100644 aaaa bbbb b.rs",
        ]);
        view.selected = Some(1);
        let version = view.ask();
        view.accept(
            version,
            status(&[
                "# branch.head main",
                "1 M. N... 100644 100644 100644 aaaa bbbb a.rs",
                "1 .M N... 100644 100644 100644 aaaa bbbb b.rs",
            ]),
        );
        let (_, entry) = view.entry(view.selected.unwrap()).unwrap();
        assert_eq!(entry.path, "b.rs");
    }

    #[test]
    fn the_message_box_edits_at_its_caret_across_lines() {
        let mut view = View::default();
        view.type_text("Fix the");
        view.type_text("\n");
        view.type_text("body");
        assert_eq!(view.caret_line_column(), (1, 4));
        view.caret_to_line_edge(false);
        view.type_text("é ");
        assert_eq!(view.message, "Fix the\né body");
        view.move_caret(-3);
        view.backspace();
        assert_eq!(view.message, "Fix th\né body");
        view.delete();
        assert_eq!(
            view.message, "Fix thé body",
            "delete takes the line break after the caret"
        );
        view.caret_to_line_edge(true);
        assert_eq!(view.caret, 12);
        assert_eq!(view.take_message(), "Fix thé body");
        assert_eq!((view.message.as_str(), view.caret), ("", 0));
    }

    fn typography() -> dacode_theme::Typography {
        dacode_theme::Typography::default()
    }

    #[test]
    fn a_click_lands_on_the_row_button_drawn_under_it() {
        let view = ready(SAMPLE);
        let rect = Rect::new(48.0, 40.0, 252.0, 700.0);
        let geometry = view.geometry(rect, 1.0, &typography());
        // The second file row is `src/changed.rs` under Changes: open, discard, stage.
        let (row, row_rect) = geometry.rows[3];
        let buttons = view.action_rects(row, row_rect, 1.0);
        assert_eq!(
            buttons.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [RowAction::Open, RowAction::Discard, RowAction::Stage]
        );
        for (action, at) in &buttons {
            let (x, y) = (at.x + at.width / 2.0, at.y + at.height / 2.0);
            assert_eq!(
                view.hit(&geometry, x, y, 1.0),
                Some(Hit::Action(row, *action))
            );
        }
        let name = (row_rect.x + 30.0, row_rect.y + row_rect.height / 2.0);
        assert_eq!(
            view.hit(&geometry, name.0, name.1, 1.0),
            Some(Hit::Row(row))
        );
        let (tool, at) = geometry.tools[0];
        assert_eq!(
            view.hit(&geometry, at.x + 2.0, at.y + 2.0, 1.0),
            Some(Hit::Tool(tool))
        );
    }

    #[test]
    fn rows_are_laid_out_only_for_the_viewport() {
        // A status of thousands must cost a frame what a status of forty does.
        let lines: Vec<String> = std::iter::once("# branch.head main".to_string())
            .chain((0..3000).map(|index| format!("? file{index}.txt")))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut view = ready(&refs);
        let rect = Rect::new(48.0, 40.0, 252.0, 600.0);
        let geometry = view.geometry(rect, 1.0, &typography());
        assert!(geometry.rows.len() <= geometry.capacity);
        assert!(geometry.capacity < 40);
        view.selected = Some(2500);
        view.reveal(geometry.capacity);
        let geometry = view.geometry(rect, 1.0, &typography());
        assert!(geometry.rows.iter().any(|(index, _)| *index == 2500));
    }

    #[test]
    fn with_no_repository_the_only_thing_to_press_is_initialize() {
        let mut view = View::default();
        let version = view.ask();
        view.accept(version, Repo::NotARepository);
        let geometry = view.geometry(Rect::new(0.0, 0.0, 252.0, 600.0), 1.0, &typography());
        let init = geometry.init.expect("a button");
        assert_eq!(
            view.hit(&geometry, init.x + 4.0, init.y + 4.0, 1.0),
            Some(Hit::Init)
        );
        assert!(geometry.tools.is_empty());
        assert_eq!(view.hit(&geometry, 100.0, 400.0, 1.0), None);
    }
}
