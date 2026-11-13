# Start page and README gallery — 0.3.0

Reviewed on 2026-11-12, Linux/X11, GTK 4.14.5 and Cairo.

The [start page](../screenshots/welcome-light.png) replaces the blank initial
editor on bare launch. It reuses the existing window actions for pipeline
New/Open/Import, work-folder New/Open, ordinary folders and text files. The
valid recent work folder is offered for explicit reopening. Command-line
paths still open directly. See the [architecture decision](../../DECISIONS.md).

The [`readme_capture::readme_screenshots`](../../ui/src/readme_capture.rs)
graphical test ran in separate light and dark sessions using temporary
libraries, configuration and project folders. It verified:

- Start-page Save has no document target; the visible page remains unchanged.
- New text leaves the start page and reuses the initial blank tab.
- Recent work opens the matching Work window.
- An explicit text-file argument bypasses the start page.
- An invalid recent folder is reported inline and its stale session slot is
  cleared. [Screenshot](evidencias/START-unavailable.png).
- At 200% text size the start page remains 800×600; cards wrap and scroll.
  [Screenshot](evidencias/START-large.png).
- The four-step screenshot fixture executes successfully and writes exactly
  four retained reads and 46 bases to its report.

The [README gallery](../screenshots/README.md) includes 13 real app captures.
Its Marketplace image was captured from the live public registry. The README
was also rendered locally with CommonMark tables and inspected in Chromium;
this is a layout preview, not a claim of exact GitHub rendering. All 59 README
links/image references and local gallery targets were checked for missing files.

## Required checks

| Command | Result |
|---|---|
| `cargo test --locked --manifest-path core/Cargo.toml --workspace` | 396 passed |
| `cargo test --locked --manifest-path ui/Cargo.toml` | 190 passed, 10 ignored by default |
| `cargo build --locked --release --manifest-path ui/Cargo.toml` | Passed |
| `python3 scripts/test_release.py` | 22 passed |
| `git diff --check` | Passed |

The new ignored capture test was explicitly run for light and dark. Other
pre-existing graphical/network tests were not rerun for this change. The
release-policy test's old ZIP timestamp normalization message is expected
fixture output. Core/UI manifests and all OPM package lock entries agree on
0.3.0. No tag, commit or release was created.

The start page was not tested with a screen reader or on Windows/macOS. The
existing Cairo graph accessibility limitations remain documented in the
[workspace QA record](workspace.md).
