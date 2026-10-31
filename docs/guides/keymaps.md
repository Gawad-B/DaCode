# Choosing how you type

For someone who has DaCode open and wants the keys to be the ones their hands already know. By the end
you will have picked a keymap, know exactly what changed when you did, and be able to rebind any of
it.

DaCode ships two keymaps. Vim's modal grammar is the default; turning it off gives you a conventional
editor. The choice is one setting, it changes live, and nothing about your open files is affected by
flipping it.

On a Mac, every <kbd>ctrl</kbd> key in this guide is also <kbd>⌘</kbd>: <kbd>⌘</kbd><kbd>S</kbd>,
<kbd>⌘</kbd><kbd>P</kbd>, <kbd>⌘</kbd>-click for go to definition, <kbd>⌘</kbd>-scroll to zoom.
<kbd>ctrl</kbd> itself keeps working, so Vim's <kbd>ctrl</kbd> keys stay where your hands expect
them. The menus, the palette and the help panel print keys the Mac way — `⇧⌘P`, `⌥⌘F`.
<kbd>⌘</kbd><kbd>H</kbd> hides DaCode, <kbd>⌥</kbd><kbd>⌘</kbd><kbd>H</kbd> hides everything else and
<kbd>⌘</kbd><kbd>M</kbd> minimises, as in every Mac application; that is why find and replace, which
is <kbd>ctrl</kbd>+<kbd>h</kbd> elsewhere, is <kbd>⌥</kbd><kbd>⌘</kbd><kbd>F</kbd> there (physical
<kbd>ctrl</kbd>+<kbd>h</kbd> still works). <kbd>⌘</kbd><kbd>Q</kbd> quits and asks about unsaved files
first.

The **left** <kbd>option</kbd> key is <kbd>alt</kbd> for every <kbd>alt</kbd> binding below; the
**right** one still types what <kbd>option</kbd> types on your keyboard — `@`, `{`, `€` and the rest.
The *Option as Alt (Mac)* row on the Editor page of the settings, or `option_as_alt` in `init.lua`,
swaps them, makes both Alt, or neither. With <kbd>⌘</kbd> held, either <kbd>option</kbd> is
<kbd>⌥</kbd>.

## Switching

The first row of the settings tab's **Editor** page is the switch, called *Vim keys* (or *Modal
editing*, once it is off). Press <kbd>ctrl</kbd>+<kbd>,</kbd> — or click the gear — pick **Editor**,
and change the row with <kbd>←</kbd>/<kbd>→</kbd>, <kbd>enter</kbd>, or a click. It applies
immediately and is written to a marked block at the end of `init.lua` a moment later, leaving your own
lines alone. There is nothing to save.

The same thing from config:

```lua
dacode.set { vim_keys = false }
```

Saving `init.lua` reloads it in place — the keymap, the theme and the metrics all change without
restarting and without touching the documents you have open. If you have never written one:

```sh
mkdir -p ~/.config/dacode && cp examples/init.lua ~/.config/dacode/init.lua
```

The first-run wizard asks this question before anything else, so on a fresh install the answer is
usually already yours. Pressing <kbd>esc</kbd> there skips the wizard and writes nothing, so it
returns next launch rather than disappearing because of one stray keypress; `--no-welcome` starts
without it entirely.

## What actually changes

Three things, and it is worth knowing all three because two of them are visible before you type
anything.

**The grammar.** With `vim_keys` on, letters are commands and `[count] ["register] operator [count]
{motion|object}` composes: `3dw`, `d2f)`, `ci"`, `"ayy`, `.` to repeat. With it off, letters insert
themselves and the shortcuts are the conventional ones.

**The number of modes.** With the grammar off there is exactly one mode and it is insert: the cursor
is a blinking bar, the accent is the insert hue, and the statusline says EDITING and goes on saying
it. Nothing can put you into a mode you cannot name — <kbd>esc</kbd> dismisses a hover or a message
box and does nothing else, which is what every conventional editor does with it.

