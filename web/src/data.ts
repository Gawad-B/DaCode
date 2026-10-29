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
 *
 * Brought up to date with the changelog's Unreleased section (the interface
 * overhaul), which is on main but not yet in a release:
 *
 *   - vex is DaCode now, and its wordmark is "DaCode" with "Da" in the logo's
 *     amber. The command stays `dacode`, lowercase, as a command does.
 *   - One face. The editor dropped its serif headings and sans labels for Fira
 *     Code everywhere, and so did this page.
 *   - The settings captions that said which source set a value are gone, so the
 *     settings shot no longer claims them.
 *   - Twelve themes, under the names the editor now gives them: Slate is the
 *     default, and the old slugs (dacode, amoled, quizzy-dark, …) still work
 *     as aliases in init.lua. "AMOLED" and "AMOLED black" were one theme twice.
 *
 * The page is bullets rather than paragraphs: one fact per line, none longer
 * than about twenty words. Keep it that way when adding a claim.
 */

export const REPO = "Gawad-B/DaCode";
export const REPO_URL = `https://github.com/${REPO}`;

export type Mode = "normal" | "insert" | "visual" | "replace";

export const MODES: Mode[] = ["normal", "insert", "visual", "replace"];

export const MODE_HUE: Record<Mode, number> = {
  normal: 250,
  insert: 155,
  visual: 70,
  replace: 25,
};


/** One line under the mini editor per mode. */
export const BLURBS: Record<Mode, string> = {
  normal: "Normal: 3dw is count, operator and motion — resolved to one Action.",
  insert: "Insert: the cursor narrows to a bar; every edit returns its own inverse.",
  visual: "Visual: selections are the data model, so multi-cursor is never a special case.",
  replace: "Replace: the warmest hue, because it is the mode that destroys text.",
};

export const HERO = {
  // One sentence under the headline. The four bullets below are the long form, and live
  // further down the page, where there is room to read them.
  lede: "Written in Rust and drawn on the GPU. Parsing, language servers, search and git all run off the UI thread, so typing never waits for them.",
  bullets: [
    "Nothing unbounded ever runs on the UI thread.",
    "Parsing, LSP, search and git run on snapshots, off-thread.",
    "An answer computed against an older version is discarded, never applied.",
    "Vim's grammar if you want it; a conventional editor if you don't.",
  ],
  specs: ["Rust · edition 2024", "Linux & Windows", "wgpu + tree-sitter", "1,010 tests"],
};

export const PRINCIPLES = [
  {
    numeral: "7 µs",
    title: "A keystroke costs the same at 100,000 lines",
    bullets: [
      "A rope edit and a repaint of the visible rows — nothing more.",
      "No full-file scan, no reparse, no disk touch.",
      "A test fails the build if 100x the document costs over 3x the time.",
    ],
  },
  {
    numeral: "5",
    title: "Five position types, zero bare integers",
    bullets: [
      "ByteOffset, CharOffset, Utf16Column, Point, DisplayPoint — never interchangeable.",
      "LSP speaks UTF-16, the rope chars, the screen display rows.",
      "Anchor survives edits on top of them.",
    ],
  },
  {
    numeral: "1",
    title: "Everything is an Action",
    bullets: [
      "Keys, the palette, macros, config and tests all speak Actions.",
      "A recorded macro survives rebinding.",
      "3dw is tested with no window open.",
    ],
  },
];

export const GRAMMAR = [
  "A cursor is a one-wide selection; every operator maps over the set.",
  "Vim keys off gives a real conventional editor, not Vim with modes hidden.",
  "One table drives the palette, F1 help, menus and this list.",
  "A test presses every key in it through the real grammar.",
];

export const MODAL_KEYS = [
  { keys: "3dw", gloss: "count · operator · motion, one Action" },
  { keys: "d2f)", gloss: "delete to the second ) on this line" },
  { keys: 'ci"', gloss: "change inside the quotes" },
  { keys: '"ayy', gloss: "yank this line into register a" },
  { keys: ".", gloss: "replay the last change" },
];

