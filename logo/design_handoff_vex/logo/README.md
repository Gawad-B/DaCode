# vex — logo assets & integration

The mark is a lowercase `v` drawn as a monospace glyph, followed by the same glyph one step
down: an operation and its repeat. It is monochrome by rule.

## Files

```
logo/
├── vex-mark.svg           primary — currentColor, use in the app/web
├── vex-mark-solo.svg      small-size variant — currentColor
├── src/                   explicit-colour SVGs (rasterizer inputs, print, Figma)
│   ├── vex-mark-ink.svg          #3d3a35 — for light surfaces
│   ├── vex-mark-paper.svg        #e8e4dc — for dark surfaces
│   ├── vex-mark-solo-ink.svg
│   └── vex-mark-solo-paper.svg
└── png/
    ├── ink/    vex-{16,24,32,48,64,128,256,512,1024}.png   dark mark, transparent
    └── paper/  vex-{16,24,32,48,64,128,256,512,1024}.png   light mark, transparent
```

**Prefer the two `currentColor` SVGs anywhere the renderer supports SVG** — the mark then
inherits the ink colour of its surface and needs no theme switching. The PNGs exist for
desktop-entry files, window icons and packaging formats that will not take SVG.

`16` and `24` are the **solo** variant (single chevron, stroke 11 → 13, re-centred); `32`
and above are the full mark. That substitution is already baked into the PNG set — do not
downscale the full mark to 16px, the 34%-opacity echo turns to mud.

## Geometry (100 × 100 em box)

| | |
|---|---|
| primary | `M20 14 L50 52 L80 14` + `M20 40 L50 78 L80 40` at `opacity 0.34` |
| solo | `M20 26 L50 66 L80 26` |
| stroke | `11` primary, `13` solo — butt caps, **miter** joins, never round |
| arms | 30 across, 38 down (52° from horizontal) |
| echo | `+26` on Y only — no rotation, no scale, no reflection |
| clear space | one stroke width (11 units) on all four sides |

## Colour rule

Monochrome, always. The four mode hues (normal 250 / insert 155 / visual 70 / replace 25)
are the UI's language for modal state; if the mark took one it would look like it was
claiming a mode. Ink on dark surfaces is `oklch(0.91 0.012 80)`, on light `oklch(0.24 0.015 70)`.

---

## 1 · Project icon

No tile colour, no gradient, no badge, no letter inside a shape. The mark sits on the
editor's own background, or on transparency.

The mark is **optically** centred, not bounding-box centred — the echo makes it
bottom-heavy. The PNGs already carry a 6% inset; when you place the SVG yourself, size it to
**52% of a rounded tile** or **58% of a square one** and nudge it up by ~2% of the tile.

### Linux (primary target)

```
# hicolor theme — scalable is what actually gets used
share/icons/hicolor/scalable/apps/vex.svg        <- src/vex-mark-paper.svg
share/icons/hicolor/16x16/apps/vex.png           <- png/paper/vex-16.png
share/icons/hicolor/24x24/apps/vex.png
share/icons/hicolor/32x32/apps/vex.png
share/icons/hicolor/48x48/apps/vex.png
share/icons/hicolor/64x64/apps/vex.png
share/icons/hicolor/128x128/apps/vex.png
share/icons/hicolor/256x256/apps/vex.png
share/icons/hicolor/512x512/apps/vex.png
```

`share/applications/vex.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=vex
GenericName=Text Editor
Comment=A modal editor that never stalls on your keystroke
Exec=vex %F
Icon=vex
Terminal=false
Categories=Utility;TextEditor;Development;
MimeType=text/plain;text/x-rust;
StartupNotify=true
StartupWMClass=vex
```

Use the **paper** (light) variant as the shipped icon: desktop environments composite app
icons onto panels and docks that are dark far more often than light.

### Window icon at runtime

`winit` wants raw RGBA, so embed a PNG at build time and decode once:

```rust
// crates/app/src/icon.rs
use winit::window::Icon;

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/vex-256.png");

pub fn window_icon() -> Option<Icon> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}
```

Then `WindowAttributes::default().with_window_icon(window_icon())`.

### macOS / Windows (post-v1, when those builds exist)

- `.icns`: `iconutil -c icns vex.iconset` from an iconset built out of `png/paper/`
  (macOS expects `icon_16x16.png`, `icon_16x16@2x.png` … `icon_512x512@2x.png`).
- `.ico`: pack 16/24/32/48/64/256 from `png/paper/` with `magick` or the `ico` crate.

---

## 2 · Inside the editor

Three placements, and nothing beyond them. The mark is chrome, not content — it must never
render inside the buffer viewport or over text.

### a. Title bar — the one persistent placement

The title bar currently opens with the `vex` wordmark in Instrument Serif italic 34px. Set
the **mark** immediately left of it:

- mark height `26px` (the full mark, not solo)
- `gap 20px` between mark and wordmark
- colour: `inkFaint` → `oklch(0.55 0.01 72)` dark / `oklch(0.64 0.012 70)` light.
  **Deliberately dimmer than the wordmark.** The mark is a maker's stamp; nothing in the
  title bar should compete with the mode indicator on the right.
- it does not change with mode. The mark is the only element in the chrome that is immune
  to the mode hue — that is what makes it legible as identity rather than state.

If the window has a native title bar and you are not drawing your own, skip this — the OS
icon is already the mark.

### b. Splash / cold-start frame

`winit` gives you a window before `wgpu` has a first frame. Fill it with `bg`
(`oklch(0.165 0.008 70)`) and the stacked lockup, centred, mark at `46px` above the wordmark
at `34px`, `gap 8px`, left edges aligned to the mark's left terminal — then discard it on
the first real frame. Never animate it, never show a progress bar; the startup budget is
supposed to make this frame nearly invisible.

### c. Empty state — no buffer open

Mark at `64px` in `inkFaint` at 40% opacity, centred in the viewport, and under it, one line
of Plex Sans `12px` `0.2em` uppercase in `inkFaint`: `⌘P  OPEN A FILE`. Nothing else — no
recent-files list, no tips carousel.

### Where the mark must not go

Statusline (it is instrumentation, every pixel is live data) · the gutter · notification
toasts · the command palette · any button · watermarked behind the buffer · scaled below
16px anywhere.

---

## Misuse

Two colours · a mode hue as the mark's colour · round caps or joins · rotation · a third
chevron · outlined strokes · a drop shadow · the solo variant above 24px · the full variant
below 32px.
