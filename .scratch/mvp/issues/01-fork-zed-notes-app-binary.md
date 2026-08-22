# 01 — Fork Zed and boot a `notes_app` binary

**What to build:** A personal fork of Zed containing a new `notes_app` workspace binary that builds on Apple Silicon macOS and opens an empty GPUI window. This establishes the repository the whole product lives in: `origin` is the personal fork, `upstream` is `zed-industries/zed`, and all notes-specific code stays in the new crate per the upstream patch policy in the spec.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [~] Fork of current Zed exists with `origin` (personal) and `upstream` (zed-industries/zed) remotes configured — `upstream` configured (push disabled); `origin` deliberately deferred, see Comments
- [x] The existing zednotes repo content (source spec, AGENTS.md, docs/agents, .scratch) is preserved in or alongside the fork per the user's preference — nothing lost
- [x] A new `notes_app` crate is registered in the Cargo workspace as a binary
- [x] `cargo run -p notes_app` opens an empty GPUI window with native macOS traffic lights and a working window lifecycle (close quits cleanly)
- [x] No generic Zed crates are modified

## Comments

**2026-08-22 — implemented.**

Repo layout: this checkout *is* the fork (`~/Code/zednotes`), cloned from `zed-industries/zed` at
tag `nightly` (`fd82517a11`). The pre-existing zednotes content was copied back into the fork's tree
at the root, so nothing was lost.

`origin` is intentionally not configured. The user chose "local only for now" over creating a public
GitHub fork or a private mirror, so nothing has been published. Adding `origin` later is a one-line
`git remote add`. `upstream` has its push URL set to `DISABLED` to make an accidental push to Zed
impossible. Note that `git remote -v` still prints a bare `origin` line: that comes from a stray
`remote.origin.proxy` key in the user's *global* git config, not from this repo — cosmetic, but it
makes the remote list misleading.

`AGENTS.md`: upstream ships `AGENTS.md` as a symlink to `.rules`. That symlink was replaced with the
zednotes `AGENTS.md`, which now also documents the fork layout and points at `.rules` for the
inherited Rust/GPUI guidance — noting that upstream's PR-hygiene rules and the `README.md` banner
HARD RULE do not apply to this fork, since it does not open pull requests against zed-industries/zed.

Three upstream files are touched, none of them a generic Zed crate: `Cargo.toml` (one `members`
line), `Cargo.lock`, and `AGENTS.md` (symlink → real file, described above). `README.md` also picked
up upstream's `> [!IMPORTANT]` PR-review banner during the session; it is left uncommitted for the
user to discard, since `.rules` forbids an agent removing it.

Tests (`crates/notes_app/src/notes_app.rs`) exercise the GPUI action-dispatch seam per the spec's
testing decisions: the window opens with the right title, the `CloseWindow` action and `cmd-w` both
close the window, and closing one of two windows leaves the other open. The one test below the seam
asserts that `notes_window_options` sets `appears_transparent: false` — that flag is what makes macOS
draw the traffic lights, and it is observable nowhere else. `cx.quit()` itself is unobservable under
GPUI's test platform (`TestPlatform::quit` is a no-op), so quitting is asserted only as far as
"no windows remain".

**Left for a human:** visually confirming the traffic lights and that clicking the red button quits.
This machine's terminal has neither Screen Recording nor Accessibility permission, so the window
could not be screenshotted or queried from here.

**Environment note:** building requires the macOS Metal toolchain
(`xcodebuild -downloadComponent MetalToolchain`, ~690 MB) — `gpui_apple`'s build script compiles
`shaders.metal` and fails without it.
