#!/usr/bin/env bash
# One-shot local install. Tries a prebuilt release binary first (x86_64,
# Vulkan build); falls back to building from source (needs a C/C++
# toolchain, cmake, clang, Vulkan headers, glslc) when there's no matching
# release, on --from-source, or on any other architecture. Installs deps via
# apt/dnf/pacman, then copies the binary + desktop entry into the user's XDG
# dirs. Never re-runs itself or checks for updates — see `nexora update` for
# that (todo), or just re-run this script.
#
# Works two ways:
#   - From a clone of this repo: ./scripts/install.sh
#   - Piped straight from GitHub, no clone needed:
#       curl -fsSL https://raw.githubusercontent.com/MiguelLopesDel/nexora/main/scripts/install.sh | bash
#     The source-build fallback clones (or updates, on a later run) main
#     into ~/.local/share/nexora/src; the prebuilt path needs no clone.
#
# Flags: --skip-deps (assume system deps are already installed),
# --from-source (skip the prebuilt binary and always build).
#
# Works on Arch/pacman too. Prefer packaging/aur/PKGBUILD (makepkg -si) or
# the AUR package if you want pacman to track nexora as an installed
# package; this script is the no-AUR-account fallback.
set -euo pipefail

REPO="MiguelLopesDel/nexora"
REPO_URL="https://github.com/$REPO.git"
CHECKOUT_DIR="$HOME/.local/share/nexora/src"
BIN_DIR="$HOME/.local/bin"
APPS_DIR="$HOME/.local/share/applications"

skip_deps=0
from_source=0
for arg in "$@"; do
    case "$arg" in
        --skip-deps) skip_deps=1 ;;
        --from-source) from_source=1 ;;
        *) echo "unknown flag: $arg" >&2; exit 1 ;;
    esac
done

install_deps() {
    if [[ "$skip_deps" -eq 1 ]]; then
        echo "==> Skipping dependency install (--skip-deps)"
        return
    fi
    if command -v apt-get >/dev/null; then
        echo "==> Installing dependencies (apt)"
        sudo apt-get update
        sudo apt-get install -y \
            libgtk-4-dev libgtk4-layer-shell-dev pulseaudio-utils \
            cmake g++ libclang-dev libvulkan-dev glslc
    elif command -v dnf >/dev/null; then
        echo "==> Installing dependencies (dnf)"
        sudo dnf install -y \
            gtk4-devel gtk4-layer-shell-devel pulseaudio-utils \
            cmake gcc-c++ clang-devel vulkan-loader-devel glslc
    elif command -v pacman >/dev/null; then
        echo "==> Installing dependencies (pacman)"
        echo "    (packaging/aur/PKGBUILD via makepkg -si is the alternative" \
            "if you'd rather have pacman track the install as a package.)"
        sudo pacman -S --needed \
            gtk4 gtk4-layer-shell libpulse cmake gcc clang \
            vulkan-headers vulkan-icd-loader shaderc
    else
        echo "Unrecognized package manager. Install the dependencies listed" \
            "in README.md manually, then re-run with --skip-deps."
        exit 1
    fi
}

install_files() {
    local bin="$1" desktop="$2"
    mkdir -p "$BIN_DIR" "$APPS_DIR"
    install -Dm755 "$bin" "$BIN_DIR/nexora"
    install -Dm644 "$desktop" "$APPS_DIR/dev.nexora.Nexora.desktop"
    echo
    echo "==> Installed $BIN_DIR/nexora"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) echo "    $BIN_DIR is not on your PATH — add it to your shell profile." ;;
    esac
    echo "==> Next: nexora config init, then bind 'nexora toggle' to a key."
}

# Try the latest GitHub Release's prebuilt x86_64 binary. Prints nothing and
# returns non-zero on any failure (no matching release, wrong arch, offline,
# missing curl/tar) so the caller can fall back to building from source.
try_prebuilt() {
    [[ "$from_source" -eq 1 ]] && return 1
    [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]] || return 1
    command -v curl >/dev/null && command -v tar >/dev/null || return 1

    local api="https://api.github.com/repos/$REPO/releases/latest"
    local asset_url
    asset_url=$(curl -fsSL "$api" \
        | grep -o '"browser_download_url": *"[^"]*x86_64-linux-gnu\.tar\.gz"' \
        | head -1 | grep -o 'https://[^"]*') || return 1
    [[ -n "$asset_url" ]] || return 1

    echo "==> Downloading prebuilt binary: $(basename "$asset_url")"
    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    curl -fsSL -o "$tmp/nexora.tar.gz" "$asset_url" || return 1
    curl -fsSL -o "$tmp/nexora.tar.gz.sha256" "$asset_url.sha256" 2>/dev/null || true
    if [[ -s "$tmp/nexora.tar.gz.sha256" ]]; then
        (cd "$tmp" && sha256sum -c nexora.tar.gz.sha256) || {
            echo "checksum mismatch, discarding download" >&2
            return 1
        }
    fi
    tar -xzf "$tmp/nexora.tar.gz" -C "$tmp" || return 1
    local extracted
    extracted="$(find "$tmp" -maxdepth 1 -type d -name 'nexora-*')"
    [[ -n "$extracted" && -x "$extracted/nexora" ]] || return 1

    install_deps
    install_files "$extracted/nexora" "$extracted/dev.nexora.Nexora.desktop"
}

build_from_source() {
    local script_path="${BASH_SOURCE[0]:-}"
    local repo_root
    if [[ -n "$script_path" && -f "$script_path" ]]; then
        # Running from a real file: assume it's a checkout of this repo.
        repo_root="$(cd "$(dirname "$script_path")/.." && pwd)"
    else
        # Piped in (curl | bash): no local checkout to run from yet.
        command -v git >/dev/null || {
            echo "git not found; install it first (needed to fetch the source)."
            exit 1
        }
        if [[ -d "$CHECKOUT_DIR/.git" ]]; then
            echo "==> Updating existing checkout in $CHECKOUT_DIR"
            git -C "$CHECKOUT_DIR" pull --ff-only
        else
            echo "==> Cloning nexora into $CHECKOUT_DIR"
            mkdir -p "$(dirname "$CHECKOUT_DIR")"
            git clone "$REPO_URL" "$CHECKOUT_DIR"
        fi
        repo_root="$CHECKOUT_DIR"
    fi
    cd "$repo_root"

    install_deps
    command -v cargo >/dev/null || {
        echo "cargo not found. Install Rust first: https://rustup.rs"
        exit 1
    }

    echo "==> Building nexora (release)"
    cargo build --release
    install_files target/release/nexora assets/dev.nexora.Nexora.desktop
}

if ! try_prebuilt; then
    echo "==> No prebuilt binary used; building from source"
    build_from_source
fi
