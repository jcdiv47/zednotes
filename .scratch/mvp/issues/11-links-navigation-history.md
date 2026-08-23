# 11 — Links + navigation history

**What to build:** The vault becomes navigable as a web of notes. Enter (in NORMAL mode) or Cmd-click on a relative Markdown link opens the target note, resolved against the current file's parent directory. Back/forward navigation history (`Ctrl+-` / `Ctrl+Shift+-`) records file, cursor position, selection, and scroll position, so following a link is always reversible. Link resolution logic is notes-specific pure logic with unit tests.

**Blocked by:** 03 — Workspace + file explorer.

**Status:** ready-for-human

- [x] Cmd-click on a relative Markdown link to a local file opens that note in a tab
- [x] Enter on a link in Vim NORMAL mode does the same
- [x] Resolution is current file's parent + relative path; links to non-existent files fail gracefully (notification, no crash)
- [x] `Ctrl+-` returns to the previous location with cursor, selection, and scroll restored; `Ctrl+Shift+-` goes forward
- [x] History spans files: link-follow then back lands exactly where the user left
- [x] Link resolution has unit tests covering nested paths, `..` traversal, and same-directory links

## Comments

**2026-08-23 — implementation complete; manual acceptance pending.**

zednotes now treats standard inline Markdown links as vault navigation. Zed's existing editor file
link handling supplies Cmd-click, while a notes-specific `Note: Follow Link` action handles Enter
in Vim NORMAL mode for both `.md` and `.markdown` files. Enter keeps its normal Vim next-line
motion when the cursor is not on a relative local link. Link parsing supports escaped destinations,
optional titles, percent-encoded paths, and file fragments; URI and absolute-path targets are not
treated as vault links.

Relative targets are normalized from the active note's parent directory. Missing targets leave the
current note active and show a non-crashing notification. The default navigation bindings explicitly
map `Ctrl+-` and `Ctrl+Shift+-` to Zed's pane history. Editor navigation entries now retain complete
selection sets in addition to the existing cursor and scroll anchors, so back/forward restores the
exact source range and fractional scroll position across files.

Pure unit tests cover same-directory, nested, and `..` resolution plus parsing and percent decoding.
GPUI tests cover Cmd-click on link text, Enter in NORMAL mode, the non-link Enter fallback, missing
targets through both activation paths, and cross-file back/forward restoration of selection and
scroll. `cargo test -p notes_app --lib` passes all 65 tests; the existing editor navigation-history
test and the four editor filename-hover regression tests pass;
`cargo check -p notes_app -p editor --all-targets`, formatting, and
`./script/clippy -p notes_app -p editor` pass.

Manual acceptance remains for clicking links and visually confirming the missing-link notification
and back/forward behavior in the running macOS app.
