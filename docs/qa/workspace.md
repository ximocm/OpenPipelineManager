# GTK workspace review — 2026-11-12

Design and usability decisions: [UI_DESIGN.md](../UI_DESIGN.md).
Implementation branch: `feat/gtk-workspace-redesign`, based on the existing
dirty `dev` checkout. Existing marketplace/storage work was retained; nothing
was committed or pushed. Development version: 0.4.1 in both manifests and locks.

## Environment and method

Linux/X11 (XFCE), GTK 4.14.5, rustc 1.98.1. The offline
`workspace_qa::workspace_visual_review` test constructs the production GTK
windows through `build_window`, maps them on the desktop display, and captures
their rendered client areas with `WidgetPaintable`/GSK's Cairo renderer. These
are running application captures, without window-manager decorations. They
are not mockups. State, libraries and generated runtime files are isolated
under `/tmp`; the user's projects and library are not used.

Baseline captures were inspected before implementation. Subsequent captures
identified narrow-canvas allocation, clipped tool controls, missing active-tab
emphasis, and overly wide large-text layouts. Those findings led to minimum
canvas sizes, scrollable native toolbars and tab strips, readable navigator
controls, and an explicit active-tab style. Final images were inspected after
those changes. Small-window graph captures use Fit, so the diagram and its text
scale together; users can zoom in or collapse Tools for larger cards.

## Screenshot evidence

| Capture | What was reviewed |
|---|---|
| [Before](evidencias/UI-before.png) | Original workspace hierarchy and truncated command |
| [Design](evidencias/UI-design.png) | Tools, graph priority, active tab and inspector |
| [Horizontal flow](evidencias/UI-horizontal.png) | Side ports and continuous links in a horizontal layout |
| [Light](evidencias/UI-light.png) | Three-pane Work workspace, focus ring and logs |
| [Dark/completed](evidencias/UI-dark.png) | Theme contrast, completed cards and real example log |
| [800×600](evidencias/UI-small.png) | Narrow workspace, visible graph and Fit controls |
| [Dialog](evidencias/UI-dialog.png) | Identity, environment, command, initial focus, fixed actions |
| [Dialog fields](evidencias/UI-fields.png) | Grouped declarations, source-output picker and field actions |
| [Validation](evidencias/UI-validation.png) | Blocker counter and inline select validation |
| [High contrast](evidencias/UI-contrast.png) | `GTK_THEME=HighContrast`, borders and visible focus |
| [Large text](evidencias/UI-large.png) | `Sans 22` (200% of the 11-point reference), 800×600 and scrollbars |
| [Live theme change](evidencias/UI-theme-switch.png) | Existing Work window restyled after GTK setting changes |

## Behavioral checks

- Five isolated review sessions passed: Adwaita light, Adwaita dark,
  HighContrast, 200% text and live desktop appearance changes.
- GTK controller dispatch selected steps, toggled their run selection through
  the store, zoomed, fitted, dragged nodes and panned. Tests assert moved node
  coordinates remain in world space and that panning changes the viewport.
- Cards have four functional ports. Pure interaction tests start connections
  from each side under zoom/pan, accept drops in the port hit area outside the
  destination card, and reconnect from the top port of a selected vertical
  dependency. Geometry tests cover both axes and both directions, approach
  cards from outside, and select the nearest port when hit areas overlap.
- Final vertical and horizontal screenshots use continuous links throughout,
  including drag previews and running edges. Order has a thin chevron, data
  flow an open arrowhead, and dependencies a filled arrowhead; broken strokes
  are no longer used to distinguish link types.
- The repository's two-step `examples/first-run.json` ran through the production
  coordinator/engine in temporary Work folders. Both steps completed and
  `outputs/report.txt` exactly matched the expected message and `mode=demo`.
- Wrap switches the native log view's wrapping mode. Follow remains enabled
  initially; turning it off suppresses automatic end scrolling.
- A live GTK appearance change preserved the graph transform, selection,
  editor content and the byte-for-byte contents of saved `run.json`.
- Unit coverage includes dependency dragging/deletion with the read-only gate,
  stale dependency selection, keyboard navigation without document mutations,
  shared render/hit-test transforms, cursor-anchored zoom, deferred node drags,
  and the existing validation and execution contracts.

## Required quality gate

| Command | Result |
|---|---|
| `cargo test --locked --manifest-path core/Cargo.toml --workspace` | 396 passed, 0 failed |
| `cargo test --locked --manifest-path ui/Cargo.toml` | 190 passed, 0 failed, 9 ignored by default |
| `cargo build --locked --release --manifest-path ui/Cargo.toml` | Passed |
| `python3 scripts/test_release.py` | 22 passed |
| `git diff --check` | Passed |

The new ignored graphical review test was explicitly run in all five modes;
the other eight pre-existing ignored graphical/network tests were not run.
The release tests' old-ZIP-timestamp normalization warning is expected fixture
output, not a failure. Intermediate compile errors in review helpers and an
opaque-grid-color assertion were corrected. The grid test now checks visible
contrast at the transformed right edge, allowing the intended translucent grid.

Reproduce from the repository root with a desktop display:

```sh
python3 scripts/review-workspace.py --output /tmp/opm-workspace-review
```

This opens native windows and executes only the self-contained repository
example in isolated temporary Work folders. It does not access the registry.
Use `--mode light`, `dark`, `contrast`, `large` or `live` for an individual pass.

## Limits

Visual review was performed on Linux/X11. Windows, macOS, Wayland, Orca, an
on-screen keyboard, touch, and exhaustive physical keyboard/mouse navigation
were not tested. Native GTK controls retain their keyboard and accessibility
semantics; custom controls have descriptive accessible names, required-field
properties, help and visible focus. Individual Cairo nodes/edges are not
separate AT-SPI objects, and dependency editing still requires pointer input.
The graph is not claimed to be fully screen-reader accessible.

At large text sizes, narrow toolbars/panes require scrolling. Inspector sections
also scroll vertically. This retains the three-pane desktop arrangement; it
does not implement libadwaita's adaptive overlay-pane behavior. No libadwaita
dependency or packaging change was introduced.
