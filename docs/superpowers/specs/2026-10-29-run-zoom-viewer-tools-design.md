# Run, zoom, viewer and on-demand tools — design

Seven requests from one round of use, built in this order. Each lands as its own commit.

## 1. The Run button runs the open file

`share::run_command` only recognised project markers, so a Python file with no `main.py` beside it
got "no run command found". The button now asks `run_command_for(root, file, platform)`:

- A project command wins only when the open file belongs to that project's toolchain (`.rs` under a
  `Cargo.toml`, `.js`/`.ts` under a `package.json`, `.go` under a `go.mod`, `.dart` under a
  `pubspec.yaml`).
- Otherwise the file runs by its extension: `python3 file.py` (`py` on Windows), `node`, `deno run`
  for `.ts` without a package, `go run`, `ruby`, `bash`/`sh`, `lua`, `perl`, `php`, `rustc` then the
  binary, `gcc`/`g++` then the binary, `javac` then `java`.
- No file and no project: the terminal opens and says so, as before.

The path is quoted for the platform's shell, and the buffer is saved first. The platform is a
parameter, so the Windows rules have tests here.

## 3. Interface zoom and an Accessibility page

`App::scale()` returns `window.scale_factor() * zoom`. Every physical-pixel computation already reads
it, so the chrome, the code, the mouse and the terminal grid move together.

- `zoom_in`, `zoom_out`, `zoom_reset` actions on `ctrl-=`/`ctrl-+`, `ctrl--`, `ctrl-0`, 50%–300% in
  10% steps; the bound lives on one type and the settings page reads it from there.
- ctrl-scroll and pinch keep changing only the code font size.
- `interface_zoom` is a config option in all three places (Lua reader, `Settings`, settings page).
- A new "Accessibility" settings page gathers interface zoom, code font size and animations.

## 4. The terminal folds while Settings is in front

`layout` gives the terminal panel only its tab strip while the settings tab is the active view; the
shells keep running and their grid is not resized to zero. The decision is made in `layout`, which
drawing and the mouse both read.

## 7. Rainbow CSV

`.csv`, `.tsv`, `.psv` colour each column from an eight-colour cycle taken from the theme. Columns are
split per *visible* line at draw time, quote-aware (`"a,b"` and `""`), so cost is bounded by the
viewport and nothing is held across an edit. The statusline names the header of the column under the
caret. A multi-line quoted field is coloured from its own line's start — stated, not hidden.
`latency.rs` gets a hundred-thousand-line CSV case.

## 2. A viewer for images, PDFs and office documents

- **Images** — PNG, JPEG, GIF (first frame), WebP, BMP, ICO via `image`; SVG via `resvg`. Decoded on
  a worker, capped in size; drawn by a new RGBA texture pipeline in `dacode-render`. Fit, zoom,
  pan, checkerboard behind transparency.
- **PDF** — `hayro`, pure Rust. Only visible pages are rasterised, on a worker, each result tagged
  with (page, zoom); a stale one is dropped. Vertical page scroll, page indicator, fit-width.
- **Office** — `.docx .xlsx .pptx .odt .ods .odp` unzipped and their text/tables extracted into a
  read-only buffer (the diff tab's mechanism: `mutate` refuses). The tab says layout is not shown.
- `open.rs` routes these instead of refusing them as binary; other binaries are still refused.

## 5 + 6. Formatters and language servers on demand

A `dacode-tools` crate holds the table: tool, pinned version, and per (OS, arch) URL + SHA-256.

- Single binaries: rust-analyzer, clangd, ruff, taplo, lua-language-server, marksman, shfmt, stylua.
- Node-based (pyright, typescript-language-server, prettier, bash/yaml/vscode-langservers): one
  portable Node.js is downloaded into the tools directory and they are installed with its npm.
- Tools live in `<state>/tools` (`~/.local/share/dacode/tools`, `%LOCALAPPDATA%\dacode\tools`).
- Lookup: configured command, then `PATH`, then the tools directory.
- Opening a file whose server or formatter is missing offers it in a notification. Nothing downloads
  without a click. Downloads run on a worker (`Version`, `poll`, `Waker`) through `curl`, behind the
  updater's `Fetch` trait so tests never reach the network. A hash mismatch is refused.
- A Tools settings page lists installed / available / missing with Install, Update, Remove.
- Not covered: jdtls (needs a JDK), gopls (offered only when `go` is present), C#.

## Testing

Behaviour-sentence tests driving the real paths: run commands per type and platform; CSV splitting
with quotes; office extraction against fixtures; tool-table lookup per platform; a checksum mismatch
refused; a download flow behind a fake `Fetch`. Screenshots at two sizes for each visible piece.
