# Handoff: vex — editor UI, landing page, visual identity

## Overview

`vex` is a modal code editor in Rust (VSCode's surface, Zed's performance, Neovim's
keybindings). This bundle covers three designs:

1. **Editor UI** (`vex Editor Paper.dc.html`) — the chosen direction. A typography-first
   editor chrome whose entire appearance is driven by the current modal state.
2. **Landing page** (`vex Landing.dc.html`) — launch page, and it doubles as the identity
   sheet (wordmark, mode palette, cursor set, type).
3. **Editor UI, alternate** (`vex Editor.dc.html`) — an earlier "Ink" direction: darker,
   more instrumented, with a left mode rail and a file tree. Kept for reference only;
   **do not implement this one** unless asked.

Two ideas hold all of it together:

- **Mode is the environment.** The mode is not a word in a corner. Gutter tint, cursor
  line, cursor *shape*, tab underline, structure highlight, statusline and accent all move
  together. Four hues at one lightness and one chroma, so it reads as one instrument
  changing state — never as four themes.
- **The hard parts of the architecture are visible UI.** The tree-sitter node path at the
  cursor, the same position in all four coordinate spaces, the async ledger showing which
  background results are fresh vs stale, and frame time against its budget.

## About the Design Files

The files in this bundle are **design references created in HTML** — prototypes of the
intended look and behavior, **not production code to copy**. They are streaming
"Design Component" files: a template plus a small logic class, rendered by `support.js`
(included only so the files open in a browser; it is not part of the design).

The real target is a **native Rust GUI** built on `winit` + `wgpu` + `cosmic-text`/`glyphon`,
per the project plan. Nothing here should be ported as HTML/CSS or wrapped in a webview.
Read these files as a specification of geometry, color, type and behavior, then implement
them in the editor's own render and UI layers (`crates/ui`, `crates/render`). If you are
instead prototyping in a web or React environment first, rebuild with that codebase's
existing patterns rather than lifting this markup.

To view a design: open the `.dc.html` file directly in a browser (keep `support.js` beside it).
Press `i`, `v`, `R`, `esc` to change mode; `⌘P`/`Ctrl-P` opens the file palette in the editor
file; the "try it" HUD in the bottom-left toggles light mode. **The HUD and the mode buttons
on the landing page are demo scaffolding — they are not part of the product.**

## Fidelity

**High fidelity.** Colors, type, spacing and states are final and exact. Recreate faithfully.
Two caveats:

- The fonts are web fonts standing in for the real thing (see *Typography*).
- The code shown in the editor is real content from `crates/text/src/anchor.rs`, but the
  syntax highlighting is hand-authored in the mock. In the product it comes from tree-sitter
  highlight queries; match the *colors*, not the specific spans.

---

## Design Tokens

### Theme — dark (default)

| Token | Value | Used for |
|---|---|---|
| `bg` | `oklch(0.165 0.008 70)` | editor canvas, page background |
| `bgAlt` | `oklch(0.185 0.009 70)` | title bar, tab strip, statusline (landing: `0.19`) |
| `bgPanel` | `oklch(0.2 0.009 70)` | structure sidebar, palette, HUD (landing: `0.215`) |
| `rule` | `oklch(0.3 0.01 70)` | 1px structural borders |
| `ruleSoft` | `oklch(0.25 0.009 70)` | 1px internal dividers, column guide |
| `ink` | `oklch(0.9 0.012 80)` | primary text (landing: `0.91`) |
| `inkDim` | `oklch(0.68 0.012 75)` | secondary text, body prose (landing: `0.7`) |
| `inkFaint` | `oklch(0.52 0.01 72)` | labels, gutter figures, tertiary (landing: `0.55`) |
| `scrim` | `oklch(0.12 0.006 70 / 0.62)` | palette overlay |
| `shadow` | `oklch(0.05 0 0 / 0.6)` | palette drop shadow, `0 30px 70px` |
| `rowActive` | `oklch(0.25 0.011 70)` | selected palette row |

Syntax, dark: keyword `oklch(0.74 0.12 300)` · type `oklch(0.83 0.09 60)` ·
function `oklch(0.82 0.09 215)` · attribute `oklch(0.66 0.09 300)` ·
comment `oklch(0.6 0.02 60)` (italic serif).

### Theme — light

