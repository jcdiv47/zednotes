# 05 — Quick open + command palette

**What to build:** The two universal navigation surfaces. `Cmd+P` opens a fuzzy file picker over the workspace's Markdown files; typing a partial name and hitting Enter opens the note. `Cmd+Shift+P` opens a command palette listing every registered GPUI action under `Domain: Action` names, searchable and executable from the keyboard. Both reuse Zed's picker/file-finder/command-palette infrastructure.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] `Cmd+P` opens quick open; results appear as you type with no perceptible lag on a large vault
- [x] Ranking honors fuzzy score, basename match, recent usage, shorter path, and currently-open state
- [x] Enter opens the selected note; Escape dismisses and restores previous focus
- [x] `Cmd+Shift+P` opens the command palette; entries come from GPUI actions (no parallel command registry) and follow `Domain: Action` naming
- [x] Executing a palette entry performs the action identically to its keyboard shortcut
- [x] Every major action shipped so far (open folder, toggle explorer, toggle preview, save) is findable in the palette

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now initializes Zed's stock file finder, so the default `Cmd+P` binding opens quick open
with Zed's fuzzy, basename, path-distance, navigation-history, and currently-open ranking. A narrow
application-wide file-filter hook was added to the generic finder; zednotes applies it inside the
background fuzzy scan before the result limit, and to history, absolute-path, and create-file rows,
so quick open exposes only `.md` and `.markdown` notes without sacrificing large-vault ranking.

The existing Zed command palette remains the sole command registry and is available on
`Cmd+Shift+P`. A notes-specific `View: Toggle Explorer` GPUI action now backs both `Cmd+B` and its
palette entry. `Workspace: Open Folder`, `Note: Toggle Preview`, and `Workspace: Save` are also
discoverable and dispatch the same actions as their shortcuts.

GPUI tests cover fuzzy partial-name opening, Markdown-only results, Enter/Escape behavior and focus
restoration, palette execution of all four major actions, and filtering before the fuzzy result
limit. `cargo test -p notes_app --lib` (20 tests), `cargo test -p file_finder --lib` (80 tests),
`cargo test -p fuzzy_nucleo --lib` (24 tests), `cargo check -p notes_app --all-targets`, and
`./script/clippy -p notes_app -p file_finder -p fuzzy_nucleo` pass. `cargo run -p notes_app` also
reached the running application without startup errors.

Manual acceptance remains for a visual keyboard pass and subjective latency check against a real
large notes vault.
