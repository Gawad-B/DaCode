# Source control

Stage, commit, push and switch branches without leaving the editor — the view is shaped after
VSCode's, so if you have used that one you already know where everything is.

Open it with <kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>g</kbd> (both keymaps), the branch icon in the
activity bar, or **Git ▸ show source control** in the menu bar. The same key closes it again.

DaCode runs the `git` on your `PATH` for everything here, with your own configuration, hooks and
credential helpers. Nothing is reimplemented, so what the view says is what `git` would say.

## The view

From the top:

- **SOURCE CONTROL**, with three buttons: **✓ Commit**, **Refresh**, and **…** — a menu of
  everything that has no button of its own (pull, push, sync, fetch, checkout, create branch,
  stash, pop stash, show history, commit (amend), discard all changes).
- **The branch**, and how far it is from its upstream: **↓ 1** commit to pull, **↑ 2** to push.
  Click the branch to switch branches, the arrows to pull or push. A detached `HEAD` is named by its
  commit.
- **The message box.** Opening the view puts the caret here. Type the message; <kbd>enter</kbd>
  starts a new line, <kbd>ctrl</kbd>+<kbd>enter</kbd> commits, <kbd>tab</kbd> or <kbd>↓</kbd> from
  the last line moves to the list, <kbd>ctrl</kbd>+<kbd>v</kbd> pastes into the box (not into the
  file behind the view).
- **The Commit button.**
- **The changes**, in up to three groups, each with its count and each folding at its heading:
  - **Merge Changes** — files a merge, rebase or stash pop stopped on. Fix the conflict in the file,
    then stage it to mark it resolved.
  - **Staged Changes** — what the next commit contains.
  - **Changes** — everything else that differs, untracked files included.

  A file staged and then edited again is in both Staged and Changes, because each half is its own
  change. Each row is the file's name, its directory dimmed, and a letter coloured as the gutter
  colours it: **M** modified, **A** added, **D** deleted, **R** renamed, **U** untracked, **C**
  conflicted.

Hover a row, or select it, for its buttons: open the file, **↶** discard, **+** stage or **−**
unstage. A heading has the group-wide ones: stage all, unstage all, discard all. Every button names
its command and key in a tooltip.

### Keys, while the list has them

| Key | Does |
|---|---|
| <kbd>↑</kbd> <kbd>↓</kbd> · <kbd>pageup</kbd> <kbd>pagedown</kbd> | move through the rows — <kbd>↑</kbd> from the first goes back to the message box |
| <kbd>enter</kbd> | open the change's diff, or fold or unfold a group |
| <kbd>space</kbd> · <kbd>s</kbd> | stage (on a heading: the whole group) |
| <kbd>u</kbd> | unstage (on a heading: everything) |
| <kbd>o</kbd> | open the file itself |
| <kbd>←</kbd> <kbd>→</kbd> | fold or unfold the group under a heading |
| <kbd>tab</kbd> | back to the message box |
| <kbd>esc</kbd> | give the keys back to the code |

A click on a file row opens its diff and leaves the keys in the list, so you can walk the changes
with the arrows and <kbd>enter</kbd>.

## Committing

Type a message and press **Commit** or <kbd>ctrl</kbd>+<kbd>enter</kbd>.

**With nothing staged**, the first press asks — *nothing is staged — press again to commit every
change* — and the second stages every change, untracked files included, and commits them. That is
VSCode's "smart commit", set to stage everything; it asks first because it commits more than you
selected.

**Commit (amend)**, in the **…** menu, folds what is staged into the last commit. With the message
box empty it keeps the last commit's message.

A commit with conflicts still listed is refused: stage each file once you have fixed it.

## Diffs

Clicking a change opens a **diff tab** beside your files. It is the unified diff `git diff` prints,
read-only, with added lines washed green, removed lines red and each hunk's `@@` line in the accent,
and the same colours as bars in the gutter:

| Opened from | It shows | Which is |
|---|---|---|
| Changes | the index against the working tree | `git diff -- FILE` |
| Staged Changes | `HEAD` against the index | `git diff --cached -- FILE` |
| an untracked file | the whole file, every line added | `git diff --no-index /dev/null FILE` |
| Merge Changes | the combined diff of the conflict | `git diff -- FILE` |

The tab is named for the file and the side — `main.rs (working tree)`, `main.rs (staged)` — and
opening the same change again goes back to it with fresh text. It scrolls, searches and copies like
any buffer, and `]c`/`[c` (<kbd>F7</kbd>) step between its hunks. Typing into it does nothing and
says why: it is git's answer, not a file. Save it with **Save As** and it becomes an ordinary,
editable file — a patch. A diff over 2 MB is cut at a line and says so.