| Token | Value |
|---|---|
| `bg` | `oklch(0.965 0.008 85)` |
| `bgAlt` | `oklch(0.975 0.008 85)` |
| `bgPanel` | `oklch(0.945 0.01 85)` (landing: `0.99 0.006 85`) |
| `rule` | `oklch(0.88 0.012 85)` |
| `ruleSoft` | `oklch(0.91 0.01 85)` |
| `ink` | `oklch(0.26 0.015 70)` (landing: `0.24`) |
| `inkDim` | `oklch(0.5 0.012 70)` (landing: `0.47`) |
| `inkFaint` | `oklch(0.68 0.012 70)` (landing: `0.64`) |
| `scrim` | `oklch(0.8 0.01 85 / 0.55)` |
| `shadow` | `oklch(0.5 0.01 70 / 0.22)` |
| `rowActive` | `oklch(0.955 0.01 85)` |

Syntax, light: keyword `oklch(0.42 0.13 300)` · type `oklch(0.44 0.11 45)` ·
function `oklch(0.45 0.1 230)` · attribute `oklch(0.5 0.1 300)` · comment `oklch(0.52 0.02 60)`.

**Dark is the default.** Set the window/root background before the first frame so there is
no light flash on startup.

### Mode accents — the core of the identity

One lightness, one chroma, four hues. Dark theme: `L 0.74, C 0.13`. Light theme: `L 0.52, C 0.13`.

| Mode | Hue | Dark | Light |
|---|---|---|---|
| normal | 250 | `oklch(0.74 0.13 250)` | `oklch(0.52 0.13 250)` |
| insert | 155 | `oklch(0.74 0.13 155)` | `oklch(0.52 0.13 155)` |
| visual | 70 | `oklch(0.74 0.13 70)` | `oklch(0.52 0.13 70)` |
| replace | 25 | `oklch(0.74 0.13 25)` | `oklch(0.52 0.13 25)` |

Derived, always from the *active* mode's hue:
- `gutterTint` — mode at alpha `0.06`, as a left-to-right gradient from transparent
- `lineTint` — mode at alpha `0.06`, or `0.14` in visual mode
- Status/tree/gutter-number accents — the mode color at full opacity

`okColor` (fresh) and `warnColor` (stale) in the async ledger reuse hues 155 and 70 at the
theme's mode lightness/chroma — so the ledger stays in the same palette.

### Typography

| Role | Font | Notes |
|---|---|---|
| Display, prose, comments, gutter figures | **Instrument Serif** (400, roman + italic) | The distinguishing choice. Comments are set *italic serif at 1.15em* of the code size; gutter line numbers are serif. |
| Code, UI values | **IBM Plex Mono** (300/400/500) | Everything monospaced. |
| Labels, small caps | **IBM Plex Sans** (400/500) | Always uppercase, `letter-spacing 0.14–0.22em`, `9–10px`. |

Stand-ins: any transitional/didone display serif with a true italic works in place of
Instrument Serif; any humanist mono with good Unicode coverage in place of Plex Mono.
Whatever is chosen, the *contrast* between typeset serif prose and mono code is the point.

Code metrics: `font-size 13.5px` (tweakable 11–18), `line-height 1.9`. Editor row height
must be identical in the gutter and the code area, or numbers stop registering with lines.

### Spacing

`4 / 6 / 8 / 10 / 12 / 14 / 16 / 18 / 20 / 22 / 24 / 26 / 34 / 40 / 44 / 56 / 62 / 92` px.
Editor horizontal frame padding is `34px`; landing page is `56px`.

### Borders, radius, shadow

**No border radius anywhere. Every corner is square.** All borders are exactly 1px `rule`,
except: the active tab's 2px bottom border in the mode color, the active code line's 2px
left border in the mode color, and the current milestone card's 2px inset outline. One
shadow only, on the command palette.

---

## Screen 1 — Editor (`vex Editor Paper.dc.html`)

**Purpose:** the editing surface. A single buffer with structure and instrumentation beside it.

### Layout — vertical stack, `height 100vh`, `min-height 640px`

1. **Title bar** — `padding 22px 34px 16px`, `bgAlt`, 1px `rule` bottom.
   Row, `align-items: baseline`, `gap 18px`:
   - `vex` — Instrument Serif italic, `34px`, `line-height 0.9`, `letter-spacing -0.015em`
   - breadcrumb `crates / text / src / anchor.rs` — Instrument Serif `17px`, `inkDim`,
     filename in `ink`
   - flex spacer
   - `NORMAL MODE` — Plex Sans `10px`, `letter-spacing 0.22em`, uppercase, `inkFaint`
   - a `34 × 3px` bar in the mode color
