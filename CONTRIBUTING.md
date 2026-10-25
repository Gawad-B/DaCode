# Contributing to DaCode

Patches are welcome, and so is an issue that only says what went wrong. This file is the front door:
how to build it, what has to pass before a pull request, and the handful of rules that are not
negotiable because breaking them has already cost this project something.

Three files matter more than this one:

- **`CLAUDE.md`** — the working notes. The conventions, the non-obvious invariants, and a *Where
  things are* table that maps what you want to change to the file it lives in. Read it before your
  first change; this document points at it rather than repeating it.
- **`PLAN.md`** — the long-form architecture and decision record, including the decisions that turned
  out wrong. When the two disagree, `PLAN.md` is the reasoning and `CLAUDE.md` is the summary.
- **`README.md`** — what DaCode is, from outside.

## Building and running

Rust **1.85** or newer — that is `rust-version` in the workspace `Cargo.toml`, and it is the minimum
supported version, not a suggestion. Edition 2024, stable toolchain, no nightly features.

**On Linux**, `winit` needs the Wayland and xkbcommon development headers. Without them the first
`cargo build` fails naming a `.pc` file rather than a package, which is a bad first five minutes:

```sh
sudo apt-get install -y libwayland-dev libxkbcommon-dev
sudo apt-get install -y rpm            # only if you want packaging/build-rpm.sh to work
```

**On Windows**, you need the MSVC toolchain and a working C compiler: `dacode-syntax` builds nine
tree-sitter grammars from C and `dacode-lua` builds Lua 5.4 from source. Those are also precisely the
two crates that cannot be type-checked for Windows from a Linux machine.

Then:

```sh
cargo build --release
cargo run -- path/to/file.rs
```

Debug builds are meant to be usable — `opt-level = 1` is set in the workspace profile for exactly
that reason — but the performance figures in the README refer to the release binary.

## What must pass before a pull request

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

**Clippy stays at zero warnings.** CI runs it with `-D warnings`, so one allowed lint is a red build.

CI runs the tests and clippy on **Linux and Windows both**, and the second is not redundant: code
inside `#[cfg(unix)]` is never linted on Windows, and code only *used* there produces warnings only
Windows sees. A `mut` that Linux needed and Windows did not sat in `main.rs` for months because of
that. `cargo fmt` is not checked by CI — the tree is formatted anyway, so run it and keep diffs about
the change rather than about whitespace.

Both jobs install `rust-analyzer` deliberately, so the live language-server tests actually execute
rather than skipping everywhere. Locally they skip when it is absent, which is the point: a test that
fails on a machine without a dependency is testing the machine.

And, when they apply:

```sh
cargo run -p dacode-input --example cheatsheet              # after touching the grammar or CHEATSHEET.md
cargo check --workspace --target x86_64-pc-windows-msvc  # 7 of 9 crates, from Linux
cargo bench -p dacode -p dacode-syntax                         # measurement, not a gate
```

If you cannot build for Windows locally, push the branch and let the Windows job tell you. It earned
its keep on its first run by catching `file://` URIs built by percent-encoding a whole Windows path,
which made rust-analyzer reject every request as an "invalid international domain name". Nothing on
Linux could have seen it.

## The one rule

**Nothing unbounded may run on the UI thread.** Background work takes a cheap rope snapshot plus the
document's `Version` and returns results tagged with the version it saw — the UI applies a result
only if that version still matches — and anything per-frame is bounded by the *viewport*, not by the
document. `CLAUDE.md`'s "The one rule" section has the whole of it, and if you add a background
producer it needs a `Version`, a `poll()` that never blocks, and a line in `about_to_wait` beside the
others.

**`crates/dacode/tests/latency.rs` is that rule as a test that fails the build.** It asserts *flatness*
rather than milliseconds — a hundredfold larger document may cost at most three times more — because
an absolute budget tight enough to catch a regression is also tight enough to fail at random on a
loaded runner. It works: injecting a document-length walk into `chrome::draw` fails it at 104x, and
one into `InsertText` at 111x.

## Verifying visual work

Do not review drawing code by reading it. `--screenshot` renders any state offscreen and exits, so
you look at the actual pixels:

```sh
DACODE_NO_SYSTEM_FONTS=1 cargo run --release -- --screenshot /tmp/dacode.png --settings
DACODE_NO_SYSTEM_FONTS=1 cargo run --release -- --screenshot /tmp/small.png --settings --size 900x700
```

`DACODE_NO_SYSTEM_FONTS=1` makes the capture reproducible and startup faster; use it every time. The
full flag list is in `CLAUDE.md` and in the README, and it covers every overlay — the palette, the
menus, the tree, the completion popup, the welcome wizard, diagnostics, folds, git marks.

**Render at more than one size.** The harness was fixed at 1600x1000 for a long time and a whole
family of defects lives below that: the settings rows collided with the footer under about 900px of
height and the toolbar was laid out at a negative x under about 450px of width. Four overflow defects
shipped for that reason, so anything with a panel in it gets captured small as well.

