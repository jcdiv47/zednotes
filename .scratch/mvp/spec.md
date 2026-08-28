# Spec — Personal Markdown Notes for macOS (MVP)

Status: ready-for-agent

**Source:** `Personal Markdown Notes for macOS — Product & Technical Specification.md` (v0.1)
**Target:** macOS 15+, Apple Silicon

## Problem Statement

I keep my notes as plain Markdown files in a directory, and no existing tool lets me work with them the way I work with source code. Apple Notes and similar apps trap content in proprietary stores and rich-text editing. Obsidian owns the files but doesn't offer editor-grade latency or real Vim modal editing. Zed and Neovim have the editing feel but aren't shaped around a note vault: no quick capture, no Markdown reading experience, no note-first navigation. I end up bouncing between tools, reaching for the mouse, and waiting on UIs — when what I want is to launch, find a note, edit it with Vim motions, and search my whole vault without ever leaving the keyboard.

## Solution

A native macOS application, built in Rust on GPUI inside a Zed fork, that combines a file explorer, Markdown editor, Vim environment, full-text search, and Markdown preview into one keyboard-first window. Plain `.md` files on the filesystem remain the sole source of truth — the app maintains no canonical database, requires no account, and performs no network requests. The app reuses Zed's editor, Vim mode, project/worktree, project panel, file finder, command palette, search, and Markdown preview infrastructure, adding only a thin notes-specific layer: note commands, session persistence, settings, and a simplified UI where the note itself is always the dominant visual element.

The MVP is done when this workflow works reliably, entirely without a mouse: launch → quick-open a note → edit with Vim → `:w` → toggle preview → search the workspace → open a result → create a new note → navigate the file tree → quit → relaunch with the previous workspace and notes restored.

## User Stories