**Git ▸ open changes**, or a right-click on the code or a tree row, opens the diff of the file you
are looking at: its unstaged changes if it has any, its staged ones if not.

It is a unified diff, not VSCode's side-by-side one. The gutter's marks in the file itself still show
every change against `HEAD` as you type.

## Branches

Click the branch — in the view, or at the left of the status line — or run **git: checkout to…**.
A picker opens over the code: **Create new branch** first, then your branches (most recently
committed first, the current one marked), then the remotes'. Type to filter; with a name typed,
the first row creates a branch of that name.

Choosing a remote branch checks it out as a new local branch tracking it. When your changes would
be overwritten, git refuses, and its reason — naming the files — is what you are told.

## Pull, push, fetch and sync

The arrows by the branch pull and push; the **…** menu has those, **Fetch**, and **Sync** (pull,
then push). Pushing a branch with no upstream publishes it to `origin` (or the only remote there
is), as VSCode's "Publish Branch" does.

**Nothing waits for a password.** git runs with terminal prompts off and ssh in batch mode (unless
you configured `core.sshCommand` or `GIT_SSH_COMMAND` yourself), because a prompt nobody can see
would leave the view saying "pushing…" forever. If the remote needs you to sign in, the push fails at
once and says so: run it once in the terminal (<kbd>ctrl</kbd>+<kbd>t</kbd>), where git *can* ask,
and your credential helper remembers it for next time.

## Stash, history and the rest

**Stash** saves your tracked changes and cleans the tree; **Pop stash** puts the newest back.
**Show history** opens the last 500 commits — hash, date, author, subject — in a read-only tab.
**Discard all changes** puts every tracked file back and deletes every untracked one; ignored files
are never touched.

Discarding is the one thing here you cannot undo, so every discard — a row's **↶**, a heading's, or
the command — asks to be pressed again, and says what it is about to throw away. Pressing it on a
different file starts the question over.

## Keeping up with changes made elsewhere

The view refreshes after anything it does, when you save, when the window comes back into focus,
and when the repository changes on disk — a commit in a terminal, a checkout in another tool. That
last is a watch on `.git`'s `HEAD`, index and refs, so it costs nothing while nothing happens, and it
waits a moment for git to finish before asking.

When `HEAD` moves, **the blame and the gutter are asked again too**, so a commit made in a terminal
shows in the blame column and the inline note as soon as it is made, not when you next save.

## No repository

In a folder git does not track, the view says so and offers **Initialize Repository**, which runs
`git init` there. Without `git` installed it says that instead.

## Every command

All of these are in the command palette (<kbd>ctrl</kbd>+<kbd>shift</kbd>+<kbd>p</kbd> — type
`git:`), under **Git** in the menu bar, and bindable in `init.lua` by the name on the right:

| Palette | Name |
|---|---|
| show source control | `source_control` |
| git: commit · git: commit (amend) | `git_commit` · `git_commit_amend` |
| git: refresh | `git_refresh` |
| git: stage file · git: unstage file · git: discard changes | `git_stage` · `git_unstage` · `git_discard` |
| git: stage all changes · git: unstage all changes · git: discard all changes | `git_stage_all` · `git_unstage_all` · `git_discard_all` |
| git: pull · git: push · git: sync (pull, then push) · git: fetch | `git_pull` · `git_push` · `git_sync` · `git_fetch` |
| git: stash · git: pop stash | `git_stash` · `git_pop_stash` |
| git: checkout to… · git: create branch… | `git_checkout` · `git_create_branch` |
| git: show history · git: open changes · git: open file | `git_history` · `git_open_changes` · `git_open_file` |
| git: initialize repository · git: more actions… | `git_init` · `git_menu` |

The per-file ones act on the row selected in the view while it has the keys, and otherwise on the
file selected in the tree or the one you are looking at — which is how **git: stage file** works
from the code's right-click menu.

## Not here yet

- **Side-by-side diffs**, and staging part of a file (a hunk or a line) from the diff.
- **A commit graph.** History is a list.
- **Several repositories in one folder**, and submodules as repositories of their own: the view is
  the repository the project folder is in.
- **Stash with untracked files**, and choosing which stash to pop.
- **Cancelling a pull or push** that is taking a long time. It runs off the editor's thread, so
  nothing freezes — it just has to finish or fail.
- A status of more than 5,000 changed files lists the first 5,000 and says how many it left out;
  **stage all** and **discard all** still act on every one of them.
