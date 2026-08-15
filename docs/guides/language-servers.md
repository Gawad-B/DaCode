# Getting diagnostics, completion and go-to-definition

For someone whose code is highlighted but who gets no errors, no completion and no
<kbd>F12</kbd>. By the end you will have a language server running, know how to point vex at a
different one, and know where to look when it does not start.

## A language server is a separate program from the language

This is the thing that catches almost everyone. Having Python installed does not give you a Python
language server; having the Rust toolchain does not give you rust-analyzer. They are different
programs, installed separately, and vex drives whichever one it finds over LSP.

vex has no marketplace and no extension runtime, so there is nothing to install *inside* the editor
in the VSCode sense. What there is instead: vex knows the common servers by name, will run the
install command for you if you ask, and will use any other server you name in `init.lua`.

When it cannot find one, the statusline says `no python server` — the *language*, not a binary you
have never heard of — and the message box says what it looked for and what would fix it. Once per
language per session: useful the first time you open a `.py` file, nagging by the third.

## What vex looks for

The first program on this list that exists on `PATH` is the one that starts.

| Language | Files | Servers vex looks for, in order | Installed by |
|---|---|---|---|
| Rust | `.rs` | `rust-analyzer` | `rustup component add rust-analyzer` |
| TypeScript, JavaScript | `.ts` `.tsx` `.js` `.jsx` `.mjs` `.cjs` | `typescript-language-server` | `npm install --global typescript-language-server typescript` |
| Python | `.py` `.pyi` | `pyright-langserver`, `basedpyright-langserver`, `pylsp` | `npm install --global pyright`; or `uv`/`pipx`/`pip` for the other two |
| C and C++ | `.c` `.h` `.cpp` `.hpp` `.cc` `.cxx` `.hh` | `clangd` | install LLVM, or `apt install clangd` |
| Go | `.go` | `gopls` | `go install golang.org/x/tools/gopls@latest` |
| Lua | `.lua` | `lua-language-server` | your package manager |
| Dart | `.dart` | `dart` (as `dart language-server --protocol=lsp`) | the Dart SDK |

Python has three because "the Python server" is not one program: pyright comes from npm and pylsp
from pip, and a machine has whichever its owner happened to install. Offering only the first and
reporting it missing is how an editor tells someone with a perfectly good language server that it
cannot find one.

Only the **active** buffer gets a language server. Background buffers get crash-recovery writes and
nothing else, so diagnostics for a file you are not looking at are not live. That is deliberate — the
alternative is one server per open file — but it is a limitation rather than an oversight, and it is
worth knowing before you go looking for the problems panel to be complete. Switching to a buffer is
what hands it to the server.

## Installing one from inside the editor

New in 0.4.1. Open the command palette with <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> and run
**install a language server for this file**. It has no key of its own in either keymap, by design —
it is not something to fire off by muscle memory.

It runs the *same command it already told you to type*, derived from the same argument list, so what
is shown and what happens cannot drift apart. The first request explains the command and arms it:

> install a Python language server? vex will run: npm install --global pyright — choose it again to
> go ahead

Choose it a second time and it runs. It is worth being precise about what it will and will not do,
because "an editor that runs package managers" is a reasonable thing to be wary of:

- **It never invokes a shell.** Commands are argument lists, not command lines. There is no word
  splitting to get wrong and nothing to inject into.
- **It never asks for root.** Anything that would need it is not offered.
- **It never acts on one request.** Two, always, with the exact command printed in between.
- **It checks the toolchain first.** `npm install -g pyright` on a machine with no Node fails with
  "program not found", which names neither npm nor the thing to go and get. vex looks for the
  toolchain first and says that npm is not installed and that this command needs it — the program
  that installs a server is usually not the server, and the thing missing is usually the toolchain.
- **It does not believe the exit code.** "The installer exited zero" is not "vex can now run it". On
  Windows both `pip install --user` and `go install` write to directories `PATH` does not cover, so
  after a successful install vex looks for the program again and says so if it is still not there,
  rather than claiming a server it cannot start.
