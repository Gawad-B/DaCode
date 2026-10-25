#!/bin/sh
# Builds dacode_<version>_amd64.deb.
#
# Assembled with `ar` and `tar` rather than `dpkg-deb`, because a .deb *is* an ar archive of three
# members and building it by hand means this script runs on Fedora, on Arch, and in a container that
# has no Debian tooling — which is where release builds usually happen.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$here/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/dacode/Cargo.toml" | head -1)"
arch=amd64
out="${1:-$root/target/packages}"

binary="$root/target/release/dacode"
[ -x "$binary" ] || { echo "build it first: cargo build --release" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The payload, laid out as it will be installed.
install -Dm755 "$binary" "$work/data/usr/bin/dacode"
install -Dm644 "$here/linux/dacode.desktop" "$work/data/usr/share/applications/dacode.desktop"
find "$here/linux/icons" -type f | while read -r icon; do
    install -Dm644 "$icon" "$work/data/usr/share/${icon#"$here/linux/"}"
done
install -Dm644 "$root/README.md" "$work/data/usr/share/doc/dacode/README.md"
install -Dm644 "$root/CHEATSHEET.md" "$work/data/usr/share/doc/dacode/CHEATSHEET.md"
install -Dm644 "$root/CHANGELOG.md" "$work/data/usr/share/doc/dacode/CHANGELOG.md"
install -Dm644 "$root/examples/init.lua" "$work/data/usr/share/doc/dacode/examples/init.lua"

# The documentation tree, laid out exactly as it is in the repository. The README links to it with
# relative paths, so shipping the one without the other installs a front page whose every link is
# dead — and the person reading it offline is the one who most needed it to work.
find "$root/docs" -type f | while read -r page; do
    install -Dm644 "$page" "$work/data/usr/share/doc/dacode/docs/${page#"$root/docs/"}"
done

size="$(du -ks "$work/data" | cut -f1)"

mkdir -p "$work/control"
cat > "$work/control/control" <<EOF
Package: dacode
Version: $version
Section: editors
Priority: optional
Architecture: $arch
Installed-Size: $size
Maintainer: dacode
Depends: libc6
Recommends: git, zenity
Description: A modal code editor that never stalls on your keystroke
 dacode is a modal editor with Vim's grammar, tree-sitter highlighting and
 language-server support, drawn directly on the GPU. Vim keys can be turned
 off for people who would rather have a conventional editor.
EOF

# Installing an icon is not the same as the desktop seeing it: GTK reads a *cache* per icon theme
# directory, and a menu entry whose icon is missing from that cache falls back to a blank tile —
# which looks exactly like shipping no icon at all. Every command is allowed to be absent, because a
# missing menu entry must never fail the installation of an editor that runs fine without one.
for script in postinst postrm; do
    cat > "$work/control/$script" <<'EOF'
#!/bin/sh
set -e
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications 2>/dev/null || true
fi
exit 0
EOF
    chmod 755 "$work/control/$script"
done

# Checksums, which `dpkg --verify` and several package managers read. After the scriptlets, which
# live in the control member and must not be listed here.
(cd "$work/data" && find . -type f -exec md5sum {} + | sed 's|\./||') > "$work/control/md5sums"

echo "2.0" > "$work/debian-binary"
(cd "$work/control" && tar --numeric-owner --owner=0 --group=0 -czf ../control.tar.gz .)
(cd "$work/data" && tar --numeric-owner --owner=0 --group=0 -czf ../data.tar.gz .)

mkdir -p "$out"
package="$out/dacode_${version}_${arch}.deb"
rm -f "$package"
# The member order is defined by the format: debian-binary, then control, then data.
(cd "$work" && ar rc "$package" debian-binary control.tar.gz data.tar.gz)

echo "$package"
