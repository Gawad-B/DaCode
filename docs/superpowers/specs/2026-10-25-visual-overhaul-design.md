# Visual overhaul — design

Items 1–4 of the plan agreed on 2026-10-25: the title bar, a motion system, restyled overlays, and a
settings tab. The features that follow it — terminal, git panel, run & tasks, debugger — get specs of
their own.

## Decisions taken

| Question | Answer |
|---|---|
| Order | Visual overhaul first (this spec), then terminal → git → run & tasks → debugger |
| Motion feel | **Expressive**: springs with a little overshoot, ~300 ms for panels and popups |
| Settings placement | **An editor tab**, VSCode's placement, with JetBrains' layout inside |
| Settings saving | **Automatic**: no save button, no ctrl-s |
| Sub-headers | **None**: a setting row is its name and its control; no `INIT.LUA` source captions |
| Animation architecture | **One motion engine** (approach A below) |

## 1. Title bar

Left to right: the mark, the wordmark, **a menu bar that is always visible** — File, Edit, Selection,
View, Go, Run, Terminal, Help — then the command-centre search box, centred, then the bell.

- The account icon and the Share button are removed. Share becomes **File → Copy link to line**
  (`share`, unchanged as a command).
- Menus open on click; once one is open, moving across the bar opens its neighbours, as in VSCode.
- `F10` / `alt` still open the bar from the keyboard. The menus read `commands::COMMANDS`, so they
  cannot list a key that does nothing.
- Menus whose items do not exist yet (Run, Terminal) are present with what does exist — Run's
  "copy run command" today — and fill in as items 5–7 land.
- Geometry: `layout::title_bar` gains `menus: Vec<(usize, Rect)>`; `menu::geometry` reads it rather
  than computing titles itself.

## 2. Motion

