#!/bin/sh
# Type-checks the editor for Apple Silicon from a Linux machine, so code behind
# `cfg(target_os = "macos")` is compiled before CI's Mac runner is the first to see it.
#
#     packaging/macos/check-from-linux.sh
#
# There is no Mac SDK here, and tree-sitter's grammars and Lua are C. The C compiler and `ar`
# given to their build scripts below only write empty files: `cargo check` never links, so
# nothing reads them, and the Rust — all of it, Mac-only code included — is type-checked for
# real. It proves the code compiles; it proves nothing about how it runs.
#
# Into `target/mac-check` on disk, with two jobs: a build directory under `/tmp` lives in memory
# on machines where `/tmp` is a RAM disk, and this one is close to a gigabyte.
set -eu

root="$(cd "$(dirname "$0")/../.." && pwd)"
fake="$root/target/fakecc"
mkdir -p "$fake"
cat > "$fake/cc" <<'EOF'
#!/bin/sh
out=""
while [ $# -gt 0 ]; do
    case "$1" in -o) out="$2"; shift ;; -o*) out="${1#-o}" ;; esac
    shift
done
[ -n "$out" ] && : > "$out"
exit 0
EOF
cat > "$fake/ar" <<'EOF'
#!/bin/sh
for arg in "$@"; do case "$arg" in *.a) printf '!<arch>\n' > "$arg" ;; esac; done
exit 0
EOF
chmod +x "$fake/cc" "$fake/ar"

rustup target add aarch64-apple-darwin >/dev/null
CC_aarch64_apple_darwin="$fake/cc" \
AR_aarch64_apple_darwin="$fake/ar" \
CARGO_TARGET_DIR="$root/target/mac-check" \
    cargo check -j 2 -p dacode --lib --bins --tests --target aarch64-apple-darwin \
    --manifest-path "$root/Cargo.toml"
