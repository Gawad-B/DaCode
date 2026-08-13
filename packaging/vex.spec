# Builds an installable rpm from an already-compiled binary.
#
# `%build` is empty on purpose: the binary is compiled by cargo before rpmbuild runs. Building Rust
# inside the rpm sandbox would mean vendoring every crate into the source tarball, which buys
# reproducibility this project does not yet need and costs a workflow nobody would run locally.

%global _build_id_links none
%define _binaries_in_noarch_packages_terminate_build 0

Name:           vex
Version:        %{?vex_version}%{!?vex_version:0.1.0}
Release:        1%{?dist}
Summary:        A modal code editor that never stalls on your keystroke

License:        MIT
URL:            https://github.com/Gawad-B/vex
BuildArch:      x86_64

# `git` for the diff gutter and `zenity` for the folder chooser: both make features work rather than
# being needed to start, so they are recommendations, not requirements.
Recommends:     git
Recommends:     zenity
Requires:       hicolor-icon-theme

%description
vex is a modal editor with Vim's grammar, tree-sitter highlighting and
language-server support, drawn directly on the GPU with wgpu. Vim keys can be
turned off for people who would rather have a conventional editor, and the
settings panel covers everything without editing a config file.

%install
install -Dm755 %{_sourcedir}/vex %{buildroot}%{_bindir}/vex
install -Dm644 %{_sourcedir}/vex.desktop %{buildroot}%{_datadir}/applications/vex.desktop
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 %{_sourcedir}/icons/hicolor/${size}x${size}/apps/vex.png \
        %{buildroot}%{_datadir}/icons/hicolor/${size}x${size}/apps/vex.png
done
install -Dm644 %{_sourcedir}/icons/hicolor/scalable/apps/vex.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/vex.svg
install -Dm644 %{_sourcedir}/README.md %{buildroot}%{_docdir}/vex/README.md
install -Dm644 %{_sourcedir}/CHEATSHEET.md %{buildroot}%{_docdir}/vex/CHEATSHEET.md
install -Dm644 %{_sourcedir}/init.lua %{buildroot}%{_docdir}/vex/examples/init.lua

%files
%{_bindir}/vex
%{_datadir}/applications/vex.desktop
%{_datadir}/icons/hicolor/*/apps/vex.*
%doc %{_docdir}/vex/README.md
%doc %{_docdir}/vex/CHEATSHEET.md
%doc %{_docdir}/vex/examples/init.lua

%changelog
* Fri Aug 14 2026 vex - 0.1.1-1
- Windows: built-in theme names load again, the folder chooser can leave the folder
  it opened in, and helper processes no longer flash console windows.

* Thu Aug 13 2026 vex - 0.1.0-1
- First packaged release.
