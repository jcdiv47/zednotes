# 09 — Settings + keymap files

**What to build:** Plain-text configuration the user owns. A Zed-style JSONC settings file in the user's config directory covers theme, vim, editor (font family/size, line height, line numbers, soft wrap), autosave, explorer (width, markdown_only, show_hidden), and preview (max_width). A separate keymap file uses GPUI key-context conventions (Workspace, Explorer, Editor, …) so any binding can be changed without a new keybinding system. `Cmd+,` opens the settings file in the editor. Missing files fall back to sensible defaults; edits take effect without restart where Zed's settings layer supports it.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-agent

- [ ] Settings load from a JSONC file under the user's config directory; a missing or partial file falls back to the spec's defaults
- [ ] Editor font family/size, line height, line numbers, and soft wrap settings visibly change the editor
- [ ] Theme setting supports `system`/`light`/`dark`; `vim_mode` can disable Vim
- [ ] A separate keymap file rebinds actions using GPUI key contexts; a rebound shortcut works and the default no longer fires
- [ ] `Cmd+,` opens the settings file as a normal editable buffer
- [ ] Invalid JSON is handled gracefully (log + fall back, no crash)
