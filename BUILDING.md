# Building Open Pipeline Manager

All commands in this guide start from the repository root. The application
needs a graphical session to run; Linux is the primary platform, and Windows
and macOS packages are built from the same sources in CI (see
[DECISIONS.md](DECISIONS.md) for the stack decision).

## Requirements

- Recent stable Rust and Cargo. No minimum Rust version (MSRV) has been set;
  the documentation review used `rustc 1.98.1`.
- A system C compiler and linker, plus `pkg-config`.
- GTK ≥ 4.10 with development headers. The desktop app embeds no webview.
- Python ≥ 3.11 for the packaging scripts (they read manifests with
  `tomllib`).

On Ubuntu 24.04:

```sh
sudo apt update
sudo apt install build-essential pkg-config libgtk-4-dev
```

On other distributions install the equivalent packages and check the
versions:

```sh
rustc --version
cargo --version
pkg-config --modversion gtk4
```

The `core/` workspace does not need GTK: it builds with plain Cargo on any
platform. Every packaging script additionally needs Python ≥ 3.11 and its
own operating system.

### On Windows

- Stable Rust with the MSVC toolchain (`rustup default stable-msvc`) and
  Visual Studio 2022 with the C++ workload (already installed on GitHub's
  Windows runners).
- MSYS2 (https://www.msys2.org), which gvsbuild uses for helper tools.
- GTK 4 built with [gvsbuild](https://github.com/wingtk/gvsbuild):

  ```sh
  pip install gvsbuild
  gvsbuild build gtk4 pkgconf
  ```

  The first build takes a long time. Afterwards add
  `C:\gtk-build\gtk\x64\release\bin` to the `PATH`, set
  `PKG_CONFIG_PATH=C:\gtk-build\gtk\x64\release\lib\pkgconfig` and set
  `PKG_CONFIG_ALLOW_SYSTEM_LIBS=1` so Cargo can find GTK through
  `pkg-config`. The last variable is mandatory: `pkgconf` strips the `-L`
  entries of its own system directories — and the gvsbuild prefix is one of
  them — so without it the `-sys` crates never pass the search path to
  `link.exe` and the build fails with
  `LNK1181: cannot open input file 'gtk-4.lib'`.
- For packaging only: the .NET SDK, which hosts WiX as a `dotnet` tool:

  ```sh
  dotnet tool install --global wix --version 6.0.2
  ```

**Running pipelines requires a POSIX shell.** Commands are quoted for a POSIX
shell, so the execution engine looks for `bash` (Git for Windows provides it)
and refuses to start any step with a clear error when no POSIX shell is
found. There is no `cmd.exe` fallback, because the single-argument quoting
guarantee is POSIX-only. Install Git for Windows
(https://git-scm.com/download/win) before running pipelines; the packaged
`INSTALL.txt` repeats this requirement.

### On macOS (Apple Silicon)

```sh
xcode-select --install   # Xcode Command Line Tools, if not installed yet
brew install gtk4 pkg-config
```

GTK 4 comes from Homebrew (`/opt/homebrew`); packaging copies its dylibs
into `opm.app` so the app does not depend on Homebrew at runtime. The
packaging scripts accept Apple Silicon (arm64) only.

## Building

```sh
# Optimized desktop application; builds the core workspace too
cargo build --locked --release --manifest-path ui/Cargo.toml

# Core workspace only
cargo build --locked --manifest-path core/Cargo.toml --workspace

# Development: build and open a file
cargo run --locked --manifest-path ui/Cargo.toml -- path/to/file.md
```

`--locked` keeps the versions from the committed `Cargo.lock` files. The
first build needs access to the dependency registries; once everything is
downloaded you can add `--offline`. If the compiler runs out of memory,
retry with `-j 1`.

Without a custom `CARGO_TARGET_DIR`, binaries end up in
`target/release/opm` (`opm.exe` on Windows). The `.cargo/config.toml` at the
repository root points both `core/` and `ui/` at the same `target/`
directory, so dependencies shared by the two manifests are compiled once
instead of twice when profiles and features match.

The development profile disables debug info and the incremental cache to
save disk space; this limits debugger detail and can slow recompilation. The
release profile strips symbols from the executable.

## Running locally

```sh
./target/release/opm
./target/release/opm file.md another.md
./target/release/opm my-project/
```

Optionally copy the executable somewhere on your `PATH`:

```sh
install -Dm755 target/release/opm "$HOME/.local/bin/opm"
```

This installs only the executable: GTK must stay installed system-wide, and
no package, desktop launcher or shared libraries are installed. To remove
that copy, delete `~/.local/bin/opm`. See the [README](README.md) for the
project files and local data used at runtime.

## Packaging per platform

[CI and releases](docs/CI.md) explains the full matrix. The same scripts run
locally; each one writes its platform's files into `dist/` next to
`SHA256SUMS` (one line per artifact). All of them need Python ≥ 3.11 and
respect `CARGO_TARGET_DIR`.

### Linux

```sh
cargo build --locked --release --manifest-path ui/Cargo.toml
python3 scripts/package-linux.py
```

`ldd` must not report any "not found" library; the script fails otherwise.
Linux x86_64 only.

### Windows

```powershell
cargo build --locked --release --manifest-path ui/Cargo.toml
dotnet tool install --global wix --version 6.0.2
python scripts\package-windows.py
```

- WiX v6 is the MSI toolchain (the `dotnet` tool installed above; the script
  prints the install command if `wix` is missing).
- The script walks the PE import and delay-import tables of `opm.exe`,
  copies every non-system DLL it needs next to the executable (GTK 4 DLLs
  and the VC++ runtime, which travels even though it lives in `System32`),
  launches the staged executable for 15 seconds (it would exit with
  `0xC0000135` if a DLL were missing) and produces the portable `.zip` and
  the `.msi`. Windows x86_64 only.

### macOS

```sh
cargo build --locked --release --manifest-path ui/Cargo.toml
python3 scripts/package-macos.py
```

- Apple Silicon only; uses `otool`, `install_name_tool`, `codesign` and
  `hdiutil` (Xcode Command Line Tools).
- The script copies the non-system Homebrew dylibs into
  `opm.app/Contents/Frameworks`, rewrites their paths to
  `@executable_path`/`@loader_path`, signs ad-hoc (Developer ID signing and
  notarization are pending: Gatekeeper warns on first launch), verifies that
  no Homebrew path remains, launches the binary for 10 seconds and creates
  the `.dmg` and the `.tar.gz`.

### Verify the checksums

```sh
(cd dist && sha256sum --check SHA256SUMS)
```

On macOS use `cd dist && shasum -a 256 --check SHA256SUMS` instead.

## Tests and checks

The automated matrix and the package generation are explained in
[CI and releases](docs/CI.md). CI runs the core workspace tests on all three
operating systems in release mode; the UI logic tests are built and run on
Linux only.

```sh
# Quality gates: run before handing work back
cargo test --locked --manifest-path core/Cargo.toml --workspace
cargo test --locked --manifest-path ui/Cargo.toml
cargo build --locked --release --manifest-path ui/Cargo.toml
python3 scripts/test_release.py

# What CI runs for the core workspace on every operating system
cargo test --locked --release --manifest-path core/Cargo.toml --workspace
```

`python3 scripts/test_release.py` covers release-tag validation and package
contents without building any Rust. The UI logic tests open no windows, but
the test binaries still need GTK to compile and link, and they do not
replace manual checks of dialogs, canvas interactions and logs.

If you have the `rustfmt` and `clippy` components installed, these are
useful local checks (CI does not run them):

```sh
cargo fmt --manifest-path core/Cargo.toml --all -- --check
cargo fmt --manifest-path ui/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path core/Cargo.toml --workspace --all-targets
cargo clippy --locked --manifest-path ui/Cargo.toml --all-targets
```

Before publishing a binary, manually verify opening and saving a project,
Markdown and Visual Mode editing, undo/redo, importing a pipeline, canvas
edits (create step, connect, delete), running selected steps with live logs,
and the validation blockers panel.

## Troubleshooting

### Cargo cannot find `Cargo.toml`

There is no workspace at the repository root. Use
`--manifest-path core/Cargo.toml` or `--manifest-path ui/Cargo.toml`, as in
the examples above.

### GTK is missing or too old

Check `pkg-config --modversion gtk4`. You need the **development** packages
and GTK ≥ 4.10. If you use a custom install prefix, set `PKG_CONFIG_PATH`
for that prefix.

### `LNK1181: cannot open input file 'gtk-4.lib'` (Windows)

`PKG_CONFIG_ALLOW_SYSTEM_LIBS=1` is missing or `PKG_CONFIG_PATH` does not
point at `C:\gtk-build\gtk\x64\release\lib\pkgconfig`; see
[On Windows](#on-windows).

### Pipelines refuse to run on Windows

The engine requires a POSIX shell (`bash` from Git for Windows) and stops
with a clear error before starting anything; there is no `cmd.exe` fallback.
See [On Windows](#on-windows).

### Cannot open the display

Run the application inside a Wayland or X11 session. Compiling and running
the tests does not require opening the application.

### `--locked` fails

A dependency change may require updating the affected lockfile. In
development, run Cargo without `--locked` to resolve it and review the
`Cargo.lock` diff. To reproduce a release, use the manifest and lockfile of
the same commit.
