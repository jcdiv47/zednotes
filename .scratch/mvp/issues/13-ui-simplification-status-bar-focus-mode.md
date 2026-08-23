# 13 — UI simplification + status bar + focus mode

**What to build:** The "strip Zed's product surfaces" pass that makes this feel like a notes app rather than a code editor. Minimal custom chrome: native traffic lights, integrated titlebar, subtle pane separation, the note visually dominant. A notes status bar shows save state, word/character count, language, Vim mode, and cursor position together in the right corner — no file path, Git, or LSP indicators. `View: Toggle Focus Mode` hides explorer, preview, and chrome details, leaving a centered editor. System/Light/Dark themes stay coordinated between UI and syntax. The macOS menu bar gets the minimal App/File/Edit/View/Go/Window structure. Remaining Zed programming-editor surfaces (minimap, diagnostics, completion, LSP indicators) are absent.

**Blocked by:** 04 — Markdown preview; 05 — Quick open + command palette; 06 — Note commands.

**Status:** ready-for-human

- [x] Status bar shows save state, live word/character counts, language, Vim mode, and Ln/Col together on the right; no file path
- [x] `Note: Statistics` reports word and character counts
- [x] View: Toggle Focus Mode leaves only a centered editor; toggling back restores the previous layout
- [x] Theme follows the system by default; Light and Dark are selectable and UI + syntax colors stay coordinated
- [x] Window chrome is minimal: native traffic lights, no ribbon, no heavy borders; no minimap, diagnostics gutter, completion, or LSP/Git status anywhere
- [x] macOS menu bar has the spec's App/File/Edit/View/Go/Window structure wired to the same actions
- [x] UI, editor, and preview fonts are independently configurable and default per the spec

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes replaces the generic dock controls with a compact right-aligned notes status:
Saved/Modified state, live word/character counts, Markdown language, Vim mode, and cursor position.
The file path is intentionally omitted. `Note: Statistics` reports the same counts through the
shared action layer.

`View: Toggle Focus Mode` (also `Cmd+Shift+Enter`) records dock visibility, centered layout, pane
maximization, tab-bar visibility, and status-bar visibility. It activates and centers the source
editor, hides explorer/preview/chrome through pane maximization, and restores the exact prior state
without destroying the preview split. System/Light/Dark actions use one dynamic theme selection for
the UI, Markdown preview, and syntax registry. UI, editor, and preview font family/size settings are
independent, with system sans, bundled monospace, and system sans defaults respectively.

The editor defaults explicitly disable the minimap, programming gutter controls, diagnostics, Git
decorations, automatic completion/edit prediction, inlay hints, LSP, breadcrumbs, and code-action
surfaces. The macOS menu bar is reduced to zednotes/File/Edit/View/Go/Window and reuses the same GPUI
actions as keybindings and the command palette.

`cargo test -p notes_app --lib --no-fail-fast` passes all 79 tests,
`cargo check -p notes_app --all-targets` passes, and `./script/clippy -p notes_app` passes with
warnings denied. Manual acceptance remains for visual spacing, native menu behavior, and a real-window
focus/theme pass.