1. As a note-taker, I want to open an arbitrary directory as my notes workspace, so that my existing folder of Markdown files works without migration or import.
2. As a note-taker, I want the app to restore my previous workspace on launch, so that I resume where I left off without any chooser dialog.
3. As a first-time user, I want a directory picker on first launch, so that I can point the app at my notes folder.
4. As a note-taker, I want Zed's Recent Projects picker available from the unobstructed project name beside the macOS traffic lights and via a command, so that I can switch between note directories quickly.
5. As a keyboard user, I want a persistent file explorer showing a hierarchical tree of my notes, so that I can see and navigate my vault's structure.
6. As a note-taker, I want the explorer to show only Markdown files and directories by default (configurable), so that non-note clutter stays out of view.
7. As a note-taker, I want hidden files and configured ignore patterns (`.git`, `.DS_Store`, `node_modules`, `target`) excluded from the explorer, so that only content I care about appears.
8. As a keyboard user, I want Vim-style navigation in the explorer (`j`/`k`/`h`/`l`, `Enter`/`o` to open, `a` new note, `A` new directory, `r` rename, `d` delete, `q` back to editor), so that file management feels like netrw/Oil.
9. As a note-taker, I want to create a new note with `Cmd+N` via a small filename/path prompt that auto-appends `.md`, opens the note, and enters insert mode, so that capturing a thought takes seconds.
10. As a note-taker, I want to create, rename, delete, duplicate, multi-select, and drag files/folders from the explorer, with undo/redo for file operations, so that I manage my vault without Finder.
11. As a note-taker, I want deletes to move files to the macOS Trash rather than permanently unlinking them, so that mistakes are recoverable.
12. As a note-taker, I want to reveal a file in Finder and copy its absolute or relative path, so that I can hand notes to other tools.
13. As a Vim user, I want Zed's Vim mode enabled by default with NORMAL, INSERT, VISUAL, VISUAL LINE, VISUAL BLOCK, and REPLACE modes, so that editing feels like my editor, not a text area.
14. As a Vim user, I want full motion support (`h j k l`, `w b e`, `0 ^ $`, `gg G`, `{ }`, `%`, `f F t T ; ,`), operators (`d c y > < =`) with compositions (`diw`, `ci"`, `dap`, `3dd`), and text objects, so that my muscle memory transfers intact.
15. As a Vim user, I want registers, macros, repeat (`.`), marks, search (`/` `?` `n` `N`), counts, join, case transforms, and undo/redo, so that advanced editing workflows work.
16. As a Vim user, I want a `:` command line supporting `:w`, `:q`, `:wq`, `:q!`, `:x`, `:e` plus app aliases (`:preview`, `:explorer`, `:notes`, `:search`), so that core Ex habits work without Vimscript.
17. As a Vim user, I want a Space leader layer for note actions (`<Space>ff` quick open, `<Space>fg` search all notes, `<Space>fn` new note, `<Space>p` palette, `<Space>e` explorer, `<Space>v` preview, `<Space>o` outline, `<Space>b*` buffers, `<Space>m*` bookmarks, `<Space>w*` panes/splits), with Which-key hints after a configurable delay, so that the whole app is reachable and discoverable from home row.
18. As a writer, I want Markdown syntax highlighting (headings, inline/fenced code, lists, blockquotes, links, checkboxes, emphasis, strikethrough) via Tree-sitter, so that documents are structurally readable while editing.
19. As a writer, I want core editor features — undo/redo, multi-line selection, clipboard, auto-indent, line movement/duplication/deletion, soft wrap, configurable line numbers, current-line highlight, bracket matching, configurable font/line-height/cursor — so that the editor feels complete.
20. As a writer, I want the editor visually simpler than a programming editor (no minimap, no diagnostics, no completion, no LSP), so that nothing competes with the text.
21. As a note-taker, I want fuzzy quick open on `Cmd+P` ranking by fuzzy score, basename match, recency, path length, and open state, so that any note is two keystrokes away.
22. As a keyboard user, I want a command palette on `Cmd+Shift+P` listing every major action sourced from GPUI actions, so that everything is discoverable without memorizing shortcuts.
23. As a note-taker, I want full-text search across all Markdown files on `Cmd+Shift+F` with incremental results, case/regex/whole-word toggles, directory scope, filename filter, and keyboard result navigation where Enter opens the match at its location, so that I can find anything in my vault.
24. As a writer, I want find and replace within the current note on `Cmd+F` with incremental highlight, next/previous, case sensitivity, regex, and replace-all, so that in-note edits are fast.
25. As a reader, I want a Markdown preview toggled with `Cmd+Shift+V` rendering headings, emphasis, links, images, ordered/unordered/task lists, blockquotes, inline and fenced code, horizontal rules, tables, and Mermaid diagrams, with task checkboxes editable from preview, so that I can read and maintain notes as documents.
26. As a reader, I want the preview to refresh about 150 ms after I stop typing, so that reading and writing stay in sync without lag while typing.
27. As a reader, I want preview content constrained to a readable width (default 760 px, centered), so that long lines don't strain reading.
28. As a reader, I want local images in Markdown resolved relative to the note's file (PNG, JPEG, GIF, WebP, SVG where supported), with remote images disabled by default, so that previews are complete, predictable, and offline.
29. As a note-taker, I want relative Markdown links (`[GPUI](projects/gpui.md)`) to open the target note on Cmd-click or Enter, resolved against the current file's parent, so that my vault is navigable as a web of notes.
30. As a note-taker, I want open notes as tabs showing filenames (full paths only when disambiguating), with `Cmd+W` close, `Cmd+Shift+[`/`]` and `gt`/`gT` switching, `Cmd+1…9` direct activation, `Ctrl+Tab` switching within the pane, and an all-pane switcher on `<Space>bb`, so that I can juggle several notes.
31. As a writer, I want split panes (split right/down, close, focus) primarily for Editor | Preview and secondarily for two notes side by side, so that I can reference while writing.
32. As a note-taker, I want autosave enabled by default (750 ms debounce, plus on deactivation, tab change, file switch, workspace close, and quit) while `:w`/`Cmd+S` remain functional, so that I never think about saving.
33. As a data owner, I want saves written atomically (temp file, fsync where appropriate, rename over destination), so that a crash mid-write never corrupts a note.
34. As a user of multiple tools, I want external file changes (from Neovim, Git, Finder, sync services) detected via filesystem watching and clean buffers reloaded automatically, so that the app never shows stale content.
35. As a user of multiple tools, I want a conflict dialog (Keep My Changes / Reload from Disk / Compare) when a file changed on disk while I had unsaved edits, so that neither version is silently lost.
36. As a user of multiple tools, I want a prompt (Keep Open / Save Again / Close) when an open file is deleted externally, with kept buffers becoming orphaned until saved, so that external deletes can't destroy in-progress work.
37. As a note-taker, I want in-app renames to update the tab and preserve the buffer, cursor, history, and recent-file references, so that renaming is non-disruptive.
38. As a note-taker, I want the session (workspace path, open files, active file, tab order, pane layout, sidebar visibility/width, preview state, cursor and scroll positions, window size/position) persisted under Application Support and restored on relaunch, so that restart is invisible.
39. As a note-taker, I want a recency-and-frequency-ranked recent-notes list on `Cmd+E`, so that returning to active notes is instant.
40. As a note-taker, I want back/forward navigation history (`Ctrl+-` / `Ctrl+Shift+-`) capturing file, cursor, selection, and scroll, so that following links is always reversible.
41. As a user, I want settings in a JSONC file at `~/.config/<app-name>/settings.json` (theme, vim, editor font/size/line-height/numbers/wrap, autosave, explorer, preview) opened via `Cmd+,`, so that configuration is plain text I own.
42. As a user, I want a separate `keymap.json` using GPUI/Zed key-context conventions, so that I can rebind anything without a new keybinding system.
43. As a user, I want System, Light, and Dark themes with coordinated UI and syntax colors, so that the app matches macOS appearance.
44. As a user, I want independent font settings for UI, editor, and preview, so that writing is monospace and reading is proportional.
45. As a writer, I want a minimal status bar showing the Project/Git/Outline panel controls together at bottom left plus Vim mode, file path, language, word/character count, cursor position, and save state — no LSP indicators, so that status is glanceable, not noisy. Git state lives in the top bar, explorer, tabs, editor diff markers, and the Git panel.
46. As a writer, I want a focus mode (`View: Toggle Focus Mode`) hiding explorer, preview, and chrome details, leaving a centered editor, so that I can write without distraction.
47. As a keyboard user, I want focus-context-dependent key dispatch (in the editor `j` moves down; in the explorer it selects the next entry; in the palette it types), so that keys always do the contextual thing.
48. As a user of an empty vault or no selected file, I want a minimal empty state listing the core shortcuts (`Cmd+N`, `Cmd+P`, `Cmd+Shift+F`, `Cmd+Shift+P`), so that the app teaches itself without onboarding.
49. As a macOS user, I want native integration — traffic lights, standard menus, open-folder dialog, Finder reveal, Trash, clipboard, IME, Dock reopen restoring the workspace window, so that the app feels native, not ported.
50. As a Neovim user, I want a `Note: Open in Neovim` command opening the current file's absolute path in nvim, with the app reloading on external save, so that I can drop to real Neovim when needed.
51. As a data owner, I want no unsolicited network requests, telemetry, analytics, or accounts, so that my notes are private by construction. Explicit Git remote commands such as fetch, pull, and push are allowed.
52. As a data owner, I want the app to never write frontmatter, UUIDs, or invisible identifiers into my Markdown, deriving titles from the first H1 (or filename), so that files stay portable across every tool.
53. As a note-taker, I want Zed's hot-exit database to checkpoint dirty editor contents without writing them to the Markdown file, then restore the dirty buffer on the next launch after a crash, so that even an autosave gap can't lose work. Do not add a parallel Recovery directory or second recovery document model.
54. As a troubleshooter, I want a rotating local log at `~/Library/Logs/zednotes/zednotes.log` (previous file `zednotes.log.old`) with standard levels defaulting to info, so that I can diagnose problems myself without sharing data.
55. As a user with a large vault, I want the app performant at 10,000 files / 500 MB (< 500 ms cold start, < 50 ms note open, < 16 ms typing and Vim latency, < 100 ms first search results), so that scale never breaks the workflow.
56. As a user with occasional huge files, I want the editor to stay usable at 10 MB of Markdown and the preview to require explicit `[Render Anyway]` above ~5 MB, so that one giant file can't hang the app.
57. As a writer, I want a `Note: Statistics` command reporting word and character counts, so that I can check length on demand.
58. As a user, I want routine outcomes (saved, renamed, folder created) as transient notifications and only destructive/ambiguous states (save failure, conflict, delete confirmation) as modal UI, so that dialogs stay rare and meaningful.
59. As a keyboard user, I want a fuzzy heading outline on `Cmd+Shift+O` / `<Space>o`, so that I can jump within long notes without a persistent panel.
60. As a note-taker, I want persistent line bookmarks with toggle, optional labels, next/previous navigation, and a vault-wide bookmarks view under `<Space>m*`, so that I can keep lightweight waypoints without modifying Markdown.
61. As a writer, I want pasting an image from the clipboard into a Markdown editor to save a uniquely named local image beside the note and insert the relative Markdown image reference, so that image capture remains file-native.

