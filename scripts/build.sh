#!/usr/bin/env bash
# Build field-eval on this host (PRoot Debian on Android).
#
# Why this wrapper exists — three host-specific traps:
#   1. PATH contains Termux's clang (Android target) ahead of Debian gcc.
#      Rust must link with /usr/bin/cc (Debian, glibc) or ld.lld fails on -lgcc_s.
#   2. Termux's CMake 4.x injects its own $PREFIX/include into every build and
#      its Android-host branch reads $PREFIX/include/android/api-level.h. Mixing
#      bionic headers with Debian libstdc++ breaks cpp-httplib. Use Debian's
#      cmake 3.25 (/usr/bin/cmake, installed via apt) and do NOT export PREFIX.
#   3. bindgen resolves <sys/cdefs.h> to the Termux (bionic) copy unless the
#      glibc multiarch include is passed explicitly.
#   4. Termux's pkg-config is the only pkg-config unless Debian's is installed.
#      It reports $PREFIX/include for OpenSSL, and Debian g++ then compiles
#      bionic headers. Debian pkg-config + libssl-dev are required.
set -euo pipefail

export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/data/data/com.termux/files/usr/bin"
export LIBCLANG_PATH="/usr/lib/llvm-14/lib"
export BINDGEN_EXTRA_CLANG_ARGS="--target=aarch64-unknown-linux-gnu --sysroot=/ -isystem/usr/lib/llvm-14/lib/clang/14.0.6/include -isystem/usr/include/aarch64-linux-gnu -isystem/usr/include"

# Two jobs: 10 GB of RAM is mostly consumed, and llama.cpp is memory-hungry.
export CMAKE_BUILD_PARALLEL_LEVEL="${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

cd "$(dirname "$0")/.."
exec cargo "$@"
