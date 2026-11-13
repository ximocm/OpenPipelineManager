# Scientific workflow workspace

This design applies the local `desktop-ui-design` and `designing-gnome-ui`
skills to OPM's existing Rust/GTK4/Cairo application. Linux, Windows and macOS
remain supported. This is an incremental workspace refinement.

## Audit and decisions

The graph is the primary document. Tools/files are navigation on the left;
step details are subordinate on the right; execution output is below. Repeated
dark panel borders, dotted splitters, compact headers and low-contrast status
fills currently give every region equal weight. The inspector has little
separation between metadata, values and validation. Command text is truncated.
The step dialog reserves fixed space above a separately scrolling field list,
which crowds long forms and small windows. Logs lack reading controls.

Keep GTK4, Cairo, native file dialogs, document tabs and the existing menu
catalog. Keep the three edge classes and the shared render/hit-test transform, node dimensions,
stored positions, transform, gestures, validation and execution gates. Keep
all persistence, dirty-state guards, confirmations and Design/Work boundaries.
Do not change field parsing or value commit timing during a visual redesign.

Use quiet bordered utility panes and a large, contrasting graph surface;
resizable panes with useful minima; section headings and readable code blocks;
a single scrollable configuration form with fixed Cancel/Save controls;
explicit log following and wrapping controls. Preserve the collapsible Tools
panel, with a normal sized symbolic control and a keyboard focus indicator.

GNOME deviations are deliberate: retain the discoverable cross-platform menu
bar instead of replacing the full command catalog with a hamburger menu;
retain GTK forms instead of adding AdwEntryRow/AdwSpinRow because field parsing
and invalid-value handling already have tested contracts. Keep the custom
Cairo graph because native list widgets cannot represent its spatial editing.

Libadwaita's OverlaySplitView and preferences rows would simplify adaptive
panes and forms. Adding it also adds a system library, Rust binding version
alignment with gtk4 0.11, minimum-version decisions, and changes to Linux,
MSYS2/Windows and Homebrew/macOS packaging and CI. No dependency is added here.

