#!/bin/sh
# Install Sakshi on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/maxsignalhq/Sakshi/main/install.sh | sh
#
# Downloads the release archive for this machine, checks its SHA-256 against
# the published checksum, and puts `sakshi` in ~/.local/bin. Nothing else on
# the system is changed. Settings, all optional:
#
#   SAKSHI_VERSION      tag to install, e.g. v0.1.0 (default: the latest release)
#   SAKSHI_INSTALL_DIR  where to put the binary (default: ~/.local/bin)

set -eu

REPO="maxsignalhq/Sakshi"
BASE="${SAKSHI_DOWNLOAD_BASE:-https://github.com/$REPO/releases}"
INSTALL_DIR="${SAKSHI_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf 'sakshi: %s\n' "$*"; }
fail() { printf 'sakshi: error: %s\n' "$*" >&2; exit 1; }

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    final_url() { curl -fsSLI -o /dev/null -w '%{url_effective}' "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
    final_url() { wget -q --max-redirect=5 -S --spider "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r'; }
else
    fail "need curl or wget to download Sakshi"
fi

# Which build fits this machine.
os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
    x86_64 | amd64) arch="x86_64" ;;
    arm64 | aarch64) arch="aarch64" ;;
    *) fail "no prebuilt binary for CPU '$arch'; install from source with: cargo install --git https://github.com/$REPO sakshi" ;;
esac
case "$os" in
    Darwin)
        # A shell running under Rosetta reports x86_64 on Apple Silicon.
        if [ "$arch" = "x86_64" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
            arch="aarch64"
        fi
        target="$arch-apple-darwin"
        ;;
    Linux)
        if ldd --version 2>&1 | grep -qi musl; then
            fail "prebuilt Linux binaries need glibc; on musl (e.g. Alpine) install from source with: cargo install --git https://github.com/$REPO sakshi"
        fi
        target="$arch-unknown-linux-gnu"
        ;;
    *) fail "unsupported system '$os'; on Windows use install.ps1" ;;
esac

# Which release.
version="${SAKSHI_VERSION:-}"
if [ -z "$version" ]; then
    # github.com redirects .../releases/latest to .../releases/tag/<tag>.
    latest="$(final_url "$BASE/latest" || true)"
    version="${latest##*/}"
    case "$version" in
        v[0-9]*) ;;
        *)
            # Fall back to the API if the redirect could not be followed.
            fetch "https://api.github.com/repos/$REPO/releases/latest" "${TMPDIR:-/tmp}/sakshi-latest.$$" 2>/dev/null || true
            version="$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "${TMPDIR:-/tmp}/sakshi-latest.$$" 2>/dev/null | head -n 1)"
            rm -f "${TMPDIR:-/tmp}/sakshi-latest.$$"
            ;;
    esac
    case "$version" in
        v[0-9]*) ;;
        *) fail "could not find the latest release; set SAKSHI_VERSION, e.g. SAKSHI_VERSION=v0.1.0" ;;
    esac
fi
case "$version" in v*) ;; *) version="v$version" ;; esac

name="sakshi-$version-$target"
url="$BASE/download/$version/$name.tar.gz"

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t sakshi)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading $name"
fetch "$url" "$tmp/$name.tar.gz" || fail "download failed: $url"
fetch "$url.sha256" "$tmp/$name.tar.gz.sha256" || fail "checksum download failed: $url.sha256"

expected="$(cut -d ' ' -f 1 < "$tmp/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d ' ' -f 1)"
else
    fail "need sha256sum or shasum to verify the download"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $name.tar.gz; not installing"

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
[ -f "$tmp/$name/sakshi" ] || fail "archive did not contain $name/sakshi"

mkdir -p "$INSTALL_DIR"
cp "$tmp/$name/sakshi" "$INSTALL_DIR/sakshi.new"
chmod 755 "$INSTALL_DIR/sakshi.new"
mv -f "$INSTALL_DIR/sakshi.new" "$INSTALL_DIR/sakshi"

say "installed $version to $INSTALL_DIR/sakshi"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) say "run 'sakshi' to start" ;;
    *) say "add $INSTALL_DIR to your PATH, e.g. in your shell profile: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
