# 02 — Spike: one Markdown file in Zed Editor with Vim

**What to build:** The `notes_app` binary initializes the minimum set of Zed subsystems (assets, settings, theme, language, editor, vim) needed to open one hard-coded local Markdown file in Zed's Editor with Vim mode enabled and save it back to disk. This is the spec's "most important implementation decision" — it retires the project's main technical risk. No custom design; Zed's stock look is fine.

**Blocked by:** 01 — Fork Zed and boot a `notes_app` binary.

**Status:** ready-for-agent

- [ ] Launching the binary opens a hard-coded `.md` file in Zed's Editor with Markdown syntax highlighting (headings, code, lists, emphasis)
- [ ] Vim mode is on by default: NORMAL/INSERT/VISUAL modes, motions, operators, text objects, registers, `.` repeat, and `/` search work
- [ ] `:w` and `Cmd+S` save the buffer to disk; the file on disk matches the editor content
- [ ] Typing and Vim motions have no perceptible latency
- [ ] No Zed main-application UI (tabs of Zed's product surfaces, terminal, collab, AI) is initialized
