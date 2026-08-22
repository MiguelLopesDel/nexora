#!/usr/bin/env bash
# One-shot local install: system deps (apt/dnf), cargo build --release, and
# copy the binary + desktop entry into the user's XDG dirs. Run from a clone
# of this repo. Never re-runs itself or checks for updates — see `nexora
# update` for that (todo) or just re-run this script after `git pull`.
#
# Arch/pacman users: use packaging/aur/PKGBUILD (or the AUR package once
# published) instead of this script.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

install_deps() {
    if command -v apt-get >/dev/null; then
        echo "==> Installing build/runtime dependencies (apt)"
        sudo apt-get update
        sudo apt-get install -y \
            libgtk-4-dev libgtk4-layer-shell-dev pulseaudio-utils \
            cmake g++ libclang-dev libvulkan-dev glslc
    elif command -v dnf >/dev/null; then
        echo "==> Installing build/runtime dependencies (dnf)"
        sudo dnf install -y \
            gtk4-devel gtk4-layer-shell-devel pulseaudio-utils \
            cmake gcc-c++ clang-devel vulkan-loader-devel glslc
    elif command -v pacman >/dev/null; then
        echo "This is an Arch-based system: use packaging/aur/PKGBUILD" \
            "(makepkg -si) or the AUR package instead of this script."
        exit 1
    else
        echo "Unrecognized package manager. Install the dependencies listed" \
            "in README.md manually, then re-run with --skip-deps."
        exit 1
    fi
}

if [[ "${1:-}" != "--skip-deps" ]]; then
    install_deps
else
    echo "==> Skipping dependency install (--skip-deps)"
fi

if ! command -v cargo >/dev/null; then
    echo "cargo not found. Install Rust first: https://rustup.rs"
    exit 1
fi

echo "==> Building nexora (release)"
cargo build --release

bin_dir="$HOME/.local/bin"
apps_dir="$HOME/.local/share/applications"
mkdir -p "$bin_dir" "$apps_dir"
install -Dm755 target/release/nexora "$bin_dir/nexora"
install -Dm644 assets/dev.nexora.Nexora.desktop "$apps_dir/dev.nexora.Nexora.desktop"

echo
echo "==> Installed $bin_dir/nexora"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "    $bin_dir is not on your PATH — add it to your shell profile." ;;
esac
echo "==> Next: nexora config init, then bind 'nexora toggle' to a key."
