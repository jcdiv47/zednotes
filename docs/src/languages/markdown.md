---
title: Markdown
description: "Configure Markdown language support in Zed, including language servers, formatting, and debugging."
---

# Markdown

Markdown support is available natively in Zed.

- Tree-sitter: [tree-sitter-markdown](https://github.com/tree-sitter-grammars/tree-sitter-markdown)
- Language Server: N/A

## Wiki Links and Backlinks {#wiki-links}

Open your notes folder as a project to link Markdown files without a language
server. Type `[[` to complete a note name. Follow a link with
{#action editor::GoToDefinition} or click it in the Markdown preview.

| Syntax                   | Target                                    |
| ------------------------ | ----------------------------------------- |
| `[[Note]]`               | A note named `Note.md` or `Note.markdown` |
| `[[folder/Note]]`        | A note in a folder                        |
| `[[/folder/Note.md]]`    | An exact path from the notes folder root  |
| `[[../Note]]`            | A path relative to the current note       |
| `[[Note\|Display text]]` | A note with custom link text              |
| `[[Note#Heading]]`       | A heading in another note                 |
| `[[#Heading]]`           | A heading in the current note             |

Links resolve within the current project's root folder containing the note.
Resolution checks the current note's folder, then the root, then a unique matching
path elsewhere in that root. If multiple notes match, use a folder-qualified path.
Paths are case-sensitive. Heading text and heading slugs are both supported.

Run {#action markdown::ToggleBacklinks} to open the Backlinks panel. It lists wiki
links from other notes to the active note, including links to its headings. Click
an excerpt to select the link in its source note, or use the arrow keys and Enter.
The panel follows the active editor or preview and updates after unsaved edits and
file changes. Text inside code, math, and frontmatter does not create backlinks.

Backlinks currently support local folders. Note embeds, block references,
frontmatter aliases, and automatic rewriting of links after renaming a note are
not supported.

## Syntax Highlighting Code Blocks

Zed supports language-specific syntax highlighting of markdown code blocks by leveraging [tree-sitter language grammars](../extensions/languages.md#grammar). All [Zed supported languages](../languages.md), including those provided by official or community extensions, are available for use in markdown code blocks. All you need to do is provide a language name after the opening <kbd>```</kbd> code fence like so:

````python
```python
import functools as ft

@ft.lru_cache(maxsize=500)
def fib(n):
    return n if n < 2 else fib(n - 1) + fib(n - 2)
```
````

## Configuration

### Format

Zed supports using Prettier to automatically re-format Markdown documents. You can trigger this manually via the {#action editor::Format} action or via the {#kb editor::Format} keyboard shortcut. Alternately, you can enable format on save.

Configure formatting in Settings ({#kb zed::OpenSettings}) under Languages > Markdown, or add to your settings file:

```json [settings]
  "languages": {
    "Markdown": {
      "format_on_save": "on"
    }
  },
```

### List Continuation

Zed automatically continues lists when you press Enter at the end of a list item. Supported list types:

- Unordered lists (`-`, `*`, or `+` markers)
- Ordered lists (numbers are auto-incremented)
- Task lists (`- [ ]` and `- [x]`)

Pressing Enter on an empty list item removes the marker and exits the list.

To disable this behavior, configure in Settings ({#kb zed::OpenSettings}) under Languages > Markdown, or add to your settings file:

```json [settings]
  "languages": {
    "Markdown": {
      "extend_list_on_newline": false
    }
  },
```

### List Indentation

Zed indents list items when you press Tab while the cursor is on a line containing only a list marker. This allows you to quickly create nested lists.

To disable this behavior, configure in Settings ({#kb zed::OpenSettings}) under Languages > Markdown, or add to your settings file:

```json [settings]
  "languages": {
    "Markdown": {
      "indent_list_on_tab": false
    }
  },
```

### Trailing Whitespace

By default Zed will remove trailing whitespace on save. If you rely on invisible trailing whitespace being converted to `<br />` in Markdown files you can disable this behavior.

Configure in Settings ({#kb zed::OpenSettings}) under Languages > Markdown, or add to your settings file:

```json [settings]
  "languages": {
    "Markdown": {
      "remove_trailing_whitespace_on_save": false
    }
  },
```
