# Files and directories

Everywhere DaCode reads or writes outside your project. Derived from `crates/dacode/src/paths.rs` (the
platform rules), `crates/dacode/src/config.rs` (what is read), `crates/dacode/src/session.rs` and
`crates/dacode/src/recovery.rs` (what is written).

## The two directories

| | Config | State |
|---|---|---|
| Holds | `init.lua`, themes, snippets, imported files | sessions, crash recovery |
| You edit it | yes | no |
| Windows: roams with the profile | yes | no |

`paths::resolve` picks each one from the environment, in this order. The first line that applies
wins, and an **empty** variable counts as unset — `APPDATA=` is how a stripped environment presents,
and joining onto it would put the config directory at the filesystem root.

### Config

| Platform | Path |
|---|---|
| any | `$DACODE_CONFIG_DIR` |
| Linux, BSD, macOS | `$XDG_CONFIG_HOME/dacode` |
| Linux, BSD, macOS | `~/.config/dacode` |
| Windows | `%APPDATA%\dacode` |
| Windows, no `APPDATA` | `%USERPROFILE%\dacode` |

### State

| Platform | Path |
|---|---|
| any | `$DACODE_STATE_DIR` |
| Linux, BSD, macOS | `$XDG_STATE_HOME/dacode` |
| Linux, BSD, macOS | `~/.local/state/dacode` |
| Windows | `%LOCALAPPDATA%\dacode` |
| Windows, no `LOCALAPPDATA` | `%USERPROFILE%\dacode` |

`paths::resolve` branches on `cfg!(windows)` and nothing else, so **macOS takes the Unix rows
above**: `~/.config/dacode` and `~/.local/state/dacode`, not `~/Library/Application Support`. That is what
the code does, deliberately: the same `init.lua` path on every Unix is the one the guides can name,
and it is where people who move between a Mac and Linux already keep their dotfiles.

The home directory is `HOME` first and `USERPROFILE` second — `HOME` is set and correct under a
POSIX shell on Windows too, so trying it first is right on every platform.

Config and state never share a directory on any platform, and there is a test that says so. The
reason is Windows: `%APPDATA%` roams with the profile and `%LOCALAPPDATA%` does not. Configuration
is the user's and should follow them between machines. A crash-recovery copy of a file that exists
on one machine is meaningless on another, and a session naming files that are not there is worse
than no session at all.

## In the config directory

| Name | Format | What it is |
|---|---|---|
| `init.lua` | Lua | The configuration. Read last, so it has the last word. Its **absence is what makes a start a first run**, so deleting it brings the wizard back. |
| `config.toml` | TOML | Hand-written, read before `init.lua`. Carries only `editor.font_size`, `editor.tab_width`, `editor.soft_wrap`, `editor.format_on_save`, `theme.name` and `theme.appearance`. |
| `imported-settings.toml` | TOML | Written by `dacode import --settings`. Same keys as `config.toml`, read before it. |
| `imported-keys.toml` | TOML | Written by `dacode import --keybindings`. **Nothing reads it back**; imported keys do not currently reach the keymap. |
| `themes/<slug>.toml` | TOML | Theme files. `config::resolve_theme` tries `themes/<slug>.toml` before the built-in of that name, so a file shadows a built-in. Written by `dacode import --theme` and by `dacode import --palette`. |
| `snippets/*.json` | VSCode snippet JSON | Read once at startup. Files without a `.json` extension are ignored; one that will not parse is skipped with a note on stderr rather than stopping the editor. |

Load order for settings is `imported-settings.toml`, then `config.toml`, then `init.lua`, each
overriding the one before. [`settings.md`](settings.md) has the full option list.

The settings tab (`ctrl-,`) writes back into `init.lua` rather than a file of its own, 600 ms after a
change. It appends a block under a marker line reading `-- dacode: written by the settings panel
(ctrl-,)` — the marker keeps the name the sidebar panel gave it, because renaming it would stop a
block written by an older version being found — replacing that block in place on later writes;
everything else in the file is preserved verbatim. Only the settings changed on the tab are in it.
Because Lua runs in order and the block is last, it wins over your own `dacode.set` calls above it —
deleting a line from the block, or resetting the setting on the tab, hands that setting back to your
code.

