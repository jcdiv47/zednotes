# 10 — Session restore + startup flow

**What to build:** Restart becomes invisible. On quit, the app persists the session (workspace path, open files, active file, tab order, pane layout, sidebar visibility/width, preview state, cursor and scroll positions, window size/position) under Application Support — never inside the notes directory. On launch it restores all of it; first launch instead shows a simple Open Notes Folder picker (no onboarding). `Workspace: Open Recent` reopens previous vaults, and `Cmd+E` (Note: Open Recent) offers a recency-and-frequency-ranked list of recent notes. Clicking the Dock icon with no window restores the workspace window.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] Quit → relaunch restores the workspace, all tabs in order, the active file, cursor and scroll positions, sidebar and preview state, and window frame
- [ ] First launch (no previous session) shows a directory picker; choosing a folder enters the workspace
- [ ] Session data lives under Application Support; nothing is written into the notes directory
- [ ] `Workspace: Open Recent` lists and reopens previous workspaces
- [ ] `Cmd+E` opens a recent-notes picker ranked by recency + frequency
- [ ] An empty workspace shows the minimal "No notes yet" state with the core shortcuts; no file selected shows the shortcut-reminder empty editor
- [ ] Dock icon click after closing the window restores the workspace window (standard macOS reopen behavior)
