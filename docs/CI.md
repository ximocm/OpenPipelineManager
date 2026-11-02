# CI and releases

The workflows run on GitHub Actions. Contributions are reviewed through
pull requests into `dev`; releases are tagged from `main` (see
[GITGUIDELINES.md](../GITGUIDELINES.md) for the branch model).

## Platform coverage

| Platform | Tests | Application and package |
| --- | --- | --- |
| Ubuntu 24.04 x86_64 | Core workspace and UI logic tests, release mode | Release binary and `.tar.gz` |
| Windows Server 2022 | Core workspace, release mode | `opm.exe` portable ZIP and MSI |
| macOS 15 (Apple Silicon) | Core workspace, release mode | `opm.app` in `.dmg` and `.tar.gz` |

The core workspace — `opm-document`, `opm-parser`, `opm-theme`, `opm-fs`,
`opm-pipeline`, `opm-exec` and `opm-store` — is tested on all three operating
systems. `opm-fs` has no file watchers, so the whole workspace runs
identically everywhere. The UI logic tests run on Linux only, matching the
layout of `ci.yml`: on Windows and macOS the UI is compiled and packaged,
which checks that it builds and starts, but does not replace manual
graphical QA.

## Continuous integration

[ci.yml](../.github/workflows/ci.yml) runs on every pull request, on pushes
to `main`, and as a reusable workflow (`workflow_call`) from the release
workflow. It uses stable Rust and dependencies locked by the committed
`Cargo.lock` files; the test matrix runs every step through `bash`, including
on Windows. All actions are pinned to commits, the workflow token is
read-only, concurrency cancels superseded runs of the same ref, and
`CARGO_TARGET_DIR` points at the workspace `target/` directory.

| Job | Runner | What it does |
| --- | --- | --- |
| Release policy | ubuntu-24.04 | `python3 scripts/test_release.py` — tag rules and package-content regression tests, no Rust build |
| Core (ubuntu-24.04) | ubuntu-24.04 | Installs GTK 4, tests core + UI logic in release mode, builds the release binary, packages `linux-x86_64` |
| Core (windows-2022) | windows-2022 | Installs WiX and gvsbuild, tests core in release mode, builds and packages `windows-x86_64` |
| Core (macos-15) | macos-15 | Installs GTK 4 from Homebrew, tests core in release mode, builds and packages `macos-aarch64` |
| CI success | ubuntu-24.04 | Aggregator: passes only when the whole matrix and the release policy succeed |

The dependency cache is restored before the build and saved even when tests
fail. Its key combines the runner OS and architecture, the `rustc` version,
and the hash of `core/Cargo.lock` plus `ui/Cargo.lock`; the registry, git
checkouts and `target/` share one cache entry per key.

On Linux, the matrix installs `libgtk-4-dev` and `pkg-config`, runs
`cargo test --locked --release --manifest-path core/Cargo.toml --workspace`
and `cargo test --locked --release --manifest-path ui/Cargo.toml`, builds
the release binary, and runs `python3 scripts/package-linux.py`, which fails
on any `ldd` "not found" library. The `linux-x86_64` artifact is kept for
14 days, so a package can be reviewed from the pull request without
publishing a release.

On Windows, the job installs WiX v6 as a `dotnet` tool
(`dotnet tool install --global wix --version 6.0.2`), installs gvsbuild,
and caches `C:\gtk-build\gtk` under a key that combines the gvsbuild
version with a digest of `GVSBUILD_PROJECTS` (`gtk4 pkgconf`). On a cache
miss it builds those projects with MSYS2 ahead of Git Bash and
`--use-env`. The environment is then configured with the gvsbuild `bin`
directory on `PATH`, `PKG_CONFIG_PATH` pointing at its `lib/pkgconfig`, and
`PKG_CONFIG_ALLOW_SYSTEM_LIBS=1` (without it `pkgconf` hides the `-L` of
its own prefix and `link.exe` fails with `LNK1181: cannot open input file
'gtk-4.lib'`). `scripts/package-windows.py` walks the PE import tables of
`opm.exe` to copy the non-system DLL closure, launches the packaged
executable under a timeout to rule out `0xC0000135` (missing DLL), and
produces the portable ZIP and the MSI. The `windows-x86_64` artifact is
kept for 14 days. The job allows 150 minutes: the first gvsbuild build
consumes a large part of the budget.

On macOS, the matrix installs GTK 4 with Homebrew and
`scripts/package-macos.py` builds `opm.app`: it copies the non-system
dylibs into `Contents/Frameworks` while walking `otool -L`, rewrites their
paths with `install_name_tool` to `@executable_path`/`@loader_path` (no
Homebrew path may remain), signs ad-hoc with `codesign`, launches the
binary briefly to rule out missing dylibs and creates the `.dmg` and the
`.tar.gz`. The `macos-aarch64` artifact is kept for 14 days.