## Implementation Decisions

- **Repository strategy: fork Zed.** The app is developed as a `notes_app` binary crate inside a fork of the `zed-industries/zed` workspace, not as an independent Cargo project consuming extracted crates. Zed's internal crate graph (editor → language → settings → workspace → project → …) makes isolation cost far exceed its benefit for a personal app. `upstream` is Zed; periodic merges keep the fork current, and releases record both the Zednotes commit and exact upstream base SHA.
- **Upstream patch policy:** prefer, in order: consume an existing Zed capability → configure it → wrap it → add a local extension point → modify a generic crate as last resort. App-specific code stays in `notes_app` (and later `notes_*` crates); patches to generic Zed crates are kept as small discrete commits to ease upstream merges.
- **Reused Zed subsystems:** gpui, editor (including bookmarks and clipboard-image paste), vim, multi_buffer, text, language(s), workspace/session hot exit, project, project_panel, outline, outline_panel, tab_switcher, which_key, recent_projects, fs, ui, theme, settings, picker, fuzzy, command_palette, file_finder, search, markdown, markdown_preview (including task toggles, source-position synchronization, and Mermaid), git, git_ui, menu. Explicitly not initialized: terminal, debugger, agent/assistant, collab, remote, extensions.
- **Notes-specific layer** (built, not reused): notes workspace orchestration, note commands (new/rename/delete/quick-capture-later), notes settings and keymaps, status bar, empty states, new-note prompt, conflict policy/UI, workspace chooser, recent-note ranking, and Markdown-link behavior. Zed continues to own document content, bookmarks, preview parsing, workspace/session persistence, and hot-exit storage. A single `notes_app` crate is acceptable initially; split into `notes_core` / `notes_ui` / `notes_session` only when boundaries emerge.
- **Actions are the API.** Every major operation is a GPUI action, invocable identically from menu, keyboard shortcut, command palette, and Vim mapping. Command naming follows `Domain: Action` (`Note: New`, `Workspace: Open Recent`, `View: Toggle Focus Mode`). No parallel command registry.
- **Do not duplicate Zed state.** The notes layer holds only note-specific policy and state Zed entities do not own (custom settings, recent-note ranking, and UI composition). Workspace roots, preview items, sessions, document content, cursors, selections, bookmarks, and undo history remain in Zed's Project/Workspace/Editor/MultiBuffer.
- **Filesystem is authoritative.** Note content, names, and folder structure live only on disk. App state (layout, cursor positions, recents, bookmarks, hot-exit checkpoints, settings, caches) lives under `~/Library/Application Support/<AppName>/` and `~/.config/<app-name>/` (settings, keymap) — never inside the notes directory unless explicitly enabled. Hot-exit contents are a recovery checkpoint, not a second canonical note store.
- **Document identity** is the normalized absolute path (`DocumentId(PathBuf)`); a move invalidates the old identity unless Zed's worktree entry IDs prove safely reusable. No UUIDs or metadata written into files. Titles are inferred (first H1, else filename); frontmatter is tolerated later but never required or auto-added.
- **Editor backend: Zed Editor + Zed Vim only in v1.** No Neovim embedding — this avoids RPC, terminal-grid emulation, duplicate buffer ownership, and IME complexity. Interop is `Note: Open in Neovim` plus reload-on-external-change. A future `nvim --embed` backend is an experiment that must never make two document models simultaneously authoritative in one pane.
- **Search: Zed project/worktree search over the live filesystem in v1**, scoped to `*.md`/`*.markdown` by default. No search database in MVP. A future SQLite FTS5 index (documents, links, headings) is strictly a rebuildable cache — Markdown → indexer → SQLite, never SQLite → Markdown.
- **External change handling state machine** (from the source spec, decision-bearing): file event → if buffer clean, reload; if dirty, compare disk content → conflict UI (`Keep My Changes` / `Reload from Disk` / `Compare`). Filesystem events come from Zed's project/worktree watcher, not an independent watcher, and fan out to explorer, editor, search, and quick-open candidates.
- **Autosave** defaults on with 750 ms debounce plus save-on-deactivation/tab-change/file-switch/close/quit; writes are atomic (temp + rename) via Zed's fs layer if it already provides this.
- **Concurrency:** UI on the GPUI main thread; workspace scanning, search, Markdown parsing, and preview generation in background via GPUI async contexts. Never scan the filesystem synchronously in render.
- **Settings** are Zed-style JSONC in `~/.config/<app-name>/settings.json` with a separate `keymap.json` using GPUI key contexts (Workspace, Explorer, Editor, Search, CommandPalette, QuickOpen, MarkdownPreview, Dialog). Zednotes settings include Which-key enablement and delay; the default is enabled at 500 ms.