**Approach A — one motion engine.** Rejected: timestamps scattered through state (cannot carry spring
velocity, spreads timing across every widget, interruptions get messy); a retained scene graph with
transitions (rewrites the renderer's model for this one feature).

`crates/dacode/src/motion.rs`:

- `Motion` holds animated values keyed by a small `Key` enum (`Sidebar`, `Palette`, `Menu`,
  `Hover(Region)`, `TabIndicator`, `Scroll(pane)`, `Bell`, …).
- `motion.to(key, target)` sets where a value is going; `motion.get(key, now) -> f32` reads it.
  A value is a spring — position and velocity — so retargeting mid-flight keeps momentum instead of
  jumping.
- Presets: `Spring::PANEL` (~300 ms, damping ratio ~0.75 — the overshoot the "expressive" answer
  asked for), `Spring::POPUP`, `Spring::HOVER` (short, no overshoot), `Spring::SCROLL` (no
  overshoot — text must not bounce past where you scrolled).
- `motion.busy(now)` — whether anything is still moving. `about_to_wait` asks it: busy means
  `request_redraw` for the next vsync; settled means `ControlFlow::Wait` as today. Nothing runs when
  nothing moves.
- Time is a parameter (`Instant`), never read inside the chrome, so tests and `--screenshot` can
  render any instant (`--at-ms N` joins the capture flags).
- Setting `animations = "full" | "reduced" | "off"`, in all three config places. Reduced: fades only,
  no movement or overshoot. Off: every value jumps to its target.

**Never animated:** typing, the cursor, selections, and scrolling that follows the cursor while
typing. These stay instant — an editor that animates keystrokes feels slow.

**Animated:**

| What | How |
|---|---|
| Popups, menus, palette, completion, hover | fade in, scale 0.96 → 1 from their anchor; reverse on close |
| Sidebar open/close | slides from the activity bar |
| Activity icons | lift on hover; the active bar slides between icons |
| Tabs | the active background glides to the new tab; a new tab grows in |
| Scrolling | smooth: wheel, touchpad, page keys and jumps animate to their target |
| Bell | a short wobble when a message arrives |
| Welcome wizard | steps cross-fade and slide |
| Settings tab | categories cross-fade; a changed value briefly glows |

## 2a. Scrolling and the touchpad

The bug: `MouseScrollDelta::PixelDelta` is divided by 40 and truncated per event
(`main.rs:1656`), so a touchpad's stream of small deltas becomes a stream of zeros and nothing
scrolls. Fix:

- Scrolling keeps a **fractional pixel offset** per pane (`Scroll(pane)` in `Motion`), not whole
  lines. `PixelDelta` adds its pixels directly — the touchpad is followed one-to-one; `LineDelta`
  adds `lines × row height` as a target the spring glides to.
- `ScrollAnchor` stays the source of truth for which line is at the top; the fractional remainder is
  drawn as a sub-row offset of the code area, clipped to it. Horizontal touchpad scrolling uses the
  same path for the column offset when wrap is off.
- Pinch-to-zoom changes the font size where winit reports `PinchGesture` (macOS today). Wayland does
  not deliver it through winit 0.30; ctrl-scroll remains the way there, and the docs say so.
- `latency.rs` gains a case: a burst of pixel deltas costs the same in a 100k-line file as in a 1k one.

## 3. Overlays

One **card** style, shared by every overlay through a `card()` helper in `chrome.rs`: rounded corners
(8 px), a 1 px border in `rule`, a soft shadow, the panel surface, sans text, keys drawn as small
rounded chips in `commands::pretty` notation (Vim notation only with Vim keys on).

Restyled: command palette (drops from the command centre, like VSCode and Zed), menus and the context
menu, completion, hover/info, which-key, the help panel (F1), project search results, the problems
list, the welcome wizard, and the empty-editor screen.

- **No sub-headers or captions that do not carry information.** A heading stays only where it names
  something the reader is choosing between.
- Palette and help-panel rows become **clickable** — closing a gap `CLAUDE.md` lists.
- Every card's rectangles come from a geometry function the mouse also reads (the existing rule).

## 4. Settings tab

Opened by the gear, `ctrl-,`, or the palette, as a tab beside the files — a tab the tab bar knows is
not a file.

- **Left: a category tree** — Appearance, Editor, Files & saving, Keys, Languages, Import from VSCode.
- **Right: the settings of the chosen category**, each a row of name, one-line description and a
  control: switch, stepper, or dropdown. **No source captions.** A ● marks a value that differs from
  the default, with a ⟲ to reset it.
- **Search** at the top filters across every category live, highlighting matches; categories with no
  match collapse.
- **Saving is automatic**: a change applies instantly and is written to `init.lua` (the panel's
  marked block, as today) a moment after the last change. A failed write says so on the status line
  and in the bell.
- **Keys page**: every command and its shortcut in a searchable list, from `commands::COMMANDS`.
  Rebinding from the page is out of scope for this spec; the page links to `init.lua` for that.
- Appearance includes theme (with live swatches), font size, transparency and `animations`.
- Geometry: `settings_ui::page(rect, …)` returns tree rows, setting rows and their control rects,
  read by drawing and by the mouse; the page scrolls through the same smooth `Scroll` key.

The old settings sidebar panel is removed; the activity bar's gear opens the tab.

## Testing

- Geometry: the size-sweeping tests extend to the menu bar, cards and settings page — nothing outside
  its box, nothing overlapping, at widths from 360 to 2560 and scales 1–2.
- Motion: springs settle; retargeting keeps velocity; `reduced` never moves, `off` jumps; `busy`
  becomes false after settling, so the editor goes back to sleep.
- Latency: animating costs nothing per keystroke; pixel-scroll bursts are flat in document size.
- Settings: auto-save writes once after a burst of changes, through `write_back`, and round-trips
  through the Lua reader.
- Visual: renders of every surface at three sizes and at mid-animation instants (`--at-ms`).

## Out of scope here

Terminal, git panel, run & tasks, debugger (their own specs); rebinding keys from the Keys page;
pinch gestures on Wayland until winit supports them.
