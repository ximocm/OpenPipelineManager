# Workspace navigation review — 2026-11-14

Native GTK4/Cairo changes on `feat/explorer-navigation`, collected under
Unreleased. No dependency, pipeline format or execution engine changes.

## Design and scope

- Pipeline, text and image tabs occupy the center pane header instead of a
  second row below a repeated Pipeline/Workspace label.
- Keep default window dimensions, pane widths, splitter positions, document
  font sizes and Cairo card geometry. Header navigation scrolls to reveal the
  active document; it does not enlarge neighboring panes or steal editor focus.
- The explorer uses a quiet sidebar surface, symbolic chevrons and file/folder
  icons, rounded hover/selection backgrounds and the folder name as its header.
  Native New file/New folder buttons call the existing filesystem operations.
- View owns Show All Files, Show Hidden Files and Refresh Files. Visibility
  options are native checked actions, independent per window, retained on
  refresh and disabled in windows without an explorer. Ctrl+Shift+. toggles
  hidden files; Ctrl+B keeps its existing sidebar behavior.
- Cards can move in Work even when structural editing is disabled. Work saves
  positions in its own embedded definition; a store isolation test verifies
  that the library and a second work folder are unchanged, including when the
  first work is running. A real GTK drag is saved and verified after reopening.
- The plain Toggle Logs / Console action (Ctrl+J or View) hides the
  pane without stopping logging. Its previous height, buffer and reading
  controls are retained. The GTK test checks a stateless action without a toolbar button,
  lines received while hidden, and restoration of the divider position.
- The inspector can hide through its header close button or View / Ctrl+Shift+I.
  Card selection reopens it at the previous width; refresh leaves it hidden.
  The GTK test checks both paths and asserts restoration of the divider.
- Initial untitled buffers stay internal. The first real file replaces the
  placeholder; New explicitly creates a document. Edited drafts retain their
  undo/redo history, and closing the final file in Work returns to Pipeline.
- Keep lazy directory loading, ignored paths, symlink restrictions, context
  operations and internal/external drag and drop. No filesystem policy changed.

## Verification

All required local checks passed: 397 core tests, 192 UI tests (11 graphical or
network tests ignored by default), release build, and 22 release-policy tests.

The explicit `explorer_navigation_review` GTK test passed in light, dark,
HighContrast and large text (`Sans 22`). It exercises native action activation
and state changes, filter independence, refresh, active-document selection,
folder expansion, file/pipeline/image switching, image closure, isolation
between windows, and disabled actions in the start page. It asserts unchanged
pane positions/window dimensions across ordinary navigation, and at normal text
size a fully visible active tab at 800×600. Long tab labels at large text sizes retain
horizontal scrolling and a full-path tooltip.

`workspace_visual_review` also passed in light and dark, including canvas
keyboard selection, check toggling, zoom, pan, node dragging, step configuration,
logs, validation and actual execution of the self-contained example. The
generated report retained its expected content.

The first narrow-window iteration exposed tab clipping beneath a permanent
scrollbar. The final header uses overlay scrolling and content coordinates to
reveal the active tab without changing pane sizes. Final screenshots were
inspected after this correction.

Visual inspection used Linux/X11 and GTK 4.14.5. Folder buttons expose their
expanded state, icon buttons have accessible names, and focus was inspected in
normal/high contrast themes. Full screen-reader and Windows/macOS desktop
verification were not performed for this change.

## Reproduce

Run from the repository root, with a desktop display and a fresh temporary
output directory. The script isolates configuration, libraries and work files.

```sh
python3 scripts/review-workspace.py --surface navigation \
  --mode light --mode dark --mode contrast --mode large \
  --output /tmp/opm-navigation-review
python3 scripts/review-workspace.py --mode light --mode dark \
  --output /tmp/opm-navigation-workspace
```

## Actual GTK captures

| Light | Dark |
| --- | --- |
| ![Header tabs and file tree in light mode](evidencias/NAV-light.png) | ![Header tabs and file tree in dark mode](evidencias/NAV-dark.png) |

| View menu | 800×600 |
| --- | --- |
| ![Native checked View actions](evidencias/NAV-view.png) | ![Active file tab remains visible at 800×600](evidencias/NAV-small.png) |

| High contrast focus | Large text |
| --- | --- |
| ![Keyboard focus on the active file](evidencias/NAV-contrast.png) | ![Navigation with Sans 22](evidencias/NAV-large.png) |

![Expanded workspace with the console hidden](evidencias/NAV-console-hidden.png)

![Canvas with the inspector hidden](evidencias/NAV-inspector-hidden.png)
