# 03 — Workspace + file explorer

**What to build:** Open a real notes directory as a workspace and navigate it from a persistent left-hand explorer. The user hits `Cmd+O`, picks a folder in a native dialog, and sees a hierarchical tree (via Zed's project/worktree + project panel) filtered to Markdown files and directories, with hidden files and configured ignore patterns excluded. Selecting a file opens it in the editor as a tab. The explorer is fully keyboard-drivable with Vim-style keys.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-human

- [x] `Cmd+O` (Workspace: Open Folder) shows a native directory picker and opens the chosen directory as the workspace
- [x] Explorer shows only `*.md`, `*.markdown`, and directories by default; `markdown_only: false` setting shows everything
- [x] Hidden files and ignore patterns (`.git`, `.DS_Store`, `node_modules`, `target`) are excluded; a toggle-hidden command exists
- [x] Enter/`o` on a file opens it in the editor as a tab; multiple files open as multiple tabs with `Cmd+W` close and `Cmd+Shift+[`/`]` switching
- [x] Explorer Vim navigation works when focused: `j`/`k` move, `h` collapse/parent, `l` expand/open, `q` returns focus to the editor
- [x] `Cmd+B` toggles the explorer; focus-context dispatch is correct (`j` in editor NORMAL moves the cursor, in explorer selects the next entry)
- [x] Reveal in Finder and copy absolute/relative path work from the explorer

## Comments

**2026-08-22 — implementation complete; manual acceptance pending.**

Every notes workspace now owns a real visible worktree and a stock Zed `ProjectPanel` in the left
dock. A narrow optional entry-filter hook was added to `project_panel`; zednotes supplies a filter
that keeps directories and shows only `.md`/`.markdown` files unless
`explorer.markdown_only` is false in `~/.config/zednotes/settings.json`. The same JSONC file accepts
`explorer.show_hidden` and `files.exclude`; the required `.git`, `.DS_Store`, `node_modules`, and
`target` exclusions are defaults.

`Workspace: Open Folder` is bound to `Cmd+O` and uses GPUI/Zed's directory-only native path prompt
before replacing the active workspace. The project panel starts open on the left, `Cmd+B` toggles
that dock, and `Explorer: Toggle Hidden Files` updates the live explorer. Zed's existing project
panel supplies reveal-in-Finder and absolute/relative path copying.

The notes override keymap keeps Zed's contextual `j`/`k`/`h` behavior, maps `l`, `Enter`, and `o` to
expand/open permanently, and maps `q` directly back to the active editor. The previous global
`Cmd+W` window-close override was removed so normal tab closing and `Cmd+Shift+[`/`]` switching work.

Thirteen notes-app tests cover JSONC settings, the directory-only prompt, default and disabled
Markdown filtering, hidden toggling, ignored paths, nested directory navigation, tab open/switch/
close behavior, focus return, left-dock toggling, and the existing editor/Vim/save flows.
`cargo test -p notes_app --lib`, `cargo check -p notes_app --all-targets`, and
`./script/clippy -p notes_app` pass. `cargo run -p notes_app` also reaches the running application
without startup errors.

Manual acceptance remains for exercising the native folder dialog and Finder reveal against a real
notes directory, plus a visual check of the left-hand tree.
