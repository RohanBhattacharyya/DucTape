#!/usr/bin/env sh
# Build and install Conductor from source on Linux.
#
#   ./scripts/install.sh                 install for this user into ~/.local
#   ./scripts/install.sh --system        install into /usr/local (uses sudo)
#   ./scripts/install.sh --no-deps       don't install system packages
#
# Run it from a checkout, or on its own to clone the repository first:
#   curl -fsSL https://raw.githubusercontent.com/Redrield/Conductor/next/scripts/install.sh | sh
#
# Installs build dependencies with your package manager (apt, dnf, pacman or zypper), a Rust
# toolchain with rustup if yours is missing or too old, and a private copy of Node.js for the
# build if yours is missing or too old.
set -eu

REPO="${CONDUCTOR_REPO:-https://github.com/Redrield/Conductor.git}"
BRANCH="${CONDUCTOR_BRANCH:-next}"
MIN_RUST="1.88"
NODE_MAJOR=22
PREFIX="$HOME/.local"
SUDO=""
INSTALL_DEPS=1

say() { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --system) PREFIX=/usr/local ;;
        --prefix) shift; PREFIX="$1" ;;
        --no-deps) INSTALL_DEPS=0 ;;
        -h|--help) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option $1" ;;
    esac
    shift
done

[ "$(uname -s)" = Linux ] || die "this script is for Linux; on macOS see the README"
if [ "$(id -u)" -ne 0 ] && has sudo; then SUDO=sudo; fi

install_deps() {
    if has apt-get; then
        say "Installing build dependencies with apt"
        $SUDO apt-get update
        $SUDO apt-get install -y build-essential pkg-config curl git \
            libwebkit2gtk-4.1-dev libgtk-3-dev libudev-dev libx11-dev
    elif has dnf; then
        say "Installing build dependencies with dnf"
        $SUDO dnf install -y gcc gcc-c++ make pkgconf-pkg-config curl git \
            webkit2gtk4.1-devel gtk3-devel systemd-devel libX11-devel
    elif has pacman; then
        say "Installing build dependencies with pacman"
        $SUDO pacman -S --needed --noconfirm base-devel curl git webkit2gtk-4.1 gtk3 systemd-libs libx11
    elif has zypper; then
        say "Installing build dependencies with zypper"
        $SUDO zypper install -y gcc gcc-c++ make pkg-config curl git \
            webkit2gtk3-devel gtk3-devel systemd-devel libX11-devel
    else
        warn "unknown package manager; install WebKitGTK 4.1, GTK 3, libudev and libX11 development files yourself"
    fi
}

version_ge() { # $1 >= $2, for dotted versions
    [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n1)" = "$2" ]
}

ensure_rust() {
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    if has rustc && version_ge "$(rustc --version | cut -d' ' -f2)" "$MIN_RUST"; then
        return
    fi
    if has rustup; then
        say "Updating the Rust toolchain"
        rustup toolchain install stable --profile minimal
        rustup default stable
    else
        say "Installing Rust with rustup (https://rustup.rs)"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        . "$HOME/.cargo/env"
    fi
}

ensure_node() {
    if has node && has npm; then
        v="$(node --version | tr -d v)"
        if version_ge "$v" 22.12 || { version_ge "$v" 20.19 && ! version_ge "$v" 21; }; then
            return
        fi
    fi
    case "$(uname -m)" in
        x86_64) arch=x64 ;;
        aarch64|arm64) arch=arm64 ;;
        *) die "no Node.js $NODE_MAJOR build for $(uname -m); install Node.js 22 yourself" ;;
    esac
    say "Downloading a private copy of Node.js $NODE_MAJOR for the build"
    base="https://nodejs.org/dist/latest-v${NODE_MAJOR}.x"
    sums="$(curl -fsSL "$base/SHASUMS256.txt")"
    file="$(printf '%s\n' "$sums" | awk -v a="linux-$arch.tar.xz" '$2 ~ a"$" {print $2; exit}')"
    [ -n "$file" ] || die "could not find a Node.js download for linux-$arch"
    curl -fsSL "$base/$file" -o "$WORK/$file"
    (cd "$WORK" && printf '%s\n' "$sums" | grep " $file\$" | sha256sum -c - >/dev/null) || die "Node.js checksum mismatch"
    tar -xJf "$WORK/$file" -C "$WORK"
    PATH="$WORK/${file%.tar.xz}/bin:$PATH"
    export PATH
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[ "$INSTALL_DEPS" = 1 ] && install_deps
has git || die "git is required"
ensure_rust
ensure_node

if [ -f "$(dirname "$0")/../Cargo.toml" ] && grep -q '^name = "conductor"' "$(dirname "$0")/../Cargo.toml"; then
    SRC="$(cd "$(dirname "$0")/.." && pwd)"
else
    say "Cloning $REPO"
    git clone --depth 1 --branch "$BRANCH" "$REPO" "$WORK/conductor"
    SRC="$WORK/conductor"
fi

say "Building Conductor (this takes a few minutes the first time)"
cd "$SRC"
cargo build --release --locked

say "Installing into $PREFIX"
if [ -w "$PREFIX" ] || { mkdir -p "$PREFIX" 2>/dev/null && [ -w "$PREFIX" ]; }; then
    make install PREFIX="$PREFIX"
else
    $SUDO make install PREFIX="$PREFIX"
fi
if has update-desktop-database; then
    update-desktop-database "$PREFIX/share/applications" >/dev/null 2>&1 || true
fi

say "Installed $PREFIX/bin/conductor"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) warn "$PREFIX/bin is not on your PATH; add it in your shell profile" ;;
esac
echo
echo "Run 'conductor' to start the driver station, or 'conductor mcp-setup' to connect an AI agent."
