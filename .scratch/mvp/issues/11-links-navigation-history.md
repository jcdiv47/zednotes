# 11 — Links + navigation history

**What to build:** The vault becomes navigable as a web of notes. Enter (in NORMAL mode) or Cmd-click on a relative Markdown link opens the target note, resolved against the current file's parent directory. Back/forward navigation history (`Ctrl+-` / `Ctrl+Shift+-`) records file, cursor position, selection, and scroll position, so following a link is always reversible. Link resolution logic is notes-specific pure logic with unit tests.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] Cmd-click on a relative Markdown link to a local file opens that note in a tab
- [ ] Enter on a link in Vim NORMAL mode does the same
- [ ] Resolution is current file's parent + relative path; links to non-existent files fail gracefully (notification, no crash)
- [ ] `Ctrl+-` returns to the previous location with cursor, selection, and scroll restored; `Ctrl+Shift+-` goes forward
- [ ] History spans files: link-follow then back lands exactly where the user left
- [ ] Link resolution has unit tests covering nested paths, `..` traversal, and same-directory links
