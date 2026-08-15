#!/bin/sh
# Builds a portable tarball: the binary, the desktop entry, icons and documentation.
#
# For people who want to run vex without a package manager, and for distributions neither .deb nor
# .rpm covers. Laid out so `install.sh` inside it works from wherever it is unpacked.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/vex/Cargo.toml" | head -1)"
out="${1:-$root/target/packages}"

[ -x "$root/target/release/vex" ] || { echo "build it first: cargo build --release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/vex-$version-x86_64-linux"

install -Dm755 "$root/target/release/vex" "$stage/bin/vex"
install -Dm644 "$here/linux/vex.desktop" "$stage/share/applications/vex.desktop"
find "$here/linux/icons" -type f | while read -r icon; do
    install -Dm644 "$icon" "$stage/share/${icon#"$here/linux/"}"
done
install -Dm644 "$root/README.md" "$stage/README.md"
install -Dm644 "$root/CHEATSHEET.md" "$stage/CHEATSHEET.md"
install -Dm644 "$root/CHANGELOG.md" "$stage/CHANGELOG.md"
# The README links into this tree relatively, so the two travel together or the links are dead.
find "$root/docs" -type f | while read -r page; do
    install -Dm644 "$page" "$stage/docs/${page#"$root/docs/"}"
done
install -Dm644 "$root/LICENSE" "$stage/LICENSE" 2>/dev/null || true
install -Dm644 "$root/examples/init.lua" "$stage/examples/init.lua"

# An installer that works from inside the unpacked directory.
cat > "$stage/install.sh" <<'INNER'
#!/bin/sh
# Installs this build for the current user. Pass a prefix to install elsewhere.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${1:-$HOME/.local}"

install -Dm755 "$here/bin/vex" "$prefix/bin/vex"

# The desktop entry gets an absolute Exec: ~/.local/bin is on an interactive shell's PATH but routinely
# absent from the desktop session's, so a launcher-clicked `Exec=vex` fails while the same command
# works in a terminal.
mkdir -p "$prefix/share/applications"
entry="$(mktemp)"
sed -e "s|^Exec=vex|Exec=$prefix/bin/vex|" \
    -e "s|^Icon=vex|TryExec=$prefix/bin/vex\nIcon=vex|" \
    "$here/share/applications/vex.desktop" > "$entry"
install -Dm644 "$entry" "$prefix/share/applications/vex.desktop"
rm -f "$entry"

find "$here/share/icons" -type f | while read -r icon; do
    install -Dm644 "$icon" "$prefix/share/${icon#"$here/share/"}"
done

command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "$prefix/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -f -t "$prefix/share/icons/hicolor" 2>/dev/null || true

echo "installed vex to $prefix"
INNER
chmod +x "$stage/install.sh"

mkdir -p "$out"
tarball="$out/vex-$version-x86_64-linux.tar.gz"
rm -f "$tarball"
(cd "$work" && tar czf "$tarball" "vex-$version-x86_64-linux")
echo "$tarball"