**The gutter.** `line_numbers` has no fixed default; it follows the grammar. Relative numbering
exists so `12j` can be read straight off the screen, and with no count to type it is a column of
numbers that are not the line's number. So you get relative with Vim keys on and absolute with them
off, unless you set it yourself.

## The conventional keymap is a real editor

It is not the modal one with the modes taken out. That distinction cost real work, and the reason is
worth stating: Vim reaches most of a conventional editor's line-editing verbs by *composition*.
Moving a line down is `ddp`; duplicating one is `yyp`; deleting a word backwards is `db`. Turn the
grammar off and there is nothing left to compose with, so every one of those has to exist as a
binding of its own or it does not exist at all. An earlier version had thirteen keys, and that is
exactly what it felt like.

So these are keys in their own right, and they are the ones a VSCode or Sublime user reaches for
first:

| Key | Does |
|---|---|
| <kbd>ctrl</kbd>+<kbd>d</kbd> | select this word, then the next copy of it |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>l</kbd> | select every copy of it |
| <kbd>alt</kbd>+<kbd>↑</kbd> <kbd>↓</kbd> | move these lines |
| <kbd>shift</kbd>+<kbd>alt</kbd>+<kbd>↑</kbd> <kbd>↓</kbd> | copy these lines |
| <kbd>ctrl</kbd>+<kbd>alt</kbd>+<kbd>↑</kbd> <kbd>↓</kbd> | add a cursor above or below |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>k</kbd> | delete this line |
| <kbd>ctrl</kbd>+<kbd>/</kbd> | comment or uncomment |
| <kbd>ctrl</kbd>+<kbd>backspace</kbd> · <kbd>ctrl</kbd>+<kbd>delete</kbd> | delete a word |

The clipboard keys are the system clipboard, not a register. The mouse is a first-class path:
double-click takes a word, triple-click a line, alt-click adds a cursor, alt-drag selects a
rectangle, and ctrl-click goes to the definition. [CHEATSHEET.md](../../CHEATSHEET.md) has the
complete list for both keymaps.

One choice worth explaining, because it looks arbitrary until you know: <kbd>ctrl</kbd>+<kbd>d</kbd>
is occurrence selection rather than "add a cursor below". It is the first shortcut a VSCode user
reaches for, and having it do something else is worse than not having it. The cursor-below gesture
is <kbd>ctrl</kbd>+<kbd>alt</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd>, which is where VSCode keeps it.

## The Vim grammar

With `vim_keys` on you get normal, insert, visual and replace modes, and the whole vocabulary
composes as you expect. Counts multiply, so `2d3w` deletes six words. Registers work with `"a`.
Text objects (`ci"`, `da(`) and the dot-repeat are there.

Two things differ from Vim on purpose. Vertical motion follows **display** rows, so `j` and `k` move
through soft-wrapped lines the way they look on screen rather than jumping a whole logical line at a
time. And only far-reaching moves enter the jumplist — `gg`, `G`, `}`, `%`, `gd`, `n`, `]d` — so
<kbd>ctrl</kbd>+<kbd>o</kbd> takes you somewhere useful instead of one word back.

The syntax tree adds four text objects of its own: `f` for a function, `t` for a type (class,
struct, enum, trait, impl, interface), `a` for an argument or parameter and `c` for a comment. They
compose like any other — `daf`, `cif`, `yat`, `cia`, `dac` — and in visual mode `vaf` selects the
function. `t` is Vim's tag object, which DaCode never had, so nothing changes meaning. They read the
tree only while it describes the text as it is: in the moment after an edit, before the background
parse has answered, they do nothing and say so in a message, rather than act on a range that has
since moved.

