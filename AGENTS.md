# zednotes

Personal Markdown notes app for macOS, developed as a `notes_app` binary crate inside a fork of
[zed-industries/zed](https://github.com/zed-industries/zed). The product/technical source spec lives
at `.original_spec.md`; the actionable MVP spec is `.scratch/mvp/spec.md`.

## Repository layout

This checkout **is** the Zed fork. `upstream` points at `zed-industries/zed` (push disabled);
`origin` is not configured yet. Everything under `crates/` except `crates/notes_*` is upstream Zed.

Notes-specific code goes in `crates/notes_app` (and later `notes_*` crates). Per the spec's upstream
patch policy, prefer in order: consume an existing Zed capability → configure it → wrap it → add a
local extension point → modify a generic crate as a last resort. Patches to generic Zed crates are
kept as small discrete commits so upstream merges stay cheap.

Run the app with `cargo run -p notes_app`. Use `./script/clippy` rather than `cargo clippy`.

## Release provenance

Tag Zednotes releases as `vMAJOR.MINOR.PATCH-zednotes` to avoid collisions with upstream Zed tags. The tagged commit records the exact source of every bundled Zed workspace crate; also record the Zednotes commit and upstream Zed base SHA, because local commits may patch generic Zed crates and a nominal Zed version alone is ambiguous. The tag's `Cargo.lock` pins third-party dependencies, and builds from a dirty worktree must be identified as non-reproducible.

## Inherited Zed agent rules

Upstream's agent rules live in `.rules` at the repo root (upstream symlinks `AGENTS.md` → `.rules`;
this fork replaces that symlink with this file). Read `.rules` — its Rust and GPUI guidance applies
to code written here — with two exceptions, because this fork does not open pull requests against
zed-industries/zed:

- The `README.md` `> [!IMPORTANT]` banner HARD RULE does not apply.
- The "Pull request hygiene" and "Rules Hygiene" sections do not apply.

## Agent skills

### Issue tracker

Issues live as local markdown files under `.scratch/<feature>/` in this repo. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage labels are used as-is (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` and `docs/adr/` at the repo root (created lazily). See `docs/agents/domain.md`.
