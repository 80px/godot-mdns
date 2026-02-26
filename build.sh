#!/usr/bin/env bash
# build.sh — compile godot-mdns and copy binaries into addons/godot-mdns/bin/.
#
# Usage:
#   ./build.sh                   # auto-detect host platform (debug)
#   ./build.sh --release         # host platform release build
#   ./build.sh --linux           # force Linux x86_64/arm64 native build
#   ./build.sh --ios             # iOS arm64 static lib  [macOS host + Xcode required]
#   ./build.sh --ios --release   # iOS arm64 static lib, release
#   ./build.sh --android         # Android arm64 + arm32 + x86_64 [cargo-ndk + NDK required]
#   ./build.sh --android --release
#   ./build.sh --windows         # Windows x86_64 DLL (cross-compile from macOS/Linux/WSL)
#   ./build.sh --windows --release
#
# WSL (Windows Subsystem for Linux):
#   Auto-detect defaults to --windows when WSL is detected, because the
#   resulting DLL is what's needed on the host Windows machine.
#   Use --linux explicitly if you really want a Linux .so from WSL.
#
# Windows cross-compile prerequisites (macOS/Linux/WSL host):
#   brew install mingw-w64                    # macOS
#   sudo apt install gcc-mingw-w64-x86-64     # Linux / WSL
#   rustup target add x86_64-pc-windows-gnu
#
# To compile natively ON Windows (no WSL), use build.ps1 instead.
#
# Android prerequisites:
#   cargo install cargo-ndk
#   rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
#   export ANDROID_NDK_HOME=/path/to/ndk

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_BASE="$SCRIPT_DIR/addons/godot-mdns/bin"
PROFILE="debug"
CARGO_ARGS=()
PLATFORM="auto"   # auto | linux | ios | android | windows

for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release"; CARGO_ARGS+=("--release") ;;
    --linux)   PLATFORM="linux" ;;
    --ios)     PLATFORM="ios" ;;
    --android) PLATFORM="android" ;;
    --windows) PLATFORM="windows" ;;
    --help|-h)
      echo "Usage: ./build.sh [OPTIONS]"
      echo ""
      echo "Compiles godot-mdns and copies binaries into addons/godot-mdns/bin/."
      echo ""
      echo "Options:"
      echo "  (none)       Auto-detect host platform, debug profile"
      echo "               On WSL: defaults to --windows (use --linux to override)"
      echo "  --release    Release profile (enables LTO; slower first build)"
      echo "  --linux      Force Linux x86_64/arm64 native build"
      echo "  --windows    Cross-compile Windows x86_64 DLL (requires MinGW)"
      echo "               Works on macOS, Linux, and WSL"
      echo "  --ios        iOS arm64 static lib     (macOS host + Xcode required)"
      echo "  --android    Android arm64 + arm32    (cargo-ndk + NDK required)"
      echo "  --help, -h   Show this help"
      echo ""
      echo "Prerequisites by platform:"
      echo "  --windows  macOS: brew install mingw-w64"
      echo "             Linux/WSL: sudo apt install gcc-mingw-w64-x86-64"
      echo "             All:  rustup target add x86_64-pc-windows-gnu"
      echo "  --ios      macOS only; rustup target add aarch64-apple-ios"
      echo "  --android  cargo install cargo-ndk"
      echo "             rustup target add aarch64-linux-android armv7-linux-androideabi"
      echo "             export ANDROID_NDK_HOME=/path/to/ndk"
      exit 0
      ;;
    *) echo "Unknown argument: $arg  (try --help)"; exit 1 ;;
  esac
done

# ── Helpers ──────────────────────────────────────────────────────────────────

# Print a step header with a timestamp.
step() { echo ""; echo "==> [$(date +%H:%M:%S)] $*"; }

# Run cargo and print elapsed time when it finishes.
# Shows a background ticker every 30 s so you know it hasn't hung.
run_cargo() {
  local start
  start=$(date +%s)
  echo "    $ cargo $*"
  if [[ "$PROFILE" == "release" ]]; then
    echo "    (release build — LTO link step can take 3-5 min on first run, be patient)"
  fi

  # Ticker: prints a dot every 30 s while cargo runs so the terminal stays active.
  ( while true; do sleep 30; echo "    ... still building ($(( $(date +%s) - start ))s elapsed)"; done ) &
  local ticker_pid=$!
  # Make sure the ticker is killed even on error.
  trap "kill $ticker_pid 2>/dev/null; trap - ERR EXIT" ERR EXIT

  cargo "$@"
  local status=$?

  kill "$ticker_pid" 2>/dev/null
  trap - ERR EXIT
  echo "    Done in $(( $(date +%s) - start ))s"
  return $status
}

# ── WSL auto-detection ───────────────────────────────────────────────────────
# Under WSL, uname -s returns "Linux" but the developer is on a Windows machine,
# so the useful artefact is the Windows DLL, not a Linux .so.
# WSL2 kernels include "microsoft" in uname -r; WSL1 has it in /proc/version.
if [[ "$PLATFORM" == "auto" ]]; then
  if [[ "$(uname -r)" =~ [Mm]icrosoft ]] || grep -qiE 'microsoft|WSL' /proc/version 2>/dev/null; then
    echo "==> WSL detected — defaulting to --windows (use --linux to build a Linux .so instead)"
    PLATFORM="windows"
  fi
fi

echo ""
echo "==> godot-mdns build  platform=${PLATFORM}  profile=${PROFILE}"
echo "==> $(date)"
cd "$SCRIPT_DIR"

