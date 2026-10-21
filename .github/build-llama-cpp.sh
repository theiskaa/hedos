#!/usr/bin/env bash
# Build hedos's own llama.cpp Linux build from the pinned source, and point
# HEDOS_LLAMA_CPP_ARCHIVE at it: in $GITHUB_ENV on a runner, else as an export
# line on stdout. Its archives are what a new pin publishes under the hedos
# release tag llama.cpp-<build> and pins by sha256: release builds carry them
# and `hedos engines install` fetches them on Linux (fetch-llama-cpp.sh).
#
# llama.cpp's own Linux archives are built on Ubuntu 24.04 and need glibc 2.38,
# GCC 13's libstdc++, libgomp and OpenSSL 3, so they do not start on Ubuntu
# 22.04 or Debian 12. This builds the same release in manylinux_2_28 (glibc
# 2.28, GCC 14) with the C++ runtime linked in, no OpenMP and no OpenSSL, so it
# starts wherever hedos does. kernel/tests/bundle_setup.rs keeps the tag,
# commit, digest and archive names here in step with the pin in
# kernel/src/engines/llama_cpp.rs.
#
#   .github/build-llama-cpp.sh <target-triple> <directory>
set -euo pipefail

target="$1"
dir="$2"
build=b11669
commit=620d2225c
source_sha256=e06ba4fa263e30fdb9f0e65f00a8965202ddaa0c51f548a67e1f9b2164d04efc
case "$target" in
  aarch64-unknown-linux-gnu)
    platform=linux/arm64
    image=quay.io/pypa/manylinux_2_28_aarch64@sha256:b873ffc316ffe8a154e3dc4872871de2ebd170db4876c8bbd983d42ff320001e
    ;;
  x86_64-unknown-linux-gnu)
    platform=linux/amd64
    image=quay.io/pypa/manylinux_2_28_x86_64@sha256:1e7b8460ad8fb3fb0fbe1cd818845159ac896b4fd17452d56e892c291f3b97a8
    ;;
  *) echo "hedos builds llama.cpp from source only for Linux, not $target" >&2; exit 1 ;;
esac

mkdir -p "$dir"
dir="$(cd "$dir" && pwd)"
source="llama.cpp-$build.tar.gz"
if [ ! -f "$dir/$source" ]; then
  curl --fail --location --retry 3 --silent --show-error \
    --output "$dir/$source.partial" "https://github.com/ggml-org/llama.cpp/archive/refs/tags/$build.tar.gz"
  mv "$dir/$source.partial" "$dir/$source"
fi
digest() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}
found="$(digest "$dir/$source")"
if [ "$found" != "$source_sha256" ]; then
  rm -f "$dir/$source"
  echo "$source is not llama.cpp $build as hedos pins it (sha256 $found)" >&2
  exit 1
fi

archive="llama-$build-bin-linux-${target%%-*}.tar.gz"
# One built here before (a CI cache) is reused only when it is still what that
# build recorded, after its --version check passed.
built="$dir/$archive.sha256"
if [ -f "$dir/$archive" ] && [ -f "$built" ] && [ "$(digest "$dir/$archive")" = "$(cat "$built")" ]; then
  reuse=1
else
  reuse=0
  rm -f "$dir/$archive" "$built"
fi
[ "$reuse" = 1 ] || docker run --rm --platform "$platform" -v "$dir:/work" "$image" bash -euo pipefail -c "
  cd /tmp
  tar -xzf /work/$source
  cd llama.cpp-$build
  # The tarball has no git history, which is where the build number and
  # commit --version reports come from otherwise.
  sed -i 's/^set(BUILD_NUMBER 0)\$/set(BUILD_NUMBER ${build#b})/; s/^set(BUILD_COMMIT \"unknown\")\$/set(BUILD_COMMIT \"$commit\")/' cmake/build-info.cmake
  cmake -B build \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=ON \
    -DGGML_NATIVE=OFF \
    -DGGML_BACKEND_DL=ON \
    -DGGML_CPU_ALL_VARIANTS=ON \
    -DGGML_OPENMP=OFF \
    -DLLAMA_CURL=OFF \
    -DLLAMA_OPENSSL=OFF \
    -DLLAMA_BUILD_TESTS=OFF \
    -DLLAMA_BUILD_EXAMPLES=OFF \
    -DCMAKE_INSTALL_RPATH='\$ORIGIN' \
    -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
    -DCMAKE_EXE_LINKER_FLAGS='-static-libstdc++ -static-libgcc' \
    -DCMAKE_SHARED_LINKER_FLAGS='-static-libstdc++ -static-libgcc' >/dev/null
  cmake --build build -j\"\$(nproc)\" --target llama-server >/dev/null
  mkdir -p /tmp/pack/llama-$build
  find build/bin -maxdepth 1 \\( -name llama-server -o -name 'lib*.so*' \\) -exec cp -P -t /tmp/pack/llama-$build/ {} +
  cp LICENSE /tmp/pack/llama-$build/
  /tmp/pack/llama-$build/llama-server --version 2>&1 | grep -q 'build ${build#b}, commit $commit'
  tar -czf /work/$archive.partial -C /tmp/pack llama-$build
  mv /work/$archive.partial /work/$archive
"
[ "$reuse" = 1 ] || digest "$dir/$archive" > "$built"

if [ -n "${GITHUB_ENV:-}" ]; then
  echo "HEDOS_LLAMA_CPP_ARCHIVE=$dir/$archive" >> "$GITHUB_ENV"
else
  printf 'export HEDOS_LLAMA_CPP_ARCHIVE=%q\n' "$dir/$archive"
fi