`dacode import`'s **default output directory is not the config directory the editor reads**.
`import_cli::default_config_dir` is `$HOME/.config/dacode`, hard-coded: it ignores `DACODE_CONFIG_DIR` and
does not fall back to `USERPROFILE`. Pass `--out` to be sure. See [`cli.md`](cli.md).

## In the state directory

| Name | Format | What it is |
|---|---|---|
| `sessions/<flattened-root>.session` | tab-separated text | One project's session. |
| `sessions/last-project` | one line | The absolute path of the last project opened. |
| `recovery/<hash>-<filename>` | text with a header line | An unsaved buffer's recovery copy. |
| `recovery/*.tmp` | — | The atomic write's temporary. Never present after a successful write. |

### Session files

Named after the project root with every non-alphanumeric character flattened to `-` and the result
trimmed, so two projects whose directories share a name do not collide.

```
# dacode session
!1600x900
*/home/me/project/src/main.rs	42	8
 /home/me/project/README.md	0	0
```

| Line | Meaning |
|---|---|
| `#…` | Comment. Skipped. |
| `!WxH` | The window size in physical pixels when the editor closed. |
| `*path⇥line⇥column` | An open file. The leading `*` marks the active one; a space marks the rest. |

Tab-separated because a path can contain almost anything else, including spaces. An unparseable
number falls back to 0 and a nonsense line still counts as naming a file, so one corrupt line does
not cost you the session. A file that no longer exists is dropped when the session loads, and the
active index follows the file it pointed at rather than the slot.

`last-project` answers a different question from the session files. Sessions are keyed by root and
say *what you were doing in this project*; `last-project` says *which project you were in*, which is
what DaCode needs when it is started from a Start Menu entry, a desktop icon or a dock with no argument
at all. It is written when a project is **opened**, not on the way out, so it survives a crash. A
recorded directory that no longer exists is ignored rather than opened.

### Recovery files

The name is the FNV-1a hash of the buffer's full path as sixteen hex digits, a `-`, then the file
name: `9a3f…c2-main.rs`. Derived from the full path so that two files called `main.rs` in different
directories keep separate copies, and so that reopening a file finds its own.

The first line is `dacode-recovery: <original path>`, followed by the buffer text verbatim. The header
is stripped when the copy is read back; it exists so a recovery file found later is traceable
without decoding the hashed name.

Written to a temporary and renamed into place, because a crash *during* the recovery write would
otherwise replace a good copy with a truncated one — losing the exact thing the file exists to save.

## Sessions and crash recovery are not the same thing

They answer to different failures, which is why they are separate mechanisms.

| | Session | Crash recovery |
|---|---|---|
| Remembers | which files were open, the cursor in each, the window size | the unsaved text of a dirty buffer |
| Scope | per project | per file |
| Written | once, when the editor closes properly | every 2 s while dirty, and forced before quitting or switching buffer |
| Removed | overwritten on the next clean close | on a successful save |
| Survives `kill -9` | no | yes, losing at most the last couple of seconds |

Nothing is written for an empty session: quitting with nothing open should not leave a file that
restores nothing. Recovery covers **every** dirty buffer, not only the visible one — background
buffers get recovery writes even though they get no watcher, no git diff and no language server.

A recovery copy is only offered when it is **newer** than the file on disk. If the saved file is at
least as new, the last session ended cleanly, the copy is stale, and it is deleted rather than
offered — restoring text you already superseded would be worse than silence.

## Environment variables

| Variable | Moves |
|---|---|
| `DACODE_CONFIG_DIR` | The config directory, on every platform. Not consulted by `dacode import`'s default output path. |
| `DACODE_STATE_DIR` | The state directory, on every platform. |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | The two directories on non-Windows platforms, before the `~` fallbacks. |
| `APPDATA`, `LOCALAPPDATA` | The two directories on Windows. |
| `HOME`, `USERPROFILE` | The home directory both fall back to. |

[`cli.md`](cli.md) lists all fourteen variables DaCode reads, including the ones that change fonts,
tracing and program lookup rather than paths.
