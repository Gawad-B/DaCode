//! The project-search results panel.
//!
//! A list of `path:line` hits that streams in while the search runs, and opens a file at the line
//! when chosen. Streaming matters: on a large project the first hit should be usable long before the
//! walk finishes, which is why the panel renders whatever has arrived instead of waiting.

use dacode_project::{ProjectSearch, SearchHit, SearchQuery};
use dacode_render::Rect;
use std::path::PathBuf;

/// How many hits the panel keeps. Past this the search is stopped: a query matching most of the
/// project is a query to refine, not a list to scroll.
pub const MAX_HITS: usize = 500;

/// Rows shown at once, at most. Fewer on a window too short to give the panel room for them.
pub const VISIBLE_ROWS: usize = 10;

/// The height of the header and of each row, at scale 1. [`crate::layout::Layout::of`] sizes the
/// panel in these, and [`Results::geometry`] divides it back into them.
pub const ROW: f32 = 22.0;

/// Where the panel's header and its visible rows are, as data: drawing, the mouse, the keys and the
/// wheel all read this, so the rows the selection moves through are the rows that are drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub header: Rect,
    /// Everything under the header: where "no matches" goes when there are no rows.
    pub list: Rect,
    /// One rectangle per visible hit: `rows[i]` is hit `first + i`.
    pub rows: Vec<Rect>,
    pub first: usize,
    /// How many rows the panel has room for.
    pub capacity: usize,
}

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
        Self::start_with(
            root,
            SearchQuery {
                pattern: query.to_string(),
                literal: true,
                ..Default::default()
            },
        )
    }

    /// Starts a search with its options chosen: what the search panel's toggles set.
    pub fn start_with(root: &std::path::Path, query: SearchQuery) -> Self {
        let pattern = query.pattern.clone();
        let search = ProjectSearch::start(
            root,
            SearchQuery {
                limit: MAX_HITS,
                ..query
            },
        );

        Self {
            query: pattern,
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

    /// How many rows the panel asks the layout for: all the hits, up to [`VISIBLE_ROWS`].
    pub fn wanted_rows(&self) -> usize {
        self.hits.len().min(VISIBLE_ROWS)
    }

    /// The hits on screen given room for `capacity` rows: a window holding the selection, which
    /// moves only when the selection would leave it.
    pub fn window(&self, capacity: usize) -> std::ops::Range<usize> {
        let capacity = capacity.min(self.hits.len());
        if capacity == 0 {
            return 0..0;
        }
        let start = self
            .selected
            .saturating_sub(capacity - 1)
            .min(self.hits.len() - capacity);
        start..start + capacity
    }

    /// Where the header and the visible rows are inside `panel`, the rectangle the layout gave it.
    ///
    /// The capacity is the panel's own: the layout asks for up to ten rows and is capped at half
    /// the editor's height, which on a short window is fewer. The window used to be a fixed ten
    /// whatever was drawn, so stepping down a 640x480 panel moved the selection onto rows that were
    /// cut off the bottom and kept it there — Enter opened a hit nobody could see.
    pub fn geometry(&self, panel: Rect, scale: f32) -> Geometry {
        let row = ROW * scale;
        let (header, list) = panel.split_top(row.min(panel.height));
        let capacity = ((list.height / row).floor().max(0.0) as usize).min(VISIBLE_ROWS);
        let window = self.window(capacity);
        let rows = window
            .clone()
            .map(|index| {
                Rect::new(
                    list.x,
                    list.y + (index - window.start) as f32 * row,
                    list.width,
                    row,
                )
            })
            .collect();
        Geometry {
            header,
            list,
            rows,
            first: window.start,
            capacity,
        }
    }

    /// Which hit a point is over, by its index in [`Results::hits`].
    pub fn hit_at(geometry: &Geometry, x: f32, y: f32) -> Option<usize> {
        geometry
            .rows
            .iter()
            .position(|row| crate::layout::contains(*row, x, y))
            .map(|at| geometry.first + at)
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

        assert_eq!(results.window(VISIBLE_ROWS), 0..VISIBLE_ROWS);
        results.move_selection(VISIBLE_ROWS as isize + 2);
        let window = results.window(VISIBLE_ROWS);
        assert_eq!(window.len(), VISIBLE_ROWS);
        assert!(window.contains(&results.selected));
    }

    #[test]
    fn a_short_panel_keeps_the_selection_on_a_drawn_row() {
        // Seven rows of room, as a 640x480 window gives it: every step down must land on one of
        // the seven, never on the three a fixed window of ten would have held below the cut.
        let hits: Vec<SearchHit> = (0..40).map(|i| hit("f.rs", i)).collect();
        let mut results = panel(hits);
        let panel_rect = Rect::new(0.0, 300.0, 600.0, ROW * 8.0);
        for _ in 0..15 {
            results.move_selection(1);
            let geometry = results.geometry(panel_rect, 1.0);
            assert_eq!(geometry.rows.len(), 7);
            let at = results.selected - geometry.first;
            assert!(
                at < geometry.rows.len(),
                "selection {} is not drawn",
                results.selected
            );
            assert!(geometry.rows[at].bottom() <= panel_rect.bottom());
        }
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
