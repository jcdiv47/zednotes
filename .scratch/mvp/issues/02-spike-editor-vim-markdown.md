# 02 — Spike: one Markdown file in Zed Editor with Vim

**What to build:** The `notes_app` binary initializes the minimum set of Zed subsystems (assets, settings, theme, language, editor, vim) needed to open one hard-coded local Markdown file in Zed's Editor with Vim mode enabled and save it back to disk. This is the spec's "most important implementation decision" — it retires the project's main technical risk. No custom design; Zed's stock look is fine.

**Blocked by:** 01 — Fork Zed and boot a `notes_app` binary.

**Status:** ready-for-human

- [x] Launching the binary opens a hard-coded `.md` file in Zed's Editor with Markdown syntax highlighting (headings, code, lists, emphasis)
- [x] Vim mode is on by default: NORMAL/INSERT/VISUAL modes, motions, operators, text objects, registers, `.` repeat, and `/` search work
- [x] `:w` and `Cmd+S` save the buffer to disk; the file on disk matches the editor content
- [ ] Typing and Vim motions have no perceptible latency
- [x] No Zed main-application UI (tabs of Zed's product surfaces, terminal, collab, AI) is initialized

## Comments

**2026-08-22 — implementation complete; manual acceptance pending.**

`notes_app` now opens `crates/notes_app/spike.md` in Zed's stock Editor using a local-only
`Project`, a Markdown-only language registry with Tree-sitter queries, the upstream Vim subsystem,
the buffer search bar required by Vim `/`, and workspace save/close routing. Vim is the app default;
both `:w` and `Cmd+S` persist to disk. The app does not initialize terminal, collaboration, or AI
product surfaces, and its HTTP client is blocked.

Five GPUI tests cover the native titlebar, Markdown grammar selection, Vim activation and command
interception, `Cmd+S`, `:w`, and `Cmd+W`. `cargo check -p notes_app --all-targets`,
`cargo test -p notes_app --lib`, and `./script/clippy -p notes_app` pass. The executable also handles
Zed's internal `--printenv` helper mode as an early exit, preventing recursive GUI launches during
shell-environment discovery.

The remaining human check is perceptual typing and motion latency during a normal manual run.
