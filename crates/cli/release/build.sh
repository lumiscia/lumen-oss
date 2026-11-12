#!/usr/bin/env bash
# Builds the `lumen` binary that @lumiscia/lumen-cli installs, with FFmpeg and its codec libraries
# linked in, into this platform's package under packages/. vcpkg.json pins what is linked, and
# ports/ and triplets/ hold the changes to vcpkg's own builds.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
manifest="$root/crates/cli/release"
target="${CARGO_TARGET_DIR:-$root/target}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) rust_target=x86_64-unknown-linux-gnu triplet=x64-linux-release package=lumen-cli-linux-x64 features=static-ffmpeg,cuda ;;
  Linux-aarch64) rust_target=aarch64-unknown-linux-gnu triplet=arm64-linux-release package=lumen-cli-linux-arm64 features=static-ffmpeg,cuda ;;
  Darwin-arm64)
    rust_target=aarch64-apple-darwin triplet=arm64-osx-13 package=lumen-cli-darwin-arm64 features=static-ffmpeg,metal
    # The oldest macOS the binary runs on; triplets/arm64-osx-13.cmake sets it for the libraries.
    export MACOSX_DEPLOYMENT_TARGET=13.0
    ;;
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

# Cargo copies the static libraries into ffmpeg-sys-next when it compiles it, and doesn't notice
# when vcpkg has rebuilt them.
cargo clean --manifest-path "$root/Cargo.toml" --release --package ffmpeg-sys-next
PKG_CONFIG_PATH="$installed/$triplet/lib/pkgconfig" cargo build \
  --manifest-path "$root/Cargo.toml" \
  --release \
  --locked \
  --package lumen-cli \
  --features "$features" \
  --config 'profile.release.strip=true'

out="$root/packages/$package"
rm -rf "$out/bin" "$out/licenses" "$out/LICENSE"
mkdir -p "$out/bin" "$out/licenses"
cp "$target/release/lumen" "$out/bin/lumen"
# x264 and x265 put the binary as a whole under the GPL; licenses/ has each part's own license.
cp "$manifest/GPL-3.0.txt" "$out/LICENSE"
cp "$root/LICENSE" "$out/licenses/lumen.txt"
cp "$root/crates/text/assets/roboto/LICENSE.txt" "$out/licenses/roboto-font.txt"
cargo about generate \
  --config "$manifest/about.toml" \
  --manifest-path "$root/crates/cli/Cargo.toml" \
  --features "$features" \
  --target "$rust_target" \
  --locked \
  --fail \
  --output-file "$out/licenses/rust-crates.txt" \
  "$manifest/about.hbs"
for copyright in "$installed/$triplet"/share/*/copyright; do
  port="$(basename "$(dirname "$copyright")")"
  case "$port" in
    # Build tools, which are not linked into the binary.
    vcpkg-* | pkgconf | ffmpeg-bin2c | shaderc | glslang | spirv-tools) ;;
    *) cp "$copyright" "$out/licenses/$port.txt" ;;
  esac
done
echo "built $out/bin/lumen"
