#!/usr/bin/env bash
# What datarig-core's pg_query dependency needs to build: a C compiler for libpg_query and
# libclang for bindgen. The hosted runners usually have both; this checks, installs what is
# missing, and fails with a clear message instead of a bindgen or cc error deep in the build.
set -euo pipefail

case "${RUNNER_OS:-$(uname -s)}" in
  Linux)
    libclang() { grep -m1 'libclang[-.0-9]*\.so' <<<"$(ldconfig -p)"; }
    if ! command -v cc >/dev/null || ! libclang >/dev/null; then
      sudo apt-get update
      sudo apt-get install -y --no-install-recommends clang libclang-dev
    fi
    if ! command -v cc >/dev/null; then
      echo "no C compiler (cc) found, even after installing clang" >&2
      exit 1
    fi
    if ! libclang; then
      echo "libclang not found by ldconfig, even after installing clang and libclang-dev" >&2
      exit 1
    fi
    ;;
  macOS | Darwin)
    # The Command Line Tools (or Xcode) ship clang and libclang.dylib; clang-sys finds them
    # through xcode-select.
    xcrun --find clang
    lib=$(find "$(xcode-select -p)" -maxdepth 6 -name libclang.dylib -print -quit)
    if [ -z "$lib" ]; then
      echo "libclang.dylib not found under $(xcode-select -p)" >&2
      exit 1
    fi
    echo "$lib"
    ;;
  Windows)
    llvm="C:/Program Files/LLVM/bin"
    if [ ! -f "$llvm/libclang.dll" ]; then
      choco install llvm -y --no-progress
    fi
    if [ ! -f "$llvm/libclang.dll" ]; then
      echo "libclang.dll not found in $llvm" >&2
      exit 1
    fi
    # bindgen looks here; the MSVC compiler comes with the image's Visual Studio.
    echo "LIBCLANG_PATH=$llvm" >> "$GITHUB_ENV"
    echo "$llvm/libclang.dll"
    ;;
  *)
    echo "unknown runner OS: ${RUNNER_OS:-$(uname -s)}" >&2
    exit 1
    ;;
esac
cc --version 2>/dev/null | head -n1 || true
