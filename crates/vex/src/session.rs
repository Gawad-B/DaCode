//! Remembering what was open, per project.
//!
//! Distinct from crash recovery, which exists to save *unsaved text*. A session is about where you
//! were: which files, in which order, at which line. The two are deliberately separate, because they
//! answer to different failures — recovery must survive a kill -9 and so is written every two
//! seconds, while a session is only interesting when the editor closed properly and can be written
//! once, on the way out.
//!
//! Keyed by project root, so opening two projects does not have them fighting over one file. Stored
//! in `$XDG_STATE_HOME/vex/sessions`, because it is state the user never edits — unlike config.

use std::path::{Path, PathBuf};

/// One file, and where the cursor was in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFile {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub files: Vec<OpenFile>,
    /// Which of them was on screen.
    pub active: usize,
}

impl Session {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// A tab-separated line per file: `path\tline\tcolumn`, with the active one marked.
    ///
    /// A hand-rolled format for the same reason the config reader is: this is four fields the editor
    /// both writes and reads, and a serialization framework for it would be dependency and compile
    /// time spent on nothing. Tabs, because a path can contain almost anything else.
    pub fn encode(&self) -> String {
        let mut out = String::from("# vex session\n");
        for (index, file) in self.files.iter().enumerate() {
            out.push_str(&format!(
                "{}{}\t{}\t{}\n",
                if index == self.active { "*" } else { " " },
                file.path.display(),
                file.line,
                file.column
            ));
        }
        out
    }

    pub fn decode(text: &str) -> Self {
        let mut session = Session::default();

        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let active = line.starts_with('*');
            let rest = &line[1.min(line.len())..];

            let mut fields = rest.split('\t');
            let Some(path) = fields.next() else { continue };
            if path.is_empty() {
                continue;
            }
            let line_number = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
            let column = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);

            if active {
                session.active = session.files.len();
            }
            session.files.push(OpenFile {
                path: PathBuf::from(path),
                line: line_number,
                column,
            });
        }

        session
    }
}

/// Where sessions are kept.
pub fn session_dir() -> PathBuf {
    crate::paths::state_dir().join("sessions")
}

/// The file a project's session is stored in.
///
/// Named after the path, with separators flattened, so two projects with the same directory name do
/// not collide — the same rule the recovery writer uses.
pub fn session_path(directory: &Path, root: &Path) -> PathBuf {
    let flattened: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    directory.join(format!("{}.session", flattened.trim_matches('-')))
}

/// Where the name of the last project worked in is kept.
///
/// One line, beside the sessions. Sessions are keyed by root, which answers "what was I doing in
/// *this* project" — but starting vex from a shortcut, a Start Menu entry or a desktop icon gives no
/// project at all, and the working directory in that case is wherever the launcher happened to point
/// (on Windows, the install directory). This is the answer to the other question: which project was
/// I in last.
fn last_project_path(directory: &Path) -> PathBuf {
    directory.join("last-project")
}

/// Records the project as the one to reopen. Called when a project is opened, not on the way out, so
/// it survives a crash — the whole point is to not have to navigate back.
pub fn remember_project(directory: &Path, root: &Path) {
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let _ = std::fs::write(last_project_path(directory), root.to_string_lossy().as_ref());
}

/// The last project worked in, if it still exists.
///
/// A directory that has since been deleted or unmounted is not offered: opening vex into a folder
/// that is not there would be worse than opening it where you are.
pub fn last_project(directory: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(last_project_path(directory)).ok()?;
    let path = PathBuf::from(text.trim());
    (path.is_dir()).then_some(path)
}

/// Saves a session for a project. Failure is not reported: losing a session is a small
/// inconvenience, and an error dialog on the way out is a larger one.
pub fn save(directory: &Path, root: &Path, session: &Session) {
    if session.is_empty() {
        return;
    }
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let _ = std::fs::write(session_path(directory, root), session.encode());
}

