# 09 — Settings + keymap files

**What to build:** Plain-text configuration the user owns. A Zed-style JSONC settings file in the user's config directory covers theme, vim, editor (font family/size, line height, line numbers, soft wrap), autosave, explorer (width, markdown_only, show_hidden), and preview (max_width). A separate keymap file uses GPUI key-context conventions (Workspace, Explorer, Editor, …) so any binding can be changed without a new keybinding system. `Cmd+,` opens the settings file in the editor. Missing files fall back to sensible defaults; edits take effect without restart where Zed's settings layer supports it.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-human

- [x] Settings load from a JSONC file under the user's config directory; a missing or partial file falls back to the spec's defaults
- [x] Editor font family/size, line height, line numbers, and soft wrap settings visibly change the editor
- [x] Theme setting supports `system`/`light`/`dark`; `vim_mode` can disable Vim
- [x] A separate keymap file rebinds actions using GPUI key contexts; a rebound shortcut works and the default no longer fires
- [x] `Cmd+,` opens the settings file as a normal editable buffer
- [x] Invalid JSON is handled gracefully (log + fall back, no crash)

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now loads one typed, partial JSONC settings model from
`~/.config/zednotes/settings.json`. Missing fields use notes-specific defaults for system theme,
Vim, bundled editor font and typography, soft wrapping, line numbers, autosave, explorer width and
visibility, and preview width. Invalid syntax or invalid numeric values are logged and reset the
notes settings to defaults instead of aborting startup.

The notes settings are translated into Zed's existing settings store, so editor typography,
line-number and wrap behavior, project-panel width/hidden-file behavior, autosave, Vim activation,
and coordinated One Light/One Dark theme selection use the normal Zed observers. The Markdown
language registry now follows global theme changes so syntax colors update with the UI. A config
directory watcher reloads both settings and `~/.config/zednotes/keymap.json` after edits or file
replacement without restarting the app.

The keymap file uses Zed's `KeymapFile` parser and GPUI context predicates. Reload rebuilds the
built-in Zed/Vim/notes bindings and applies user bindings last with user precedence; invalid files
log and fall back to the default map. The notes defaults themselves are context-scoped where Zed
has competing bindings, allowing user bindings in the same context to replace them. `Cmd+,` and
the application menu create the initial JSONC file when needed and open it through the normal
file-backed editor path in a hidden config worktree.

GPUI tests cover complete and partial parsing, invalid input, applying font/size/line-height,
line-number, wrap, theme, explorer-width, and Vim changes, live settings/keymap reload, contextual
binding replacement (including proving the old action no longer fires), and `Cmd+,` file creation
and editing. `cargo test -p notes_app` passes all 50 tests, the notes-app formatting check passes,
and `./script/clippy -p notes_app` passes with warnings denied.

Manual acceptance remains for a visual pass over light/dark/system switching and typography in the
running macOS app.
