# 08 — Autosave + external change handling

**What to build:** The data-safety layer. Autosave is on by default: 750 ms after the last edit, plus on app deactivation, tab change, file switch, workspace close, and quit — while `:w`/`Cmd+S` keep working. Writes are atomic (temp file + rename) so a crash mid-write can't corrupt a note. A filesystem watcher (via Zed's worktree layer, not a custom watcher) detects external creates/modifies/deletes/renames: clean buffers reload silently; a dirty buffer whose file changed on disk gets a Keep My Changes / Reload from Disk / Compare dialog; an externally deleted open file gets Keep Open / Save Again / Close, with kept buffers orphaned until saved. Explorer, search, and quick-open candidates update on events.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] Editing then waiting ~750 ms writes the file to disk without user action; `autosave.enabled`/`delay_ms` settings are honored
- [x] Switching tabs, deactivating the app, and quitting all flush unsaved changes first
- [x] Saves are atomic: killing the app mid-save never leaves a truncated or corrupted file
- [x] Editing a clean open file externally (e.g. from another editor) reloads the buffer with the new content
- [x] Editing a dirty open file externally shows the three-way conflict dialog; neither version is ever silently lost
- [x] Deleting an open file externally shows the Keep Open / Save Again / Close prompt; Keep Open leaves an orphaned buffer that can be re-saved
- [x] Externally created/deleted files appear/disappear in the explorer and quick-open results without restart

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now maps its `autosave.enabled` and `autosave.delay_ms` JSONC settings into Zed's
workspace autosave setting, defaulting to a 750 ms trailing delay. Pending edits are also flushed
when another tab or file becomes active, when the app deactivates, when a workspace closes, and
before the custom quit action exits. `Cmd+S` and Vim `:w` continue to use the normal editor save
path.

Real filesystem text saves now write and sync a temporary sibling before atomically replacing the
destination, preserving existing file permissions. The implementation keeps Zed's worktree
watcher as the sole filesystem event source: clean buffers retain its silent reload behavior,
while dirty external edits show Keep My Changes / Reload from Disk / Compare. Compare opens a
read-only on-disk copy beside the dirty buffer. External deletion shows Keep Open / Save Again /
Close; keeping the buffer leaves its contents open and re-saveable.

Explorer and quick-open continue to refresh from worktree events. zednotes disables quick-open's
missing-path creation row so a deleted note actually disappears from results; note creation remains
available through `Note: New`.

GPUI integration tests cover custom and disabled autosave, tab/deactivation/quit flushing, clean
reloads, every conflict and deletion choice, and live explorer/quick-open creation and deletion.
`cargo test -p notes_app --lib` (44 tests), a 20-seed deactivation sweep,
`cargo test -p file_finder --lib` (80 tests), RealFs atomic-write tests,
`cargo check -p notes_app --all-targets`, and
`./script/clippy -p notes_app -p file_finder -p fs` pass.

Manual acceptance remains for inspecting the dialogs in the running macOS app and interrupting a
real large-file save to confirm the atomic replacement behavior at the product level.
