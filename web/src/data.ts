/*
 * Every string here is drawn from the repository it describes — README.md,
 * CHANGELOG.md, CLAUDE.md, CHEATSHEET.md and the release assets on GitHub.
 * The design handoff's copy was written before any of it shipped, so where the
 * two disagree the repository wins. Notable replacements, so the next person
 * knows they were deliberate:
 *
 *   - `cargo install dacode` is not real; dacode is not published to crates.io. The
 *     hero now offers the actual artifact for the visitor's platform.
 *   - "no Windows or macOS build in v1" was true when it was written. Windows
 *     has shipped since 0.1.1 and has been run by real people since 0.3.0.
 *     macOS is the remaining one, and nobody has ever run dacode there.
 *   - "no settings GUI" was a non-goal; there has been a clickable settings
 *     panel since 0.1.0.
 *   - The mock's `"ayif` demonstrates a structural text object. Those are a
 *     *known gap* — the grammar rows below are keys the cheatsheet test presses
 *     through the real grammar on every build.
 *   - The M0–M8 roadmap is pre-history. What shipped is the changelog.
 */

export const REPO = "Gawad-B/Vex";
export const REPO_URL = `https://github.com/${REPO}`;

export type Mode = "normal" | "insert" | "visual" | "replace";

export const MODES: Mode[] = ["normal", "insert", "visual", "replace"];

export const MODE_HUE: Record<Mode, number> = {
  normal: 250,
  insert: 155,
  visual: 70,
  replace: 25,
};

export const BLURBS: Record<Mode, string> = {
  normal:
    "Normal: the cursor is a one-wide selection. 3dw is a count, an operator and a motion resolved into a single Action — the same value a keymap, the palette, a macro or a test can produce.",
  insert:
    "Insert: the cursor narrows to a bar and the chrome cools. Every text change goes through one path that returns its own inverse, so undo grouping is policy rather than guesswork.",
  visual:
    "Visual: selections are the data model, not a mode-specific overlay. Split one into many and every operator maps over the set — multi-cursor is never a special case.",
  replace:
    "Replace: the warmest hue in the palette, because it is the mode that destroys text. Shape and colour both change before you type, not after.",
};

export const HERO = {
  blurb:
    "One rule is enforced above every other: nothing unbounded ever runs on the UI thread. Parsing, language servers, project search and git run against cheap snapshots tagged with the document version they saw — an answer computed against an older version is discarded, never applied.",
  specs: ["Rust · edition 2024", "Linux & Windows", "wgpu + tree-sitter", "744 tests"],
};

export const PRINCIPLES = [
  {
    numeral: "7 µs",
    hue: 250,
    title: "A keystroke costs the same at 100,000 lines",
    body: "Keystroke to glyph is bounded work: a rope edit and a repaint of the visible rows. No full-file scan, no reparse, no disk touch. A test asserts the flatness rather than a millisecond count — a hundredfold larger document may cost at most three times more — so it fails the build on a loaded machine for the right reason and not the wrong one.",
  },
  {
    numeral: "5",
    hue: 155,
    title: "Five position types, zero bare integers",
    body: "ByteOffset, CharOffset, Utf16Column, Point and DisplayPoint are distinct types that never interchange, and Anchor survives edits on top of them. LSP speaks UTF-16, the rope speaks characters, the screen speaks display rows. Editors get their worst bugs where “position” is ambiguous. Here it never is.",
  },
  {
    numeral: "1",
    hue: 70,
    title: "One vocabulary: everything is an Action",
    body: "Keys resolve to Actions, the palette lists them, macros record them, config binds them and tests drive them headless. That is why a recorded macro survives rebinding, and why 3dw can be tested with no window open at all.",
  },
];

export const MODAL_KEYS = [
  { keys: "3dw", gloss: "count · operator · motion, resolved to one Action" },
  { keys: "d2f)", gloss: "delete to the second closing paren on this line" },
  { keys: 'ci"', gloss: "change what is inside the quotes, leaving them" },
  { keys: '"ayy', gloss: "yank this line into register a" },
  { keys: ".", gloss: "replay the last change — it was already an Action" },
];

export const FLAT_KEYS = [
  { keys: "ctrl-d", gloss: "select the word, then the next copy of it" },
  { keys: "alt-↑ ↓", gloss: "move these lines — Vim composes ddp; there is nothing to compose with" },
  { keys: "ctrl-/", gloss: "comment or uncomment" },
  { keys: "alt-drag", gloss: "select a rectangle: one cursor per line, same columns" },
  { keys: "F1", gloss: "every key that works right now, in the grammar you chose" },
];

export type ShotId = "editor" | "commands" | "settings" | "tree" | "light";