export const FLAT_KEYS = [
  { keys: "Ctrl+D", gloss: "select the word, then its next copy" },
  { keys: "Alt+↑ ↓", gloss: "move these lines" },
  { keys: "Ctrl+/", gloss: "comment or uncomment" },
  { keys: "Alt+drag", gloss: "rectangular selection, one cursor per line" },
  { keys: "F1", gloss: "every key that works right now" },
];

export type ShotId = "editor" | "commands" | "settings" | "tree" | "light";

export const SHOTS: { id: ShotId; label: string; bullets: string[] }[] = [
  {
    id: "editor",
    label: "editing",
    bullets: [
      "A Rust file in the default theme, Slate.",
      "Menu bar in the title bar; errors, warnings and mode in the status bar.",
      "Rendered by the editor itself: dacode --screenshot out.png.",
    ],
  },
  {
    id: "commands",
    label: "commands",
    bullets: [
      "The palette drops from the search box; every row is clickable.",
      "Palette, F1 help, menus and which-key read one table.",
      "A test presses every key in it.",
    ],
  },
  {
    id: "settings",
    label: "settings",
    bullets: [
      "A tab: category tree on the left, explained rows on the right.",
      "One search across every page. ● marks a change, ⟲ undoes it.",
      "No save button — written to init.lua 600 ms later.",
    ],
  },
  {
    id: "tree",
    label: "file tree",
    bullets: [
      "Create, rename and delete without leaving the editor.",
      "Click the tree and the arrows walk files; open one and keys return.",
      "Splits, project-wide search and replace, a file watcher.",
    ],
  },
  {
    id: "light",
    label: "light theme",
    bullets: [
      "A light theme: the logo's amber deepens so it does not read as yellow.",
      "Twelve built-in themes, plus importers for VSCode colour schemes.",
      "Colours authored in Oklch: four mode hues, one lightness, one chroma.",
    ],
  },
];

/*
 * What the current release added, from the changelog's 1.1.1 section. Each entry
 * is a claim about the download above.
 */
export const OVERHAUL: { title: string; bullets: string[] }[] = [
  {
    title: "Servers that install themselves",
    bullets: [
      "pyright, rust-analyzer, clangd, ruff, prettier and more, downloaded for you.",
      "No npm, pip or rustup needed first; every download is checksummed.",
    ],
  },
  {
    title: "Images and PDFs in a tab",
    bullets: [
      "Zoom and scroll pictures and PDF pages beside your code.",
      "Word, Excel and PowerPoint files show their text, read only.",
    ],
  },
  {
    title: "Zoom everything",
    bullets: [
      "Ctrl+= and Ctrl+- scale menus, panels and code together.",
      "A new Accessibility page in Settings keeps it.",
    ],
  },
  {
    title: "Rainbow CSV",
    bullets: [
      "A colour for each column, quoted commas and all.",
      "The status line names the column under the caret.",
    ],
  },
  {
    title: "Run the file you have open",
    bullets: [
      "▶ runs a lone Python, Node, Go or C file in the terminal.",
      "A file in a project still runs the project.",
    ],
  },
  {
    title: "Fixed",
    bullets: [
      "Closing a terminal tab no longer leaves its shell running.",
      "The terminal steps aside while Settings is open.",
    ],
  },
];

export type ReleaseCard = {
  name: string;
  body: string;
  state: "shipped" | "current" | "unreleased" | "next";
};