CI renders every overlay on each push and keeps the PNGs as a build artifact, which means the check
still applies to a reviewer with no GPU to hand. Attach the relevant PNG to any pull request that
changes how something looks.

## Adding a user-visible command: four places

Everything the editor can do is an `Action` — keys resolve to one, the palette lists them, macros
record them, config binds them, tests drive them. That is why a macro survives rebinding and why
`3dw` can be tested with no window. Adding one means adding all four of:

1. the `Action` variant,
2. its name in `Action::from_name`,
3. that same name in `Action::NAMES`,
4. a row in `dacode_input::commands::COMMANDS`.

Three tests fail until all four exist, and that is deliberate — the name is public API that people's
`init.lua` files depend on, and `COMMANDS` is the *only* description of what the keys are. The
command palette, the F1 help panel, the menu bar and the which-key popup all read that one table, and
a test presses every non-modal key in it through the real grammar. Four features each keeping their
own list would disagree within a month, and a discoverability feature that lies is worse than none: it
teaches a key that does nothing.

## Adding a config option: three places

`crates/dacode-lua/src/lib.rs` (the Lua reader), `crates/dacode/src/config.rs` (the settings struct) and
`crates/dacode/src/settings_ui.rs` (the panel). All three, by design. Add it to fewer and it will
silently not work somewhere — most often it reads from `init.lua` and is invisible in the panel, or
the panel writes a value the reader then rejects at the next start.

## Testing conventions

The long version is in `CLAUDE.md`. The parts that get missed:

- **Test names are sentences about behaviour**: `a_stale_reply_is_refused_whole`, not `test_apply_2`.
  And when the behaviour is not obvious, a comment says *why it matters*. A test that only asserts is
  a test nobody can safely change.
- **Verify a guard by injecting a violation.** The first version of `crates/dacode/tests/conventions.rs`
  scanned zero files — cargo runs tests with the crate as the working directory, not the workspace —
  and passed while checking nothing. The second version looked for context a few lines away and the
  injected violation happened to sit under an unrelated match: green, and worthless. If you add a
  guard, break the thing it guards and watch it go red before you trust it.
- **A test must drive the real code path, not a copy of it.** The session ordering test originally
  wrote out its own restore loop, so it passed whatever `main` did — including with the defect
  injected. If a defect is not reachable from a test, that is the first thing to fix.
- **Never mutate process-wide state in a test.** Environment variables in particular: `setenv` is not
  thread-safe against the `getenv` other tests are doing in parallel, and it once deadlocked this
  suite inside glibc for twenty minutes. Take the value as a parameter instead, as
  `paths::resolve(Where, bool, &Environment)` does — which has the pleasant side effect of making the
  Windows rules testable from Linux.
- **A test that names a program must name one the platform guarantees.** `cmd` on Windows, `sh`
  elsewhere. Asserting `which("sh")` passed everywhere it had ever run, including the Windows runner,
  whose image ships Git's `sh.exe` — then failed on the first real Windows developer machine.
- **A test needing an external program skips when it is absent**, and skips on `dacode_lsp::can_start`
  rather than on the program merely existing: on a rustup machine `rust-analyzer` is a shim that is
  present whether or not the component is, and speaking LSP to it fails in a way that looks like a
  DaCode bug. CI failed on exactly that.
- Property tests (`proptest`) for anything about coordinates or text invariants.

## Commit messages and pull requests

Commit messages are prose, not bullet lists of files — git already knows which files changed. Say
what changed, why, and what you rejected on the way; the last part is the one that is worth reading a
year later. The same goes for the pull request description.

Comments follow the same rule: they explain **why**, never what, and several of the best ones in this
codebase are a record of something that went wrong. Prefer stating an honest limitation in a doc
comment over a clever workaround that hides it.

British-ish spelling in prose is fine. Identifiers are American — `color` in code, "colour" in text —
because the wgpu and CSS vocabulary around them is.

## Where to start

`CLAUDE.md`'s *Where things are* table is the fastest route from "I want to change X" to the file. A
few common ones:

| Want to change | Look at |
|---|---|
| A key binding | `crates/dacode-input/src/grammar.rs`, then `commands.rs` and `CHEATSHEET.md` |
| What a command does | `crates/dacode/src/execute.rs` |
| How something looks | `crates/dacode/src/chrome.rs` |
| Where something is on screen | `crates/dacode/src/layout.rs` |
| A theme | `crates/dacode-theme/src/theme.rs` |
| Language-server behaviour | `crates/dacode-lsp/src/client.rs`, applied in `crates/dacode/src/edits.rs` |

Before a large change, open an issue. Some of the obvious missing features — an integrated terminal,
git blame, structural text objects — are *deliberate* omissions, listed under "Known gaps" at the
foot of `CLAUDE.md` and argued out in `PLAN.md`. It is a short read and it will save you building
something that gets declined for a reason nobody told you about.

By contributing you agree that your work is licensed under the MIT licence, like the rest of the
repository, and that you will follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security problems go
through [SECURITY.md](SECURITY.md) rather than a public issue.
