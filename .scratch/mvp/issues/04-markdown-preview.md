# 04 — Markdown preview

**What to build:** A readable rendered view of the current note. `Cmd+Shift+V` toggles an Editor | Preview split using Zed's Markdown preview; edits re-render after a short debounce; content is constrained to a readable width. Local images render; remote images stay off by default; extremely large documents require explicit opt-in to render.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-agent

- [ ] `Cmd+Shift+V` (Note: Toggle Preview) opens/closes an Editor | Preview split of the active note
- [ ] Rendering covers headings, emphasis, links, images, ordered/unordered/task lists, blockquotes, inline and fenced code, horizontal rules, and tables
- [ ] Preview refreshes ~150 ms after the last edit; typing in the editor never stutters while the preview updates
- [ ] Content width is constrained (default 760 px, configurable) and centered in the pane
- [ ] Local images resolve relative to the note's file (PNG/JPEG/GIF/WebP, SVG where supported); remote images do not load by default
- [ ] Documents over ~5 MB show "Preview disabled for large document" with a Render Anyway affordance
- [ ] A preview-only mode (Note: Open Preview) exists as a command