### Existing-capability rule (do not reimplement)

- Keep using Zed Editor/Vim for motions, operators, registers, macros, marks, search, undo/redo, multi-cursor editing, clipboard, IME, and Markdown highlighting; test integration, not a parallel editor model.
- Keep using Project Panel for multi-selection, drag-and-drop, and file-operation undo/redo. Zednotes may configure or wrap these actions but must not add a second explorer mutation stack.
- Keep using Markdown Preview for editor-to-preview source-position synchronization, preview-to-source navigation, interactive task checkboxes, local/remote image policy, and Mermaid rendering.
- Keep using Editor's Markdown clipboard-image paste, which writes a uniquely named image beside the note and inserts a relative Markdown reference.
- Keep using Project/Editor bookmarks and Workspace/Editor serialization for persistent bookmarks and hot-exit recovery. Zednotes supplies defaults, menus, and leader mappings only.
- **Maintenance order keeps the fork cheap.** Merge upstream first, run the notes integration suite, then consume/configure newly available Zed capabilities before adding local code. Keep generic-crate patches small and independently reviewable; keep product composition in `notes_app`.
- **Distribution:** manual builds as a `.app` (optionally signed/notarized DMG). No updater, no App Store, no telemetry.

## Testing Decisions

- **What makes a good test here:** dispatch a GPUI action against a test app context over a temporary notes directory, then assert only on externally observable outcomes — files on disk (created, renamed, saved content, trashed) and workspace state (open items, active item, pane layout, preview visibility). Never assert on internal Zed structures, render output, or intermediate notes-layer state.
- **Primary seam (the only seam): the GPUI action-dispatch layer**, matching the "Actions Are the API" rule — tests exercise exactly the surface that keyboard, palette, menu, and Vim mappings share, using `#[gpui::test]` and Zed's test app contexts.
- **Covered through that seam:** New Note (prompt → `.md` created → opened in insert mode), rename/delete (Trash) with tab/buffer preservation, Toggle Preview, Toggle Explorer, Quick Open selection, workspace search → open result at match, autosave debounce writing to disk, external-change reload and conflict paths, session and dirty-buffer hot-exit restore round-trips, outline/tab-switcher/bookmark actions, Which-key leader discovery, focus-context key dispatch, and the custom leader mappings.
- **Unit tests below the seam only for notes-specific pure logic:** path normalization/DocumentId, relative link resolution, title extraction (H1 vs filename), settings parsing and defaults, session serialization.
- **Not re-tested:** Zed's editor and Vim behavior (motions, operators, text objects, registers, macros). After upstream merges, run the relevant upstream editor/vim suites — including Zed's Neovim-comparison Vim tests — rather than duplicating them.
- **Prior art:** the Zed codebase itself — its crates' `#[gpui::test]` suites (workspace, project_panel, file_finder, vim) are the pattern to copy for structure, test contexts, and fake-fs usage.
- **Integration acceptance flows** (asserted through the same seam): create → edit → autosave → restart → restore; external edit → detect → reload; create nested note → quick open finds it; workspace search → open result → cursor at match; toggle preview → edit → preview updates.

