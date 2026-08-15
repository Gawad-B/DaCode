#!/bin/sh
# Removes a per-user install made by install.sh. Pass the same prefix it was given:
#   ./uninstall.sh /usr/local
#
# The icon cache is the reason this script exists rather than being a line of documentation saying
# "delete three files". `gtk-update-icon-cache` writes an index of what a theme directory contains,
# and GTK *trusts* that index: an entry for `vex` makes it hand out
# `$prefix/share/icons/hicolor/256x256/apps/vex.png` without asking whether the file is still there.
# So deleting the icons by hand leaves a cache that promises an icon nobody can open — and because
# ~/.local comes before /usr in the icon search path, that stale promise also shadows the icon a
# later .rpm or .deb install put in the right place. The symptom is a blank tile with the name under
# it, in the launcher and beside the running window, with a correctly installed icon sitting
# untouched one directory down the search path. Refreshing the cache after the removal is what makes
# it go away.
set -eu

prefix="${1:-$HOME/.local}"
here="$(cd "$(dirname "$0")" && pwd)"

rm -f "$prefix/bin/vex"
rm -f "$prefix/share/applications/vex.desktop"

# Only the files this install put there, named from the same tree install.sh reads, so a shared
# hicolor directory keeps everybody else's icons.
find "$here/icons" -type f | while read -r icon; do
    rm -f "$prefix/share/${icon#"$here/"}"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true
fi

echo "removed vex from $prefix"