/// Loads a project's session, dropping files that no longer exist.
///
/// Checked here rather than at open time: a session listing three files where one has since been
/// deleted should restore the two, not show an error about the third.
pub fn load(directory: &Path, root: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(session_path(directory, root)).ok()?;
    let mut session = Session::decode(&text);

    let active_path = session
        .files
        .get(session.active)
        .map(|file| file.path.clone());
    session.files.retain(|file| file.path.exists());

    // The active index refers to a position in a list that just changed under it.
    session.active = active_path
        .and_then(|path| session.files.iter().position(|file| file.path == path))
        .unwrap_or(0);

    (!session.is_empty()).then_some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vex-session-{}-{label}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session() -> Session {
        Session {
            files: vec![
                OpenFile {
                    path: PathBuf::from("/project/src/main.rs"),
                    line: 42,
                    column: 8,
                },
                OpenFile {
                    path: PathBuf::from("/project/README.md"),
                    line: 0,
                    column: 0,
                },
            ],
            active: 1,
        }
    }

    #[test]
    fn a_session_round_trips() {
        let decoded = Session::decode(&session().encode());
        assert_eq!(decoded, session());
    }

    #[test]
    fn a_path_containing_spaces_survives() {
        // Tab-separated precisely so this works; a space-separated format would split the path.
        let original = Session {
            files: vec![OpenFile {
                path: PathBuf::from("/some project/a file.rs"),
                line: 3,
                column: 1,
            }],
            active: 0,
        };
        assert_eq!(Session::decode(&original.encode()), original);
    }

    #[test]
    fn a_corrupt_line_is_skipped_rather_than_failing_the_whole_session() {
        let decoded = Session::decode(" /a.rs\t5\t0\nnonsense\n /b.rs\tnot-a-number\tx\n");
        assert_eq!(decoded.files.len(), 3, "every line names a file");
        assert_eq!(
            decoded.files[2].line, 0,
            "unparseable numbers fall back to 0"
        );
    }

    #[test]
    fn saving_and_loading_drops_files_that_no_longer_exist() {
        let dir = scratch("missing");
        let present = dir.join("present.rs");
        std::fs::write(&present, "x").unwrap();

        let original = Session {
            files: vec![
                OpenFile {
                    path: dir.join("gone.rs"),
                    line: 1,
                    column: 0,
                },
                OpenFile {
                    path: present.clone(),
                    line: 7,
                    column: 2,
                },
            ],
            active: 1,
        };

        save(&dir, Path::new("/project"), &original);
        let loaded = load(&dir, Path::new("/project")).expect("loads");

        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].path, present);
        assert_eq!(
            loaded.active, 0,
            "the active index follows the file it pointed at"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_projects_do_not_share_a_session() {
        let dir = scratch("projects");
        let file = dir.join("f.rs");
        std::fs::write(&file, "x").unwrap();

        let one = Session {
            files: vec![OpenFile {
                path: file.clone(),
                line: 1,
                column: 0,
            }],
            active: 0,
        };
        let two = Session {
            files: vec![OpenFile {
                path: file.clone(),
                line: 99,
                column: 0,
            }],
            active: 0,
        };

        save(&dir, Path::new("/home/a/project"), &one);
        save(&dir, Path::new("/home/b/project"), &two);

        assert_eq!(
            load(&dir, Path::new("/home/a/project")).unwrap().files[0].line,
            1
        );
        assert_eq!(
            load(&dir, Path::new("/home/b/project")).unwrap().files[0].line,
            99
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_session_is_not_written() {
        // Quitting with nothing open should not leave a file that restores nothing.
        let dir = scratch("empty");
        save(&dir, Path::new("/project"), &Session::default());
        assert!(load(&dir, Path::new("/project")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_session_is_not_an_error() {
        assert!(load(Path::new("/nonexistent/vex"), Path::new("/project")).is_none());
    }
}