A count reaches outwards: `2af` is the function around the function you are in — inside a closure,
the function holding it — and `d2af`, `2daf` and `v3at` work the way `d2w` does. For arguments the
count climbs lists the same way, so `2aa` on an argument of an inner call is that whole call, as an
argument of the outer one; it never means "two arguments", which is how targets.vim and
nvim-treesitter-textobjects read it too. A count past the outermost one finds nothing and says so,
rather than taking the outermost instead. Comments do not nest, so `2ac` finds nothing.

With Vim keys off, the same objects are <kbd>alt</kbd>+<kbd>f</kbd>, <kbd>alt</kbd>+<kbd>t</kbd> and
<kbd>alt</kbd>+<kbd>a</kbd>, and <kbd>shift</kbd>+<kbd>alt</kbd>+<kbd>→</kbd> grows the selection to
the enclosing piece of syntax, as it does in VSCode. <kbd>shift</kbd>+<kbd>alt</kbd>+<kbd>←</kbd>
shrinks it back the way it came — expand three times and shrink three times and the caret is on the
character it started on. A selection made some other way, or one an edit has gone through since,
has no way back to retrace, so shrinking steps to the piece of syntax under the cursor inside it.
All five are in the Selection menu.

## The keys that are DaCode's own

These are the editor's, not Vim's, and they work in both keymaps:

| Key | Does |
|---|---|
| <kbd>F1</kbd> | every key that works right now, filterable |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> | every command, searchable, with its key beside it |
| <kbd>F10</kbd> · right-click | open the menu bar (it is always on the title bar to click) · context menu |
| <kbd>ctrl</kbd>+<kbd>p</kbd> · <kbd>ctrl</kbd>+<kbd>b</kbd> | file palette · buffer palette |
| <kbd>ctrl</kbd>+<kbd>e</kbd> | file tree |
| <kbd>ctrl</kbd>+<kbd>`</kbd> · <kbd>ctrl</kbd>+<kbd>t</kbd> | the terminal — and back out of it |
| <kbd>ctrl</kbd>+<kbd>,</kbd> | settings |
| <kbd>ctrl</kbd>+<kbd>h</kbd> | find and replace in this file |
| <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>o</kbd> | open a folder |
| <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>v</kbd> · <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>s</kbd> | split beside · split below |
| <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>w</kbd> · <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>c</kbd> | move between panes · close the pane |
| <kbd>ctrl</kbd>+<kbd>s</kbd> · <kbd>ctrl</kbd>+<kbd>q</kbd> | save · quit |
| <kbd>ctrl</kbd>+<kbd>w</kbd> · <kbd>ctrl</kbd>+<kbd>F4</kbd> | close this file |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>t</kbd> | go to a symbol anywhere in the project (`gS` with Vim keys) |
| `gb`, or <kbd>alt</kbd>+<kbd>b</kbd> without Vim keys | who changed each line, as a column in the gutter |
| `gB`, or <kbd>shift</kbd>+<kbd>alt</kbd>+<kbd>b</kbd> without Vim keys | the whole commit that changed this line |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>g</kbd> | source control — staging, committing, branches; see [its guide](source-control.md) |

Plain **find** is one of the few that differ: `/` with the grammar on, <kbd>ctrl</kbd>+<kbd>f</kbd>
with it off. Project-wide search is <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>f</kbd> without the
grammar and `:grep pattern` with it.

Turning the grammar off does not cost you the file tree or the split commands: the
<kbd>ctrl</kbd>+<kbd>k</kbd> prefix still waits for its second key, and the which-key popup still
says what can follow.

**Vim's window commands are after <kbd>ctrl</kbd>+<kbd>k</kbd>, not <kbd>ctrl</kbd>+<kbd>w</kbd>.**
<kbd>ctrl</kbd>+<kbd>w</kbd> closes the file, in every mode of both keymaps — insert mode included,
where Vim would delete a word (<kbd>ctrl</kbd>+<kbd>backspace</kbd> does that) — because it is what
every tabbed application on the desktop closes a tab with. The second keys did not change:
`ctrl-w v` is <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>v</kbd>, `ctrl-w s` is <kbd>ctrl</kbd>+<kbd>k</kbd>
<kbd>s</kbd>, and so on through `c`, `q`, `w`, `W` and `h` `j` `k` `l`. Settings, which
<kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>s</kbd> also opened, is <kbd>ctrl</kbd>+<kbd>,</kbd>.
<kbd>ctrl</kbd>+<kbd>t</kbd> is the terminal's second key, and the project's symbol picker it used
to open is <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>t</kbd>.

A few commands have no key at all in either keymap — toggling the theme and changing the font size
among them, and installing a language server. They are in the command palette, which is why the
palette exists.

### The terminal

<kbd>ctrl</kbd>+<kbd>`</kbd> opens a terminal under the code, running your `$SHELL` (PowerShell on
Windows) in the project's root, and gives it the keys. The same key takes them back and hides the
panel; the shell keeps running behind it, and opening the panel again finds it where you left it.
With the panel showing and the keys elsewhere, <kbd>ctrl</kbd>+<kbd>`</kbd> gives the terminal the
keys without hiding anything. It is also under **Terminal** in the menu bar, and the ▶ run button
types the project's run command into it.

While the terminal has the keys, **every key is the shell's** — <kbd>ctrl</kbd>+<kbd>w</kbd> deletes a
word, <kbd>ctrl</kbd>+<kbd>p</kbd> recalls the last command, <kbd>ctrl</kbd>+<kbd>c</kbd> interrupts,
<kbd>Esc</kbd> is Escape — with these exceptions:

| Key | Does, from the terminal |
|---|---|
| <kbd>ctrl</kbd>+<kbd>`</kbd> · <kbd>ctrl</kbd>+<kbd>t</kbd> | hide the terminal and give the keys back to the code |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>c</kbd> | copy the mouse selection |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>v</kbd> | paste |
| <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> | the command palette, from which everything else is reachable |
| <kbd>ctrl</kbd>+<kbd>pagedown</kbd> · <kbd>ctrl</kbd>+<kbd>pageup</kbd> | the next or previous terminal tab — inside Vim these used to be its own tab keys; `gt` and `gT` still are |