- **It offers nothing it cannot honestly run.** clangd, lua-language-server and the Dart SDK come
  from a system package manager whose name differs per distribution and which wants root, so they
  keep the advice and get no button. An editor offering a button that cannot work is worse than one
  that does not offer it.

The install runs on a worker thread — `rustup component add` takes tens of seconds — so the editor
stays live while it happens. Output is collected and reported at the end rather than streamed: a
package manager's progress is carriage-return animation that means nothing once it is text, and the
useful report is "it worked" or the last few lines of why it did not. When it works, the server
starts immediately; you do not have to reopen the file.

Where more than one command would work, vex picks the one your machine can actually run. For
basedpyright and pylsp it tries `uv`, then `pipx`, then `pip`, because `pip install` is refused
outright under PEP 668 on Debian, Ubuntu, Fedora, Arch and Homebrew Python — picking one at build
time would mean choosing which platform the button is broken on.

## Naming your own server

The built-in table is a default, not a limit. In `init.lua`:

```lua
-- A project's own virtualenv.
vex.server("python", { command = ".venv/bin/pylsp" })

-- A language vex has never heard of has to say which files it serves.
vex.server("zig", { command = "zls", args = { "--stdio" }, extensions = { "zig", "zon" } })
```

Three things about this are load-bearing.

**A command containing a path separator is a path, not a name.** `PATH` is a list of directories, so
looking `.venv/bin/pylsp` up in it finds nothing, ever — and a venv's own server is the main reason
to name one. A leading `~/` expands to your home directory.

**A relative path resolves against the project root**, not the process's working directory. For a
windowed editor that directory is wherever the desktop launched it from, and never what you meant by
`.venv`.

**Yours goes ahead of the built-ins rather than replacing them.** Naming a venv's server does not
also throw away the fallback for when that venv is not there. The statusline names whichever one
actually started, so the order is visible rather than a guess.

A language vex already knows — `rust`, `typescript`, `python`, `cpp`, `go`, `lua`, `dart` — fills in
the extension list for you. Any other name must list its own, because a server that can never match
a file is a line of config that silently does nothing, which is the worst outcome available. On
Windows the lookup honours `PATHEXT`, so the `.cmd` shims that npm installs are found.