export const RELEASES: ReleaseCard[] = [
  {
    name: "0.1.0 · first release",
    body: "Vim grammar, tree-sitter, LSP, splits, file tree, git gutter, Lua config, on the GPU.",
    state: "shipped",
  },
  {
    name: "0.1.1 · first Windows run",
    body: "Fixed file:// URIs that broke LSP, and console windows flashing.",
    state: "shipped",
  },
  {
    name: "0.2.0 · the other editor",
    body: "Vim keys off: a real conventional editor. Four ways to discover a key.",
    state: "shipped",
  },
  {
    name: "0.3.0 · the mouse",
    body: "Toolbar, right-click menu, clickable settings and a clickable first-run wizard.",
    state: "shipped",
  },
  {
    name: "0.4.0 · the half-built half",
    body: "Find and replace, rectangular selection, closable tabs, crash recovery, formatting.",
    state: "shipped",
  },
  {
    name: "0.4.1 · one afternoon",
    body: "An afternoon of real use beat two rounds of review. Servers install in-editor.",
    state: "shipped",
  },
  {
    name: "0.4.2 · what moved",
    body: "Highlights, diagnostics and the file index now follow the text as you type.",
    state: "shipped",
  },
  {
    name: "0.4.3 · the code face",
    body: "Fira Code bundled, replacing IBM Plex Mono. Comments set as code.",
    state: "shipped",
  },
  {
    name: "1.0.0 · the overhaul",
    body: "vex becomes DaCode: menu bar, settings tab, springs, one face, twelve themes.",
    state: "shipped",
  },
  {
    name: "1.1.0 · the missing pieces",
    body: "Source control, a terminal, blame, text objects, key rebinding, self-update.",
    state: "shipped",
  },
  {
    name: "1.1.1 · nothing to install first ←",
    body: "Servers and formatters download themselves; images and PDFs in a tab; zoom.",
    state: "current",
  },
  {
    name: "next · search off-thread",
    body: "Buffer search is still linear; it moves to a versioned worker.",
    state: "next",
  },
  {
    name: "next · opening a file",
    body: "Still synchronous, but bounded; binary and non-UTF-8 files are refused.",
    state: "next",
  },
  {
    name: "next · macOS",
    body: "No packaging, and nobody has run it there yet.",
    state: "next",
  },
];

/*
 * The name and the mark. The cursor reading of the amber block is an
 * interpretation, not something the repository states — confirm or correct it.
 */
/**
 * Said up front, before the download: what kind of project this is and who made which part.
 * The hero carries `short`; the full note is in the About section, one click away.
 */
export const PROJECT_NOTE = {
  title: "A vibe-coded project",
  short: "Vibe-coded: AI wrote the Rust; I chose the architecture and designed the editor.",
  bullets: [
    "I don't know Rust: the code was written with AI.",
    "I chose the architecture and the design system.",
    "I designed the editor and this landing page.",
    "Built for my own use, published so anyone can try it.",
  ],
};

export const NAME_BULLETS: { text: string; arabic?: string; arabicLabel?: string }[] = [
  {
    text: "The mark is ḍād with a fatha, read “Ḍa” — the first syllable of DaCode.",
    arabic: "ضَ",
    arabicLabel: "ḍād with fatha",
  },
  {
    text: "Arabic is called “the language of the ḍād”: the letter is said to belong to no other language.",
    arabic: "لغة الضاد",
    arabicLabel: "lughat aḍ-ḍād",
  },
  { text: "DaCode = Ḍa + Code." },
  { text: "The amber block beside the letter is a block cursor — the editor's caret." },
  { text: "“Da” is amber in the wordmark, as in the editor's title bar." },
];

export const IDENTITY_CAPTIONS = {
  mark: "“Da” in the logo's amber; the caret after it takes the mode's hue.",
  palette: "One lightness, one chroma, four hues — states of one instrument.",
  cursors: "Shape says what the next key will do. Colour only confirms it.",
  type: "Fira Code Regular for everything. Rank by size and case, never weight.",
};

export const SCOPE = {
  notTitle: "What DaCode will not do",
  not: [
    "No plugin system: extension code is never run.",
    "Extension data imports fine — themes, snippets, keybindings, .vsix files.",
    "No remote editing or collaboration.",
    "No macOS packaging yet.",
  ],
  successTitle: "Success has one criterion",
  success: [
    "Daily use, not feature parity.",
    "DaCode is written inside DaCode; friction is the bug tracker.",
    "0.4.1 came from one afternoon of real use.",
  ],
  stamp: "used daily by its author · windows since 0.3.0",
};