2. **Tab strip** — `padding 0 34px`, `bgAlt`, 1px `ruleSoft` bottom, `font-size 12px`.
   Active tab: `padding 9px 16px 8px 0`, `ink`, 2px bottom border in the mode color,
   `margin-bottom -1px` so it sits on the rule. Inactive: `inkFaint`.
   Right-aligned hint `⌘P FILES · i / v / R / ESC MODE`, Plex Sans `10px`, `0.14em`, `inkFaint`.
3. **Body** — row, `flex: 1`, `min-height: 0`:
   - **Gutter** — `width 78px`, `padding-right 20px`, right-aligned, flex column,
     Instrument Serif, `inkFaint`; background `linear-gradient(to right, transparent, gutterTint)`.
     Absolute line numbers 18–35; the cursor's line (22) in the mode color.
     **Relative numbering is intended in the product** — absolute at the cursor, relative
     above and below.
   - **Code area** — `flex: 1`, `padding 0 40px 0 6px`, `white-space: pre`, flex column.
     Cursor line: `lineTint` background, 2px left border in the mode color, bled to the
     full column width. A 1px `ruleSoft` vertical rule at `left 660px` is the 92-column guide.
     Inlay hints render as `inkFaint` *italic* (e.g. `: CharOffset` after `let mut offset`).
     A trailing serif-italic annotation `⟨ left | right ⟩` on the cursor line shows the
     variant hint for the focused field.
   - **Structure sidebar** — `width 292px`, `bgPanel`, 1px `rule` left. Sections divided by
     1px `rule`, each `padding 18px 24px`, `gap 9px`. Section labels are Plex Sans `9.5px`
     `0.2em` uppercase `inkFaint`; rows are `11.5px` mono, label `inkDim` left, value right.
     1. Header `Structure` — Instrument Serif `20px`, `padding 22px 24px 12px`
     2. **Node path** — the tree-sitter ancestor chain, indented 12px per level with `└`
        connectors in `inkFaint`; deepest node in the mode color; then
        `byte 0x2f1‥0x2fb · 10 chars` at `10.5px`
     3. **Coordinate spaces** — `Point 22 : 13`, `DisplayPoint 24 : 13`,
        `ByteOffset 0x2f1`, `UTF-16 (lsp) 753`
     4. **Async ledger** — `buffer v1284`, `syntax tree v1284 fresh` (green),
        `diagnostics v1281 stale` (amber)
     5. spacer, then a footer pull-quote: Instrument Serif `15px`, `inkDim`,
        "Nothing on the UI thread may ever be unbounded in cost." + label `3.1 MS OF 16.6`
4. **Statusline** — `height 34px`, `padding 0 34px`, `bgAlt`, 1px `rule` top, `11px`, `inkDim`.
   Left to right: mode name (Plex Sans `10px`, `0.2em`, uppercase, mode color) · 1px × 12px
   `rule` divider with `margin 0 16px` · selection count · divider · pending grammar keys ·
   spacer · `22:13`, `rust · utf-8 · lf`, `rust-analyzer`, frame time `3.1 ms` in `ink`,
   `gap 18px`.

### Command palette — overlay, hidden by default

Full-bleed `scrim`. Centered card `width 600px`, `padding-top 110px` from the top of the
window, `bgPanel`, 1px `rule`, shadow `0 30px 70px shadow`.
- Header `padding 18px 24px`, 1px `ruleSoft` bottom: the query in Instrument Serif italic
  `22px` with a 2px × 20px blinking caret in the mode color; right, `4 OF 1284` in
  Plex Sans `9.5px` `0.16em` `inkFaint`.
- Rows `padding 9px 24px`, `12.5px`, `gap 16px`: a right-aligned match score (`width 26px`),
  the filename with matched substrings in the mode color, then the directory in `inkFaint`.
  Selected row: `rowActive` background, score and filename brighten.
- Footer `padding 11px 24px`, 1px `ruleSoft` top: `↵ OPEN · ⌃V SPLIT · ESC DISMISS`.

The matcher is `nucleo`; highlight exactly the matched characters it reports.

### Cursor shapes — normative

| Mode | Shape |
|---|---|
| normal | `0.6em` wide × `1.1em`, mode at alpha `0.3`, 2px solid bottom border in the mode color. **Not** blinking. |
| insert | `2px` wide × `1.05em`, solid mode color, blinking: `0–48%` opacity 1, `58–100%` opacity `0.12`, `1.1s`, `steps(1)` |
| visual | the selection range fills in mode-hue at alpha `0.14`; the primary cursor keeps the normal block |
| replace | the normal block, filled solid (alpha `0.5`) |

