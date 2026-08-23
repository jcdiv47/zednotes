# 10 — Session restore + startup flow

**What to build:** Restart becomes invisible. On quit, the app persists the session (workspace path, open files, active file, tab order, pane layout, sidebar visibility/width, preview state, cursor and scroll positions, window size/position) under Application Support — never inside the notes directory. On launch it restores all of it; first launch instead shows a simple Open Notes Folder picker (no onboarding). `Workspace: Open Recent` reopens previous vaults, and `Cmd+E` (Note: Open Recent) offers a recency-and-frequency-ranked list of recent notes. Clicking the Dock icon with no window restores the workspace window.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] Quit → relaunch restores the workspace, all tabs in order, the active file, cursor and scroll positions, sidebar and preview state, and window frame
- [x] First launch (no previous session) shows a directory picker; choosing a folder enters the workspace
- [x] Session data lives under Application Support; nothing is written into the notes directory
- [x] `Workspace: Open Recent` lists and reopens previous workspaces
- [x] `Cmd+E` opens a recent-notes picker ranked by recency + frequency
- [x] An empty workspace shows the minimal "No notes yet" state with the core shortcuts; no file selected shows the shortcut-reminder empty editor
- [x] Dock icon click after closing the window restores the workspace window (standard macOS reopen behavior)

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now places its database and session state under
`~/Library/Application Support/zednotes`, restores the previous session on startup, and shows a
directory-only Open Notes Folder picker when no restorable session exists. The old production path
that opened the bundled `spike.md` has been removed. Workspace serialization is explicitly flushed
after autosave and before quit, including tab and pane state, dock state and width, preview items,
window bounds, and editor cursor/scroll metadata that Zed normally writes on separate throttled
tasks.

The File menu and command palette expose `Workspace: Open Recent` through a local recent-workspace
picker. `Cmd+E` and `Note: Open Recent` open the Markdown-only file finder with a persisted
recency-and-frequency score applied to its recent-note section. Every empty pane renders a
notes-specific state: “No notes yet” plus creation/open shortcuts for an empty vault, or “Select a
note” plus navigation/search shortcuts when the vault contains notes.

On macOS, closing the final window no longer quits the process. The application reopen callback
activates an existing notes window or restores the latest workspace, with session restore and the
first-launch picker as fallbacks. GPUI integration coverage exercises first-launch folder choice;
the recent pickers; tabs, active note, pane/preview layout, explorer visibility and 333 px width,
cursor, fractional scroll, and a 1040×740 window frame across a serialized restart; and Dock-style
reopen after the last window closes.

`cargo test -p notes_app` passes all 56 tests, `cargo test -p file_finder --lib` passes all 80
tests, `cargo check -p notes_app -p file_finder -p workspace -p editor --all-targets` passes, and
`./script/clippy -p notes_app -p file_finder -p workspace -p editor` passes with warnings denied.

Manual acceptance remains for a real process quit/relaunch and a visual pass over the two empty
states, recent pickers, restored frame placement, and Dock click behavior in the running macOS app.