Each artifact upload fails if `dist/` is empty. Every package carries the
binary, `LICENSE`, `NOTICE`, `README.md`, `BUILDING.md`, `CHANGELOG.md`, a
generated `INSTALL.txt`, the `third-party/` license inventory including
`minmark-Apache-2.0.txt`, and `shared-libraries.txt`. `SHA256SUMS` is written
beside the archives in `dist/` and published as its own release asset, not
inside the packages.
Details in [Publishing](PUBLISHING.md).

## Release workflow

[release.yml](../.github/workflows/release.yml) triggers on tags `v*` and
on manual dispatch with a `tag` input.

| Job | Runner | What it does |
| --- | --- | --- |
| validate | ubuntu-24.04 | `python3 scripts/validate-release.py <tag>` — tag format, workflow ref, ancestry, versions |
| checks | reusable | Runs the complete CI workflow via `workflow_call` on the same ref |
| publish | ubuntu-24.04 | Verifies checksums and creates the draft release `opm <tag>` |

`validate-release.py` enforces all of the following:

- the tag matches `v\d+\.\d+\.\d+(-…)?`, for example `v0.2.0` or
  `v0.2.0-rc.1`;
- the tag's commit is the commit checked out as the workflow ref (select
  the tag as the ref, not `main`);
- the tag's commit is an ancestor of `origin/main`;
- the tag equals the version in `core/Cargo.toml`
  (`[workspace.package].version`) and in `ui/Cargo.toml`
  (`[package].version`), and both manifest versions are equal.

Only the `publish` job gets `contents: write`; everything else stays
read-only. It downloads the three platform artifacts, merges their files
into one `dist/`, concatenates the per-platform `SHA256SUMS`, runs
`sha256sum --check` over the result, and creates a **draft** release titled
`opm <tag>` with `--verify-tag`, the validated target commit and generated
notes. The workflow never pushes code and never modifies branch
protections; a draft keeps the final publication under the maintainer's
control.

### Re-running a failed workflow

Use **Re-run jobs** for the run that failed, or **Run workflow** selecting
the same tag as the workflow reference and entering it in the `tag` input —
`validate-release.py` rejects any other combination. If a release already
exists for that tag, `publish` stops with "Release already exists; review
it manually before retrying": inspect the existing draft before deciding
whether to delete it and rerun. Never overwrite the assets of an existing
release.

## Cutting a release

1. Choose the version and update both manifests to the same value in one
   change: `core/Cargo.toml` `[workspace.package].version` and
   `ui/Cargo.toml` `[package].version`. Refresh and commit the affected
   lockfiles (any Cargo command without `--locked` rewrites them), and move
   the finished entries of `Unreleased` in `CHANGELOG.md` to a section with
   the real version and date. Leave pending work out of the notes.
2. Open the release pull request `dev` → `main` and merge it only after CI
   passes, as required by [GITGUIDELINES.md](../GITGUIDELINES.md).
3. From that commit on `main`, create and push the tag:

   ```sh
   git switch main
   git pull --ff-only
   git tag -a v0.2.0 -m 'opm 0.2.0'
   git push origin v0.2.0
   ```

4. [release.yml](../.github/workflows/release.yml) validates the tag and
   versions, reruns the full CI matrix through `workflow_call`, and creates
   a **draft** release titled `opm <tag>` containing the Linux, Windows and
   macOS artifacts (`.tar.gz`, `.zip`, `.msi`, `.dmg`), `SHA256SUMS` and
   the generated notes.
5. Review the draft: read the notes, verify the checksums, test the
   application in a graphical session on the platforms you can reach, then
   publish it manually. Mark test versions as **prerelease** by hand — the
   workflow never sets that flag.

## Reproduce locally

```sh
cargo test --locked --release --manifest-path core/Cargo.toml --workspace
cargo test --locked --release --manifest-path ui/Cargo.toml
cargo build --locked --release --manifest-path ui/Cargo.toml
python3 scripts/test_release.py

# Package the current platform (one per operating system):
python3 scripts/package-linux.py    # Linux x86_64 -> .tar.gz
python scripts/package-windows.py   # Windows x86_64 -> .zip and .msi
python3 scripts/package-macos.py    # macOS arm64 -> .dmg and .tar.gz

(cd dist && sha256sum --check SHA256SUMS)   # macOS: shasum -a 256 --check SHA256SUMS
```

Each packaging script needs Python ≥ 3.11, its own operating system, and
the dependencies of [BUILDING.md](../BUILDING.md) (on Windows additionally
the .NET SDK with WiX v6 and the gvsbuild environment; on macOS the Xcode
Command Line Tools). All of them respect `CARGO_TARGET_DIR` when it is
defined. The release policy tests need no Rust build at all. None of this
replaces manual checks of windows, dialogs, the canvas and live logs before
publishing.
