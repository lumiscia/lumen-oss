#!/usr/bin/env bash
# Builds the `lumen` binary that @lumiscia/lumen-cli installs, with FFmpeg and its codec libraries
# linked in, into this platform's package under packages/. vcpkg.json pins what is linked.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
manifest="$root/crates/cli/release"
target="${CARGO_TARGET_DIR:-$root/target}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) triplet=x64-linux-release package=lumen-cli-linux-x64 features=static-ffmpeg ;;
  Linux-aarch64) triplet=arm64-linux-release package=lumen-cli-linux-arm64 features=static-ffmpeg ;;
  Darwin-arm64) triplet=arm64-osx-release package=lumen-cli-darwin-arm64 features=static-ffmpeg,metal ;;
  *)
    echo "no lumen release build for $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

vcpkg="$target/vcpkg"
baseline="$(sed -n 's/.*"builtin-baseline": "\(.*\)".*/\1/p' "$manifest/vcpkg.json")"
if [ "$(git -C "$vcpkg" rev-parse HEAD 2>/dev/null)" != "$baseline" ]; then
  mkdir -p "$vcpkg"
  git -C "$vcpkg" init -q
  git -C "$vcpkg" fetch -q --depth 1 https://github.com/microsoft/vcpkg.git "$baseline"
  git -C "$vcpkg" checkout -q FETCH_HEAD
  "$vcpkg/bootstrap-vcpkg.sh" -disableMetrics
fi

export VCPKG_DEFAULT_BINARY_CACHE="${VCPKG_DEFAULT_BINARY_CACHE:-$target/vcpkg-cache}"
mkdir -p "$VCPKG_DEFAULT_BINARY_CACHE"
installed="$target/vcpkg-installed"
"$vcpkg/vcpkg" install \
  --x-manifest-root="$manifest" \
  --x-install-root="$installed" \
  --triplet="$triplet" \
  --host-triplet="$triplet"

PKG_CONFIG_PATH="$installed/$triplet/lib/pkgconfig" cargo build \
  --manifest-path "$root/Cargo.toml" \
  --release \
  --locked \
  --package lumen-cli \
  --features "$features" \
  --config 'profile.release.strip=true'

out="$root/packages/$package"
rm -rf "$out/bin" "$out/licenses"
mkdir -p "$out/bin" "$out/licenses"
cp "$target/release/lumen" "$out/bin/lumen"
cp "$root/LICENSE" "$out/licenses/lumen.txt"
for copyright in "$installed/$triplet"/share/*/copyright; do
  port="$(basename "$(dirname "$copyright")")"
  case "$port" in
    # Build tools, which are not linked into the binary.
    vcpkg-* | pkgconf | ffmpeg-bin2c | shaderc | glslang | spirv-tools) ;;
    *) cp "$copyright" "$out/licenses/$port.txt" ;;
  esac
done
echo "built $out/bin/lumen"
