# 05 — Quick open + command palette

**What to build:** The two universal navigation surfaces. `Cmd+P` opens a fuzzy file picker over the workspace's Markdown files; typing a partial name and hitting Enter opens the note. `Cmd+Shift+P` opens a command palette listing every registered GPUI action under `Domain: Action` names, searchable and executable from the keyboard. Both reuse Zed's picker/file-finder/command-palette infrastructure.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] `Cmd+P` opens quick open; results appear as you type with no perceptible lag on a large vault
- [ ] Ranking honors fuzzy score, basename match, recent usage, shorter path, and currently-open state
- [ ] Enter opens the selected note; Escape dismisses and restores previous focus
- [ ] `Cmd+Shift+P` opens the command palette; entries come from GPUI actions (no parallel command registry) and follow `Domain: Action` naming
- [ ] Executing a palette entry performs the action identically to its keyboard shortcut
- [ ] Every major action shipped so far (open folder, toggle explorer, toggle preview, save) is findable in the palette
