# 04 — Markdown preview

**What to build:** A readable rendered view of the current note. `Cmd+Shift+V` toggles an Editor | Preview split using Zed's Markdown preview; edits re-render after a short debounce; content is constrained to a readable width. Local images render; remote images stay off by default; extremely large documents require explicit opt-in to render.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-human

- [x] `Cmd+Shift+V` (Note: Toggle Preview) opens/closes an Editor | Preview split of the active note
- [x] Rendering covers headings, emphasis, links, images, ordered/unordered/task lists, blockquotes, inline and fenced code, horizontal rules, and tables
- [x] Preview refreshes ~150 ms after the last edit; typing in the editor never stutters while the preview updates
- [x] Content width is constrained (default 760 px, configurable) and centered in the pane
- [x] Local images resolve relative to the note's file (PNG/JPEG/GIF/WebP, SVG where supported); remote images do not load by default
- [x] Documents over ~5 MB show "Preview disabled for large document" with a Render Anyway affordance
- [x] A preview-only mode (Note: Open Preview) exists as a command

## Comments

**2026-08-22 — implementation complete; manual acceptance pending.**

zednotes now initializes Zed's Markdown preview subsystem and exposes notes-specific `Note: Toggle
Preview` and `Note: Open Preview` actions. `Cmd+Shift+V` opens a right-hand preview while retaining
editor focus, then closes that preview and its empty pane on the next invocation. The preview-only
action opens the rendered note as a tab in the current pane.

The reused preview renders Zed's full Markdown surface and resolves local image paths relative to
the source note. zednotes defaults preview content to a centered 760 px maximum width, blocks HTTP
and HTTPS images, and requires explicit `Render Anyway` approval above 5 MiB. These defaults can be
overridden under `preview` in `~/.config/zednotes/settings.json` with `max_width`,
`allow_remote_images`, and `max_file_size_bytes`.

The generic preview gained small configurable policy hooks while preserving Zed's own defaults. Its
refresh is now a true 150 ms trailing debounce, and oversized source is checked before converting
the rope to a string or starting Markdown parsing, keeping the editor responsive.

Tests cover split toggle/open-only behavior, preview defaults and JSONC overrides, trailing debounce,
the large-document gate/approval path, existing local/remote-worktree image resolution, and the
upstream preview regression suite. The preview and notes-app test suites, all-target check, and
release clippy pass. `cargo run -p notes_app` also built, launched, and remained running during a
process smoke check.

Manual acceptance remains for visual inspection of the split, typography, representative Markdown
constructs, local image formats, and the large-document prompt in the live app.
