# 06 — Note commands: new, rename, delete, duplicate

**What to build:** The note-lifecycle actions. `Cmd+N` (Note: New) shows a small filename/path prompt; the user types e.g. a nested relative path, `.md` is appended if missing, intermediate directories are created, and the note opens in insert mode. Rename preserves the open tab, buffer, cursor, and history. Delete moves to the macOS Trash. Duplicate and folder create/rename/delete round out explorer file management. All are GPUI actions invocable from explorer keys (`a`, `A`, `r`, `d`), shortcuts, and the palette.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] `Cmd+N` prompt → typing `projects/new-idea` creates `projects/new-idea.md`, opens it, and enters insert mode
- [x] `.md` is auto-appended when missing; creating inside a non-existent subdirectory creates the directory
- [x] Note: Rename updates the tab title and preserves the buffer, cursor position, and undo history; recent-file references follow the new path
- [x] Note: Delete moves the file to the macOS Trash (recoverable), never permanently unlinks; open tab handles the deletion gracefully
- [x] Duplicate file, and create/rename/delete directory all work from the explorer (`a` new note, `A` new directory, `r` rename, `d` delete)
- [x] Routine outcomes (created, renamed) show transient notifications; delete uses confirmation where configured — no modal for routine success
- [x] All commands appear in the command palette under `Note:` names

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

The note lifecycle is exposed entirely through `Note:` GPUI actions. `Cmd+N` and explorer `a`
reuse the project panel's inline path prompt with a zednotes name transform that preserves explicit
`.md`/`.markdown` names and appends `.md` otherwise. Zed's filesystem layer creates missing parent
directories, the new file opens through the normal workspace path, and a configured post-create
action switches the new editor into Vim INSERT mode.

Rename, duplicate, and directory operations delegate to Zed's project/project-panel state rather
than copying buffer state. Rename therefore retains the same editor and buffer entities (including
cursor and undo history) while the workspace migrates recent-navigation paths. `Note: Delete`
always delegates to the recoverable project-panel Trash action with its confirmation prompt; an
open deleted note is closed through the normal project deletion event. Explorer bindings are `a`,
`A`, `r`, and `d`, and the Note menu and command palette expose New, New Directory, Rename, Delete,
and Duplicate. Successful creates and renames emit five-second transient workspace notifications.

GPUI tests cover nested `Cmd+N` creation and INSERT mode, extension handling, palette execution,
explorer file/folder creation, folder rename/delete, duplication, Trash confirmation and recovery
tracking, open-tab deletion, and rename preservation of editor identity, content, undo history, and
recent paths. `cargo test -p notes_app --lib` (27 tests), `cargo test -p project_panel --lib` (121
tests), `cargo check -p notes_app --all-targets`, and `./script/clippy -p notes_app -p project_panel`
pass. `cargo run -p notes_app` also reaches the running application without startup errors.

Manual acceptance remains for a visual keyboard pass and verifying that a real file appears in and
can be restored from macOS Trash.
