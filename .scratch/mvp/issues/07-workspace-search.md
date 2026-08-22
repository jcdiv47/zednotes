# 07 — Workspace full-text search

**What to build:** Search across the whole vault and within a note. `Cmd+Shift+F` opens workspace search (Zed project search over the live filesystem, scoped to Markdown by default) with incremental results grouped by file with line context; arrow keys navigate results, Enter opens the note at the exact match, Escape returns focus to the editor. `Cmd+F` gives in-note find/replace with incremental highlighting.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] `Cmd+Shift+F` searches all `*.md`/`*.markdown` files in the workspace; first results arrive fast on a large vault
- [ ] Case-sensitive, regex, and whole-word toggles work; directory scope and filename filter narrow the search
- [ ] Results are keyboard-navigable; Enter opens the file with the cursor at the match; Escape restores editor focus
- [ ] `Cmd+F` in-note find shows incremental highlights with next/previous, case sensitivity, regex, replace, and replace-all
- [ ] Vim `/` search continues to work independently of the find UI
- [ ] Search reflects the live disk state (a just-saved change is findable; no stale index)
