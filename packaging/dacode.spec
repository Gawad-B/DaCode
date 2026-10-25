# Builds an installable rpm from an already-compiled binary.
#
# `%build` is empty on purpose: the binary is compiled by cargo before rpmbuild runs. Building Rust
# inside the rpm sandbox would mean vendoring every crate into the source tarball, which buys
# reproducibility this project does not yet need and costs a workflow nobody would run locally.

%global _build_id_links none
%define _binaries_in_noarch_packages_terminate_build 0

Name:           dacode
Version:        %{?dacode_version}%{!?dacode_version:0.1.0}
Release:        1%{?dist}
Summary:        A modal code editor that never stalls on your keystroke

License:        MIT
URL:            https://github.com/Gawad-B/dacode
BuildArch:      x86_64

# `git` for the diff gutter and `zenity` for the folder chooser: both make features work rather than
# being needed to start, so they are recommendations, not requirements.
Recommends:     git
Recommends:     zenity
Requires:       hicolor-icon-theme

%description
dacode is a modal editor with Vim's grammar, tree-sitter highlighting and
language-server support, drawn directly on the GPU with wgpu. Vim keys can be
turned off for people who would rather have a conventional editor, and the
settings panel covers everything without editing a config file.

%install
install -Dm755 %{_sourcedir}/dacode %{buildroot}%{_bindir}/dacode
install -Dm644 %{_sourcedir}/dacode.desktop %{buildroot}%{_datadir}/applications/dacode.desktop
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 %{_sourcedir}/icons/hicolor/${size}x${size}/apps/dacode.png \
        %{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/dacode.png
done
install -Dm644 %{_sourcedir}/icons/hicolor/scalable/apps/dacode.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/dacode.svg
install -Dm644 %{_sourcedir}/README.md %{buildroot}%{_docdir}/dacode/README.md
install -Dm644 %{_sourcedir}/CHEATSHEET.md %{buildroot}%{_docdir}/dacode/CHEATSHEET.md
install -Dm644 %{_sourcedir}/CHANGELOG.md %{buildroot}%{_docdir}/dacode/CHANGELOG.md
install -Dm644 %{_sourcedir}/init.lua %{buildroot}%{_docdir}/dacode/examples/init.lua

# The documentation tree. The README links into it with relative paths, so installing one without
# the other gives you a front page whose every link is dead.
mkdir -p %{buildroot}%{_docdir}/dacode
cp -r %{_sourcedir}/docs %{buildroot}%{_docdir}/dacode/
find %{buildroot}%{_docdir}/dacode/docs -type d -exec chmod 755 {} +
find %{buildroot}%{_docdir}/dacode/docs -type f -exec chmod 644 {} +

# Installing an icon is not the same as the desktop seeing it. GTK reads a *cache* built per icon
# theme directory, and a menu entry whose icon is not in that cache falls back to a blank tile — the
# same thing a missing icon looks like, which is why this is easy to ship without noticing. `|| :`
# throughout: a desktop-file or icon-cache tool that is absent must not fail the installation of an
# editor that runs perfectly well without a menu entry.
%post
touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
update-desktop-database &>/dev/null || :

%postun
if [ $1 -eq 0 ]; then
    touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
    gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :
    update-desktop-database &>/dev/null || :
fi

%posttrans
gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :

%files
%{_bindir}/dacode
%{_datadir}/applications/dacode.desktop
%{_datadir}/icons/hicolor/*/apps/dacode.*
%doc %{_docdir}/dacode/README.md
%doc %{_docdir}/dacode/CHEATSHEET.md
%doc %{_docdir}/dacode/CHANGELOG.md
%doc %{_docdir}/dacode/examples/init.lua
%doc %{_docdir}/dacode/docs

%changelog
* Sat Aug 16 2026 dacode - 0.4.3-1
- The code face is Fira Code, bundled, replacing IBM Plex Mono: the code font
  is the one thing you look at for hours, and an unfamiliar one is read as a
  mistake before it is read as a choice.
- Comments are set as code — same face, upright, same size — rather than in
  italic serif a size larger, which could not hold the monospaced column.

* Sat Aug 15 2026 dacode - 0.4.2-1
- Typing no longer smears the colours of the rest of the file: the highlight
  spans are moved with the text they describe, rather than describing where it
  used to be until the next parse lands.
- A diagnostic's underline stays under the code it is about, for the same
  reason and over the far longer wait a language server takes to answer.
- A file created outside the editor appears without reopening the folder. The
  project's directories are watched, and anything appearing in one re-walks.
- JavaScript is served as JavaScript: it shared the TypeScript entry, so every
  .js file was announced to the language server as TypeScript, which is
  indistinguishable from the server not supporting the file.

* Fri Aug 14 2026 dacode - 0.4.1-1
- The icon is visible on a light application menu: it was a near-white mark on
  transparency, which is a blank tile rather than a faint icon. The rpm now also
  refreshes the icon cache, without which a correctly installed icon still shows
  as blank.
- With Vim keys off, escape no longer enters a mode there is no way out of.
- The mouse wheel scrolls whatever is under it — the settings panel, the key
  list, the command palette, the file tree, the results and the problems list.
- A message wraps instead of stopping mid-word, and goes away when you do
  something else rather than waiting for escape.
- The toolbar's labels no longer run together.
- A language server can be installed from inside the editor, by running the
  command it was already telling you to type.

* Fri Aug 14 2026 dacode - 0.4.0-1
- Find and replace, rectangular selection, horizontal scrolling, closable tabs,
  file operations from the tree, and crash recovery for every dirty buffer.
- Completion uses what the server sent, including auto-import; go to a symbol in
  the file or the project; formatting works with no language server at all.
- Thirty-three defects found by an adversarial review, three of which could lose
  work: a panic while typing in the search box, a crash losing every unsaved
  buffer but one, and `d5G` deleting to the end of the file.

* Fri Aug 14 2026 dacode - 0.3.0-1
- Every ctrl-shift binding works again. Shift arrives applied to the character, so
  ctrl-shift-p was seen as P and never matched: the command palette, delete-line,
  select-all-occurrences, redo and project search were all dead in 0.2.0.
- The editor can be driven with a mouse: a toolbar, a right-click menu with the
  clipboard verbs in it, a settings panel that changes on a click, and a first-run
  wizard whose cards can be clicked.
- New settings: autosave, line_numbers, and save as on ctrl-shift-s.
- Starting with no argument reopens the last project rather than the working
  directory.

* Fri Aug 14 2026 dacode - 0.2.1-1
- With Vim keys off, the chrome no longer reports a mode it does not have, and the
  hint line advertises f1 and the command palette instead of Vim's keys.
- The search box steps through matches with enter, says which match you are on,
  and hands its query to a project-wide search with ctrl-shift-f.

* Fri Aug 14 2026 dacode - 0.2.0-1
- The non-modal mode is a conventional editor rather than the modal one with its
  modes removed: line moves, duplication, comment toggle, occurrence selection,
  word deletion, and a mouse that selects words, lines and extra cursors.
- Copy and paste use the system clipboard, which they did not before.
- F1 lists the keys, ctrl-shift-P every command, and there is a menu bar and a
  context menu; all of them show the shortcut for what they offer.

* Fri Aug 14 2026 dacode - 0.1.1-1
- Windows: built-in theme names load again, the folder chooser can leave the folder
  it opened in, and helper processes no longer flash console windows.

* Thu Aug 13 2026 dacode - 0.1.0-1
- First packaged release.
