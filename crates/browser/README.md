# blob-1-browser

`blob-1-browser` is the headless CEF companion for `blob-1 serve`. It listens on a Unix socket, renders off-screen at 1280x800 by default, and keeps one persistent CEF request context per profile.

## Build requirements (Linux x86_64)

- Rust and Cargo
- CMake, Ninja, and a C++ compiler
- Network access on the first build

The `cef-dll-sys` build script downloads the CEF binary distribution that matches `cef` v158, extracts it under Cargo's build output, compiles `libcef_dll_wrapper`, and copies the Linux runtime files into `target/debug` or `target/release`. To reuse one CEF download across clean builds, set `CEF_PATH`; the crate accepts either a directory containing the extracted `cef_linux_x86_64` tree or a parent under which it can download `<cef-version>/cef_linux_x86_64`.

Build:

```sh
cargo build -p blob-1-browser --bin blob-1-browser
```

On this machine, use the rustup Cargo shim explicitly and the uv-installed Ninja:

```sh
PATH="$HOME/.local/bin:$PATH" ~/.cargo/bin/cargo build -p blob-1-browser --bin blob-1-browser
```

`crates/browser` is a workspace member but not a default member. A plain `cargo check` or `cargo test` at the workspace root does not download or compile CEF. Naming the package, or explicitly passing `--workspace`, does.

## Runnable distribution

Produce a self-contained directory:

```sh
crates/browser/scripts/dist.sh --release
# default output: target/browser-release-dist
# or choose one:
crates/browser/scripts/dist.sh --release /tmp/blob-browser-dist
```

Set `CARGO=$HOME/.cargo/bin/cargo` first if the Cargo on `PATH` is not usable. The output directory contains:

- `blob-1-browser`
- `libcef.so`
- `libvk_swiftshader.so`, `libvulkan.so.1`, `vk_swiftshader_icd.json` (when shipped by the CEF revision)
- `icudtl.dat`, `resources.pak`, `chrome_100_percent.pak`, `chrome_200_percent.pak`
- `v8_context_snapshot.bin` (and `snapshot_blob.bin` when shipped)
- `chrome-sandbox`
- `locales/*.pak`

The executable is linked with an `$ORIGIN` rpath, so these files must remain beside it. Chromium renderer, GPU, and utility subprocesses re-execute the same `blob-1-browser` binary; the binary handles that re-exec before parsing its own CLI.

Run with no display server:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY \
  /tmp/blob-browser-dist/blob-1-browser \
  --socket /tmp/blob-browser.sock \
  --profiles-dir /tmp/blob-browser-profiles
```

The socket is replaced when it is a stale Unix socket, is chmodded `0600`, and accepts one client at a time. Frames are newline-delimited JSON using the types in `crates/browser-protocol`.
