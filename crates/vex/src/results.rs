//! The project-search results panel.
//!
//! A list of `path:line` hits that streams in while the search runs, and opens a file at the line
//! when chosen. Streaming matters: on a large project the first hit should be usable long before the
//! walk finishes, which is why the panel renders whatever has arrived instead of waiting.

use std::path::PathBuf;
use vex_project::{ProjectSearch, SearchHit, SearchQuery};

/// How many hits the panel keeps. Past this the search is stopped: a query matching most of the
/// project is a query to refine, not a list to scroll.
pub const MAX_HITS: usize = 500;

/// Rows shown at once.
pub const VISIBLE_ROWS: usize = 10;

pub struct Results {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    /// The running search, dropped (and cancelled) when it finishes or the panel closes.
    search: Option<ProjectSearch>,
    pub finished: bool,
    /// Set when the hit cap was reached, so the count is not presented as complete.
    pub truncated: bool,
}

impl Results {
    /// A finished panel built from hits someone else found — language-server references, say.
    ///
    /// Sharing the panel rather than growing a second list view: both are `path:line` plus a line of
    /// context that you move through and open, and two implementations of that would drift.
    pub fn from_hits(root: &std::path::Path, query: &str, hits: Vec<SearchHit>) -> Self {
        let _ = root;
        Self {
            query: query.to_string(),
            hits,
            selected: 0,
            search: None,
            finished: true,
            truncated: false,
        }
    }

    /// Starts a search. Literal, matching `/` in a buffer; `:grep` is where regex would go.
    pub fn start(root: &std::path::Path, query: &str) -> Self {
        let search = ProjectSearch::start(
            root,
            SearchQuery {
                pattern: query.to_string(),
                literal: true,
                limit: MAX_HITS,
                ..Default::default()
            },
        );

        Self {
            query: query.to_string(),
            hits: Vec::new(),
            selected: 0,
            search: Some(search),
            finished: false,
            truncated: false,
        }
    }

    /// Collects whatever has arrived. Returns true if anything changed, so the caller only redraws
    /// when there is something new.
    pub fn poll(&mut self) -> bool {
        let Some(search) = &self.search else {
            return false;
        };

        let incoming = search.poll();
        let mut changed = !incoming.is_empty();

        for hit in incoming {
            if self.hits.len() >= MAX_HITS {
                self.truncated = true;
                break;
            }
            self.hits.push(hit);
        }

        if search.is_finished() || self.truncated {
            // Dropping the handle cancels the walk, which matters when the cap stopped us early.
            self.search = None;
            self.finished = true;
            changed = true;
        }

        changed
    }

    pub fn selection(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() - 1;
        self.selected = match delta {
            d if d < 0 => self.selected.saturating_sub(d.unsigned_abs()),
            d => (self.selected + d as usize).min(last),
        };
    }

    /// The window of hits to draw, and the selection's index within it.
    pub fn visible(&self) -> (&[SearchHit], usize) {
        if self.hits.len() <= VISIBLE_ROWS {
            return (&self.hits, self.selected);
        }
        let start = self
            .selected
            .saturating_sub(VISIBLE_ROWS - 1)
            .min(self.hits.len() - VISIBLE_ROWS);
        (
            &self.hits[start..start + VISIBLE_ROWS],
            self.selected - start,
        )
    }

    /// A path shown relative to the project root, since the absolute one is mostly noise.
    pub fn relative(&self, hit: &SearchHit, root: &std::path::Path) -> String {
        hit.path
            .strip_prefix(root)
            .unwrap_or(&hit.path)
            .to_string_lossy()
            .to_string()
    }

    pub fn summary(&self) -> String {
        let files = {
            let mut paths: Vec<&PathBuf> = self.hits.iter().map(|hit| &hit.path).collect();
            paths.sort_unstable();
            paths.dedup();
            paths.len()
        };

        let plural = |count: usize, word: &str| {
            if count == 1 {
                format!("{count} {word}")
            } else {
                format!("{count} {word}s")
            }
        };

        // A capped count reads `500+ hits`, with the marker on the number rather than the word.
        let hits = if self.truncated {
            format!("{}+ hits", self.hits.len())
        } else {
            plural(self.hits.len(), "hit")
        };

        format!(
            "{hits} in {}{}",
            plural(files, "file"),
            if self.finished {
                ""
            } else {
                " — searching…"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, line: u64) -> SearchHit {
        SearchHit {
            path: PathBuf::from(path),
            line_number: line,
            line: "let needle = 1;".into(),
            range: 4..10,
        }
    }

    fn panel(hits: Vec<SearchHit>) -> Results {
        Results {
            query: "needle".into(),
            hits,
            selected: 0,
            search: None,
            finished: true,
            truncated: false,
        }
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut results = panel(vec![hit("a.rs", 1), hit("b.rs", 2), hit("c.rs", 3)]);

        results.move_selection(1);
        assert_eq!(results.selected, 1);
        results.move_selection(99);
        assert_eq!(results.selected, 2);
        results.move_selection(-99);
        assert_eq!(results.selected, 0);
    }

    #[test]
    fn an_empty_panel_has_no_selection_and_does_not_panic() {
        let mut results = panel(Vec::new());
        assert!(results.selection().is_none());
        results.move_selection(1);
    }

    #[test]
    fn the_window_follows_the_selection() {
        let hits: Vec<SearchHit> = (0..30).map(|i| hit("f.rs", i)).collect();
        let mut results = panel(hits);

        let (rows, cursor) = results.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert_eq!(cursor, 0);

        results.move_selection(VISIBLE_ROWS as isize + 2);
        let (rows, cursor) = results.visible();
        assert_eq!(rows.len(), VISIBLE_ROWS);
        assert!(cursor < VISIBLE_ROWS);
        assert_eq!(
            rows[cursor].line_number,
            results.hits[results.selected].line_number
        );
    }

    #[test]
    fn paths_are_shown_relative_to_the_root() {
        let results = panel(vec![hit("/project/src/main.rs", 3)]);
        assert_eq!(
            results.relative(&results.hits[0], std::path::Path::new("/project")),
            "src/main.rs"
        );
    }

    #[test]
    fn the_summary_counts_files_not_just_hits() {
        let results = panel(vec![
            hit("/p/a.rs", 1),
            hit("/p/a.rs", 9),
            hit("/p/b.rs", 2),
        ]);
        assert_eq!(results.summary(), "3 hits in 2 files");
        assert_eq!(
            panel(vec![hit("/p/a.rs", 1)]).summary(),
            "1 hit in 1 file",
            "singular counts must not read as plural"
        );
    }

    #[test]
    fn an_unfinished_search_says_so_and_a_capped_one_marks_the_count() {
        let mut results = panel(vec![hit("/p/a.rs", 1)]);
        results.finished = false;
        assert!(results.summary().contains("searching"));

        results.finished = true;
        results.truncated = true;
        assert_eq!(results.summary(), "1+ hits in 1 file");
        assert!(
            results.summary().starts_with("1+"),
            "a capped count must not read as complete: {}",
            results.summary()
        );
    }
}