Clicking the code also gives the keys back, and clicking the terminal hands them to it. Typing
`exit` closes that terminal.

**Tabs.** The header's **+** starts another shell in a tab of its own, beside the ones already
running; each tab is named by the title its shell sets, or by the shell's name until it sets one.
Click a tab to show it, or its **×** to end that shell; the header's own **×** only hides the panel,
and everything keeps running behind it. Closing the last tab hides the panel. The same commands —
new, next, previous, end — are under **Terminal** in the menu bar and in the command palette.

**The mouse.** Drag over the text to select it; a double-click selects a word and a triple-click a
line, and shift-click extends the selection. The selection stays on its text as the view scrolls,
and goes when the program clears the screen under it. <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>c</kbd>
copies it, and a right-click offers copy, paste, a new terminal and ending this one. The wheel
scrolls the history; inside a full-screen program such as `less` it scrolls that program instead.

A program that asks for the mouse — `htop`, `vim` with `mouse=a`, `tmux`, `less --mouse` — gets it:
clicks, drags, releases and the wheel go to the program, not to a selection or the context menu.
Hold <kbd>shift</kbd> to select and scroll anyway, as in every other terminal.

**Its height.** Drag the panel's top edge — the pointer turns into up-and-down arrows over it — to
make it taller or shorter. It stops at about four rows of terminal and about six of code, and the
height is remembered per project, with the open files.

Not there yet: splitting a terminal, dragging a tab to reorder it, and reporting the mouse's
movement to a program while no button is held.

### Source control