## Out of Scope

Everything the source spec defers or excludes:

- **v1.1+ (deliberately deferred, designed-for):** quick capture (`Note: Quick Capture`), wiki links (`[[...]]`), daily notes, external rename detection beyond best-effort.
- **Later phases:** backlinks, tags, SQLite FTS index, templates, embedded Neovim backend.
- **Not planned:** cloud sync, iCloud, CRDTs, collaboration, accounts, mobile/web versions, Electron/WebView shell, plugins/marketplace, databases as canonical storage, block IDs/Notion-style editing, WYSIWYG Markdown, canvas/drawing/handwriting, PDF annotation, AI/embeddings/semantic search, publishing, browser clipping, task/calendar systems, telemetry, auto-updates, Mac App Store, Intel macOS (optional at best).
- Visual polish before daily-driving; LaTeX/footnotes in preview (optional, not MVP).

## Further Notes

- **Acceptance bar:** the MVP workflow in the Solution section must work mouse-free, and the app must survive a week of exclusive daily note-taking without needing another editor for routine tasks. The single success metric: opening another editor should feel slower than staying in this app.
- **Performance targets are product targets, not hard guarantees:** cold start < 500 ms, note open < 50 ms, typing/Vim/quick-open < 16 ms per frame, preview refresh < 200 ms post-debounce, first search results < 100 ms, at 10k files / 500 MB (stretch 50k / 2 GB).
- **Desired character:** fast, quiet, predictable, dense, keyboard-native, filesystem-native. Not social, cloud-first, dashboard-heavy, plugin-heavy, database-centric, animated, or touch-oriented. The note is always the dominant visual element.
- **Historical development order** (completed foundation): fork Zed → notes_app binary → GPUI window → editor/language/settings/theme → Vim/save → workspace/project → project panel → file finder → command palette → Markdown preview/search → note actions → autosave/session restore → simplified UI. New work starts from daily-driving evidence and upstream capability audits, not by rebuilding this sequence.
