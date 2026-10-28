# Bringing your setup over

For someone moving from VSCode who wants their theme, their keys and their snippets without
re-creating them by hand. By the end you will have imported what is portable, and you will know
exactly what is not — and what DaCode does instead.

## Look before you import

```sh
dacode import --from-vscode --list
```

That reads your VSCode user directory, prints what it found, and imports nothing. It is worth
running first, because the answer depends on what you actually have: a `keybindings.json` you never
wrote is not going to appear.

The directory is found in the usual places — `%APPDATA%\Code\User` on Windows, `~/.config/Code/User`
elsewhere — and VSCodium and Code - OSS are looked for under the same roots, so a fork works too.
Name it explicitly if your install is somewhere unusual: `dacode import --from-vscode
/path/to/Code/User --list`.

## Then take what you want

**Nothing is imported unless you name it.** An importer that quietly replaced your theme because you
wanted your keybindings is one nobody runs twice.

```sh
dacode import --from-vscode --settings --theme --keybindings --snippets
```

| Flag | What comes across | Written as |
|---|---|---|
| `--settings` | editor preferences from `settings.json` | `imported-settings.toml` |
| `--theme` | your colour theme, converted to DaCode's palette | `themes/<slug>.toml` |
| `--keybindings` | bindings that have a DaCode counterpart | `imported-keys.toml` — **written, but not yet read; see below** |
| `--snippets` | snippet definitions, one file per language | `snippets/` |
| `--language-config` | brackets, comment tokens, auto-close pairs | recognised and reported, not yet used |
| `--language-server` | a language server the extension names | recognised and reported, not yet used |
| `--all` | everything the source offers | |

Every item is accounted for in a report afterwards: **imported** with a count, **skipped** because
you did not select it, **not present** because the source has none of it, or **unsupported with the
reason**. Silence about the last of those would look like a bug in DaCode instead of a deliberate
boundary. `dacode import` exits non-zero if anything you selected failed.

The same four common items are a checklist on the **Import** page of the settings tab
(<kbd>ctrl</kbd>+<kbd>,</kbd>) — tick with <kbd>→</kbd> or a click, then *import now* — and the first-run wizard's third question is
that checklist, with nothing ticked by default. So this never needs a terminal.

**One caveat about where it writes.** `dacode import`'s default output directory is `$HOME/.config/dacode`,
hard-coded: it does not consult `DACODE_CONFIG_DIR`, and it does not fall back to `USERPROFILE` the way
the editor does, so on Windows without `HOME` it resolves relative to the working directory. If you
have moved your config, pass `--out` and be sure.

### What settings actually carry over

Nine keys are read: `editor.fontSize`, `editor.fontFamily`, `editor.tabSize`,
`editor.insertSpaces`, `editor.formatOnSave`, `editor.wordWrap`, `editor.rulers`,
`editor.lineNumbers` and `workbench.colorTheme`. JSON-with-comments is fine — VSCode's own
`settings.json` ships with comments and trailing commas, and rejecting those would make the importer
fail on almost every real configuration.