References: [GNOME utility panes](https://developer.gnome.org/hig/patterns/containers/utility-panes.html),
[accessibility](https://developer.gnome.org/hig/guidelines/accessibility.html).
The companion reference files named by the GNOME skill are absent locally.

## Design system

| Token | Light | Dark | Use |
|---|---|---|---|
| Document | #ffffff | #15181c | Node cards, fields, editor |
| Workspace | #f6f8fa | #1f2429 | Canvas, console, panel chrome |
| Text | #1f2328 | #e6e6e6 | Titles and content |
| Secondary text | #57606a | #a9b4bf | IDs, captions, hints |
| Border | #d0d7de | #46515d | Panel and card outlines |
| Accent | #1565c0 | #78b8ff | Selection and focus |
| Success | #2e7d32 | #82cf8a | Completed |
| Warning | #9a5000 | #ffc078 | Running, warnings |
| Danger | #c62828 | #ff8c86 | Failed, blockers |
| Data link | #0f766e | #64d8cb | Output links with open arrows |
| Dependency | #7440b8 | #c4a0ff | Editable solid links |

System UI font; monospace for commands, IDs and output. Body inherits desktop
font size; headings use weight rather than oversized type. Spacing scale:
4/8/12/16/24 px. Pane radius 6 px, card radius 8 px, borders 1 px.
Controls retain native GTK hover/disabled behavior; focus uses a visible accent
outline. Status always includes a word, not color alone. Selected cards have
an accent outline. Read-only code is selectable and wraps. Long names and
paths wrap or ellipsize with their full text available in tooltips.

## Interaction and state contract

Native controls keep Tab/Shift+Tab, Enter, Space and normal text shortcuts.
Canvas: Ctrl+wheel zoom, drag empty space to pan, drag cards to move,
drag any of four ports to edit dependencies, Delete/Backspace removes a selected dependency,
Escape exits brush mode. Add keyboard graph selection and run check toggling,
keyboard pan/zoom, and accessible descriptions. Each node has top, bottom,
left and right ports. Edge geometry chooses facing ports according to relative
node positions; it adapts automatically between vertical and horizontal flows.
The active drag preview starts at the chosen port. All links are continuous:
order is thin with chevrons, output has open arrows, dependency has filled
arrows. Running edges pulse in width when GTK animations are enabled. No
orientation or port metadata is added to the document format. Cairo node children are not
individual AT-SPI objects; full screen-reader graph editing remains a limitation.

Menus remain File (New/Open/Open Folder/Save/Save As/Close Tab), Project
(New/Open/Import/Export/Save pipeline; New/Open work; Update from library),
Edit (Undo/Redo/Find/Replace), Pipeline (Marketplace/Create step/Run selected/Stop),
View (Markdown/Visual, Explorer), Help (Shortcuts, About). Existing dialog labels
and commands are preserved. Toolbar actions mirror those commands.

| Action | Existing shortcut (all current builds) |
|---|---|
| New text / Open | Ctrl+N / Ctrl+O |
| Open folder | Ctrl+Shift+O |
| Save / Save as | Ctrl+S / Ctrl+Shift+S |
| Close tab | Ctrl+W |
| Undo / Redo | Ctrl+Z / Ctrl+Shift+Z |
| Find / Replace | Ctrl+F / Ctrl+H |
| Toggle Markdown/Visual | Ctrl+Alt+M |
| Toggle Explorer | Ctrl+B |
| Canvas select previous/next | Left / Right |
| Canvas run checkbox | Space |
| Canvas pan | Alt+arrows |
| Canvas zoom / fit | + / - / Home |

This work preserves the current Ctrl-based cross-platform shortcut map rather
than making an untested macOS modifier change. No new OS-reserved chords.

Document state (steps, dependencies, values, layout) keeps its existing native
document/work persistence. Session state keeps explorer visibility/width and
the last-work TTL. Viewport, graph selection, log wrap/follow and in-progress
drags remain per-window and ephemeral. Theme follows GTK settings; no new
preference format. First launch retains the existing empty workspace flow.

## Implementation and review checklist

- [x] Context, pane hierarchy, control patterns and dependency decision defined.
- [x] Shared light/dark colors, typography, borders, focus and statuses.
- [x] Workspace priority, resize behavior and symbolic pane control.
- [x] Cards and edges retain geometry and gestures with improved readability.
- [x] Inspector grouping, long text and accessible field names.
- [x] Scrollable dialog, fixed actions, Escape and labeled controls.
- [x] Logs follow/wrap controls and readable empty/selected states.
- [x] Running-app screenshots: baseline, light, dark, 800x600, dialog, logs.
- [x] Check high contrast, large text, keyboard focus and long content.
- [x] Core/UI tests, release build and release-policy checks; record limitations.

Verification results and screenshot paths are recorded in `docs/qa/workspace.md`.

## 0.3.0 launch page

Bare launch opens a native start page with two groups: Pipeline design
(Open/New/Import) and Work folders (Open/New/Browse). GtkFlowBox wraps the
cards on narrow windows; GtkScrolledWindow keeps actions reachable with large
text. Buttons reuse the existing `win.*` actions, native dialogs and validation
gates. One primary action is emphasized. Open file and New text remain available.

A valid recent work folder is offered explicitly instead of reopening itself.
The existing one-hour TTL, state file and invalid-session cleanup remain. A
missing/corrupt recent folder produces an inline message. Explicit command-line
paths bypass the start page. Save on the start page does nothing; New text
reuses the initial blank tab. Ctrl+W closes the start window.

- [x] Native controls, shared palette, symbolic icons, keyboard actions.
- [x] Review real light/dark and 800×600 captures.
- [x] Verify start-page Save, New text, recent-work reopening and file-path bypass.
- [x] Verify large-text start page and failed-recent notice.
