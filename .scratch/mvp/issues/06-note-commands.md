# 06 — Note commands: new, rename, delete, duplicate

**What to build:** The note-lifecycle actions. `Cmd+N` (Note: New) shows a small filename/path prompt; the user types e.g. a nested relative path, `.md` is appended if missing, intermediate directories are created, and the note opens in insert mode. Rename preserves the open tab, buffer, cursor, and history. Delete moves to the macOS Trash. Duplicate and folder create/rename/delete round out explorer file management. All are GPUI actions invocable from explorer keys (`a`, `A`, `r`, `d`), shortcuts, and the palette.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-agent

- [ ] `Cmd+N` prompt → typing `projects/new-idea` creates `projects/new-idea.md`, opens it, and enters insert mode
- [ ] `.md` is auto-appended when missing; creating inside a non-existent subdirectory creates the directory
- [ ] Note: Rename updates the tab title and preserves the buffer, cursor position, and undo history; recent-file references follow the new path
- [ ] Note: Delete moves the file to the macOS Trash (recoverable), never permanently unlinks; open tab handles the deletion gracefully
- [ ] Duplicate file, and create/rename/delete directory all work from the explorer (`a` new note, `A` new directory, `r` rename, `d` delete)
- [ ] Routine outcomes (created, renamed) show transient notifications; delete uses confirmation where configured — no modal for routine success
- [ ] All commands appear in the command palette under `Note:` names