export const SHOTS: { id: ShotId; label: string; caption: string }[] = [
  {
    id: "editor",
    label: "editing",
    caption:
      "Tree-sitter highlighting on a worker thread, a git diff gutter, sticky scroll and inlay hints from the language server. Every capture on this page is rendered offscreen by the editor itself — dacode --screenshot out.png renders any state and exits, which is how the visual work gets verified in a repository whose author cannot take a screen capture at all.",
  },
  {
    id: "commands",
    label: "commands",
    caption:
      "Every command, searchable, each with its key beside it. The palette, the help panel on F1, the menu bar and the which-key popup all read one table in the source, so they cannot drift apart — and a test presses every non-modal key in it through the real grammar.",
  },
  {
    id: "settings",
    label: "settings",
    caption:
      "Settings you change by clicking, including which of the default, your init.lua or a VSCode import last set each one. There is a Lua config with hot reload underneath, but you never have to open it.",
  },
  {
    id: "tree",
    label: "file tree",
    caption:
      "A file tree, splits, project-wide search and replace, and a file watcher on the buffer you are looking at. Create, rename and delete without leaving the editor.",
  },
  {
    id: "light",
    label: "light theme",
    caption:
      "Five built-in themes, and importers for VSCode colour schemes in four formats. Colours are authored in Oklch, which is why the four mode accents share one lightness and one chroma and read as one instrument changing state.",
  },
];

export type ReleaseCard = {
  name: string;
  body: string;
  state: "shipped" | "current" | "next";
};

export const RELEASES: ReleaseCard[] = [
  {
    name: "0.1.0 · first release",
    body: "Vim's grammar, tree-sitter, LSP, splits, file tree, git gutter, Lua config, session restore — drawn on the GPU with wgpu.",
    state: "shipped",
  },
  {
    name: "0.1.1 · first Windows run",
    body: "One person on Windows found what CI could not: file:// URIs broke every LSP feature, and helper processes flashed console windows.",
    state: "shipped",
  },
  {
    name: "0.2.0 · the other editor",
    body: "Vim keys off gives a conventional editor, not a modal one with the modes hidden. Four ways to discover a key, all reading one table.",
    state: "shipped",
  },
  {
    name: "0.3.0 · the mouse",
    body: "A toolbar, a right-click menu, a clickable settings panel and a clickable first-run wizard — which had been asking, with the arrow keys, whether you wanted the mouse.",
    state: "shipped",
  },
  {
    name: "0.4.0 · the half-built half",
    body: "Find and replace, rectangular selection, horizontal scroll, closable tabs, crash recovery for every dirty buffer, symbols, formatting with no server at all.",
    state: "shipped",
  },
  {
    name: "0.4.1 · one afternoon",
    body: "One person installing 0.4.0 and using it for an afternoon found more than two rounds of adversarial review had. Language servers now install from inside the editor.",
    state: "shipped",
  },
  {
    name: "0.4.2 · what moved",
    body: "Three things the editor held outside the document — highlight spans, diagnostics, the file index — described text by where it used to be. Typing smeared the colours of the file below the cursor; a new file was invisible until the folder was reopened.",
    state: "shipped",
  },
  {
    name: "0.4.3 · the code face ←",
    body: "Fira Code, bundled, replacing IBM Plex Mono — the code font is the one thing you look at for hours, and an unfamiliar one is read as a mistake before it is read as a choice. Comments are set as code rather than italic serif.",
    state: "current",
  },
  {
    name: "next · search off-thread",
    body: "Buffer search is still linear in the document. The fix is the shape syntax and git already use: a worker with a version tag.",
    state: "next",
  },
  {
    name: "next · opening a file",
    body: "Still synchronous, though bounded now, and binary or non-UTF-8 files are refused with a sentence rather than mangled.",
    state: "next",
  },
  {
    name: "next · macOS",
    body: "No packaging, and nobody has ever run dacode there. Stated rather than hidden — the Linux and Windows builds are the ones in real use.",
    state: "next",
  },
];

export const IDENTITY_CAPTIONS = {
  mark: "Italic serif wordmark, lowercase always, with the cursor block set as a fourth letter.",
  palette:
    "One lightness, one chroma, four hues. Modes read as states of one instrument, not four themes.",
  cursors: "Shape says what the next keystroke will do. Colour only confirms it.",
  type: "Prose is typeset, not printed. Code and comments both stay mono — a comment is code, and a proportional face cannot hold the column. Labels stay small and spaced.",
};

export const SCOPE = {
  notTitle: "What dacode will not do",
  not: "No plugin system and no extension runtime — extension *code* is never run, because a JavaScript runtime is most of what makes VSCode heavy. Extension *data* imports fine: themes, snippets, keybindings, language configuration, .vsix files. No integrated terminal, no git blame, no remote editing, no collaboration, no macOS packaging. Deferring the extension API is what keeps the rest honest; it constrains every internal API the moment it exists.",
  successTitle: "Success has one criterion",
  success:
    "Not feature parity — daily use. dacode is written inside dacode, and its own bug tracker is the friction of using it. That is where 0.4.1 came from: an afternoon of real use, which found more than two rounds of adversarial review had.",
  stamp: "used daily by its author · windows since 0.3.0",
};