The full field list is in the [settings reference](../reference/settings.md#vexserverlanguage-spec).

## What you get once one is running

| Key (Vim) | Key (conventional) | Does |
|---|---|---|
| `K` | — | hover: the type and docs under the cursor |
| `gd` | <kbd>F12</kbd>, ctrl-click | go to definition |
| `gu` | <kbd>shift</kbd>+<kbd>F12</kbd> | find references, into the results panel |
| `gs` | <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>o</kbd> | a symbol in this file — a searchable outline |
| `gS` | <kbd>ctrl</kbd>+<kbd>t</kbd> | a symbol anywhere in the project |
| `gr` | <kbd>F2</kbd> | rename everywhere |
| `gf` | right-click | format the buffer |
| `ga` | right-click | code actions, into the palette |
| `gi` | — | toggle inlay hints (or the `inlay_hints` row in settings) |
| `]d` `[d` | <kbd>F8</kbd> <kbd>shift</kbd>+<kbd>F8</kbd> | next / previous diagnostic |
| `gD` | — | the diagnostics list in the sidebar |
| <kbd>ctrl</kbd>+<kbd>space</kbd> | <kbd>ctrl</kbd>+<kbd>space</kbd> | completion |

Anything with no conventional key is in the command palette
(<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd>), and the right-click menu carries go-to-definition,
find references, rename, code actions and format with their keys printed beside them.

Completion is the one worth describing, because it carries more than a list of names. Each suggestion
shows its **type** beside it, in the order the *server* ranked them rather than one vex invented.
Accepting one inserts what the server asked for, so `println!` lands as `println!()` with the cursor
already inside and <kbd>tab</kbd> walks any further stops. And it brings its **import**: completing
`HashMap` in a file with no `use std::collections::HashMap` adds that line as part of the same undo
step.

One limitation to know before you use <kbd>F2</kbd> on a widely used symbol: **a rename is only
undoable in the file you have open.** The open buffer goes through its undo history; other files are
written straight to disk. Making it undoable needs a workspace-level history, and inventing half of
one would be worse than saying plainly what happened — so the editor reports how many files it
touched.

## Formatting

`gf`, or turn on `format_on_save`. There are two answers to "how is this file formatted", and vex
asks the server at the handshake which one applies: if it advertises `documentFormattingProvider`,
the request goes there — rust-analyzer runs rustfmt, gopls runs gofmt. If it does not, or there is no
server at all, vex runs a formatter itself.

That second path is not an edge case. pyright says no, so Python formats through ruff or black; a
plain JavaScript, CSS or Markdown project has no language server in the first place.

| Language | Formatters vex looks for |
|---|---|
| Rust | `rustfmt` |
| JavaScript, TypeScript | `prettier`, `biome` |
| Python | `ruff`, `black` |
| C, C++ | `clang-format` |
| Go | `gofmt` |
| JSON, CSS, HTML, Markdown, YAML | `prettier` |

Name your own the same way you name a server, with the same path rules:

```lua
vex.formatter("python", { command = ".venv/bin/black", args = { "-" } })
vex.formatter("nix", { command = "nixfmt", extensions = { "nix" } })
```

It runs on a worker thread, from the project root so it finds your `.prettierrc` or `pyproject.toml`.
Text goes in on stdin and comes back on stdout — never the path, because a formatter handed the file
would rewrite it behind a buffer you have unsaved changes in. A result computed against text you have
since typed into is discarded rather than merged, and only the part that actually changed is
replaced, so the cursor stays where you left it.

## When it does not work

**Read the statusline first.** It distinguishes cases that look identical from the outside:

| It says | Meaning |
|---|---|
| `no server for this language` | vex knows no server for this kind of file |
| `no python server` | it knows one and could not find it — the message box says what it looked for |
| `pyright-langserver starting` | the handshake is in progress |
| `pyright-langserver found but would not start` | it is there and the spawn failed; this one is vex's fault, not your `PATH`'s |
| `scratch buffer` | the buffer has no path, so there is no language to serve |

**Open the settings panel.** <kbd>ctrl</kbd>+<kbd>,</kbd>, then scroll to the language-server
section. It lists every language vex knows, with the full path of the server it found or the line
that would install one — and a row for each server `init.lua` named, first, so a mistyped path shows
up instead of hiding behind a working fallback. These rows are a report, not a control: they do not
cycle, and pressing one puts the whole line in the info box, which is how a path too long for a
narrow sidebar becomes readable. There is a formatter section below it doing the same job for
formatters.

The section is always below the fold, so scroll. Headless: `vex --screenshot out.png --settings
--row N` scrolls the panel to a row.

**Turn on tracing.** `VEX_LSP_TRACE=1 vex file.py` logs every message to and from the server, plus
why a reply was dropped. A reply computed against a document version you have since typed past is
discarded by design, and the trace says so rather than leaving you wondering why nothing appeared.

**A configured server that is "not found"** almost always means the relative path was resolved
against the project root and you meant something else. The message names the root it used.

**A rustup machine always has a `rust-analyzer` file** whether or not the component is installed — it
is a shim that exists either way and exits with an explanation when the component is missing. So "the
program is there" is not "the server works", which is why vex keeps the server's stderr and reports
it.

**If a server seems not to support something, check what vex asked for.** A capability the client
does not declare is a feature the server silently withholds — rust-analyzer will not offer an
out-of-scope name for auto-import unless the client advertises that it can resolve additional edits,
because computing an import for each of a hundred candidates up front is too slow to be worth doing.
The list comes back looking perfectly normal with the useful entry missing. vex declares both that
and label details; if you are chasing something similar, the handshake is the first place to look
rather than the last.

## See also

- [Configuration](configuration.md) — where `init.lua` lives and how it is loaded
- [Settings reference](../reference/settings.md) — `vex.server`, `vex.formatter`, `format_on_save`,
  `inlay_hints`
- [Command line reference](../reference/cli.md) — `VEX_LSP_TRACE` and the rest of the environment
- [CHEATSHEET.md](../../CHEATSHEET.md) — every language-server key in both keymaps
