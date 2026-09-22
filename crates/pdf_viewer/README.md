# PDF viewing in Zednotes

Opening a local `.pdf` file opens a read-only workspace tab. The viewer provides
previous/next page controls, zoom, fit-to-page, manual reload, and split panes.
Enable **Continuous** in the toolbar to scroll vertically through the document.
In this mode, **Fit width** fits the widest page to the viewport, the page counter
follows scrolling, and Previous/Next scroll to the corresponding page. Toggle
Continuous off to return to single-page viewing.

Parsing and rasterization run on the background executor. Continuous mode retains
only visible pages and one neighboring page on either side; single-page mode
retains the current page. Raster dimensions are capped at 4096 pixels per side.
Errors appear in the tab, and Reload retries reading the file while preserving
page, zoom, and viewing mode.

## Engine assessment

[kkpdf-zed](https://github.com/kk376/kkpdf-zed) is pinned to
`4dbe33725a44161175fa38590e7860eaf73355b0`. It supplies a reusable Pdfium engine
and document metadata, but does not implement GPUI's `Render` or Zed's `Item`
traits. This crate supplies that integration. Production rendering explicitly
disables the upstream mock fallback. The dependency offers Apache-2.0 or
GPL-3.0-or-later licensing.

## Native runtime

For macOS or Linux development, run from the repository root:

```sh
script/install-pdfium
cargo run --profile release-fast -p zed --bin zed -- path/to/document.pdf
```

Restart Zednotes after installing the runtime. The installer downloads Pdfium
Chromium 8066 from `bblanchon/pdfium-binaries`, verifies a pinned SHA-256 digest,
and installs the library and its license notices under
`~/.local/share/kkpdf-zed/lib`. An explicit destination and target architecture
may be passed as the first and second arguments.

`script/bundle-mac` includes the matching library and notices in the app bundle
and signs the dylib when signing a distribution. Linux packages and Windows
bundles must supply Pdfium separately. The upstream loader also accepts
`PDFIUM_LIB_PATH` (full library filename) and `PDFIUM_LIB_DIR` (directory).

## Validation

```sh
cargo check -p pdf_viewer -p zed
cargo test -p pdf_viewer
cargo test -p pdf_viewer -- --include-ignored
```

The native test uses a real two-page PDF with different page sizes and colors.
It checks page count, dimensions, zoom, RGBA-to-BGRA conversion, raster limits,
out-of-range pages, and malformed-document rejection. A GPUI regression test
opens a single-file worktree, clicks Next, changes zoom, switches viewing modes,
scrolls across a page boundary, and verifies Reload preserves navigation state.
Layout tests cover mixed page sizes, fit-to-width, and the visible page range.

## Current limits

Local files only. No text selection/search, editing, automatic reload,
or session restoration. Password-protected PDFs report
an opening error. Pdfium parses documents in-process, as in the upstream engine.
