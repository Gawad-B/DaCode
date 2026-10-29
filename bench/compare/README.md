# DaCode against VSCode

```sh
cargo build --release -p dacode
python3 bench/compare/compare.py          # about five minutes; writes web/src/benchmark.json
```

The landing page's "Against VSCode" section reads `web/src/benchmark.json` and nothing else, so
rerunning this is the only way its numbers change. The feature checklist beside it is written by
hand in `web/src/versus.ts`.

## What it needs

Linux, `Xwayland`, `libX11`, `libXtst`, Python 3.10+ with numpy, VSCode at `/usr/share/code`, and
nothing else: the X calls go through ctypes (`x11.py`). It starts its own rootful Xwayland on
display `:9` — a window appears on the desktop for the length of the run — and both editors are
launched into it, so the harness can read pixels and press keys without touching the real session.
Leave the machine alone while it runs; anything else using the CPU lands in both editors' numbers.

**Keep that window visible and uncovered.** A hidden Xwayland window presents on a one-second
fallback timer instead of the compositor's frame callbacks, so every keystroke would read ~1000 ms.
The harness refuses a keystroke run that looks like that rather than recording it.

## What it measures, and what it does not

Every number is measured from outside the editor, the same way for both. Neither is asked to report
on itself.

- **Startup**: launch until the last frame that changed more than 0.25% of the screen, with the file
  highlighted. A frame-time readout or a status-bar item ticking over is not startup; syntax colour
  arriving is. Ten runs after one discarded warm-up, so both read from a warm disk cache.
- **Keystroke**: an XTEST key press until the pixels of the edited line change, alternating typing a
  character and deleting it, two hundred times. This is the editor's share — input handling, layout,
  GPU, and Xwayland's copy — and not glass-to-glass: the desktop compositor and the monitor come
  after it and are the same for both.
- **Memory**: PSS summed over every process the editor started. PSS splits shared pages among the
  processes sharing them, so Electron's many processes are not each charged for the same libraries.
- **Idle CPU**: twenty seconds with a file open and nothing happening.
- **On disk**: the installed program — `dacode` on `PATH`, `/usr/share/code`.

To keep it fair, and to lean towards VSCode wherever a choice had to be made:

- VSCode runs on a throwaway profile with no extensions and AI features off (`chat.disableAIFeatures`)
  — a fresh profile otherwise opens the Copilot panel and starts `copilot-runtime`. The other settings
  changed only take noise off the screen being watched: caret blink, welcome page, recommendation
  toasts, update checks.
- The files are CSS: both editors highlight it and, here, neither runs a server for it. VSCode's
  built-in CSS server is disabled to match DaCode, which has none. Rust was tried first and dropped,
  because with rust-analyzer off `PATH` DaCode shows a card saying so, and timing a card is not timing
  startup.
- Both windows are 1600×1000 on the same display. Both use their default fonts and themes.

One machine is one machine: the numbers are this hardware's, and the page says which.
