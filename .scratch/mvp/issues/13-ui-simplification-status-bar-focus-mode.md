# 13 — UI simplification + status bar + focus mode

**What to build:** The "strip Zed's product surfaces" pass that makes this feel like a notes app rather than a code editor. Minimal custom chrome: native traffic lights, integrated titlebar, subtle pane separation, the note visually dominant. A notes status bar shows Vim mode, file path, language, word/character count, cursor position, and save state — no Git or LSP indicators. `View: Toggle Focus Mode` hides explorer, preview, and chrome details, leaving a centered editor. System/Light/Dark themes stay coordinated between UI and syntax. The macOS menu bar gets the minimal App/File/Edit/View/Go/Window structure. Remaining Zed programming-editor surfaces (minimap, diagnostics, completion, LSP indicators) are absent.

**Blocked by:** 04 — Markdown preview; 05 — Quick open + command palette; 06 — Note commands.

**Status:** ready-for-agent

- [ ] Status bar shows Vim mode (left), relative file path (center), language + word count + Ln/Col + save state (right); word count updates as you type
- [ ] `Note: Statistics` reports word and character counts
- [ ] View: Toggle Focus Mode leaves only a centered editor; toggling back restores the previous layout
- [ ] Theme follows the system by default; Light and Dark are selectable and UI + syntax colors stay coordinated
- [ ] Window chrome is minimal: native traffic lights, no ribbon, no heavy borders; no minimap, diagnostics gutter, completion, or LSP/Git status anywhere
- [ ] macOS menu bar has the spec's App/File/Edit/View/Go/Window structure wired to the same actions
- [ ] UI, editor, and preview fonts are independently configurable and default per the spec