# ── Windows (cross-compile from macOS/Linux via MinGW) ──────────────────────
if [[ "$PLATFORM" == "windows" ]]; then
  if ! command -v x86_64-w64-mingw32-gcc &>/dev/null; then
    echo "ERROR: MinGW cross-compiler not found."
    echo "  macOS: brew install mingw-w64"
    echo "  Linux: sudo apt install gcc-mingw-w64-x86-64"
    exit 1
  fi
  step "Compiling Windows x86_64 ..."
  run_cargo build "${CARGO_ARGS[@]}" --target x86_64-pc-windows-gnu
  SRC="$SCRIPT_DIR/target/x86_64-pc-windows-gnu/$PROFILE/godot_mdns.dll"
  DST="$OUT_BASE/windows/x86_64/$PROFILE/godot_mdns.dll"
  mkdir -p "$(dirname "$DST")"
  cp "$SRC" "$DST"
  step "Done: $DST"
  exit 0
fi

# ── iOS ──────────────────────────────────────────────────────────────────────
if [[ "$PLATFORM" == "ios" ]]; then
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "ERROR: iOS builds require a macOS host with Xcode installed."; exit 1
  fi
  step "Compiling iOS arm64 ..."
  run_cargo build "${CARGO_ARGS[@]}" --target aarch64-apple-ios
  DST="$OUT_BASE/ios/arm64/$PROFILE/libgodot_mdns.a"
  mkdir -p "$(dirname "$DST")"
  cp "$SCRIPT_DIR/target/aarch64-apple-ios/$PROFILE/libgodot_mdns.a" "$DST"
  step "Done: $DST"
  exit 0
fi

# ── Android ──────────────────────────────────────────────────────────────────
if [[ "$PLATFORM" == "android" ]]; then
  if ! command -v cargo-ndk &>/dev/null; then
    echo "ERROR: cargo-ndk not found. Install with: cargo install cargo-ndk"; exit 1
  fi
  if [[ -z "${ANDROID_NDK_HOME:-}" && -z "${ANDROID_NDK_ROOT:-}" ]]; then
    echo "ERROR: ANDROID_NDK_HOME or ANDROID_NDK_ROOT must point to the Android NDK."; exit 1
  fi

  for ABI in arm64-v8a armeabi-v7a x86_64; do
    step "Compiling Android $ABI ..."
    run_cargo ndk -t "$ABI" --platform 21 -- build "${CARGO_ARGS[@]}"
    case "$ABI" in
      arm64-v8a)   OUT_ARCH="arm64"  ; TRIPLE="aarch64-linux-android" ;;
      armeabi-v7a) OUT_ARCH="arm32"  ; TRIPLE="armv7-linux-androideabi" ;;
      x86_64)      OUT_ARCH="x86_64" ; TRIPLE="x86_64-linux-android" ;;
    esac
    DST="$OUT_BASE/android/$OUT_ARCH/$PROFILE/libgodot_mdns.so"
    mkdir -p "$(dirname "$DST")"
    cp "$SCRIPT_DIR/target/$TRIPLE/$PROFILE/libgodot_mdns.so" "$DST"
    step "Done: $DST"
  done
  exit 0
fi

# ── Native host platform (or --linux explicit) ───────────────────────────────
# Guard: --linux on a non-Linux host is unsupported.
if [[ "$PLATFORM" == "linux" && "$(uname -s)" != Linux* ]]; then
  echo "ERROR: --linux requires a Linux (or WSL) host."
  exit 1
fi
OS="$(uname -s)"
case "$OS" in
  Linux*)
    ARCH="$(uname -m)"
    case "$ARCH" in
      x86_64)  TRIPLE="x86_64-unknown-linux-gnu"  ; OUT_ARCH="x86_64" ;;
      aarch64) TRIPLE="aarch64-unknown-linux-gnu" ; OUT_ARCH="arm64"  ;;
      *) echo "Unsupported Linux arch: $ARCH"; exit 1 ;;
    esac
    step "Compiling Linux $OUT_ARCH ..."
    run_cargo build "${CARGO_ARGS[@]}" --target "$TRIPLE"
    SRC="$SCRIPT_DIR/target/$TRIPLE/$PROFILE/libgodot_mdns.so"
    DST="$OUT_BASE/linux/$OUT_ARCH/$PROFILE/libgodot_mdns.so"
    mkdir -p "$(dirname "$DST")"
    cp "$SRC" "$DST"
    step "Done: $DST"
    ;;

  Darwin*)
    step "Compiling x86_64-apple-darwin ..."
    run_cargo build "${CARGO_ARGS[@]}" --target x86_64-apple-darwin
    step "Compiling aarch64-apple-darwin ..."
    run_cargo build "${CARGO_ARGS[@]}" --target aarch64-apple-darwin
    DST="$OUT_BASE/macos/universal/$PROFILE/libgodot_mdns.dylib"
    mkdir -p "$(dirname "$DST")"
    step "Creating universal binary with lipo ..."
    lipo -create \
      "$SCRIPT_DIR/target/x86_64-apple-darwin/$PROFILE/libgodot_mdns.dylib" \
      "$SCRIPT_DIR/target/aarch64-apple-darwin/$PROFILE/libgodot_mdns.dylib" \
      -output "$DST"
    step "Done: $DST"
    ;;

  MINGW*|MSYS*|CYGWIN*)
    # Native MSYS2/MinGW shell on Windows — build the Windows DLL directly.
    step "Compiling Windows x86_64 (native MSYS2/MinGW) ..."
    run_cargo build "${CARGO_ARGS[@]}" --target x86_64-pc-windows-gnu
    SRC="$SCRIPT_DIR/target/x86_64-pc-windows-gnu/$PROFILE/godot_mdns.dll"
    DST="$OUT_BASE/windows/x86_64/$PROFILE/godot_mdns.dll"
    mkdir -p "$(dirname "$DST")"
    cp "$SRC" "$DST"
    step "Done: $DST"
    ;;

  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac
