# 12 — Vim leader layer + Ex command aliases

**What to build:** The Space leader layer that makes the whole app home-row reachable, mapped onto the actions earlier tickets created (no duplicate implementations): `<Space>ff` quick open, `<Space>fg` search all notes, `<Space>fn` new note, `<Space>fr` recent note, `<Space>p` command palette, `<Space>e` toggle explorer, `<Space>v` toggle preview, `<Space>b n/p/d` buffer next/previous/close, `<Space>w h/j/k/l` pane focus, `<Space>w v/s` splits, `<Space>s f/p` search file/workspace. Plus the Ex command line: keep Zed's `:`-into-palette approach with `:w :q :wq :q! :x :e` and app aliases `:preview :explorer :notes :search`.

**Blocked by:** 04 — Markdown preview; 05 — Quick open + command palette; 07 — Workspace full-text search.

**Status:** ready-for-human

- [x] All listed `<Space>` mappings fire the same GPUI actions as their `Cmd` shortcuts and palette entries
- [x] Leader mappings only apply in Vim NORMAL mode in the editor; `<Space>` in INSERT mode types a space
- [x] The leader key is configurable via settings (`vim.leader`)
- [x] `:w :q :wq :q! :x :e` work; `:preview :explorer :notes :search` invoke the corresponding app actions
- [x] `<Space>w v`/`<Space>w s` create vertical/horizontal splits; `<Space>w h/j/k/l` move focus between panes; two notes side by side works
- [x] Notes-specific Vim mapping tests exist for `<Space>ff`, `<Space>fg`, `<Space>e`, `<Space>v`

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now installs the complete leader layer as notes-specific keybindings over the existing
file-finder, search, note, command-palette, pane, and workspace actions. The bindings are limited to
`Editor && VimControl && vim_mode == normal`; the leader's original normal-mode action is suppressed,
while insert mode remains ordinary text input. `vim.leader` defaults to `space`, accepts one GPUI
keystroke, hot-reloads with `settings.json`, and preserves the user's loaded `keymap.json` overrides.

The stock Zed Vim command interceptor continues to implement `:w`, `:q`, `:wq`, `:q!`, `:x`, and
`:e`. Default command-palette aliases add `:preview`, `:explorer`, `:notes`, and `:search`, dispatching
the same actions used by menus and shortcuts rather than introducing a second command registry.

GPUI tests cover the entire mapping table, the four required leader integrations, insert-mode space,
leader reconfiguration, user-keymap preservation, vertical and horizontal splits, directional pane
focus, two different notes side by side, all six built-in Ex commands, and all four app aliases.
`cargo test -p notes_app --lib --no-fail-fast` passes all 74 tests,
`cargo check -p notes_app --all-targets` passes, and `./script/clippy -p notes_app` passes with warnings
denied. Manual acceptance remains for a real-window keyboard pass with the default and a customized
leader.
