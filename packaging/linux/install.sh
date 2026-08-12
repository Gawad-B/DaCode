#!/bin/sh
# Installs vex for the current user: the binary, the desktop entry and the icon theme.
#
# Per-user by default (no root, nothing outside $HOME). Pass a prefix to install elsewhere:
#   ./install.sh /usr/local
set -eu

prefix="${1:-$HOME/.local}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."

if [ ! -x "$root/target/release/vex" ]; then
    echo "build it first: cargo build --release" >&2
    exit 1
fi

install -Dm755 "$root/target/release/vex" "$prefix/bin/vex"

# The desktop entry gets an *absolute* Exec, rather than the bare `vex` the template carries.
# A per-user install lands in ~/.local/bin, which is on an interactive shell's PATH but is
# routinely absent from the desktop session's — so a launcher-clicked `Exec=vex` fails while the
# same command works in a terminal. TryExec is the same path, which also makes the entry hide
# itself if the binary is ever removed.
entry="$(mktemp)"
sed -e "s|^Exec=vex|Exec=$prefix/bin/vex|" \
    -e "s|^Icon=vex|TryExec=$prefix/bin/vex\nIcon=vex|" \
    "$here/vex.desktop" > "$entry"
install -Dm644 "$entry" "$prefix/share/applications/vex.desktop"
rm -f "$entry"

# The icon theme, every size the mark ships in. `scalable` is what modern desktops actually use;
# the rasters are for panels and docks that still pick a fixed size.
find "$here/icons" -type f | while read -r icon; do
    install -Dm644 "$icon" "$prefix/share/${icon#"$here/"}"
done

# Refresh the caches, if the tools are present. Neither is fatal: the entry works either way, it
# just may not appear in a running session until it is relaunched.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$prefix/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" || true
fi

echo "installed vex to $prefix"