<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>g</kbd> opens the source-control view with the caret in its
commit message box, as in VSCode, and closes it again; Vim's own <kbd>ctrl</kbd>+<kbd>g</kbd> is
left alone. In the box <kbd>enter</kbd> is a new line and <kbd>ctrl</kbd>+<kbd>enter</kbd> commits.
In its list <kbd>space</kbd> or <kbd>s</kbd> stages, <kbd>u</kbd> unstages, <kbd>enter</kbd> opens
the diff and <kbd>o</kbd> the file. Every other git command — pull, push, checkout, stash — has a
button there and a `git:` entry in the command palette rather than a key of its own; bind one in
`init.lua` by its name if you want one. [Source control](source-control.md) has the rest.

### Scrolling and zooming

A touchpad scrolls the code by the pixel, following the fingers; a wheel glides three lines a notch
and always comes to rest on a whole line. With soft wrap off, a sideways swipe — or
<kbd>shift</kbd> with a wheel — scrolls across.

<kbd>ctrl</kbd> with the wheel or a two-finger scroll changes the font size a step at a time, and so
does pinching where the window system reports a pinch: macOS today. winit, which DaCode's window is
built on, does not deliver the gesture on Wayland, X11 or Windows, so on Linux and Windows
<kbd>ctrl</kbd>-scroll is the way to zoom.

## Rebinding

### On the Keys page

Open the settings tab (<kbd>ctrl</kbd>+<kbd>,</kbd>) at **Keys**, click a command — or select it
and press <kbd>Enter</kbd> — and press the keys you want. That is the whole of it:

- **Any shortcut**: a key held with <kbd>ctrl</kbd> or <kbd>alt</kbd> (shift too, if you like), or a
  function key. A letter or an arrow on its own is typing, or a word of Vim's grammar, and is
  refused with a sentence; `dacode.map` below binds those, per mode.
- **Two keys, if the first is <kbd>ctrl</kbd>+<kbd>k</kbd>.** It is already the prefix of
  <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>o</kbd> and the pane commands, so the row waits for a second key
  after it — <kbd>ctrl</kbd>+<kbd>k</kbd> <kbd>ctrl</kbd>+<kbd>x</kbd>. No other key starts a
  sequence: it would have to be held back to see what followed.
- <kbd>Esc</kbd> cancels; <kbd>Backspace</kbd> or <kbd>Delete</kbd> leaves the command with no key.
  Clicking anywhere also cancels.
- **A key another command has is asked about first.** The row says which command has it; press it
  again to move it, or <kbd>Esc</kbd> to leave things as they were. While a row waits, no key runs
  anything — <kbd>ctrl</kbd>+<kbd>w</kbd> does not close the tab you are on.

A new key **replaces** the command's built-in shortcuts — give toggle-comment
<kbd>ctrl</kbd>+<kbd>alt</kbd>+<kbd>m</kbd> and <kbd>ctrl</kbd>+<kbd>/</kbd> stops toggling
comments — in both grammars and every mode. Vim's own keys for it stay: undo given
<kbd>ctrl</kbd>+<kbd>alt</kbd>+<kbd>z</kbd> still undoes on `u`, because `u` is part of a language
rather than a shortcut that could be moved. A changed row shows a ● and a ⟲ that puts it back
(<kbd>Delete</kbd> on the row does the same), and **Reset every key to the built-in** at the top
of the page puts back the lot.

It applies at once, and everywhere a key is printed — the palette, F1, the menus, the which-key
popup, the toolbar's tooltips — shows the new one. A moment later it is written into `init.lua` as
a block of its own, in the form you would write by hand:

```lua
-- dacode: keys set on the Keys page (ctrl-,). Edit freely — rewritten when a key changes there.
dacode.keys {
  close_buffer = "<c-s-w>",
  toggle_comment = "<c-k><c-c>",
  quit = "",
}
```

One command a line, by the name `dacode.actions` lists: a key, a list of them
(`{ "<a-b>", "<f6>" }`), or `""` for none. Hand-written `dacode.keys` calls work the same way, and
the file stays the one place keys are kept — the page reads its block back at the next start
through the same reader. Search on the page finds a command by its key in any spelling:
`ctrl-w`, `Ctrl+W` and `<c-w>` all find "close this file".

