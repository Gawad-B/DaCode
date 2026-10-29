# web — the DaCode landing page

Vite + React + TypeScript. No CSS framework, no UI library, no images beyond the editor
screenshots: every other graphic on the page is a rectangle, which is what the design handoff asked
for. That bundle is not in the repo — it is exported HTML nothing builds from; `PLAN.md` §6 records
what it said.

```sh
npm install
npm run dev        # http://localhost:5173
npm run build      # -> dist/, static, deployable anywhere
npm run preview    # serve dist/ locally
```

`base` is `"./"`, so the built site works from a domain root, a GitHub Pages project subpath and a
`file://` preview without being rebuilt for each. There is no deploy workflow — that was a
deliberate choice, not an omission.

## What drives the page

**True black and one teal.** The page is dark-only: `--bg` is `#000000` and the accent is teal
`#14b8a6`. Every other colour in `:root` in `styles.css` is a step from one of those two — near-black
surfaces, low-alpha or teal-grey hairlines, grey text, lighter/darker/thinner teals for hover,
pressed and tint. Use the tokens, never a literal.

**The editor's mode hues are the exception.** `--mode-h` on the root element is the only thing that
changes when the mode changes, and `oklch(var(--mode-l) var(--mode-c) var(--mode-h))` colours only
what shows a mode: the caret by the wordmark, the mode panel, the mini editor's cursor, the cursor
set and the swatches. One lightness, one chroma, four hues — left exactly as the editor draws them.

**The logo.** `src/assets/mark.png` is the ض mark built from the editor's coverage mask
(`crates/dacode-render/assets/mark.png`: red is the letter, drawn in the editor's light ink; green is
the block, in the brand amber), transparent so it sits on black. It is in the header, the hero, the
name section, the identity card and the footer. `public/favicon.png` and `apple-touch-icon.png` are
the finished app icons from `packaging/linux/icons/hicolor/`, on their own dark tile.

**Bullets, not paragraphs.** Every block of copy is a list of one-line facts, rendered by `Bullets` in
`App.tsx`; keep each under about twenty words.

**The mode cycles until it is touched.** `useMode` advances every 3.4s and stops for good on the
first click or keypress, because a control that keeps moving after you have taken hold of it has
been taken away. `i`, `v`, `R` and `Esc` work, as they do in the editor.

**Nothing animates the cursor.** Shape and position are instant; only the ambient tints cross-fade,
at 70ms. Shape is what communicates the mode — it survives a colourblind reader and a bad screen —
so it must not arrive late.

**The page is ordered for a first-time visitor.** Promise, action and the product on the first
screen; then proof (the three ratios, the benchmarks, the VSCode coverage); then the tour, what is
new, the keys; then history, the name and the scope, with the full download last. Every section
opens with the same head — a teal label, the heading, one sentence — and detail a reader may not
want (every number, the method, the full checklist, older releases) sits behind a `details`
disclosure rather than on the page. Content is held to about 1240px by `--pad-x`; backgrounds still
run edge to edge.

**The charts are drawn from `benchmark.json`, and only from it.** `components/Versus.tsx` is the
emphasis form: DaCode in `#11a697`, VSCode in `#6b6b6b`, a pair checked with a palette validator on
this surface, one axis per chart and a legend because there are two series. The coverage meter is
one teal stepping darker (`#14b8a6`, `#0f766e`, track `#164e4a`), so "missing" reads as empty track
rather than as a third colour. Every mark has a tooltip on hover *and* focus, and every value is
also printed beside its mark or in the table under "every number" — a tooltip never gates a value.
Marks are square, like everything else here.

## Two things worth not breaking

**The hidden state of a scroll reveal is applied by script, never by the stylesheet.** `useReveal`
adds `.animate` to the root before anything is hidden, and a 2s timer reveals whatever the
observer has not. An `IntersectionObserver` does not fire in a tab that is not compositing, and a
transition does not advance there either — so the fallback drops the transition as well as the
hidden state, or it would fade in from zero and land on exactly the blank page it is preventing.
That failure was real: the first version of this page measured 40 hidden sections and 0 revealed.

**A grid or flex item defaults to `min-width: auto`.** One unbreakable shell command in the install
box pushed the hero column 119px past a 375px viewport and gave the whole page a horizontal
scrollbar. The `min-width: 0` block near the top of the hero rules in `styles.css` is what holds
that shut; check `document.documentElement.scrollWidth` against `clientWidth` at 375, 768 and 1440
after any layout change.

## Where the copy comes from

`src/data.ts` — and every string in it is from this repository: `README.md`, `CHANGELOG.md`,
`CLAUDE.md`, `CHEATSHEET.md`, and the release assets on GitHub. The design handoff's copy was
written before any of it shipped, so where the two disagree the repository wins. The header comment
in that file lists each replacement and why, including the three that were outright false by the
time the page was built: `cargo install dacode` (DaCode is not on crates.io), "no Windows build in v1"
(Windows has shipped since 0.1.1), and a structural text object in the grammar table (still a known
gap).

The screenshots are imported from `../docs/images`, not copied — two copies of a screenshot drift
the moment one of them is regenerated. `vite.config.ts` allows the parent directory for that reason.
The same goes for the face and the logo: `styles.css` declares Fira Code with `@font-face` from the
editor's own `crates/dacode-render/fonts/FiraCode-Regular.ttf` (OFL, licence beside it, credited in
the footer). There are no font links.

**One face, one weight.** As in the editor, everything is Fira Code Regular. Headings rank by size,
case, tracking and colour; `font-synthesis: none` and the `h1`–`h3` reset keep the browser from
faking a bold or an italic that is not bundled. The wordmark is "DaCode" with "Da" in `--brand`,
the values of `Theme::brand()` in `dacode-theme`.

## The download button

`src/useRelease.ts`. It reads the platform, never the browser: `userAgentData.platform` where it
exists, the userAgent where it does not. A phone is deliberately "unknown" — there is no build for
it, and pretending otherwise hands someone a 9 MB file they cannot run. Whatever it decides, three
buttons underneath let the visitor say otherwise.

The version and the asset URLs are a table in that file, not a request to the GitHub API: the
reasons are in its comments, and `crates/dacode/tests/conventions.rs` fails the build if the table
names a version the crate is not. **When a release ships, update it there** — it is the only
hard-coded version on the page; the "what changed" section reads it too.

macOS gets no download button. There is no macOS build and nobody has ever run DaCode there; the
button says "Build from source" and the note says why. That is the honest button, and it should stay
that way until someone has actually run it.
