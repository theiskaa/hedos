#!/usr/bin/env bash
# Fetch the llama.cpp build hedos carries for a target triple into a
# directory, and point HEDOS_LLAMA_CPP_ARCHIVE at it: in $GITHUB_ENV on a
# runner, else on stdout. On macOS that is llama.cpp's own release archive,
# whose sha256 the build script checks against the pin in
# kernel/src/engines/llama_cpp.rs; on Linux, build-llama-cpp.sh builds it from
# the pinned source. kernel/tests/bundle_setup.rs keeps the names here in step
# with that pin.
#
#   .github/fetch-llama-cpp.sh <target-triple> <directory>
set -euo pipefail

target="$1"
dir="$2"
base=https://github.com/ggml-org/llama.cpp/releases/download/b11669
case "$target" in
  aarch64-apple-darwin) file=llama-b11669-bin-macos-arm64.tar.gz ;;
  x86_64-apple-darwin) file=llama-b11669-bin-macos-x64.tar.gz ;;
  aarch64-unknown-linux-gnu | x86_64-unknown-linux-gnu)
    exec "$(dirname "$0")/build-llama-cpp.sh" "$target" "$dir" ;;
  *) echo "hedos carries no llama.cpp build for $target" >&2; exit 1 ;;
esac

mkdir -p "$dir"
curl --fail --location --retry 3 --silent --show-error --output "$dir/$file" "$base/$file"
archive="$(cd "$dir" && pwd)/$file"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "HEDOS_LLAMA_CPP_ARCHIVE=$archive" >> "$GITHUB_ENV"
else
  printf 'export HEDOS_LLAMA_CPP_ARCHIVE=%q\n' "$archive"
fi
