#!/usr/bin/env bash
# Fetch the llama.cpp build hedos carries for a target triple into a
# directory, and point HEDOS_LLAMA_CPP_ARCHIVE at it: in $GITHUB_ENV on a
# runner, else on stdout. On macOS that is llama.cpp's own release archive; on
# Linux hedos's own build of the pinned source (made by build-llama-cpp.sh and
# published under the llama.cpp-<build> release tag), since llama.cpp's own
# Linux archives need glibc 2.38. The build script checks either against the
# pin in kernel/src/engines/llama_cpp.rs; kernel/tests/bundle_setup.rs keeps
# the names here in step with it.
#
#   .github/fetch-llama-cpp.sh <target-triple> <directory>
set -euo pipefail

target="$1"
dir="$2"
llama=https://github.com/ggml-org/llama.cpp/releases/download/b11669
hedos=https://github.com/theiskaa/hedos/releases/download/llama.cpp-b11669
case "$target" in
  aarch64-apple-darwin) base=$llama; file=llama-b11669-bin-macos-arm64.tar.gz ;;
  x86_64-apple-darwin) base=$llama; file=llama-b11669-bin-macos-x64.tar.gz ;;
  aarch64-unknown-linux-gnu) base=$hedos; file=llama-b11669-bin-linux-aarch64.tar.gz ;;
  x86_64-unknown-linux-gnu) base=$hedos; file=llama-b11669-bin-linux-x86_64.tar.gz ;;
  *) echo "hedos carries no llama.cpp build for $target" >&2; exit 1 ;;
esac

mkdir -p "$dir"
curl --fail --location --retry 3 --silent --show-error --output "$dir/$file.partial" "$base/$file"
mv "$dir/$file.partial" "$dir/$file"
archive="$(cd "$dir" && pwd)/$file"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "HEDOS_LLAMA_CPP_ARCHIVE=$archive" >> "$GITHUB_ENV"
else
  printf 'export HEDOS_LLAMA_CPP_ARCHIVE=%q\n' "$archive"
fi
