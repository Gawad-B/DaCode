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

**One hue.** `--mode-h` on the root element is the only thing that changes when the mode changes;
every accent on the page is `oklch(var(--mode-l) var(--mode-c) var(--mode-h))`. That is the identity
from the handoff — one lightness, one chroma, four hues, so the modes read as four states of one
instrument rather than four themes. `--mode-l` is the only token the light theme moves (0.74 → 0.52),
so accents stay legible on paper.

**The mode cycles until it is touched.** `useMode` advances every 3.4s and stops for good on the
first click or keypress, because a control that keeps moving after you have taken hold of it has
been taken away. `i`, `v`, `R` and `Esc` work, as they do in the editor.

**Nothing animates the cursor.** Shape and position are instant; only the ambient tints cross-fade,
at 70ms. Shape is what communicates the mode — it survives a colourblind reader and a bad screen —
so it must not arrive late.

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

## The download button

`src/useRelease.ts`. It reads the platform, never the browser: `userAgentData.platform` where it
exists, the userAgent where it does not. A phone is deliberately "unknown" — there is no build for
it, and pretending otherwise hands someone a 9 MB file they cannot run. Whatever it decides, three
buttons underneath let the visitor say otherwise.

The version and the asset URLs come from `api.github.com/repos/Gawad-B/Vex/releases/latest`, so the
page cannot go stale behind a release. It ships knowing 0.4.1 anyway, because that API is rate
limited per IP without a token and a download button that fails closed on the sixty-first visitor of
the hour is worse than one link out of date. **When the fallback moves, update it in that file** —
it is the only hard-coded version on the page.

macOS gets no download button. There is no macOS build and nobody has ever run DaCode there; the
button says "Build from source" and the note says why. That is the honest button, and it should stay
that way until someone has actually run it.
