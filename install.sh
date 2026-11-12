#!/usr/bin/env bash
# One command to a working setup: builds we-linux and, unless it is there
# already, the low-footprint fork of penguin-engine (the renderer), and puts
# them in ~/.local (PREFIX names another place).
#
#   ./install.sh              we-linux, and the engine when it is missing
#   ./install.sh --engine     build the engine again too (an update)
#   ./install.sh --no-web     the engine without web wallpapers: a much
#                             shorter build (no Servo)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
engine_repo=${WE_LINUX_ENGINE_REPO:-https://github.com/MoonTiwzy/penguin-engine}
engine_src="${XDG_CACHE_HOME:-$HOME/.cache}/we-linux/penguin-engine"
again="" features=()
for arg in "$@"; do
    case $arg in
    --engine) again=1 ;;
    --no-web) features=(--no-default-features) ;;
    *) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
    esac
done

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
command -v cargo >/dev/null || { echo "cargo is needed: https://rustup.rs" >&2; exit 1; }

say "building we-linux"
cargo build --release --manifest-path "$here/Cargo.toml"
install -Dm755 "$here/target/release/we-linux" "$prefix/bin/we-linux"
install -Dm644 "$here/assets/we-linux.desktop" "$prefix/share/applications/we-linux.desktop"
install -Dm644 "$here/assets/we-linux.svg" "$prefix/share/icons/hicolor/scalable/apps/we-linux.svg"

if [[ -n $again || ! -x $prefix/bin/penguind ]]; then
    say "building penguin-engine (the renderer), the first time takes a while"
    if [[ -d $engine_src/.git ]]; then
        # This clone is the installer's own: it follows the branch wherever it goes.
        git -C "$engine_src" fetch -q --depth 1 origin low-footprint
        git -C "$engine_src" reset -q --hard FETCH_HEAD
    else
        mkdir -p "$(dirname "$engine_src")"
        git clone -q --depth 1 -b low-footprint "$engine_repo" "$engine_src"
    fi
    if ! cargo build --release --manifest-path "$engine_src/Cargo.toml" "${features[@]}" -p penguind -p penguinctl -p scene-viewer; then
        cat >&2 <<'HINT'

The engine did not build. It needs a C/C++ toolchain, clang, cmake, python and
the headers of mpv, Wayland, libxkbcommon, fontconfig, ALSA and PulseAudio:
  Arch:    sudo pacman -S --needed base-devel clang cmake python mpv wayland libxkbcommon fontconfig alsa-lib libpulse vulkan-icd-loader ffmpeg
  Fedora:  sudo dnf install gcc-c++ clang cmake python3 mpv-devel wayland-devel libxkbcommon-devel fontconfig-devel alsa-lib-devel pulseaudio-libs-devel vulkan-loader ffmpeg
  Debian:  sudo apt install build-essential clang cmake python3 libmpv-dev libwayland-dev libxkbcommon-dev libfontconfig-dev libasound2-dev libpulse-dev libvulkan1 ffmpeg
HINT
        exit 1
    fi
    for bin in penguind penguinctl scene-viewer; do
        install -Dsm755 "$engine_src/target/release/$bin" "$prefix/bin/$bin"
    done
fi

say "installed to $prefix/bin: we-linux, penguind, penguinctl, scene-viewer"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "  add $prefix/bin to your PATH" ;; esac
echo "  run: we-linux"
echo "  keep the wallpaper after a login: start 'we-linux daemon' with your session"
echo "  (Hyprland: exec-once = we-linux daemon)"