### In `init.lua`, per mode

`dacode.map(mode, keys, action)` in `init.lua`:

```lua
dacode.set { leader = "<space>" }

dacode.map("normal", "<leader>w", "save")
dacode.map("insert", "<c-s>", "save")
dacode.map("v", "<leader>y", "copy")
```

The mode is `normal`, `insert`, `visual` or `replace`; the single letters `n`, `i`, `v` and `r` also
work. The keys are a *sequence*, not a single key. The action is one of the names in `dacode.actions`,
which is a plain Lua array you can print from your own config — it is filtered through the same
lookup the binder uses, so a name that no longer resolves disappears from the list rather than being
advertised and then rejected. A typo earns a did-you-mean at startup, not a key that silently does
nothing.

Four rules that are not obvious:

**Later wins.** Binding the same mode and sequence twice behaves like assignment. The alternative —
accumulating a binding that can never fire — is worse.

**`leader` applies to bindings made afterwards.** It is expanded when the binding is created, so
change it before the `dacode.map` calls that use it.

**Bindings are consulted before the built-in grammar, but only when no command is half-typed.** So
`d<leader>` stays the grammar's business, and a config file can never leave an operator dangling.
With Vim's keys off, the mode you are always in is `insert`, so that is the mode to bind. The Keys
page's keys are asked before `dacode.map`'s, and unlike them they are shown wherever a key is.

**In insert mode only modified and named keys can be bound.** A binding on a bare letter would have
to hold that letter back to see what came next, and typing would stutter. <kbd>ctrl</kbd>+<kbd>s</kbd>
and <kbd>F5</kbd> are fine; `jk` is not.

Operators and text objects are deliberately not in the vocabulary. `d` is not an action, it is half
of one, and a binding that produced a dangling operator would leave the grammar mid-command with no
way out. Motions are reachable through the `move_*` names, which collapse to a caret the way a plain
motion does.

### Macros survive rebinding

Everything user-visible in DaCode is an `Action`: keys resolve to one, the palette lists them, config
binds them, and **macros record them**. A recorded macro holds actions rather than keystrokes, so
rebinding a key afterwards does not break a macro that used it, and a replay is one undo step. Record
with `q` and a letter, stop with `q`, replay with `@` and that letter.

The full `dacode.map` reference — every field, every mode spelling, and the exact resolution order — is
in [the settings reference](../reference/settings.md#vexmapmode-keys-action).

## Finding a key you do not know

Three paths, and they all read the same table in the source, so they cannot disagree with each other
or with what the editor actually does.

<kbd>F1</kbd> lists every key that works **right now**, in the keymap you actually chose, and filters
as you type. It is the one to reach for when you have just switched keymaps.

<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> is every command, searchable, each with its key beside
it. Use it twice for the same thing and you will have learned the shortcut.

A **half-typed chord** shows what can follow it. Press <kbd>ctrl</kbd>+<kbd>k</kbd> and wait: the
which-key popup lists the keys that complete it — a folder, and the window commands. Same for `g`,
`z` and `]` with the grammar on. It sits at the top left of the code area rather than following the
cursor — a popup that moved as you typed would be harder to read than a still one.

The toolbar, the menu bar and the right-click menu all print the shortcut for whatever they do, so
the mouse teaches the keyboard and then gets out of the way.

## See also

- [CHEATSHEET.md](../../CHEATSHEET.md) — every key in both keymaps, checked against the real grammar
  on every build
- [Configuration](configuration.md) — where `init.lua` lives, and what happens when it and the
  settings tab disagree
- [Settings reference](../reference/settings.md) — `vim_keys`, `leader`, `line_numbers` and the rest
- [Language servers](language-servers.md) — what <kbd>F2</kbd>, <kbd>F12</kbd> and
  <kbd>ctrl</kbd>+<kbd>space</kbd> need in order to do anything
