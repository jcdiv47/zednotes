# 08 — Autosave + external change handling

**What to build:** The data-safety layer. Autosave is on by default: 750 ms after the last edit, plus on app deactivation, tab change, file switch, workspace close, and quit — while `:w`/`Cmd+S` keep working. Writes are atomic (temp file + rename) so a crash mid-write can't corrupt a note. A filesystem watcher (via Zed's worktree layer, not a custom watcher) detects external creates/modifies/deletes/renames: clean buffers reload silently; a dirty buffer whose file changed on disk gets a Keep My Changes / Reload from Disk / Compare dialog; an externally deleted open file gets Keep Open / Save Again / Close, with kept buffers orphaned until saved. Explorer, search, and quick-open candidates update on events.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] Editing then waiting ~750 ms writes the file to disk without user action; `autosave.enabled`/`delay_ms` settings are honored
- [ ] Switching tabs, deactivating the app, and quitting all flush unsaved changes first
- [ ] Saves are atomic: killing the app mid-save never leaves a truncated or corrupted file
- [ ] Editing a clean open file externally (e.g. from another editor) reloads the buffer with the new content
- [ ] Editing a dirty open file externally shows the three-way conflict dialog; neither version is ever silently lost
- [ ] Deleting an open file externally shows the Keep Open / Save Again / Close prompt; Keep Open leaves an orphaned buffer that can be re-saved
- [ ] Externally created/deleted files appear/disappear in the explorer and quick-open results without restart
