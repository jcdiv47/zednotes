# 03 — Workspace + file explorer

**What to build:** Open a real notes directory as a workspace and navigate it from a persistent left-hand explorer. The user hits `Cmd+O`, picks a folder in a native dialog, and sees a hierarchical tree (via Zed's project/worktree + project panel) filtered to Markdown files and directories, with hidden files and configured ignore patterns excluded. Selecting a file opens it in the editor as a tab. The explorer is fully keyboard-drivable with Vim-style keys.

**Blocked by:** 02 — Spike: one Markdown file in Zed Editor with Vim.

**Status:** ready-for-agent

- [ ] `Cmd+O` (Workspace: Open Folder) shows a native directory picker and opens the chosen directory as the workspace
- [ ] Explorer shows only `*.md`, `*.markdown`, and directories by default; `markdown_only: false` setting shows everything
- [ ] Hidden files and ignore patterns (`.git`, `.DS_Store`, `node_modules`, `target`) are excluded; a toggle-hidden command exists
- [ ] Enter/`o` on a file opens it in the editor as a tab; multiple files open as multiple tabs with `Cmd+W` close and `Cmd+Shift+[`/`]` switching
- [ ] Explorer Vim navigation works when focused: `j`/`k` move, `h` collapse/parent, `l` expand/open, `q` returns focus to the editor
- [ ] `Cmd+B` toggles the explorer; focus-context dispatch is correct (`j` in editor NORMAL moves the cursor, in explorer selects the next entry)
- [ ] Reveal in Finder and copy absolute/relative path work from the explorer