Four of those are written and then read by nothing: `font_family`, `insert_spaces`, `ruler_column`
and `relative_line_numbers` have no home in DaCode's settings yet, so `--settings` can report bringing
over settings that do nothing. That is recorded in the
[settings reference](../reference/settings.md#where-the-three-disagree) rather than hidden.

### What keybindings carry over

Only bindings whose command has a DaCode counterpart — undo, redo, save, select all, indent, outdent,
delete line, case transforms, the cursor motions, and so on. The rest are reported by name rather
than dropped silently, because most VSCode commands come from extensions and refer to behaviour DaCode
does not have, and a key that quietly did nothing would leave you wondering.

The mapping is deliberately conservative. A wrong mapping is worse than a missing one: a key that
does the wrong thing destroys text, while a key that does nothing merely disappoints.

**Be aware of the gap here.** `imported-keys.toml` is written, and nothing reads it back. The other
three items land in files the editor loads at startup — `imported-settings.toml` is a layer in the
config load order, a theme goes into `themes/`, snippets into `snippets/` — but the keybindings file
is a *record* of what could be translated, not config that takes effect. Treat it as a worksheet:
open it, and turn the lines you care about into `dacode.map` calls in `init.lua`. See
[choosing how you type](keymaps.md#rebinding) for the syntax.

## Extensions: data yes, code never

This is the boundary, and it is worth being exact about it because it is the main difference between
the two editors.

**VSCode extension code cannot run in DaCode, and this is not a gap waiting to be filled.** There are
two independent reasons:

**Legal.** Microsoft's marketplace terms restrict use to in-scope products — Visual Studio, VS Code,
Codespaces, Azure DevOps. Non-Microsoft editors may not fetch from it; this is what Cursor and
Windsurf hit in 2025. **Open VSX**, run by the Eclipse Foundation, is the legally redistributable,
API-compatible registry, and is what DaCode targets.

**Architectural.** Extensions are JavaScript against the `vscode` API in Node. Supporting them means
embedding a JS runtime, reimplementing that API, and paying a Node-sized process per extension —
which is precisely what makes VSCode heavy, and the opposite of this project's premise.

What *is* portable is extension **data**, and the **language server** that most valuable extensions
merely wrap. §5a of [PLAN.md](../../PLAN.md) has the full account, including what the importer defers
and why.

### Reading an extension package

A `.vsix` is a zip with a manifest, and DaCode can open one and sort what it contributes into what it
can use and what it cannot, with a reason attached to every refusal:

```sh
dacode import --vsix ~/Downloads/something.vsix --list
dacode import --vsix ~/Downloads/something.vsix --theme --snippets
```

| Contribution | Verdict |
|---|---|
| colour theme | portable |
| snippets | portable |
| language configuration | portable — recognised, not yet consumed |
| tree-sitter grammar | informational; DaCode uses its own bundled grammars |
| a language server | informational, and usually the valuable part — drive it over LSP directly |
| a debug adapter | informational; usable over DAP once DaCode has debugging |
| anything needing the extension to run | unsupported, with that as the reason |

Or fetch by publisher and name from Open VSX, which downloads the package into the output directory
and keeps it, so re-importing later needs no network:

```sh
dacode import --openvsx zhuangtongfa.material-theme --theme
```

An extension's `main` entry point is inert throughout. No JavaScript is executed, ever.

## What you will find missing, and what DaCode does instead

| In VSCode | In DaCode |
|---|---|
| the marketplace | language servers driven over LSP, plus `dacode import` for extension data. Rust, Python, TypeScript, JavaScript, C, C++, Go, Lua and Dart are known out of the box, and the editor will install one for you — see [language servers](language-servers.md) |
| the integrated terminal | <kbd>ctrl</kbd>+<kbd>`</kbd>, as in VSCode, or <kbd>ctrl</kbd>+<kbd>t</kbd>: a panel under the code running your `$SHELL` (PowerShell on Windows) in the project root. While it has focus every key is the shell's except <kbd>ctrl</kbd>+<kbd>`</kbd>, <kbd>ctrl</kbd>+<kbd>t</kbd>, <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>v</kbd> and <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd>; clicking the code gives the keys back. One terminal, no splits or tabs, and no mouse selection yet — see [keymaps](keymaps.md#the-terminal) |
| debugging | not present. A debug adapter is recognised in a `.vsix` and reported, nothing more |
| the minimap | not present |
| code lens | not present; it needs LSP code lenses |
| inline suggestions | not present |
| icon themes | DaCode uses no icons — every graphic element is text or a rectangle |
| `editor.cursorBlinking` | the cursor's shape and blink are defined by the editing mode, not by a setting |
| `editor.fontLigatures` | ligatures follow the font, and are on when it has them |
| git blame | the caret line's author, age and summary after its end, as GitLens does (`inline_blame`, on by default); `gb` / <kbd>alt</kbd>+<kbd>b</kbd> for a gutter column, `gB` / <kbd>shift</kbd>+<kbd>alt</kbd>+<kbd>b</kbd> for the whole commit |

The ones that are `settings.json` keys — the minimap, code lens, inline suggestions, icon themes,
cursor blinking, ligatures and the terminal's shell setting — are named individually in the import report
when your settings mention them, each with its reason, rather than being lumped into "unknown".

Two more worth knowing, stated plainly because they are limitations rather than choices: buffer
search is still linear in the document, so a rare query in a very large file walks all of it; and
only the **active** buffer gets a file watcher, a git diff and a language server, so a file changed
on disk behind an unfocused buffer is noticed when you switch to it rather than when it happens.

## The keys you already know

Set `vim_keys = false` — the first row of the settings tab's **Editor** page, or the first question the first-run
wizard asks — and the conventional keymap is the one you have muscle memory for:
<kbd>ctrl</kbd>+<kbd>d</kbd> selects the next occurrence, <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>l</kbd>
every occurrence, <kbd>alt</kbd>+<kbd>↑</kbd> moves a line,
<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>k</kbd> deletes one, <kbd>ctrl</kbd>+<kbd>/</kbd> comments,
<kbd>F12</kbd> goes to the definition, <kbd>F2</kbd> renames,
<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> is the command palette and <kbd>ctrl</kbd>+<kbd>p</kbd>
is the file palette. Alt-click adds a cursor, alt-drag selects a rectangle, and the clipboard keys
are the system clipboard.

That keymap is a first-class editor rather than the modal one with the modes removed — the reasoning
is in [choosing how you type](keymaps.md), along with how to rebind any of it.

## See also

- [Command line reference](../reference/cli.md#dacode-import) — every `dacode import` flag and source
- [Configuration](configuration.md) — turning what you imported into an `init.lua` you own
- [Settings reference](../reference/settings.md) — what the imported TOML layer can and cannot carry
- [Themes](themes.md) — importing a colour scheme on its own, in any of four formats
- [Language servers](language-servers.md) — the replacement for most of what you had extensions for
