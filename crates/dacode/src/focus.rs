//! Where the keyboard goes.
//!
//! There used to be no such thing. A key was offered to the settings panel, then to the file tree,
//! and only then to the editor — so with the tree merely *open*, `↓` moved through files instead of
//! lines, whatever you had just clicked. Being open is not the same as being the thing you are
//! talking to, and the router now asks the one question that distinguishes them.
//!
//! A click sets focus and nothing else does by surprise: opening a view hands it the keys, because
//! that is why it was opened, and closing it hands them back.

use crate::layout::{Layout, contains};

/// The surface that receives keys the grammar would otherwise take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The text — or the settings tab, while it is the tab shown where the text would be. The
    /// default, and what every key means unless somewhere else was chosen.
    #[default]
    Editor,
    /// Whatever the sidebar is showing that takes keys: the file tree.
    Sidebar,
    /// The results panel under the editor.
    Panel,
    /// The terminal under the editor. Takes every key but three — see [`crate::terminal::route`].
    Terminal,
}

/// What a press at `(x, y)` focuses, or `None` for chrome that should leave focus where it was.
///
/// Reads the same [`Layout`] drawing and the mouse read, so what looks like the sidebar is what
/// takes the keys. The title, activity and tab bars and the status line are `None` on purpose:
/// pressing a toolbar button is not a decision to type somewhere else, and stealing focus there
/// would make `ctrl-b` followed by a click on the bell land the next keystroke in the wrong place.
pub fn focus_for(layout: &Layout, x: f32, y: f32) -> Option<Focus> {
    if layout.sidebar.is_some_and(|rect| contains(rect, x, y)) {
        Some(Focus::Sidebar)
    } else if layout.results.is_some_and(|rect| contains(rect, x, y)) {
        Some(Focus::Panel)
    } else if layout.terminal.is_some_and(|rect| contains(rect, x, y)) {
        Some(Focus::Terminal)
    } else if contains(layout.editor, x, y) {
        Some(Focus::Editor)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dacode_render::Rect;

    #[test]
    fn a_click_decides_where_the_keys_go() {
        let layout = Layout::of(Rect::new(0.0, 0.0, 1400.0, 900.0), 1.0, Some(4), 1.0);
        let sidebar = layout.sidebar.expect("asked for one");
        assert_eq!(
            focus_for(&layout, sidebar.x + 5.0, sidebar.y + 50.0),
            Some(Focus::Sidebar)
        );
        assert_eq!(
            focus_for(&layout, layout.editor.x + 50.0, layout.editor.y + 50.0),
            Some(Focus::Editor)
        );
        let results = layout.results.expect("asked for one");
        assert_eq!(
            focus_for(&layout, results.x + 5.0, results.y + 5.0),
            Some(Focus::Panel)
        );
        assert_eq!(
            focus_for(&layout, layout.title.x + 5.0, layout.title.y + 5.0),
            None,
            "the title bar leaves focus where it was"
        );
    }

    #[test]
    fn clicking_the_terminal_gives_it_the_keys_and_clicking_the_code_takes_them_back() {
        // The way out of a focused terminal that needs no key at all, at the smallest window this
        // editor is drawn at as well as an ordinary one.
        for (width, height) in [(640.0, 480.0), (1400.0, 900.0)] {
            let layout = Layout::of(Rect::new(0.0, 0.0, width, height), 1.0, None, 0.0)
                .with_terminal(true, 1.0);
            let terminal = layout.terminal.expect("asked for one");
            assert_eq!(
                focus_for(&layout, terminal.x + 20.0, terminal.bottom() - 5.0),
                Some(Focus::Terminal)
            );
            assert_eq!(
                focus_for(&layout, layout.editor.x + 50.0, layout.editor.y + 20.0),
                Some(Focus::Editor)
            );
        }
    }
}