Shape must communicate the mode before color does — this survives colorblindness and
low-light screens, and it is the identity.

## Screen 2 — Landing page (`vex Landing.dc.html`)

`min-height 100vh`, dark default, horizontal padding `56px`, every section separated by 1px
`rule`. Sections top to bottom:

1. **Nav** — `padding 26px 56px`. `vex` in Instrument Serif italic `27px`, an `8 × 19px`
   mode-color block beside it, spacer, then `DESIGN / ROADMAP / IDENTITY / SCOPE` in
   Plex Sans `10px` `0.2em` uppercase `gap 28px`, and a bordered theme toggle
   (`padding 6px 10px`).
2. **Hero** — 2 columns `1.15fr 1fr`.
   Left, `padding 92px 56px 80px`, `gap 34px`: headline Instrument Serif
   `clamp(50px, 6.2vw, 92px)`, `line-height 0.96`, `letter-spacing -0.02em` — *"A modal editor
   that never* stalls *on your keystroke."* with `stalls` in italic; a 46ch Plex Sans `17px`
   `line-height 1.62` `inkDim` paragraph; an install box (`bgPanel`, 1px `rule`,
   `padding 15px 20px`, `width: fit-content`) reading `$ cargo install vex` with a blinking
   `8 × 15px` block; then three Plex Sans `10px` `0.18em` uppercase specs, `gap 40px`.
   Right, `bgAlt`, `padding 56px 56px 48px`, `gap 22px`: the label `MODE IS THE ENVIRONMENT`
   with the live mode name in Instrument Serif italic `20px`; four equal mode buttons
   (`padding 11px 0`, 1px `rule`, `11px`, hover borders and text in that mode's own hue); a
   miniature editor (`bgPanel`, 1px `rule`, `padding 20px 0`, `font-size 13px`,
   `line-height 1.95`, serif gutter `width 58px`) showing the same `Anchor` struct with the
   live cursor; then a per-mode paragraph in Plex Sans `13.5px` `inkDim`.
3. **Three principles** — 3 equal columns, `padding 54px 44px`, `gap 16px`, dividers 1px
   `ruleSoft`. Each: a big Instrument Serif `44px` numeral in a mode hue (250 / 155 / 70), an
   Instrument Serif `25px` heading, a Plex Sans `14px` `line-height 1.62` `inkDim` paragraph.
   Content: **16.6** the frame budget is the spec · **5** five position types, zero bare
   integers · **1** one vocabulary: everything is an Action.
4. **Grammar** — 2 columns `1fr 1.15fr`. Left (`bgAlt`, `padding 58px 56px`): Instrument
   Serif `40px` "Vim's grammar. / A selection set underneath." + 42ch `15px` paragraph.
   Right: label `PENDING GRAMMAR`, then four rows, each a `92px 1fr` grid with `gap 20px`,
   `padding 13px 0`, 1px `ruleSoft` between — the key sequence in mode color, the gloss in
   `inkDim`: `3dw`, `"ayif`, `.`, `⌥s`.
5. **Roadmap** — `padding 62px 56px 68px`. Heading Instrument Serif `40px` + a `13px`
   `inkFaint` note. Then a 3-column grid with `gap 1px` on a `rule` background (the gap *is*
   the grid line). Nine cards, `bgAlt`, `padding 24px 26px`, `gap 10px`: milestone name in
   Instrument Serif `22px`, bar in Plex Sans `13px` `inkDim`. M0–M2 (done) use hue 155;
   M4–M8 (future) use `inkFaint`; **M3, the current milestone, uses `bgPanel`, a 2px inset
   outline in the mode color, its title in the mode color and its body in full `ink`.**
6. **Identity** — `bgAlt`, `padding 62px 56px 68px`. Heading Instrument Serif `40px`, then 4
   columns `gap 44px`. Each: an uppercase label, a `118px`-tall specimen box (`bgPanel`,
   1px `rule`), and a Plex Sans `12.5px` caption.
   - **Mark** — `vex` in Instrument Serif italic `58px` + a `9 × 40px` mode-color block,
     `gap 4px`. Lowercase always; the cursor block is treated as a fourth letter.
   - **Mode palette** — four full-bleed swatches, each labelled with its hue number in `bg`
     at `9px`.
   - **Cursor set** — the four shapes at rest beside their names.
   - **Type** — the three faces stacked as specimens.
7. **Scope** — 2 equal columns, `padding 58px 56px 66px`. Left: "What vex will not do" +
   the v1 non-goals. Right (`bgAlt`): "Success has one criterion" + the daily-use framing,
   closing with a `26 × 2px` mode-color rule and `SELF-HOSTING SINCE M3`.

---

## Interactions & Behavior

### Mode changes — the whole point

One state variable, `mode ∈ {normal, insert, visual, replace}`, drives every accent on
screen. Everything mode-derived recomputes from that one hue: rail/tab/statusline accents,
gutter number, gutter gradient, cursor line tint and border, active node in the structure
tree, palette caret and match highlights, cursor shape.

Transitions in the mock are instant. In the product, the *cursor* must be instant (it is
feedback for a keystroke); the ambient tints may cross-fade, but keep it under **80ms** and
never animate the cursor's position or shape.

Keys in the mock: `i` insert, `v` visual, `R` replace, `Esc` normal, `⌘P`/`Ctrl-P` toggles
the palette (and `Esc` closes it). While the palette is open, mode keys are swallowed. In
the product these come from the real modal state machine — the mock's listener is a stand-in.

### Statusline copy per mode

| Mode | pending | selection |
|---|---|---|
| normal | `3d` | `1 selection` |
| insert | `—` | `1 selection` |
| visual | `2 sel · d` | `2 selections · 1 primary` |
| replace | `—` | `1 selection` |

Pending shows the partially-typed grammar (count, register, operator) as it accumulates and
clears when it resolves to an Action.

### Instrumentation — must be live, not decorative

- **Frame time** in the statusline and against the `16.6 ms` budget in the sidebar footer.
- **Async ledger**: every background producer reports the document version its result was
  computed against. Equal to the buffer version → `fresh` in hue 155. Behind → `stale` in
  hue 70, and the result is *discarded, never applied*. This panel is the §1.1 contract made
  visible, and it is how you debug it.
- **Coordinate panel**: all four spaces for the primary cursor, recomputed per cursor move.
- **Node path**: the tree-sitter ancestor chain of the smallest node containing the cursor.

### Hover states

Buttons and mode chips: border and text take the relevant mode hue (or `ink` for neutral).
No fills, no lifts, no transforms. Nav links go from `inkDim` to a lighter accent.

### Theme

`theme ∈ {dark, light}`, default **dark**, exposed as a setting. Every token above swaps;
mode lightness moves `0.74 → 0.52` so accents stay legible on paper. Set the root/window
background before the first paint.

## State Management

| State | Type | Source in the product |
|---|---|---|
| `mode` | enum | the modal state machine (`crates/input`) |
| `pending` | string | the partially-resolved grammar buffer |
| `selections` | set of ranges + primary index | `crates/workspace` — a cursor is a 1-wide selection |
| `theme` | enum | config (`~/.config/vex/init.lua`), hot-reloaded |
| `paletteOpen` + query + results | bool, string, ranked list | palette controller over `nucleo` |
| `bufferVersion` | u64 | `crates/buffer` |
| `syntaxVersion`, `diagnosticsVersion` | u64 | background services; compared to `bufferVersion` for fresh/stale |
| `cursorPoint` | Point / DisplayPoint / ByteOffset / UTF-16 | `crates/text` conversions |
| `nodePath` | Vec of node kinds | tree-sitter tree at the cursor |
| `frameTimeMs` | f32 | render loop, rolling |
| `fontSize` | f32, 11–18 | config |

Layout flags in the mock (`showStructure`, `showInlayHints`) correspond to real settings:
toggle the sidebar, toggle inlay hints.

## Assets

**None.** No images, no icon fonts, no SVG. Every graphic element is a rectangle: cursors,
swatches, the mode bar, dividers, the frame-time bars in the alternate design. The `└` tree
connectors and `·⟨⟩‥←▲` are text characters. This is deliberate — keep it that way, and let
the type do the work.

Fonts: Instrument Serif, IBM Plex Mono, IBM Plex Sans — all open-licensed (SIL OFL / Google
Fonts). Bundle them with the binary rather than fetching at runtime.

## Files

| File | What it is |
|---|---|
| `vex Editor Paper.dc.html` | **The editor design to implement.** Dark default, light toggle. |
| `vex Landing.dc.html` | **The landing page + identity sheet to implement.** |
| `vex Editor.dc.html` | Alternate "Ink" editor direction — reference only, not chosen. |
| `support.js` | Runtime that renders the `.dc.html` files in a browser. Not part of the design. |

Not included: screenshots. Ask if you want them added.
