#!/usr/bin/env bash
set -euo pipefail

crate_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$crate_dir/../.." && pwd)"
cargo_bin="${CARGO:-cargo}"
profile="debug"
cargo_args=()

if [[ "${1:-}" == "--release" ]]; then
  profile="release"
  cargo_args+=(--release)
  shift
fi

output="${1:-$repo_root/target/browser-$profile-dist}"
if [[ -z "$output" || "$output" == "/" ]]; then
  echo "refusing unsafe output directory: $output" >&2
  exit 2
fi
tmp="$output.tmp.$$"
trap 'rm -rf "$tmp"' EXIT

(
  cd "$repo_root"
  "$cargo_bin" build -p blob-1-browser --bin blob-1-browser "${cargo_args[@]}"
)

build_dir="$repo_root/target/$profile"
mkdir -p "$tmp"
install -m 0755 "$build_dir/blob-1-browser" "$tmp/blob-1-browser"

# The CEF runtime: CEF_PATH when set, else the newest distribution the
# cef-dll-sys build extracted. Its copy into target/<profile> is not reliable
# on every runner, so that is only the last resort.
cef_dir="${CEF_PATH:-}"
if [[ -n "$cef_dir" && -d "$cef_dir/cef_linux_x86_64" ]]; then
  cef_dir="$cef_dir/cef_linux_x86_64"
fi
if [[ -z "$cef_dir" || ! -f "$cef_dir/libcef.so" ]]; then
  cef_dir="$(ls -dt "$build_dir"/build/cef-dll-sys-*/out/cef_linux_x86_64 2>/dev/null | head -n 1 || true)"
fi
if [[ -z "$cef_dir" || ! -f "$cef_dir/libcef.so" ]]; then
  cef_dir="$build_dir"
fi
if [[ ! -f "$cef_dir/libcef.so" || ! -d "$cef_dir/locales" ]]; then
  echo "no CEF runtime found (looked in CEF_PATH, $build_dir/build/cef-dll-sys-*/out, $build_dir)" >&2
  exit 1
fi

# cef-dll-sys copies the CEF runtime into Cargo's target directory during the
# build. Copy the runtime subset CEF documents for Linux; files marked absent
# in a given CEF revision are skipped.
for file in \
  libcef.so \
  libEGL.so \
  libGLESv2.so \
  libvk_swiftshader.so \
  libvulkan.so.1 \
  vk_swiftshader_icd.json \
  v8_context_snapshot.bin \
  snapshot_blob.bin \
  icudtl.dat \
  resources.pak \
  chrome_100_percent.pak \
  chrome_200_percent.pak \
  chrome-sandbox
do
  if [[ -e "$cef_dir/$file" ]]; then
    cp -a "$cef_dir/$file" "$tmp/$file"
  fi
done
cp -a "$cef_dir/locales" "$tmp/locales"

# Upstream libcef.so carries debug symbols (~1.4 GiB); release builds ship it
# stripped (~285 MiB).
if [[ "$profile" == "release" ]]; then
  strip --strip-unneeded "$tmp/libcef.so"
fi

rm -rf "$output"
mv "$tmp" "$output"
trap - EXIT
printf 'distribution: %s\n' "$output"
