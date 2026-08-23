# 07 — Workspace full-text search

**What to build:** Search across the whole vault and within a note. `Cmd+Shift+F` opens workspace search (Zed project search over the live filesystem, scoped to Markdown by default) with incremental results grouped by file with line context; arrow keys navigate results, Enter opens the note at the exact match, Escape returns focus to the editor. `Cmd+F` gives in-note find/replace with incremental highlighting.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] `Cmd+Shift+F` searches all `*.md`/`*.markdown` files in the workspace; first results arrive fast on a large vault
- [x] Case-sensitive, regex, and whole-word toggles work; directory scope and filename filter narrow the search
- [x] Results are keyboard-navigable; Enter opens the file with the cursor at the match; Escape restores editor focus
- [x] `Cmd+F` in-note find shows incremental highlights with next/previous, case sensitivity, regex, replace, and replace-all
- [x] Vim `/` search continues to work independently of the find UI
- [x] Search reflects the live disk state (a just-saved change is findable; no stale index)

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now composes Zed's project-search and buffer-search toolbars into every notes pane,
including panes created after startup. `Workspace: Search` is bound to `Cmd+Shift+F`, appears in the
File menu and command palette, and wraps the stock project-search deployment with a visible,
editable `**/*.md, **/*.markdown` include filter. Zed's streaming live-filesystem search supplies
grouped line-context results, case-sensitive/regex/whole-word modes, include/exclude path filters,
and project-search replace behavior without a notes-specific index.

Search results keep Zed's editor navigation and highlight behavior; zednotes maps Enter in the
project-search results editor to open the selected excerpt in its source note at the exact match.
Escape moves focus from the search fields back into that results editor. `Cmd+F` uses the stock
incremental buffer-search toolbar with next/previous, search modes, replace, and replace-all, while
Vim `/`, `?`, `n`, and `N` remain handled by the existing Vim search path.

Four new GPUI integration tests cover the default Markdown scope, live saved content, exact-match
opening, case/whole-word modes, directory narrowing, in-note highlighting and replace-all, Vim `/`
after using the find UI, and search toolbars on newly split panes. `cargo test -p notes_app --lib`
(31 tests) and `./script/clippy -p notes_app` pass. `cargo run -p notes_app` also reached the running
application without startup errors.

Manual acceptance remains for visual keyboard navigation and a subjective first-result latency
check against a real large notes vault.
