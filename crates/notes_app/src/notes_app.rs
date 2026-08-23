//! zednotes' application layer.
//!
//! Everything notes-specific lives in this crate rather than in the generic Zed
//! crates of the fork, so that merges from `upstream` stay cheap.

use std::{
    cell::Cell,
    cmp::Reverse,
    io::ErrorKind,
    path::Path,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use assets::Assets;
use client::{Client, UserStore};
use db::kvp::KeyValueStore;
use editor::Editor;
use fs::{Fs, PathEventKind, RealFs};
use futures::StreamExt as _;
#[cfg(test)]
use gpui::WindowHandle;
use gpui::{
    Action as _, AnyElement, App, AppContext as _, AsyncApp, BorrowAppContext as _, Bounds,
    DismissEvent, Entity, Focusable as _, Global, KeyBinding, Keystroke, Menu, MenuItem,
    PathPromptOptions, Render, Subscription, Task, TaskExt as _, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use http_client::BlockedHttpClient;
use language::{Buffer, BufferEvent, Capability, DiskState, Language, LanguageRegistry};
use markdown_preview::markdown_preview_view::MarkdownPreviewView;
use node_runtime::NodeRuntime;
use percent_encoding::percent_decode_str;
use picker::{Picker, PickerDelegate};
use project::Project;
use project_panel::{
    EntryFilter, Event as ProjectPanelEvent, ProjectPanel, ProjectPanelOptions,
    project_panel_settings::ProjectPanelSettings,
};
use search::{BufferSearchBar, ProjectSearchView, project_search::ProjectSearchBar};
use semver::Version;
use serde::{Deserialize, Serialize};
use session::{AppSession, Session};
use settings::{
    AutosaveSetting, BufferLineHeight, CommandAliasTarget, DockSide, FontFamilyName, KeybindSource,
    KeymapFile, KeymapFileLoadResult, Settings as _, SettingsStore, SoftWrap, ThemeAppearanceMode,
    ThemeName, ThemeSelection,
};
use theme::{ActiveTheme as _, GlobalTheme, LoadThemes};
use util::ResultExt as _;
use vim::ModeIndicator;
use workspace as zed_workspace;
use zed_workspace::{
    AppState, CloseIntent, Event as WorkspaceEvent, ItemHandle as _, MultiWorkspace, OpenMode,
    OpenOptions, OpenVisible, Pane, SaveIntent, SerializedWorkspaceLocation, SessionWorkspace,
    SplitDirection, Toast, Workspace, WorkspaceStore,
    dock::DockPosition,
    item::SaveOptions,
    notifications::{DetachAndPromptErr as _, NotificationId},
    pane::Event as PaneEvent,
};

const APP_NAME: &str = "zednotes";
const DEFAULT_MARKDOWN_SEARCH_FILTER: &str = "**/*.md, **/*.markdown";
const DEFAULT_EXCLUDED_PATHS: &[&str] = &[".git", ".DS_Store", "node_modules", "target"];
const DEFAULT_PREVIEW_MAX_WIDTH: u32 = 760;
const DEFAULT_PREVIEW_MAX_FILE_SIZE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_AUTOSAVE_DELAY_MS: u64 = 750;
const DEFAULT_EXPLORER_WIDTH: u32 = 240;
const DEFAULT_EDITOR_FONT_FAMILY: &str = ".ZedMono";
const DEFAULT_EDITOR_FONT_SIZE: f32 = 15.0;
const DEFAULT_EDITOR_LINE_HEIGHT: f32 = 1.55;
const DEFAULT_VIM_LEADER: &str = "space";
const RECENT_NOTES_KEY: &str = "notes_recent_usage";
const MAX_RECENT_NOTES: usize = 512;
const INITIAL_SETTINGS_CONTENT: &str = r#"{
  // system, light, or dark
  "theme": "system",
  "vim_mode": true,

  "vim": {
    "leader": "space",
  },

  "editor": {
    "font_family": ".ZedMono",
    "font_size": 15,
    "line_height": 1.55,
    "line_numbers": false,
    "soft_wrap": true,
  },

  "autosave": {
    "enabled": true,
    "delay_ms": 750,
  },

  "explorer": {
    "width": 240,
    "markdown_only": true,
    "show_hidden": false,
  },

  "preview": {
    "max_width": 760,
  },
}
"#;

actions!(
    notes,
    [
        /// Opens the user-owned zednotes settings file.
        OpenSettings,
        /// Quits zednotes.
        Quit,
    ]
);

actions!(
    workspace,
    [
        /// Opens a directory as the notes workspace.
        OpenFolder,
        /// Opens a recently used notes workspace.
        #[action(name = "Open Recent")]
        OpenRecentWorkspace,
        /// Searches Markdown notes in the workspace.
        Search,
    ]
);

actions!(
    explorer,
    [
        /// Toggles hidden files in the explorer.
        ToggleHiddenFiles,
        /// Returns focus from the explorer to the active editor.
        FocusEditor,
    ]
);

actions!(
    note,
    [
        /// Creates a new Markdown note.
        New,
        /// Creates a new directory in the notes workspace.
        NewDirectory,
        /// Renames the selected note or directory.
        Rename,
        /// Moves the selected note or directory to the system Trash.
        Delete,
        /// Duplicates the selected note or directory.
        Duplicate,
        /// Opens a recency-and-frequency-ranked list of recent notes.
        #[action(name = "Open Recent")]
        OpenRecentNote,
        /// Toggles a preview split for the active note.
        TogglePreview,
        /// Opens the active note as a preview-only tab.
        OpenPreview,
        /// Opens the relative Markdown link under the cursor.
        FollowLink,
    ]
);

actions!(
    view,
    [
        /// Toggles the notes explorer.
        ToggleExplorer,
    ]
);

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum NotesTheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq)]
struct NotesEditorSettings {
    font_family: String,
    font_size: f32,
    line_height: f32,
    line_numbers: bool,
    soft_wrap: bool,
}

impl Default for NotesEditorSettings {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_EDITOR_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_EDITOR_FONT_SIZE,
            line_height: DEFAULT_EDITOR_LINE_HEIGHT,
            line_numbers: false,
            soft_wrap: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExplorerSettings {
    width: u32,
    markdown_only: bool,
    show_hidden: bool,
    excluded_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewSettings {
    max_width: u32,
    allow_remote_images: bool,
    max_file_size_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AutosaveSettings {
    enabled: bool,
    delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NotesVimSettings {
    leader: String,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
        }
    }
}

impl Default for NotesVimSettings {
    fn default() -> Self {
        Self {
            leader: DEFAULT_VIM_LEADER.to_owned(),
        }
    }
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            max_width: DEFAULT_PREVIEW_MAX_WIDTH,
            allow_remote_images: false,
            max_file_size_bytes: Some(DEFAULT_PREVIEW_MAX_FILE_SIZE_BYTES),
        }
    }
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self {
            width: DEFAULT_EXPLORER_WIDTH,
            markdown_only: true,
            show_hidden: false,
            excluded_paths: DEFAULT_EXCLUDED_PATHS
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct NotesSettings {
    theme: NotesTheme,
    vim_mode: bool,
    vim: NotesVimSettings,
    editor: NotesEditorSettings,
    autosave: AutosaveSettings,
    explorer: ExplorerSettings,
    preview: PreviewSettings,
}

impl Default for NotesSettings {
    fn default() -> Self {
        Self {
            theme: NotesTheme::System,
            vim_mode: true,
            vim: NotesVimSettings::default(),
            editor: NotesEditorSettings::default(),
            autosave: AutosaveSettings::default(),
            explorer: ExplorerSettings::default(),
            preview: PreviewSettings::default(),
        }
    }
}

#[derive(Clone)]
struct CurrentNotesSettings(NotesSettings);

impl Global for CurrentNotesSettings {}

#[derive(Default)]
struct CurrentUserKeyBindings(Vec<KeyBinding>);

impl Global for CurrentUserKeyBindings {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct RecentNotes {
    entries: Vec<RecentNoteUsage>,
}

impl Global for RecentNotes {}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RecentNoteUsage {
    path: PathBuf,
    open_count: u64,
    last_opened: u64,
}

impl RecentNotes {
    fn record(&mut self, path: PathBuf, opened_at: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == path) {
            entry.open_count = entry.open_count.saturating_add(1);
            entry.last_opened = opened_at;
        } else {
            self.entries.push(RecentNoteUsage {
                path,
                open_count: 1,
                last_opened: opened_at,
            });
        }
        self.entries.sort_by_key(|entry| Reverse(entry.last_opened));
        self.entries.truncate(MAX_RECENT_NOTES);
    }

    fn score(&self, path: &Path) -> f64 {
        let Some(entry) = self.entries.iter().find(|entry| entry.path == path) else {
            return 0.0;
        };
        entry.last_opened as f64 + (entry.open_count as f64 + 1.0).ln() * 86_400.0
    }
}

struct NotesEmptyState {
    project: Entity<Project>,
    _subscription: Subscription,
}

impl NotesEmptyState {
    fn new(project: Entity<Project>, cx: &mut gpui::Context<Self>) -> Self {
        let _subscription = cx.subscribe(&project, |_, _, event, cx| {
            if matches!(
                event,
                project::Event::WorktreeAdded(_)
                    | project::Event::WorktreeRemoved(_)
                    | project::Event::WorktreeUpdatedEntries(_, _)
            ) {
                cx.notify();
            }
        });
        Self {
            project,
            _subscription,
        }
    }

    fn has_markdown_notes(&self, cx: &App) -> bool {
        self.project.read(cx).visible_worktrees(cx).any(|worktree| {
            worktree
                .read(cx)
                .snapshot()
                .files(false, 0)
                .any(|entry| is_markdown_path(entry.path.as_std_path()))
        })
    }
}

impl Render for NotesEmptyState {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let has_notes = self.has_markdown_notes(cx);
        let title = if has_notes {
            "Select a note"
        } else {
            "No notes yet"
        };
        let shortcuts = if has_notes {
            "⌘P Open note    ⌘N New note    ⌘⇧F Search"
        } else {
            "⌘N New note    ⌘O Open folder    ⌘P Find note"
        };
        let colors = cx.theme().colors();

        div()
            .key_context("NotesEmptyState")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .text_color(colors.text)
            .child(div().text_size(px(18.)).child(title))
            .child(div().text_color(colors.text_muted).child(shortcuts))
    }
}

struct RecentWorkspacePickerDelegate {
    workspace: WeakEntity<Workspace>,
    workspaces: Vec<zed_workspace::RecentWorkspace>,
    filtered_indices: Vec<usize>,
    selected_index: usize,
}

impl RecentWorkspacePickerDelegate {
    fn new(
        workspace: WeakEntity<Workspace>,
        workspaces: Vec<zed_workspace::RecentWorkspace>,
    ) -> Self {
        let filtered_indices = (0..workspaces.len()).collect();
        Self {
            workspace,
            workspaces,
            filtered_indices,
            selected_index: 0,
        }
    }

    fn workspace_label(workspace: &zed_workspace::RecentWorkspace) -> String {
        workspace
            .identity_paths
            .ordered_paths()
            .next()
            .or_else(|| workspace.paths.ordered_paths().next())
            .and_then(|path| path.file_name())
            .map_or_else(
                || "Notes Folder".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
    }

    fn workspace_path(workspace: &zed_workspace::RecentWorkspace) -> String {
        workspace
            .paths
            .ordered_paths()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

impl PickerDelegate for RecentWorkspacePickerDelegate {
    type ListItem = AnyElement;

    fn name() -> &'static str {
        "notes recent workspaces"
    }

    fn match_count(&self) -> usize {
        self.filtered_indices.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut gpui::Context<Picker<Self>>,
    ) {
        self.selected_index = index.min(self.filtered_indices.len().saturating_sub(1));
        cx.notify();
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search recent notes folders…".into()
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut gpui::Context<Picker<Self>>,
    ) -> Task<()> {
        let query = query.to_lowercase();
        self.filtered_indices = self
            .workspaces
            .iter()
            .enumerate()
            .filter_map(|(index, workspace)| {
                let label = Self::workspace_label(workspace).to_lowercase();
                let path = Self::workspace_path(workspace).to_lowercase();
                (query.is_empty() || label.contains(&query) || path.contains(&query))
                    .then_some(index)
            })
            .collect();
        self.selected_index = self
            .selected_index
            .min(self.filtered_indices.len().saturating_sub(1));
        cx.notify();
        Task::ready(())
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Picker<Self>>,
    ) {
        let Some(workspace_index) = self.filtered_indices.get(self.selected_index).copied() else {
            return;
        };
        let Some(recent_workspace) = self.workspaces.get(workspace_index) else {
            return;
        };
        let paths = recent_workspace.paths.paths().to_vec();
        if let Some(workspace) = self.workspace.upgrade() {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.open_workspace_for_paths(OpenMode::Activate, paths, window, cx)
                })
                .detach_and_log_err(cx);
        }
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut gpui::Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        index: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut gpui::Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let workspace_index = self.filtered_indices.get(index).copied()?;
        let workspace = self.workspaces.get(workspace_index)?;
        let colors = cx.theme().colors();
        Some(
            div()
                .id(("recent-workspace", index))
                .w_full()
                .px_3()
                .py_2()
                .rounded_sm()
                .when(selected, |element| element.bg(colors.element_selected))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(Self::workspace_label(workspace))
                        .child(
                            div()
                                .text_color(colors.text_muted)
                                .child(Self::workspace_path(workspace)),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[derive(Debug, Default, Deserialize)]
struct NotesSettingsContent {
    theme: Option<NotesTheme>,
    vim_mode: Option<bool>,
    vim: Option<NotesVimSettingsContent>,
    editor: Option<NotesEditorSettingsContent>,
    autosave: Option<AutosaveSettingsContent>,
    explorer: Option<ExplorerSettingsContent>,
    files: Option<FileSettingsContent>,
    preview: Option<PreviewSettingsContent>,
}

#[derive(Debug, Default, Deserialize)]
struct NotesVimSettingsContent {
    leader: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct NotesEditorSettingsContent {
    font_family: Option<String>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    line_numbers: Option<bool>,
    soft_wrap: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct AutosaveSettingsContent {
    enabled: Option<bool>,
    delay_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ExplorerSettingsContent {
    width: Option<u32>,
    markdown_only: Option<bool>,
    show_hidden: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FileSettingsContent {
    exclude: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct PreviewSettingsContent {
    max_width: Option<u32>,
    allow_remote_images: Option<bool>,
    max_file_size_bytes: Option<u64>,
}

fn parse_notes_settings(content: &str) -> Result<NotesSettings> {
    let content = settings::parse_json_with_comments::<NotesSettingsContent>(content)
        .context("failed to parse zednotes settings")?;
    let mut settings = NotesSettings::default();
    settings.theme = content.theme.unwrap_or(settings.theme);
    settings.vim_mode = content.vim_mode.unwrap_or(settings.vim_mode);
    if let Some(vim) = content.vim {
        settings.vim.leader = vim.leader.unwrap_or(settings.vim.leader);
    }
    if let Some(editor) = content.editor {
        settings.editor.font_family = editor.font_family.unwrap_or(settings.editor.font_family);
        settings.editor.font_size = editor.font_size.unwrap_or(settings.editor.font_size);
        settings.editor.line_height = editor.line_height.unwrap_or(settings.editor.line_height);
        settings.editor.line_numbers = editor.line_numbers.unwrap_or(settings.editor.line_numbers);
        settings.editor.soft_wrap = editor.soft_wrap.unwrap_or(settings.editor.soft_wrap);
    }
    if let Some(autosave) = content.autosave {
        settings.autosave.enabled = autosave.enabled.unwrap_or(settings.autosave.enabled);
        settings.autosave.delay_ms = autosave.delay_ms.unwrap_or(settings.autosave.delay_ms);
    }
    if let Some(explorer) = content.explorer {
        settings.explorer.width = explorer.width.unwrap_or(settings.explorer.width);
        settings.explorer.markdown_only = explorer
            .markdown_only
            .unwrap_or(settings.explorer.markdown_only);
        settings.explorer.show_hidden = explorer
            .show_hidden
            .unwrap_or(settings.explorer.show_hidden);
    }
    if let Some(excluded_paths) = content.files.and_then(|files| files.exclude) {
        settings.explorer.excluded_paths = excluded_paths;
    }
    if let Some(preview) = content.preview {
        settings.preview.max_width = preview.max_width.unwrap_or(settings.preview.max_width);
        settings.preview.allow_remote_images = preview
            .allow_remote_images
            .unwrap_or(settings.preview.allow_remote_images);
        settings.preview.max_file_size_bytes = preview
            .max_file_size_bytes
            .or(settings.preview.max_file_size_bytes);
    }

    anyhow::ensure!(
        !settings.editor.font_family.trim().is_empty(),
        "editor.font_family cannot be empty"
    );
    anyhow::ensure!(
        settings.editor.font_size > 0.0,
        "editor.font_size must be greater than zero"
    );
    anyhow::ensure!(
        settings.editor.line_height >= 1.0,
        "editor.line_height must be at least 1.0"
    );
    anyhow::ensure!(
        !settings.vim.leader.is_empty() && !settings.vim.leader.chars().any(char::is_whitespace),
        "vim.leader must be one GPUI keystroke"
    );
    Keystroke::parse(&settings.vim.leader)
        .with_context(|| format!("invalid vim.leader {:?}", settings.vim.leader))?;
    anyhow::ensure!(
        settings.explorer.width > 0,
        "explorer.width must be positive"
    );
    anyhow::ensure!(
        settings.preview.max_width > 0,
        "preview.max_width must be positive"
    );
    Ok(settings)
}

fn config_dir_path() -> PathBuf {
    paths::home_dir().join(".config").join(APP_NAME)
}

pub fn application_support_dir() -> PathBuf {
    paths::home_dir()
        .join("Library")
        .join("Application Support")
        .join(APP_NAME)
}

fn notes_settings_path() -> PathBuf {
    config_dir_path().join("settings.json")
}

fn notes_keymap_path() -> PathBuf {
    config_dir_path().join("keymap.json")
}

fn load_recent_notes(cx: &mut App) {
    let key_value_store = KeyValueStore::global(cx);
    let recent_notes = match key_value_store.read_kvp(RECENT_NOTES_KEY) {
        Ok(Some(content)) => serde_json::from_str(&content).unwrap_or_else(|error| {
            log::error!("failed to load recent note usage: {error:#}");
            RecentNotes::default()
        }),
        Ok(None) => RecentNotes::default(),
        Err(error) => {
            log::error!("failed to read recent note usage: {error:#}");
            RecentNotes::default()
        }
    };
    cx.set_global(recent_notes);
}

fn recent_note_score(path: &Path, cx: &App) -> f64 {
    cx.try_global::<RecentNotes>()
        .map_or(0.0, |recent_notes| recent_notes.score(path))
}

fn record_recent_note(path: PathBuf, cx: &mut App) {
    if !is_markdown_path(&path) {
        return;
    }
    if !cx.has_global::<RecentNotes>() {
        cx.set_global(RecentNotes::default());
    }
    let opened_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let content = cx.update_global::<RecentNotes, _>(|recent_notes, _| {
        recent_notes.record(path, opened_at);
        serde_json::to_string(recent_notes)
    });
    let Ok(content) = content else {
        log::error!("failed to serialize recent note usage");
        return;
    };
    let key_value_store = KeyValueStore::global(cx);
    cx.background_spawn(async move {
        key_value_store
            .write_kvp(RECENT_NOTES_KEY.to_owned(), content)
            .await
            .log_err();
    })
    .detach();
}

fn load_notes_settings() -> NotesSettings {
    let path = notes_settings_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => parse_notes_settings(&content).unwrap_or_else(|error| {
            log::error!(
                "failed to load {}: {error:#}; using defaults",
                path.display()
            );
            NotesSettings::default()
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => NotesSettings::default(),
        Err(error) => {
            log::error!("failed to read {}: {error}; using defaults", path.display());
            NotesSettings::default()
        }
    }
}

fn exclusion_glob(path: &str) -> String {
    if path.contains('/') || path.contains('*') || path.contains('?') {
        path.to_owned()
    } else {
        format!("**/{path}")
    }
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

fn markdown_note_filename(filename: &str) -> String {
    if is_markdown_path(Path::new(filename)) {
        filename.to_owned()
    } else {
        format!("{filename}.md")
    }
}

fn markdown_link_target_at(text: &str, cursor_offset: usize) -> Option<String> {
    const SCAN_LIMIT: usize = 4096;

    if cursor_offset > text.len() || !text.is_char_boundary(cursor_offset) {
        return None;
    }
    let mut scan_start = cursor_offset.saturating_sub(SCAN_LIMIT);
    while !text.is_char_boundary(scan_start) {
        scan_start += 1;
    }
    let mut scan_end = cursor_offset.saturating_add(SCAN_LIMIT).min(text.len());
    while !text.is_char_boundary(scan_end) {
        scan_end -= 1;
    }
    let text = &text[scan_start..scan_end];
    let cursor_offset = cursor_offset - scan_start;
    let bytes = text.as_bytes();
    let mut link_start = 0;

    while link_start < bytes.len() {
        if bytes[link_start] != b'['
            || is_escaped(bytes, link_start)
            || (link_start > 0 && bytes[link_start - 1] == b'!')
        {
            link_start += 1;
            continue;
        }

        let Some(link_text_end) = find_balanced_delimiter(bytes, link_start, b'[', b']') else {
            link_start += 1;
            continue;
        };
        let destination_start = link_text_end + 1;
        if bytes.get(destination_start) != Some(&b'(') {
            link_start += 1;
            continue;
        }
        let Some(link_end) = find_balanced_delimiter(bytes, destination_start, b'(', b')') else {
            link_start += 1;
            continue;
        };

        if (link_start..=link_end).contains(&cursor_offset) {
            return parse_markdown_link_destination(&text[destination_start + 1..link_end]);
        }
        link_start = link_end + 1;
    }

    None
}

fn find_balanced_delimiter(bytes: &[u8], start: usize, opening: u8, closing: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in bytes.iter().copied().enumerate().skip(start) {
        if is_escaped(bytes, index) {
            continue;
        }
        if byte == opening {
            depth = depth.saturating_add(1);
        } else if byte == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn parse_markdown_link_destination(contents: &str) -> Option<String> {
    let contents = contents.trim_start();
    if let Some(contents) = contents.strip_prefix('<') {
        let end = contents
            .as_bytes()
            .iter()
            .enumerate()
            .find_map(|(index, byte)| {
                (*byte == b'>' && !is_escaped(contents.as_bytes(), index)).then_some(index)
            })?;
        return unescape_markdown_destination(&contents[..end]);
    }

    let bytes = contents.as_bytes();
    let mut nested_parentheses = 0usize;
    let mut end = bytes.len();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index = index.saturating_add(2);
            continue;
        }
        match bytes[index] {
            b'(' => nested_parentheses = nested_parentheses.saturating_add(1),
            b')' if nested_parentheses > 0 => nested_parentheses -= 1,
            byte if byte.is_ascii_whitespace() && nested_parentheses == 0 => {
                end = index;
                break;
            }
            _ => {}
        }
        index += 1;
    }

    unescape_markdown_destination(contents[..end].trim_end())
}

fn unescape_markdown_destination(destination: &str) -> Option<String> {
    if destination.is_empty() {
        return None;
    }

    let mut result = String::with_capacity(destination.len());
    let mut characters = destination.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            result.push(characters.next()?);
        } else {
            result.push(character);
        }
    }
    Some(result)
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let preceding_backslashes = bytes[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count();
    preceding_backslashes % 2 == 1
}

fn resolve_relative_markdown_link(current_file: &Path, target: &str) -> Option<PathBuf> {
    let path_end = target.find(['#', '?']).unwrap_or(target.len());
    let target = &target[..path_end];
    if target.is_empty() || target.starts_with("//") || has_uri_scheme(target) {
        return None;
    }

    let target = percent_decode_str(target).decode_utf8().ok()?;
    let target = Path::new(target.as_ref());
    if target.is_absolute() {
        return None;
    }

    let parent = current_file.parent()?;
    util::paths::normalize_lexically(&parent.join(target)).ok()
}

fn has_uri_scheme(target: &str) -> bool {
    let Some(colon_index) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon_index];
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || (index > 0 && b"+-.".contains(&byte))
        })
}

fn markdown_entry_filter(markdown_only: bool) -> EntryFilter {
    Arc::new(move |path: &Path, is_directory| {
        is_directory || !markdown_only || is_markdown_path(path)
    })
}

fn markdown_language(name: &str, grammar: tree_sitter::Language) -> Result<Arc<Language>> {
    let language = Language::new(grammars::load_config(name), Some(grammar))
        .with_queries(grammars::load_queries(name))
        .with_context(|| format!("failed to load Tree-sitter queries for {name}"))?;
    Ok(Arc::new(language))
}

fn markdown_languages(cx: &App) -> Result<Arc<LanguageRegistry>> {
    let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
    languages.add(markdown_language(
        "markdown-inline",
        tree_sitter_md::INLINE_LANGUAGE.into(),
    )?);
    languages.add(markdown_language(
        "markdown",
        tree_sitter_md::LANGUAGE.into(),
    )?);
    languages.set_theme(cx.theme().clone());
    Ok(languages)
}

fn notes_vim_key_bindings(leader: &str) -> Vec<KeyBinding> {
    const CONTEXT: Option<&str> = Some("Editor && VimControl && vim_mode == normal");
    let keys = |suffix: &str| format!("{leader} {suffix}");

    vec![
        KeyBinding::new(leader, gpui::NoAction, CONTEXT),
        KeyBinding::new(
            &keys("f f"),
            zed_workspace::ToggleFileFinder::default(),
            CONTEXT,
        ),
        KeyBinding::new(&keys("f g"), Search, CONTEXT),
        KeyBinding::new(&keys("f n"), New, CONTEXT),
        KeyBinding::new(&keys("f r"), OpenRecentNote, CONTEXT),
        KeyBinding::new(&keys("p"), zed_actions::command_palette::Toggle, CONTEXT),
        KeyBinding::new(&keys("e"), ToggleExplorer, CONTEXT),
        KeyBinding::new(&keys("v"), TogglePreview, CONTEXT),
        KeyBinding::new(
            &keys("b n"),
            zed_workspace::ActivateNextItem::default(),
            CONTEXT,
        ),
        KeyBinding::new(
            &keys("b p"),
            zed_workspace::ActivatePreviousItem::default(),
            CONTEXT,
        ),
        KeyBinding::new(
            &keys("b d"),
            zed_workspace::CloseActiveItem::default(),
            CONTEXT,
        ),
        KeyBinding::new(&keys("w h"), zed_workspace::ActivatePaneLeft, CONTEXT),
        KeyBinding::new(&keys("w j"), zed_workspace::ActivatePaneDown, CONTEXT),
        KeyBinding::new(&keys("w k"), zed_workspace::ActivatePaneUp, CONTEXT),
        KeyBinding::new(&keys("w l"), zed_workspace::ActivatePaneRight, CONTEXT),
        KeyBinding::new(
            &keys("w v"),
            zed_workspace::SplitVertical::default(),
            CONTEXT,
        ),
        KeyBinding::new(
            &keys("w s"),
            zed_workspace::SplitHorizontal::default(),
            CONTEXT,
        ),
        KeyBinding::new(
            &keys("s f"),
            zed_actions::buffer_search::Deploy::find(),
            CONTEXT,
        ),
        KeyBinding::new(&keys("s p"), Search, CONTEXT),
    ]
}

fn bind_default_editor_keymaps(cx: &mut App) -> Result<()> {
    for (path, source) in [
        (settings::DEFAULT_KEYMAP_PATH, KeybindSource::Default),
        (settings::VIM_KEYMAP_PATH, KeybindSource::Vim),
    ] {
        let mut key_bindings = KeymapFile::load_asset_allow_partial_failure(path, cx)?;
        for key_binding in &mut key_bindings {
            key_binding.set_meta(source.meta());
        }
        cx.bind_keys(key_bindings);
    }
    let mut leader_bindings =
        notes_vim_key_bindings(&cx.global::<CurrentNotesSettings>().0.vim.leader);
    for key_binding in &mut leader_bindings {
        key_binding.set_meta(KeybindSource::Vim.meta());
    }
    cx.bind_keys(leader_bindings);
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, Some("Workspace")),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("Editor")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("MarkdownPreview")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("ProjectPanel")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("Workspace")),
        KeyBinding::new("cmd-n", New, Some("Workspace")),
        KeyBinding::new("cmd-b", ToggleExplorer, Some("Workspace")),
        KeyBinding::new("cmd-shift-f", Search, Some("Pane")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new(
            "cmd-shift-v",
            TogglePreview,
            Some("Editor && extension == md"),
        ),
        KeyBinding::new("cmd-shift-v", TogglePreview, Some("MarkdownPreview")),
        KeyBinding::new(
            "enter",
            FollowLink,
            Some("Editor && extension == md && VimControl && vim_mode == normal"),
        ),
        KeyBinding::new(
            "enter",
            FollowLink,
            Some("Editor && extension == markdown && VimControl && vim_mode == normal"),
        ),
        KeyBinding::new("ctrl--", zed_workspace::GoBack, Some("Pane")),
        KeyBinding::new("ctrl-_", zed_workspace::GoForward, Some("Pane")),
        KeyBinding::new(
            "cmd-s",
            zed_workspace::Save { save_intent: None },
            Some("Editor"),
        ),
        KeyBinding::new(
            "enter",
            editor::actions::OpenExcerpts,
            Some("ProjectSearchView > Editor"),
        ),
        KeyBinding::new("a", New, Some("ProjectPanel && not_editing")),
        KeyBinding::new("shift-a", NewDirectory, Some("ProjectPanel && not_editing")),
        KeyBinding::new("r", Rename, Some("ProjectPanel && not_editing")),
        KeyBinding::new("d", Delete, Some("ProjectPanel && not_editing")),
        KeyBinding::new(
            "enter",
            project_panel::OpenPermanent,
            Some("ProjectPanel && not_editing"),
        ),
        KeyBinding::new(
            "o",
            project_panel::OpenPermanent,
            Some("ProjectPanel && not_editing"),
        ),
        KeyBinding::new(
            "l",
            project_panel::OpenPermanent,
            Some("ProjectPanel && not_editing"),
        ),
        KeyBinding::new("q", FocusEditor, Some("ProjectPanel && not_editing")),
    ]);
    Ok(())
}

fn reload_editor_keymaps(mut user_key_bindings: Vec<KeyBinding>, cx: &mut App) -> Result<()> {
    cx.clear_key_bindings();
    bind_default_editor_keymaps(cx)?;
    for key_binding in &mut user_key_bindings {
        key_binding.set_meta(KeybindSource::User.meta());
    }
    if cx.has_global::<CurrentUserKeyBindings>() {
        cx.update_global::<CurrentUserKeyBindings, _>(|current, _| {
            current.0 = user_key_bindings.clone()
        });
    } else {
        cx.set_global(CurrentUserKeyBindings(user_key_bindings.clone()));
    }
    cx.bind_keys(user_key_bindings);
    Ok(())
}

fn reload_editor_keymap_content(content: &str, cx: &mut App) -> Result<()> {
    match KeymapFile::load(content, cx) {
        KeymapFileLoadResult::Success { key_bindings } => reload_editor_keymaps(key_bindings, cx),
        KeymapFileLoadResult::SomeFailedToLoad {
            key_bindings,
            error_message,
        } => {
            log::error!(
                "failed to load part of {}: {error_message}",
                notes_keymap_path().display()
            );
            reload_editor_keymaps(key_bindings, cx)
        }
        KeymapFileLoadResult::JsonParseFailure { error } => {
            log::error!(
                "failed to parse {}: {error:#}; using default keymap",
                notes_keymap_path().display()
            );
            reload_editor_keymaps(Vec::new(), cx)
        }
    }
}

fn load_user_keymap(cx: &mut App) -> Result<()> {
    let path = notes_keymap_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => reload_editor_keymap_content(&content, cx),
        Err(error) if error.kind() == ErrorKind::NotFound => reload_editor_keymaps(Vec::new(), cx),
        Err(error) => {
            log::error!(
                "failed to read {}: {error}; using default keymap",
                path.display()
            );
            reload_editor_keymaps(Vec::new(), cx)
        }
    }
}

fn init_editor_subsystems(cx: &mut App) -> Result<()> {
    zed_actions::init();
    command_palette::init(cx);
    editor::init(cx);
    file_finder::init_with_file_filter_file_creation_and_history_ranker(
        Arc::new(is_markdown_path),
        false,
        Arc::new(recent_note_score),
        cx,
    );
    markdown_preview::init(cx);
    project_panel::init(cx);
    search::init(cx);
    vim::init(cx);
    // Vim activates its settings observer at the end of this effect cycle, so
    // reapply the configured mode after that observer is live.
    cx.defer(|cx| {
        let vim_mode = cx.global::<CurrentNotesSettings>().0.vim_mode;
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_default_settings(cx, |settings| {
                settings.vim_mode = Some(vim_mode);
                settings.helix_mode = Some(false);
            });
        });
    });
    load_user_keymap(cx)?;

    cx.on_action(quit);
    observe_external_file_changes(cx);
    cx.set_menus([
        Menu::new(APP_NAME).items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Open Recent…", OpenRecentWorkspace),
            MenuItem::action("Search Workspace", Search),
        ]),
        Menu::new("Note").items([
            MenuItem::action("New Note…", New),
            MenuItem::action("Open Recent…", OpenRecentNote),
            MenuItem::action("New Directory…", NewDirectory),
            MenuItem::action("Rename…", Rename),
            MenuItem::action("Move to Trash…", Delete),
            MenuItem::action("Duplicate", Duplicate),
            MenuItem::action("Toggle Preview", TogglePreview),
            MenuItem::action("Open Preview", OpenPreview),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Explorer", ToggleExplorer),
            MenuItem::action("Toggle Hidden Files", ToggleHiddenFiles),
        ]),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.on_window_closed(|cx, _window_id| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();

    Ok(())
}

fn apply_explorer_settings(settings: &ExplorerSettings, cx: &mut App) {
    let excluded_paths = settings
        .excluded_paths
        .iter()
        .map(|path| exclusion_glob(path))
        .collect::<Vec<_>>();
    let hide_hidden = !settings.show_hidden;
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            let project_panel = content.project_panel.get_or_insert_default();
            project_panel.dock = Some(DockSide::Left);
            project_panel.starts_open = Some(true);
            project_panel.default_width = Some((settings.width as f32).into());
            project_panel.hide_gitignore = Some(true);
            project_panel.hide_hidden = Some(hide_hidden);
            project_panel.auto_fold_dirs = Some(false);

            let exclusions = content
                .project
                .worktree
                .file_scan_exclusions
                .get_or_insert_default();
            for excluded_path in excluded_paths {
                if !exclusions.0.contains(&excluded_path) {
                    exclusions.0.push(excluded_path);
                }
            }
        });
    });
}

fn apply_editor_settings(settings: &NotesEditorSettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.theme.buffer_font_family =
                Some(FontFamilyName::from(settings.font_family.clone()));
            content.theme.buffer_font_size = Some(settings.font_size.into());
            content.theme.buffer_line_height = Some(BufferLineHeight::Custom(settings.line_height));
            content.editor.gutter.get_or_insert_default().line_numbers =
                Some(settings.line_numbers);
            content.project.all_languages.defaults.soft_wrap = Some(if settings.soft_wrap {
                SoftWrap::EditorWidth
            } else {
                SoftWrap::None
            });
        });
    });
}

fn apply_theme_setting(theme: NotesTheme, cx: &mut App) {
    let mode = match theme {
        NotesTheme::System => ThemeAppearanceMode::System,
        NotesTheme::Light => ThemeAppearanceMode::Light,
        NotesTheme::Dark => ThemeAppearanceMode::Dark,
    };
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.theme.theme = Some(ThemeSelection::Dynamic {
                mode,
                light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
                dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
            });
        });
    });
}

fn apply_preview_settings(settings: &PreviewSettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            let preview = content.markdown_preview.get_or_insert_default();
            preview.limit_content_width = Some(true);
            preview.max_width = Some((settings.max_width as f32).into());
            preview.allow_remote_images = Some(settings.allow_remote_images);
            preview.max_file_size_bytes = settings.max_file_size_bytes;
        });
    });
}

fn apply_autosave_settings(settings: AutosaveSettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.workspace.autosave = Some(if settings.enabled {
                AutosaveSetting::AfterDelay {
                    milliseconds: settings.delay_ms.into(),
                }
            } else {
                AutosaveSetting::Off
            });
            content.workspace.close_on_file_delete = Some(false);
        });
    });
}

fn apply_notes_settings(settings: NotesSettings, cx: &mut App) {
    let leader_changed = cx
        .try_global::<CurrentNotesSettings>()
        .is_some_and(|current| current.0.vim.leader != settings.vim.leader);
    apply_theme_setting(settings.theme, cx);
    apply_editor_settings(&settings.editor, cx);
    apply_explorer_settings(&settings.explorer, cx);
    apply_preview_settings(&settings.preview, cx);
    apply_autosave_settings(settings.autosave, cx);
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.vim_mode = Some(settings.vim_mode);
            content.helix_mode = Some(false);
            for (alias, target) in [
                ("preview", "note::TogglePreview"),
                ("explorer", "view::ToggleExplorer"),
                ("notes", "file_finder::Toggle"),
                ("search", "workspace::Search"),
            ] {
                content
                    .workspace
                    .command_aliases
                    .entry(alias.to_owned())
                    .or_insert_with(|| CommandAliasTarget::new(target));
            }
        });
    });
    if cx.has_global::<CurrentNotesSettings>() {
        cx.update_global::<CurrentNotesSettings, _>(|current, _| current.0 = settings);
    } else {
        cx.set_global(CurrentNotesSettings(settings));
    }
    if leader_changed {
        let user_key_bindings = cx
            .try_global::<CurrentUserKeyBindings>()
            .map(|bindings| bindings.0.clone());
        if let Some(user_key_bindings) = user_key_bindings {
            reload_editor_keymaps(user_key_bindings, cx).log_err();
        }
    }
    cx.refresh_windows();
}

fn autosave_enabled(cx: &App) -> bool {
    cx.global::<CurrentNotesSettings>().0.autosave.enabled
}

fn quit(_: &Quit, cx: &mut App) {
    let autosave_enabled = autosave_enabled(cx);
    let windows = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<MultiWorkspace>())
        .collect::<Vec<_>>();

    cx.spawn(async move |cx| {
        for window in windows {
            let workspaces = window
                .update(cx, |multi_workspace, _, _| {
                    multi_workspace.workspaces().cloned().collect::<Vec<_>>()
                })
                .log_err()
                .unwrap_or_default();

            for workspace in workspaces {
                if autosave_enabled {
                    let save_tasks = window
                        .update(cx, |_, window, cx| {
                            workspace
                                .update(cx, |workspace, cx| autosave_tasks(workspace, window, cx))
                        })
                        .log_err()
                        .unwrap_or_default();
                    for save_task in save_tasks {
                        save_task.await?;
                    }
                }

                let should_quit = window
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            workspace.prepare_to_close(CloseIntent::Quit, window, cx)
                        })
                    })
                    .log_err();
                if let Some(should_quit) = should_quit
                    && !should_quit.await?
                {
                    return anyhow::Ok(());
                }

                let flush_task = window
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            workspace.flush_serialization(window, cx)
                        })
                    })
                    .log_err();
                if let Some(flush_task) = flush_task {
                    flush_task.await;
                }
            }
        }

        cx.update(|cx| cx.quit());
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn open_disk_comparison(
    editor: gpui::Entity<Editor>,
    workspace: gpui::Entity<Workspace>,
    project: gpui::Entity<Project>,
    path: PathBuf,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Task<Result<()>> {
    let fs = <dyn Fs>::global(cx).clone();
    let language = editor
        .read(cx)
        .buffer()
        .read(cx)
        .as_singleton()
        .and_then(|buffer| buffer.read(cx).language().cloned());
    let title = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("{name} (On Disk)"))
        .unwrap_or_else(|| "On Disk".to_owned());

    window.spawn(cx, async move |cx| {
        let disk_text = fs.load(&path).await?;
        workspace.update_in(cx, |workspace, window, cx| {
            let disk_buffer = cx.new(|cx| {
                let mut buffer = Buffer::local(disk_text, cx);
                buffer.set_capability(Capability::ReadOnly, cx);
                if let Some(language) = language {
                    buffer.set_language(Some(language), cx);
                }
                buffer
            });
            let disk_editor =
                cx.new(|cx| Editor::for_buffer(disk_buffer, Some(project.clone()), window, cx));
            let disk_multi_buffer = disk_editor.read(cx).buffer().clone();
            disk_multi_buffer.update(cx, |buffer, cx| buffer.set_title(title, cx));
            workspace.split_item(SplitDirection::Right, Box::new(disk_editor), window, cx);
        })?;
        Ok(())
    })
}

fn prompt_for_external_file_change(
    editor: &mut Editor,
    buffer: gpui::Entity<Buffer>,
    prompt_open: Rc<Cell<bool>>,
    window: &mut Window,
    cx: &mut gpui::Context<Editor>,
) {
    if prompt_open.get() {
        return;
    }
    let (was_deleted, has_conflict, path) = {
        let buffer = buffer.read(cx);
        (
            buffer
                .file()
                .is_some_and(|file| file.disk_state() == DiskState::Deleted),
            buffer.has_conflict(),
            buffer
                .file()
                .and_then(|file| file.as_local().map(|file| file.abs_path(cx))),
        )
    };
    if !was_deleted && !has_conflict {
        return;
    }

    let Some(workspace) = editor.workspace() else {
        return;
    };
    if workspace
        .read(cx)
        .pane_for_item_id(cx.entity_id())
        .is_none()
    {
        return;
    }
    let project = workspace.read(cx).project().clone();

    prompt_open.set(true);
    let (message, buttons): (&str, &[&str]) = if was_deleted {
        (
            "This note was deleted outside zednotes.",
            &["Keep Open", "Save Again", "Close"],
        )
    } else {
        (
            "This note changed on disk while you have unsaved edits.",
            &["Keep My Changes", "Reload from Disk", "Compare"],
        )
    };
    let answer = window.prompt(gpui::PromptLevel::Warning, message, None, buttons, cx);
    cx.spawn_in(window, async move |editor, cx| {
        let answer = answer.await.ok();
        prompt_open.set(false);
        let Some(answer) = answer else {
            return Ok(());
        };
        let Some(editor) = editor.upgrade() else {
            return Ok(());
        };

        if was_deleted {
            match answer {
                0 => {}
                1 => {
                    let save = cx.update(|window, cx| {
                        editor.save(
                            SaveOptions {
                                format: false,
                                force_format: false,
                                autosave: false,
                            },
                            project,
                            window,
                            cx,
                        )
                    })?;
                    save.await?;
                }
                2 => {
                    workspace.update_in(cx, |workspace, window, cx| {
                        if let Some(pane) = workspace.pane_for_item_id(editor.entity_id()) {
                            pane.update(cx, |pane, cx| {
                                pane.remove_item(editor.entity_id(), true, false, window, cx);
                            });
                        }
                    })?;
                }
                _ => {}
            }
        } else {
            match answer {
                0 => {}
                1 => {
                    let reload = cx.update(|window, cx| editor.reload(project, window, cx))?;
                    reload.await?;
                }
                2 => {
                    if let Some(path) = path {
                        let compare = cx.update(|window, cx| {
                            open_disk_comparison(editor, workspace, project, path, window, cx)
                        })?;
                        compare.await?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })
    .detach_and_prompt_err(
        "Failed to handle the external note change",
        window,
        cx,
        |_, _, _| None,
    );
}

fn observe_external_file_changes(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, window, cx| {
        let Some(window) = window else {
            return;
        };
        let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
            return;
        };
        let prompt_open = Rc::new(Cell::new(false));
        let change_generation = Rc::new(Cell::new(0_u64));

        cx.subscribe_in(
            &buffer,
            window,
            move |_, buffer, event: &BufferEvent, window, cx| {
                if matches!(event, BufferEvent::Saved | BufferEvent::Reloaded) {
                    change_generation.set(change_generation.get().wrapping_add(1));
                } else if event == &BufferEvent::FileHandleChanged {
                    let generation = change_generation.get().wrapping_add(1);
                    change_generation.set(generation);
                    let change_generation = change_generation.clone();
                    let buffer = buffer.clone();
                    let prompt_open = prompt_open.clone();
                    cx.spawn_in(window, async move |editor, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(25))
                            .await;
                        if change_generation.get() != generation {
                            return;
                        }
                        editor
                            .update_in(cx, |editor, window, cx| {
                                prompt_for_external_file_change(
                                    editor,
                                    buffer,
                                    prompt_open,
                                    window,
                                    cx,
                                );
                            })
                            .log_err();
                    })
                    .detach();
                }
            },
        )
        .detach();
    })
    .detach();
}

fn close_preview(
    workspace: &mut Workspace,
    preview: gpui::Entity<MarkdownPreviewView>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let source_editor = preview.read(cx).source_editor();
    let Some(pane) = workspace.pane_for_item_id(preview.entity_id()) else {
        return;
    };
    pane.update(cx, |pane, cx| {
        pane.close_item_by_id(preview.entity_id(), SaveIntent::Skip, window, cx)
    })
    .detach_and_log_err(cx);
    if let Some(source_editor) = source_editor {
        source_editor.focus_handle(cx).focus(window, cx);
    }
}

fn toggle_preview(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    if let Some(preview) = workspace.active_item_as::<MarkdownPreviewView>(cx) {
        close_preview(workspace, preview, window, cx);
        return;
    }

    let Some(editor) = MarkdownPreviewView::resolve_active_item_as_markdown_editor(workspace, cx)
    else {
        return;
    };
    let existing_preview = workspace
        .items_of_type::<MarkdownPreviewView>(cx)
        .find(|preview| preview.read(cx).is_previewing_editor(&editor, cx));
    if let Some(preview) = existing_preview {
        close_preview(workspace, preview, window, cx);
    } else {
        let pane = workspace.active_pane().clone();
        MarkdownPreviewView::open_preview_to_the_side_of_pane(workspace, editor, pane, window, cx);
    }
}

fn open_preview(workspace: &mut Workspace, window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    let Some(editor) = MarkdownPreviewView::resolve_active_item_as_markdown_editor(workspace, cx)
    else {
        return;
    };
    let pane = workspace.active_pane().clone();
    MarkdownPreviewView::open_preview_in_pane(workspace, editor, pane, window, cx);
}

fn follow_link(workspace: &mut Workspace, window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        return;
    };
    let target_path = editor.update(cx, |editor, cx| {
        let cursor = editor.newest_selection(cx).head();
        let buffer = editor.buffer().read(cx).as_singleton()?;
        let buffer = buffer.read(cx);
        let source_path = buffer.file()?.as_local()?.abs_path(cx);
        let cursor_offset = buffer.snapshot().point_to_offset(cursor);
        let target = markdown_link_target_at(&editor.text(cx), cursor_offset)?;
        resolve_relative_markdown_link(&source_path, &target)
    });

    let Some(target_path) = target_path else {
        match cx.build_action("vim::NextLineStart", None) {
            Ok(action) => window.dispatch_action(action, cx),
            Err(error) => log::error!("failed to build vim::NextLineStart: {error:#}"),
        }
        return;
    };

    let fs = workspace.app_state().fs.clone();
    cx.spawn_in(window, async move |workspace, cx| {
        if !fs.is_file(&target_path).await {
            workspace.update(cx, |workspace, cx| {
                workspace.show_toast(
                    Toast::new(
                        NotificationId::Named("notes-missing-link".into()),
                        format!("Linked note not found: {}", target_path.display()),
                    )
                    .autohide(),
                    cx,
                );
            })?;
            return anyhow::Ok(());
        }

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    target_path,
                    OpenOptions {
                        visible: Some(OpenVisible::None),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })?
            .await?;
        anyhow::Ok(())
    })
    .detach_and_prompt_err("Failed to open linked note", window, cx, |_, _, _| None);
}

fn dispatch_project_panel_action(
    workspace: &mut Workspace,
    action_name: &str,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    workspace.open_panel::<ProjectPanel>(window, cx);
    let Some(panel) = workspace.panel::<ProjectPanel>(cx) else {
        return;
    };
    panel.focus_handle(cx).focus(window, cx);
    match cx.build_action(action_name, None) {
        Ok(action) => window.dispatch_action(action, cx),
        Err(error) => log::error!("failed to build {action_name}: {error:#}"),
    }
}

fn open_recent_workspace_picker(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let database = zed_workspace::WorkspaceDb::global(cx);
    let fs = workspace.app_state().fs.clone();
    let workspace_handle = cx.entity().downgrade();
    cx.spawn_in(window, async move |workspace, cx| {
        let recent_workspaces = database
            .recent_project_workspaces(fs.as_ref())
            .await?
            .into_iter()
            .filter(|workspace| matches!(workspace.location, SerializedWorkspaceLocation::Local))
            .collect::<Vec<_>>();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_modal(window, cx, |window, cx| {
                Picker::uniform_list(
                    RecentWorkspacePickerDelegate::new(workspace_handle, recent_workspaces),
                    window,
                    cx,
                )
                .show_scrollbar(true)
            });
        })?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn open_settings_file(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let path = notes_settings_path();
    let config_dir = config_dir_path();
    let fs = workspace.app_state().fs.clone();
    let project = workspace.project().clone();
    cx.spawn_in(window, async move |workspace, cx| {
        if !fs.is_file(&path).await {
            fs.save(&path, &INITIAL_SETTINGS_CONTENT.into(), Default::default())
                .await
                .with_context(|| format!("failed to create {}", path.display()))?;
        }
        project
            .update(cx, |project, cx| {
                project.find_or_create_worktree(config_dir, false, cx)
            })
            .await?;
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    path,
                    OpenOptions {
                        visible: Some(OpenVisible::None),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })?
            .await?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn init_workspace_composition(
    app_state: Arc<AppState>,
    explorer_settings: ExplorerSettings,
    cx: &mut App,
) {
    let entry_filter = markdown_entry_filter(explorer_settings.markdown_only);
    cx.observe_new(move |workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };

        configure_workspace(workspace, window, cx);

        workspace.register_action({
            let app_state = app_state.clone();
            move |workspace, _: &OpenFolder, window, cx| {
                zed_workspace::prompt_for_open_path_and_open(
                    workspace,
                    app_state.clone(),
                    PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Open Notes Folder".into()),
                    },
                    false,
                    window,
                    cx,
                );
            }
        });
        workspace.register_action(|workspace, _: &OpenRecentWorkspace, window, cx| {
            open_recent_workspace_picker(workspace, window, cx);
        });
        workspace.register_action(|_, _: &OpenRecentNote, window, cx| {
            window.dispatch_action(
                zed_workspace::ToggleFileFinder {
                    separate_history: true,
                    include_ignored: Some(false),
                }
                .boxed_clone(),
                cx,
            );
        });
        workspace.register_action(|_: &mut Workspace, _: &ToggleHiddenFiles, _, cx| {
            let hide_hidden = ProjectPanelSettings::get_global(cx).hide_hidden;
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store.update_default_settings(cx, |content| {
                    content.project_panel.get_or_insert_default().hide_hidden = Some(!hide_hidden);
                });
            });
        });
        workspace.register_action(|workspace, _: &OpenSettings, window, cx| {
            open_settings_file(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &FocusEditor, window, cx| {
            workspace.focus_center_pane(window, cx);
        });
        workspace.register_action(|workspace, _: &ToggleExplorer, window, cx| {
            workspace.toggle_dock(DockPosition::Left, window, cx);
        });
        workspace.register_action(|workspace, _: &Search, window, cx| {
            ProjectSearchView::deploy_search(
                workspace,
                &zed_workspace::DeploySearch {
                    included_files: Some(DEFAULT_MARKDOWN_SEARCH_FILTER.to_owned()),
                    ..Default::default()
                },
                window,
                cx,
            );
        });
        workspace.register_action(|workspace, _: &TogglePreview, window, cx| {
            toggle_preview(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &OpenPreview, window, cx| {
            open_preview(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &FollowLink, window, cx| {
            follow_link(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &New, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::NewFile", window, cx);
        });
        workspace.register_action(|workspace, _: &NewDirectory, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::NewDirectory", window, cx);
        });
        workspace.register_action(|workspace, _: &Rename, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::Rename", window, cx);
        });
        workspace.register_action(|workspace, _: &Delete, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::Trash", window, cx);
        });
        workspace.register_action(|workspace, _: &Duplicate, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::Duplicate", window, cx);
        });

        let panel_task = cx.spawn_in(window, {
            let entry_filter = entry_filter.clone();
            async move |workspace, cx| {
                let panel = ProjectPanel::load_with_options(
                    workspace.clone(),
                    ProjectPanelOptions {
                        entry_filter: Some(entry_filter),
                        new_file_name_transformer: Some(Arc::new(markdown_note_filename)),
                        post_create_action: Some(Box::new(vim::SwitchToInsertMode)),
                    },
                    cx.clone(),
                )
                .await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    cx.subscribe_in(&panel, window, |workspace, _, event, _, cx| {
                        let message = match event {
                            ProjectPanelEvent::EntryCreated {
                                project_path,
                                is_directory,
                            } => Some(format!(
                                "Created {} {}",
                                if *is_directory { "folder" } else { "note" },
                                project_path.path.as_unix_str()
                            )),
                            ProjectPanelEvent::EntryRenamed {
                                old_project_path,
                                new_project_path,
                                is_directory,
                            } => Some(format!(
                                "Renamed {} {} to {}",
                                if *is_directory { "folder" } else { "note" },
                                old_project_path.path.as_unix_str(),
                                new_project_path.path.as_unix_str()
                            )),
                            _ => None,
                        };
                        if let Some(message) = message {
                            workspace.show_toast(
                                Toast::new(
                                    NotificationId::Named("notes-entry-operation".into()),
                                    message,
                                )
                                .autohide(),
                                cx,
                            );
                        }
                    })
                    .detach();
                    workspace.add_panel(panel, window, cx);
                    workspace.finish_dock_restoration(cx);
                })?;
                anyhow::Ok(())
            }
        });
        workspace.set_panels_task(panel_task);
    })
    .detach();
}

fn watch_configuration_files(fs: Arc<dyn Fs>, cx: &mut App) {
    let config_dir = config_dir_path();
    let settings_path = notes_settings_path();
    let keymap_path = notes_keymap_path();
    cx.spawn(async move |cx| {
        let (mut events, _watcher) = fs.watch(&config_dir, Duration::from_millis(100)).await;
        while let Some(events) = events.next().await {
            let mut settings_changed = false;
            let mut keymap_changed = false;
            for event in events {
                let config_rescanned =
                    event.kind == Some(PathEventKind::Rescan) && event.path == config_dir;
                settings_changed |= event.path == settings_path || config_rescanned;
                keymap_changed |= event.path == keymap_path || config_rescanned;
            }

            if settings_changed {
                let settings = match fs.load(&settings_path).await {
                    Ok(content) => parse_notes_settings(&content).unwrap_or_else(|error| {
                        log::error!(
                            "failed to load {}: {error:#}; using defaults",
                            settings_path.display()
                        );
                        NotesSettings::default()
                    }),
                    Err(error) => {
                        if fs.is_file(&settings_path).await {
                            log::error!(
                                "failed to read {}: {error:#}; using defaults",
                                settings_path.display()
                            );
                        }
                        NotesSettings::default()
                    }
                };
                cx.update(|cx| apply_notes_settings(settings, cx));
            }

            if keymap_changed {
                let content = match fs.load(&keymap_path).await {
                    Ok(content) => content,
                    Err(error) => {
                        if fs.is_file(&keymap_path).await {
                            log::error!(
                                "failed to read {}: {error:#}; using default keymap",
                                keymap_path.display()
                            );
                        }
                        "[]".to_owned()
                    }
                };
                cx.update(|cx| {
                    reload_editor_keymap_content(&content, cx).log_err();
                });
            }
        }
    })
    .detach();
}

/// Initializes the local-only services needed by the file-backed editor.
pub fn init(session: Session, cx: &mut App) -> Result<Arc<AppState>> {
    release_channel::init(Version::new(0, 1, 0), cx);
    gpui_tokio::init(cx);
    settings::init(cx);
    theme_settings::init(LoadThemes::All(Box::new(Assets)), cx);
    let notes_settings = load_notes_settings();
    apply_notes_settings(notes_settings.clone(), cx);

    if let Err(error) = std::fs::create_dir_all(config_dir_path()) {
        log::error!("failed to create zednotes config directory: {error}");
    }

    let fs: Arc<dyn Fs> = Arc::new(RealFs::new(None, cx.background_executor().clone()));
    <dyn Fs>::set_global(fs.clone(), cx);
    cx.set_http_client(Arc::new(BlockedHttpClient::new()));
    load_recent_notes(cx);

    let client = Client::production(cx);
    Client::set_global(client.clone(), cx);
    client::init(&client, cx);
    Project::init(&client, cx);

    let languages = markdown_languages(cx)?;
    let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
    let workspace_store = cx.new(|cx| WorkspaceStore::new(client.clone(), cx));
    let session = cx.new(|cx| AppSession::new(session, cx));
    let app_state = Arc::new(AppState {
        languages,
        client,
        user_store,
        workspace_store,
        fs,
        build_window_options: notes_window_options,
        node_runtime: NodeRuntime::unavailable(),
        session,
    });
    cx.observe_global::<GlobalTheme>({
        let languages = app_state.languages.clone();
        move |cx| languages.set_theme(cx.theme().clone())
    })
    .detach();

    AppState::set_global(app_state.clone(), cx);
    zed_workspace::init(app_state.clone(), cx);
    init_editor_subsystems(cx)?;
    init_workspace_composition(app_state.clone(), notes_settings.explorer, cx);
    watch_configuration_files(app_state.fs.clone(), cx);
    Ok(app_state)
}

fn configure_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let active_pane = workspace.active_pane().clone();
    configure_pane(workspace, &active_pane, window, cx);
    let workspace_entity = cx.entity();
    cx.subscribe_in(
        &workspace_entity,
        window,
        move |workspace, _, event, window, cx| {
            if let WorkspaceEvent::PaneAdded(pane) = event {
                configure_pane(workspace, pane, window, cx);
            }
        },
    )
    .detach();
    cx.observe_window_activation(window, move |workspace, window, cx| {
        if autosave_enabled(cx) && !window.is_window_active() {
            autosave_workspace(workspace, window, cx);
        }
    })
    .detach();
    workspace.status_bar().update(cx, |status_bar, cx| {
        let mode_indicator = cx.new(|cx| ModeIndicator::new(window, cx));
        status_bar.add_right_item(mode_indicator, window, cx);
    });
}

fn autosave_workspace(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    for task in autosave_tasks(workspace, window, cx) {
        task.detach_and_log_err(cx);
    }
}

fn autosave_tasks(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut App,
) -> Vec<gpui::Task<Result<()>>> {
    let project = workspace.project().clone();
    let items = workspace
        .panes()
        .iter()
        .flat_map(|pane| {
            pane.read(cx)
                .items()
                .map(|item| item.boxed_clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    items
        .into_iter()
        .map(|item| Pane::autosave_item(item.as_ref(), project.clone(), window, cx))
        .collect()
}

fn configure_pane(
    workspace: &Workspace,
    pane: &gpui::Entity<Pane>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let languages = workspace.project().read(cx).languages().clone();
    let empty_state = cx.new(|cx| NotesEmptyState::new(workspace.project().clone(), cx));
    pane.update(cx, |pane, cx| {
        pane.set_should_display_welcome_page(false);
        pane.set_render_empty_state(
            {
                let empty_state = empty_state.clone();
                move |_, _| empty_state.clone().into_any_element()
            },
            cx,
        );
        pane.toolbar().update(cx, |toolbar, cx| {
            let buffer_search_bar = cx.new(|cx| BufferSearchBar::new(Some(languages), window, cx));
            toolbar.add_item(buffer_search_bar, window, cx);
            let project_search_bar = cx.new(|_| ProjectSearchBar::new());
            toolbar.add_item(project_search_bar, window, cx);
        });
    });
    cx.subscribe_in(pane, window, |workspace, _, event, window, cx| {
        if let PaneEvent::ActivateItem { local, .. } = event {
            if autosave_enabled(cx) {
                autosave_workspace(workspace, window, cx);
            }
            if *local {
                let active_path = workspace
                    .active_item(cx)
                    .and_then(|item| item.project_path(cx))
                    .and_then(|project_path| {
                        workspace
                            .project()
                            .read(cx)
                            .absolute_path(&project_path, cx)
                    });
                if let Some(active_path) = active_path {
                    record_recent_note(active_path, cx);
                }
            }
        }
    })
    .detach();
}

fn notes_window_options(_display: Option<uuid::Uuid>, cx: &mut App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1024.), px(700.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(APP_NAME.into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        window_min_size: Some(size(px(480.), px(360.))),
        ..Default::default()
    }
}

#[cfg(test)]
fn open_notes_window_for_path(
    app_state: Arc<AppState>,
    project: gpui::Entity<Project>,
    path: PathBuf,
    cx: &mut App,
) -> Result<WindowHandle<MultiWorkspace>> {
    let root_path = path
        .parent()
        .context("the initial note must have a parent directory")?
        .to_owned();
    let worktree = project.update(cx, |project, cx| {
        project.find_or_create_worktree(root_path, true, cx)
    });
    let buffer = project.update(cx, |project, cx| project.open_local_buffer(&path, cx));
    let options = notes_window_options(None, cx);
    let window_handle = cx.open_window(options, {
        let project = project.clone();
        move |window, cx| {
            window.set_window_title(APP_NAME);
            let workspace = cx.new(|cx| Workspace::new(None, project, app_state, window, cx));
            cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
        }
    })?;

    cx.spawn(async move |cx| {
        worktree.await?;
        let buffer = buffer.await?;
        window_handle.update(cx, |multi_workspace, window, cx| {
            let workspace = multi_workspace.workspace().clone();
            let editor = cx.new(|cx| Editor::for_buffer(buffer, Some(project), window, cx));
            workspace.update(cx, |workspace, cx| {
                workspace.add_item_to_active_pane(Box::new(editor), None, true, window, cx);
            });
        })?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);

    Ok(window_handle)
}

async fn restore_session_workspaces(app_state: Arc<AppState>, cx: &mut AsyncApp) -> Result<usize> {
    let (last_session_id, last_session_window_stack, database) = cx.update(|cx| {
        let session = app_state.session.read(cx);
        (
            session.last_session_id().map(str::to_owned),
            session.last_session_window_stack(),
            zed_workspace::WorkspaceDb::global(cx),
        )
    });
    let Some(last_session_id) = last_session_id else {
        return Ok(0);
    };
    let Some(locations) = zed_workspace::last_session_workspace_locations(
        &database,
        &last_session_id,
        last_session_window_stack,
        app_state.fs.as_ref(),
    )
    .await
    else {
        return Ok(0);
    };
    restore_workspace_locations(locations, app_state, cx).await
}

async fn restore_last_workspace(app_state: Arc<AppState>, cx: &mut AsyncApp) -> Result<usize> {
    let database = cx.update(|cx| zed_workspace::WorkspaceDb::global(cx));
    let Some((workspace_id, location, paths)) =
        zed_workspace::last_opened_workspace_location(&database, app_state.fs.as_ref()).await
    else {
        return Ok(0);
    };
    restore_workspace_locations(
        vec![SessionWorkspace {
            workspace_id,
            location,
            paths,
            window_id: None,
        }],
        app_state,
        cx,
    )
    .await
}

async fn restore_workspace_locations(
    locations: Vec<SessionWorkspace>,
    app_state: Arc<AppState>,
    cx: &mut AsyncApp,
) -> Result<usize> {
    let serialized_workspaces =
        cx.update(|cx| zed_workspace::read_serialized_multi_workspaces(locations, cx));
    let mut restored_count = 0;
    for serialized_workspace in serialized_workspaces {
        match zed_workspace::restore_multiworkspace(serialized_workspace, app_state.clone(), cx)
            .await
        {
            Ok(_) => restored_count += 1,
            Err(error) => log::error!("failed to restore notes workspace: {error:#}"),
        }
    }
    Ok(restored_count)
}

async fn open_first_launch_folder_picker(
    app_state: Arc<AppState>,
    cx: &mut AsyncApp,
) -> Result<()> {
    let prompt_app_state = app_state.clone();
    let open_task = cx.update(|cx| {
        zed_workspace::open_new(
            OpenOptions::default(),
            app_state,
            cx,
            move |workspace, window, cx| {
                zed_workspace::prompt_for_open_path_and_open(
                    workspace,
                    prompt_app_state,
                    PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Open Notes Folder".into()),
                    },
                    false,
                    window,
                    cx,
                );
            },
        )
    });
    open_task.await
}

async fn restore_or_prompt(app_state: Arc<AppState>, cx: &mut AsyncApp) -> Result<()> {
    let had_previous_session =
        cx.update(|cx| app_state.session.read(cx).last_session_id().is_some());
    let mut restored_count = restore_session_workspaces(app_state.clone(), cx).await?;
    if restored_count == 0 && had_previous_session {
        restored_count = restore_last_workspace(app_state.clone(), cx).await?;
    }
    if restored_count == 0 {
        open_first_launch_folder_picker(app_state, cx).await?;
    }
    Ok(())
}

pub fn start(cx: &mut App) {
    let app_state = AppState::global(cx);
    cx.spawn(async move |cx| restore_or_prompt(app_state, cx).await)
        .detach_and_log_err(cx);
}

async fn reopen_workspace(app_state: Arc<AppState>, cx: &mut AsyncApp) -> Result<()> {
    let has_workspace_window = cx.update(|cx| {
        cx.windows()
            .iter()
            .any(|window| window.downcast::<MultiWorkspace>().is_some())
    });
    if has_workspace_window {
        zed_workspace::activate_any_workspace_window(cx);
        return Ok(());
    }
    let mut restored_count = restore_last_workspace(app_state.clone(), cx).await?;
    if restored_count == 0 {
        restored_count = restore_session_workspaces(app_state.clone(), cx).await?;
    }
    if restored_count == 0 {
        open_first_launch_folder_picker(app_state, cx).await?;
    }
    Ok(())
}

pub fn reopen(cx: &mut App) {
    let Some(app_state) = AppState::try_global(cx) else {
        return;
    };
    cx.spawn(async move |cx| reopen_workspace(app_state, cx).await)
        .detach_and_log_err(cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_palette_hooks::GlobalCommandPaletteInterceptor;
    use editor::{DisplayPoint, SelectionEffects, display_map::DisplayRow};
    use fs::FakeFs;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};
    use project::ProjectPath;
    use serde_json::{Value, json};
    use settings::SettingsStore;
    use std::{path::Path, time::Duration};
    use util::rel_path::rel_path;

    const TEST_NOTE_PATH: &str = "/notes/spike.md";
    const TEST_NOTE: &str = "# Spike\n\n- **fast** editing\n";

    struct TestWindow {
        fs: Arc<FakeFs>,
        project: gpui::Entity<Project>,
        editor: gpui::Entity<Editor>,
        window: WindowHandle<MultiWorkspace>,
        cx: VisualTestContext,
    }

    async fn test_window_with_settings(
        cx: &mut TestAppContext,
        explorer_settings: ExplorerSettings,
        autosave_settings: AutosaveSettings,
        tree: Value,
        initial_note_path: &str,
    ) -> TestWindow {
        let app_state = cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            let app_state = AppState::test(cx);
            AppState::set_global(app_state.clone(), cx);
            zed_workspace::init(app_state.clone(), cx);
            let notes_settings = NotesSettings {
                explorer: explorer_settings.clone(),
                autosave: autosave_settings,
                ..NotesSettings::default()
            };
            apply_notes_settings(notes_settings, cx);
            init_editor_subsystems(cx).expect("failed to initialize editor test subsystems");
            init_workspace_composition(app_state.clone(), explorer_settings, cx);
            app_state
        });

        let fs = app_state.fs.as_fake().clone();
        fs.insert_tree("/notes", tree).await;
        let project = Project::test(app_state.fs.clone(), [Path::new("/notes")], cx).await;
        project.read_with(cx, |project, _| {
            project.languages().add(
                markdown_language("markdown-inline", tree_sitter_md::INLINE_LANGUAGE.into())
                    .expect("failed to create inline Markdown language"),
            );
            project.languages().add(
                markdown_language("markdown", tree_sitter_md::LANGUAGE.into())
                    .expect("failed to create Markdown language"),
            );
        });

        let window = cx
            .update(|cx| {
                open_notes_window_for_path(
                    app_state,
                    project.clone(),
                    PathBuf::from(initial_note_path),
                    cx,
                )
            })
            .expect("failed to open the notes window");
        let window_cx = VisualTestContext::from_window(window.into(), cx);
        window_cx.run_until_parked();
        let editor = window
            .read_with(cx, |multi_workspace, cx| {
                multi_workspace
                    .workspace()
                    .read(cx)
                    .active_item_as::<Editor>(cx)
                    .expect("the Markdown editor should be active")
            })
            .expect("failed to read the notes window");
        TestWindow {
            fs,
            project,
            editor,
            window,
            cx: window_cx,
        }
    }

    async fn test_window_with(
        cx: &mut TestAppContext,
        explorer_settings: ExplorerSettings,
        tree: Value,
        initial_note_path: &str,
    ) -> TestWindow {
        test_window_with_settings(
            cx,
            explorer_settings,
            AutosaveSettings::default(),
            tree,
            initial_note_path,
        )
        .await
    }

    async fn test_window(cx: &mut TestAppContext) -> TestWindow {
        test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await
    }

    fn project_path(
        project: &gpui::Entity<Project>,
        path: &str,
        cx: &TestAppContext,
    ) -> ProjectPath {
        let worktree_id = project.read_with(cx, |project, cx| {
            project
                .visible_worktrees(cx)
                .next()
                .expect("test project should have a visible worktree")
                .read(cx)
                .id()
        });
        ProjectPath {
            worktree_id,
            path: rel_path(path).into(),
        }
    }

    fn focus_project_path(test: &mut TestWindow, path: &str, cx: &mut TestAppContext) {
        let project_path = project_path(&test.project, path, cx);
        test.cx.update(|window, cx| {
            let workspace =
                Workspace::for_window(window, cx).expect("test window should contain a workspace");
            let panel = workspace
                .read(cx)
                .panel::<ProjectPanel>(cx)
                .expect("notes workspace should contain a project panel");
            panel.update(cx, |panel, cx| {
                panel.select_path_for_test(project_path, cx);
            });
            workspace.update(cx, |workspace, cx| {
                workspace.focus_panel::<ProjectPanel>(window, cx);
            });
        });
    }

    fn active_editor_path(test: &TestWindow, cx: &TestAppContext) -> PathBuf {
        active_editor(test, cx).read_with(cx, |editor, cx| {
            editor
                .buffer()
                .read(cx)
                .as_singleton()
                .and_then(|buffer| {
                    buffer
                        .read(cx)
                        .file()
                        .and_then(|file| file.as_local().map(|file| file.abs_path(cx)))
                })
                .expect("active editor should be file-backed")
        })
    }

    fn active_editor(test: &TestWindow, cx: &TestAppContext) -> gpui::Entity<Editor> {
        test.window
            .read_with(cx, |multi_workspace, cx| {
                multi_workspace
                    .workspace()
                    .read(cx)
                    .active_item_as::<Editor>(cx)
                    .expect("an editor should be active")
            })
            .expect("failed to read the notes window")
    }

    fn workspace(test: &TestWindow, cx: &TestAppContext) -> gpui::Entity<Workspace> {
        test.window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .expect("failed to read the notes window")
    }

    fn settle_external_change(test: &TestWindow) {
        test.cx.executor().advance_clock(Duration::from_millis(25));
        test.cx.run_until_parked();
    }

    fn buffer_search_bar(test: &TestWindow, cx: &TestAppContext) -> gpui::Entity<BufferSearchBar> {
        workspace(test, cx).read_with(cx, |workspace, cx| {
            workspace
                .active_pane()
                .read(cx)
                .toolbar()
                .read(cx)
                .item_of_type::<BufferSearchBar>()
                .expect("notes panes should have a buffer search bar")
        })
    }

    fn active_project_search(
        test: &TestWindow,
        cx: &TestAppContext,
    ) -> gpui::Entity<ProjectSearchView> {
        workspace(test, cx).read_with(cx, |workspace, cx| {
            workspace
                .active_item_as::<ProjectSearchView>(cx)
                .expect("workspace search should be active")
        })
    }

    fn execute_palette_command(test: &mut TestWindow, query: &str, cx: &mut TestAppContext) {
        test.cx.simulate_keystrokes("cmd-shift-p");
        assert!(
            workspace(test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<command_palette::CommandPalette>(cx)
                .is_some()),
            "Cmd+Shift+P should open the command palette"
        );
        test.cx.simulate_input(query);
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
    }

    fn execute_vim_command(test: &mut TestWindow, command: &str, cx: &mut TestAppContext) {
        test.cx.simulate_keystrokes(":");
        assert!(
            workspace(test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<command_palette::CommandPalette>(cx)
                .is_some()),
            "colon should open the Vim command line"
        );
        test.cx.simulate_input(command);
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
    }

    #[gpui::test]
    fn test_notes_windows_keep_the_native_macos_titlebar(cx: &mut TestAppContext) {
        let titlebar = cx
            .update(|cx| notes_window_options(None, cx))
            .titlebar
            .expect("a notes window must have a titlebar");

        assert!(
            !titlebar.appears_transparent,
            "the traffic lights must be drawn by macOS, not by us"
        );
    }

    #[test]
    fn test_session_database_uses_zednotes_application_support() {
        assert_eq!(
            application_support_dir(),
            paths::home_dir()
                .join("Library")
                .join("Application Support")
                .join("zednotes")
        );
        assert!(!application_support_dir().starts_with(env!("CARGO_MANIFEST_DIR")));
    }

    #[test]
    fn test_relative_markdown_link_resolution() {
        for (current_file, target, expected) in [
            ("/vault/current.md", "target.md", "/vault/target.md"),
            (
                "/vault/projects/current.md",
                "nested/target.md",
                "/vault/projects/nested/target.md",
            ),
            (
                "/vault/projects/current.md",
                "../target.md",
                "/vault/target.md",
            ),
        ] {
            assert_eq!(
                resolve_relative_markdown_link(Path::new(current_file), target),
                Some(PathBuf::from(expected))
            );
        }
    }

    #[test]
    fn test_relative_markdown_link_resolution_decodes_paths_and_strips_fragments() {
        assert_eq!(
            resolve_relative_markdown_link(
                Path::new("/vault/current.md"),
                "project%20notes.md#details"
            ),
            Some(PathBuf::from("/vault/project notes.md"))
        );
        assert_eq!(
            resolve_relative_markdown_link(Path::new("/vault/current.md"), "https://example.com"),
            None
        );
        assert_eq!(
            resolve_relative_markdown_link(Path::new("/vault/current.md"), "/absolute.md"),
            None
        );
    }

    #[test]
    fn test_markdown_link_target_at_accepts_link_text_destination_and_title() {
        let text = r#"See [Project notes](projects/notes\ \(draft\).md "Draft") today"#;
        assert_eq!(
            markdown_link_target_at(
                text,
                text.find("Project").expect("link text should be present")
            ),
            Some("projects/notes (draft).md".to_owned())
        );
        assert_eq!(
            markdown_link_target_at(
                text,
                text.find("notes\\")
                    .expect("link destination should be present")
            ),
            Some("projects/notes (draft).md".to_owned())
        );
        assert_eq!(
            markdown_link_target_at(
                text,
                text.find("today").expect("plain text should be present")
            ),
            None
        );
    }

    #[test]
    fn test_recent_note_score_combines_recency_and_frequency() {
        let frequent = PathBuf::from("/notes/frequent.md");
        let recent = PathBuf::from("/notes/recent.md");
        let stale = PathBuf::from("/notes/stale.md");
        let history = RecentNotes {
            entries: vec![
                RecentNoteUsage {
                    path: frequent.clone(),
                    open_count: 16,
                    last_opened: 1_000_000,
                },
                RecentNoteUsage {
                    path: recent.clone(),
                    open_count: 1,
                    last_opened: 1_086_400,
                },
                RecentNoteUsage {
                    path: stale.clone(),
                    open_count: 1,
                    last_opened: 900_000,
                },
            ],
        };

        assert!(history.score(&recent) > history.score(&stale));
        assert!(history.score(&frequent) > history.score(&recent));
    }

    #[gpui::test]
    async fn test_empty_state_distinguishes_empty_and_unselected_vaults(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
        });
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/empty", json!({ "readme.txt": "not a note" }))
            .await;
        fs.insert_tree("/with-notes", json!({ "one.md": "# One" }))
            .await;
        let empty_project = Project::test(fs.clone(), [Path::new("/empty")], cx).await;
        let notes_project = Project::test(fs, [Path::new("/with-notes")], cx).await;
        let empty_state = cx.new(|cx| NotesEmptyState::new(empty_project, cx));
        let notes_state = cx.new(|cx| NotesEmptyState::new(notes_project, cx));

        assert!(!empty_state.read_with(cx, NotesEmptyState::has_markdown_notes));
        assert!(notes_state.read_with(cx, NotesEmptyState::has_markdown_notes));
    }

    #[gpui::test]
    async fn test_recent_note_and_workspace_pickers_open(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-e");
        test.cx.run_until_parked();
        assert!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()),
            "Cmd+E should open the recent-note file finder"
        );
        test.cx.simulate_keystrokes("escape");
        test.cx.run_until_parked();

        test.cx.update(|window, cx| {
            window.dispatch_action(OpenRecentWorkspace.boxed_clone(), cx);
        });
        test.cx.run_until_parked();
        assert!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<Picker<RecentWorkspacePickerDelegate>>(cx)
                .is_some()),
            "Workspace: Open Recent should open the recent-workspace picker"
        );
    }

    #[gpui::test]
    async fn test_first_launch_opens_only_the_notes_folder_picker(cx: &mut TestAppContext) {
        let test = test_window(cx).await;
        let app_state = cx.read(AppState::global);
        test.window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to remove the setup window");
        test.fs
            .insert_tree("/first-notes", json!({ "welcome.md": "# Welcome" }))
            .await;
        cx.run_until_parked();

        let mut async_cx = cx.to_async();
        open_first_launch_folder_picker(app_state, &mut async_cx)
            .await
            .expect("failed to open the first-launch folder picker");
        cx.run_until_parked();

        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(!options.files);
            assert!(options.directories);
            assert!(!options.multiple);
            Some(vec![PathBuf::from("/first-notes")])
        });
        cx.run_until_parked();
        let workspace_windows = cx.read(|cx| {
            cx.windows()
                .into_iter()
                .filter_map(|window| window.downcast::<MultiWorkspace>())
                .collect::<Vec<_>>()
        });
        assert_eq!(workspace_windows.len(), 1);
        assert_eq!(
            workspace_windows[0]
                .read_with(cx, |multi_workspace, cx| multi_workspace
                    .workspace()
                    .read(cx)
                    .root_paths(cx))
                .expect("failed to read the first-launch window"),
            vec![Arc::<Path>::from(Path::new("/first-notes"))],
            "first launch should enter the selected notes folder"
        );
    }

    #[gpui::test]
    async fn test_restores_tabs_active_note_preview_layout_and_explorer_state(
        cx: &mut TestAppContext,
    ) {
        use session::Session;

        let test = test_window(cx).await;
        let app_state = cx.read(AppState::global);
        test.window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to remove the setup window");
        test.fs
            .insert_tree(
                "/session-notes",
                json!({
                    "one.md": "# One\n",
                    "two.md": (0..40).map(|line| format!("line {line}\n")).collect::<String>(),
                }),
            )
            .await;
        cx.run_until_parked();

        let session_id = cx.read(|cx| app_state.session.read(cx).id().to_owned());
        let zed_workspace::OpenResult { window, .. } = cx
            .update(|cx| {
                Workspace::new_local(
                    vec![PathBuf::from("/session-notes")],
                    app_state.clone(),
                    None,
                    None,
                    None,
                    OpenMode::Activate,
                    cx,
                )
            })
            .await
            .expect("failed to open the session workspace");
        cx.run_until_parked();

        for path in ["/session-notes/one.md", "/session-notes/two.md"] {
            window
                .update(cx, |multi_workspace, window, cx| {
                    multi_workspace.workspace().update(cx, |workspace, cx| {
                        workspace.open_abs_path(
                            PathBuf::from(path),
                            OpenOptions::default(),
                            window,
                            cx,
                        )
                    })
                })
                .expect("failed to schedule note open")
                .await
                .expect("failed to open note");
        }
        cx.run_until_parked();

        window
            .update(cx, |multi_workspace, window, cx| {
                let editor = multi_workspace
                    .workspace()
                    .read(cx)
                    .active_item_as::<Editor>(cx)
                    .expect("the second note should be active");
                editor.update(cx, |editor, cx| {
                    editor.change_selections(
                        SelectionEffects::no_scroll(),
                        window,
                        cx,
                        |selections| {
                            selections
                                .select_display_ranges([DisplayPoint::new(DisplayRow(20), 0)
                                    ..DisplayPoint::new(DisplayRow(20), 0)]);
                        },
                    );
                    editor.set_scroll_position(point(0., 4.5), window, cx);
                });
            })
            .expect("failed to set persisted editor position");

        window
            .update(cx, |multi_workspace, window, cx| {
                window.resize(size(px(1040.), px(740.)));
                multi_workspace.workspace().update(cx, |workspace, cx| {
                    let editor = workspace
                        .active_item_as::<Editor>(cx)
                        .expect("the second note should be active");
                    let pane = workspace.active_pane().clone();
                    MarkdownPreviewView::open_preview_to_the_side_of_pane(
                        workspace, editor, pane, window, cx,
                    );
                    assert!(workspace.set_panel_size_state::<ProjectPanel>(
                        zed_workspace::dock::PanelSizeState {
                            size: Some(px(333.)),
                            flex: None,
                        },
                        window,
                        cx,
                    ));
                    if workspace.left_dock().read(cx).is_open() {
                        workspace.toggle_dock(DockPosition::Left, window, cx);
                    }
                });
            })
            .expect("failed to configure persisted workspace state");
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();

        let flush_task = window
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.workspace().update(cx, |workspace, cx| {
                    workspace.flush_serialization(window, cx)
                })
            })
            .expect("failed to flush workspace state");
        flush_task.await;
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to remove the original session window");
        cx.run_until_parked();

        cx.update(|cx| {
            app_state.session.update(cx, |app_session, _| {
                app_session.replace_session_for_test(Session::test_with_old_session(session_id));
            });
        });
        let mut async_cx = cx.to_async();
        assert_eq!(
            restore_session_workspaces(app_state.clone(), &mut async_cx)
                .await
                .expect("failed to restore notes session"),
            1
        );
        cx.run_until_parked();

        let restored_window = cx
            .read(|cx| {
                cx.windows()
                    .into_iter()
                    .find_map(|window| window.downcast::<MultiWorkspace>())
            })
            .expect("the restored notes window should exist");
        restored_window
            .read_with(cx, |multi_workspace, cx| {
                let workspace = multi_workspace.workspace().read(cx);
                let editor_paths = workspace
                    .panes()
                    .iter()
                    .find_map(|pane| {
                        let paths = pane
                            .read(cx)
                            .items()
                            .filter_map(|item| item.downcast::<Editor>())
                            .filter_map(|editor| {
                                editor.read(cx).buffer().read(cx).as_singleton().and_then(
                                    |buffer| {
                                        buffer.read(cx).file().and_then(|file| {
                                            file.as_local().map(|file| file.abs_path(cx))
                                        })
                                    },
                                )
                            })
                            .collect::<Vec<_>>();
                        (paths.len() == 2).then_some(paths)
                    })
                    .expect("the restored editor pane should contain both tabs");
                assert_eq!(
                    editor_paths,
                    vec![
                        PathBuf::from("/session-notes/one.md"),
                        PathBuf::from("/session-notes/two.md"),
                    ]
                );
                assert_eq!(
                    workspace.active_item_as::<Editor>(cx).and_then(|editor| {
                        editor
                            .read(cx)
                            .buffer()
                            .read(cx)
                            .as_singleton()
                            .and_then(|buffer| {
                                buffer
                                    .read(cx)
                                    .file()
                                    .and_then(|file| file.as_local().map(|file| file.abs_path(cx)))
                            })
                    }),
                    Some(PathBuf::from("/session-notes/two.md"))
                );
                assert_eq!(
                    workspace.items_of_type::<MarkdownPreviewView>(cx).count(),
                    1
                );
                assert!(workspace.panes().len() >= 2);
                assert!(!workspace.left_dock().read(cx).is_open());
                assert_eq!(
                    workspace
                        .panel_size_state::<ProjectPanel>(cx)
                        .and_then(|state| state.size),
                    Some(px(333.))
                );
            })
            .expect("failed to inspect the restored notes window");
        assert_eq!(
            restored_window
                .update(cx, |_, window, _| window.window_bounds().get_bounds().size)
                .expect("failed to inspect restored window bounds"),
            size(px(1040.), px(740.))
        );
        let (cursor, scroll_y) = restored_window
            .update(cx, |multi_workspace, _, cx| {
                let editor = multi_workspace
                    .workspace()
                    .read(cx)
                    .items_of_type::<Editor>(cx)
                    .find(|editor| {
                        editor
                            .read(cx)
                            .buffer()
                            .read(cx)
                            .as_singleton()
                            .is_some_and(|buffer| {
                                buffer.read(cx).file().is_some_and(|file| {
                                    file.as_local().is_some_and(|file| {
                                        file.abs_path(cx).as_path()
                                            == Path::new("/session-notes/two.md")
                                    })
                                })
                            })
                    })
                    .expect("the second editor should be restored");
                editor.update(cx, |editor, cx| {
                    let selections = editor
                        .selections
                        .display_ranges(&editor.display_snapshot(cx));
                    (selections[0].start, editor.scroll_position(cx).y)
                })
            })
            .expect("failed to inspect the restored editor position");
        assert_eq!(cursor, DisplayPoint::new(DisplayRow(20), 0));
        assert!(
            (scroll_y - 4.5).abs() < 0.01,
            "restored scroll was {scroll_y}"
        );

        restored_window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to close the restored window");
        cx.run_until_parked();
        assert!(cx.read(|cx| cx.windows().is_empty()));
        let mut async_cx = cx.to_async();
        reopen_workspace(app_state, &mut async_cx)
            .await
            .expect("Dock reopen failed");
        cx.run_until_parked();
        let reopened_window = cx
            .read(|cx| {
                cx.windows()
                    .into_iter()
                    .find_map(|window| window.downcast::<MultiWorkspace>())
            })
            .expect("Dock reopen should restore the workspace window");
        assert_eq!(
            reopened_window
                .read_with(cx, |multi_workspace, cx| multi_workspace
                    .workspace()
                    .read(cx)
                    .root_paths(cx))
                .expect("failed to inspect the Dock-reopened window"),
            vec![Arc::<Path>::from(Path::new("/session-notes"))]
        );
    }

    #[gpui::test]
    async fn test_opens_markdown_in_vim_mode(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let editor = test.editor;

        let (language_name, has_grammar) = editor.read_with(cx, |editor, cx| {
            editor
                .buffer()
                .read(cx)
                .as_singleton()
                .and_then(|buffer| {
                    buffer
                        .read(cx)
                        .language()
                        .map(|language| (language.name().to_string(), language.grammar().is_some()))
                })
                .unwrap_or_default()
        });
        assert_eq!(language_name, "Markdown");
        assert!(has_grammar, "Markdown syntax highlighting needs a grammar");

        test.cx.simulate_keystrokes("i x escape");
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_cmd_s_saves_the_editor_buffer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("i x escape cmd-s");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[test]
    fn test_parses_autosave_settings_from_jsonc() {
        assert_eq!(
            parse_notes_settings("{}")
                .expect("default settings should parse")
                .autosave,
            AutosaveSettings {
                enabled: true,
                delay_ms: 750,
            }
        );

        let settings = parse_notes_settings(
            r#"
            {
                "autosave": {
                    "enabled": false,
                    "delay_ms": 1250,
                },
            }
            "#,
        )
        .expect("settings should parse")
        .autosave;

        assert_eq!(
            settings,
            AutosaveSettings {
                enabled: false,
                delay_ms: 1250,
            }
        );
    }

    #[test]
    fn test_parses_editor_theme_vim_and_explorer_width_settings() {
        let settings = parse_notes_settings(
            r#"
            {
                "theme": "dark",
                "vim_mode": false,
                "vim": {
                    "leader": "ctrl-space",
                },
                "editor": {
                    "font_family": "Test Mono",
                    "font_size": 18.5,
                    "line_height": 1.4,
                    "line_numbers": true,
                    "soft_wrap": false,
                },
                "explorer": {
                    "width": 320,
                },
            }
            "#,
        )
        .expect("settings should parse");

        assert_eq!(settings.theme, NotesTheme::Dark);
        assert!(!settings.vim_mode);
        assert_eq!(settings.vim.leader, "ctrl-space");
        assert_eq!(settings.editor.font_family, "Test Mono");
        assert_eq!(settings.editor.font_size, 18.5);
        assert_eq!(settings.editor.line_height, 1.4);
        assert!(settings.editor.line_numbers);
        assert!(!settings.editor.soft_wrap);
        assert_eq!(settings.explorer.width, 320);
        assert_eq!(settings.autosave, AutosaveSettings::default());
        assert_eq!(settings.preview, PreviewSettings::default());
    }

    #[test]
    fn test_invalid_settings_are_rejected_for_the_loader_to_fall_back() {
        assert!(parse_notes_settings("{ invalid").is_err());
        assert!(parse_notes_settings(r#"{ "editor": { "line_height": 0.5 } }"#).is_err());
        assert!(parse_notes_settings(r#"{ "vim": { "leader": "" } }"#).is_err());
        assert!(parse_notes_settings(r#"{ "vim": { "leader": "g g" } }"#).is_err());
        assert!(parse_notes_settings(r#"{ "vim": { "leader": "ctrl-alt-x-y" } }"#).is_err());
    }

    #[gpui::test]
    async fn test_applying_settings_updates_editor_theme_vim_and_explorer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let settings = NotesSettings {
            theme: NotesTheme::Dark,
            vim_mode: false,
            editor: NotesEditorSettings {
                font_family: "Test Mono".to_owned(),
                font_size: 18.0,
                line_height: 1.4,
                line_numbers: true,
                soft_wrap: false,
            },
            explorer: ExplorerSettings {
                width: 320,
                ..ExplorerSettings::default()
            },
            ..NotesSettings::default()
        };

        cx.update(|cx| apply_notes_settings(settings, cx));
        test.cx.run_until_parked();

        let (
            font_family,
            font_size,
            line_height,
            theme_mode,
            line_numbers,
            soft_wrap,
            explorer_width,
        ) = cx.update(|cx| {
            let theme = theme_settings::ThemeSettings::get_global(cx);
            (
                theme.buffer_font.family.to_string(),
                theme.buffer_font_size(cx),
                theme.buffer_line_height.value(),
                theme.theme.mode(),
                editor::EditorSettings::get_global(cx).gutter.line_numbers,
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .soft_wrap,
                ProjectPanelSettings::get_global(cx).default_width,
            )
        });
        assert_eq!(font_family, "Test Mono");
        assert_eq!(font_size, px(18.0));
        assert_eq!(line_height, 1.4);
        assert_eq!(theme_mode, Some(ThemeAppearanceMode::Dark));
        assert!(line_numbers);
        assert_eq!(soft_wrap, SoftWrap::None);
        assert_eq!(explorer_width, px(320.0));

        test.cx.simulate_keystrokes("x");
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}"),
            "disabling Vim should make normal text input insert immediately"
        );
    }

    #[gpui::test]
    async fn test_user_keymap_rebind_survives_vim_leader_reload(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        cx.update(|cx| {
            reload_editor_keymap_content(
                r#"
                [
                    {
                        "context": "Workspace",
                        "bindings": {
                            "cmd-b": "note::TogglePreview",
                        },
                    },
                ]
                "#,
                cx,
            )
            .expect("user keymap should load");
        });
        let mut settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        settings.vim.leader = "ctrl-space".to_owned();
        cx.update(|cx| apply_notes_settings(settings, cx));
        test.cx.run_until_parked();

        let workspace = workspace(&test, cx);
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        test.cx.simulate_keystrokes("cmd-b");
        test.cx.run_until_parked();

        assert!(
            workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()),
            "the default Cmd+B action must not fire after it is rebound"
        );
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            2,
            "the rebound preview action should fire"
        );
    }

    #[gpui::test]
    async fn test_settings_and_keymap_edits_reload_without_restart(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let config_dir = config_dir_path();
        test.fs
            .insert_tree(
                &config_dir,
                json!({
                    "settings.json": "{}",
                    "keymap.json": "[]",
                }),
            )
            .await;
        cx.update(|cx| watch_configuration_files(test.fs.clone(), cx));
        test.cx.run_until_parked();

        test.fs
            .save(
                &notes_settings_path(),
                &r#"{ "editor": { "line_numbers": true } }"#.into(),
                Default::default(),
            )
            .await
            .unwrap();
        test.cx.run_until_parked();
        assert!(cx.update(|cx| editor::EditorSettings::get_global(cx).gutter.line_numbers));

        test.fs
            .save(
                &notes_keymap_path(),
                &r#"
                [
                    {
                        "context": "Workspace",
                        "bindings": {
                            "cmd-b": "note::TogglePreview",
                        },
                    },
                ]
                "#
                .into(),
                Default::default(),
            )
            .await
            .unwrap();
        test.cx.run_until_parked();

        let workspace = workspace(&test, cx);
        test.cx.simulate_keystrokes("cmd-b");
        test.cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            2
        );
    }

    #[gpui::test]
    async fn test_cmd_comma_creates_and_opens_settings_as_an_editor(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let settings_path = notes_settings_path();
        assert!(!test.fs.is_file(&settings_path).await);

        test.cx.simulate_keystrokes("cmd-,");
        test.cx.run_until_parked();

        assert_eq!(active_editor_path(&test, cx), settings_path);
        assert_eq!(
            test.fs.load(&notes_settings_path()).await.unwrap(),
            INITIAL_SETTINGS_CONTENT
        );
    }

    #[gpui::test]
    async fn test_autosave_uses_the_configured_trailing_delay(cx: &mut TestAppContext) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: true,
                delay_ms: 2500,
            },
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        test.cx.simulate_keystrokes("i x escape");
        test.cx
            .executor()
            .advance_clock(Duration::from_millis(1000));
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            TEST_NOTE
        );

        test.cx
            .executor()
            .advance_clock(Duration::from_millis(1500));
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
        assert!(!cx.has_pending_prompt());
    }

    #[gpui::test]
    async fn test_disabled_autosave_leaves_edits_unsaved(cx: &mut TestAppContext) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: 1,
            },
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        test.cx.simulate_keystrokes("i x escape");
        test.cx.executor().advance_clock(Duration::from_secs(2));
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            TEST_NOTE
        );
    }

    #[gpui::test]
    async fn test_tab_switch_flushes_pending_autosave(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "beta.md": "# Beta\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        test.cx.simulate_keystrokes("i x escape");
        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new("/notes/alpha.md")).await.unwrap(),
            "x# Alpha\n"
        );
    }

    #[gpui::test]
    async fn test_window_deactivation_flushes_pending_autosave(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.update(|window, _| window.activate_window());
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("i x escape");
        test.cx.deactivate_window();
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_quit_flushes_pending_autosave(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("i x escape");
        test.cx.dispatch_action(Quit);
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_clean_external_edit_reloads_without_prompt(cx: &mut TestAppContext) {
        let test = test_window(cx).await;

        test.fs
            .save(
                Path::new(TEST_NOTE_PATH),
                &"# Changed outside\n".into(),
                Default::default(),
            )
            .await
            .unwrap();
        settle_external_change(&test);

        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            "# Changed outside\n"
        );
        assert!(!cx.has_pending_prompt());
    }

    #[gpui::test]
    async fn test_dirty_external_edit_can_keep_or_reload_changes(cx: &mut TestAppContext) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            },
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        test.cx.simulate_keystrokes("i x escape");
        test.fs
            .save(
                Path::new(TEST_NOTE_PATH),
                &"# Changed outside\n".into(),
                Default::default(),
            )
            .await
            .unwrap();
        settle_external_change(&test);

        assert_eq!(
            cx.pending_prompt()
                .expect("conflict prompt should be open")
                .0,
            "This note changed on disk while you have unsaved edits."
        );
        cx.simulate_prompt_answer("Keep My Changes");
        test.cx.run_until_parked();
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}")
        );

        test.fs
            .save(
                Path::new(TEST_NOTE_PATH),
                &"# Changed again\n".into(),
                Default::default(),
            )
            .await
            .unwrap();
        settle_external_change(&test);
        cx.simulate_prompt_answer("Reload from Disk");
        test.cx.run_until_parked();
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            "# Changed again\n"
        );
    }

    #[gpui::test]
    async fn test_dirty_external_edit_compare_opens_disk_copy_beside_buffer(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            },
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        test.cx.simulate_keystrokes("i x escape");
        test.fs
            .save(
                Path::new(TEST_NOTE_PATH),
                &"# Disk copy\n".into(),
                Default::default(),
            )
            .await
            .unwrap();
        settle_external_change(&test);
        cx.simulate_prompt_answer("Compare");
        test.cx.run_until_parked();

        let workspace = workspace(&test, cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            2
        );
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| {
                workspace
                    .active_item_as::<Editor>(cx)
                    .expect("the on-disk copy should be active")
                    .read_with(cx, |editor, cx| editor.text(cx))
            }),
            "# Disk copy\n"
        );
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_external_delete_can_keep_the_buffer_open(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.cx.simulate_keystrokes("i x escape");

        test.fs
            .remove_file(Path::new(TEST_NOTE_PATH), Default::default())
            .await
            .unwrap();
        settle_external_change(&test);
        assert_eq!(
            cx.pending_prompt().expect("delete prompt should be open").0,
            "This note was deleted outside zednotes."
        );
        cx.simulate_prompt_answer("Keep Open");
        test.cx.run_until_parked();
        assert_eq!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_pane()
                .read(cx)
                .items_len()),
            1
        );
        assert!(!test.fs.is_file(Path::new(TEST_NOTE_PATH)).await);
    }

    #[gpui::test]
    async fn test_external_delete_can_save_the_buffer_again(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.cx.simulate_keystrokes("i x escape");

        test.fs
            .remove_file(Path::new(TEST_NOTE_PATH), Default::default())
            .await
            .unwrap();
        settle_external_change(&test);
        cx.simulate_prompt_answer("Save Again");
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_external_delete_can_close_the_orphaned_buffer(cx: &mut TestAppContext) {
        let test = test_window(cx).await;

        test.fs
            .remove_file(Path::new(TEST_NOTE_PATH), Default::default())
            .await
            .unwrap();
        settle_external_change(&test);
        cx.simulate_prompt_answer("Close");
        test.cx.run_until_parked();

        assert_eq!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_pane()
                .read(cx)
                .items_len()),
            0
        );
    }

    #[gpui::test]
    async fn test_cmd_n_creates_nested_markdown_note_and_enters_insert_mode(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-n");
        test.cx.simulate_input("projects/new-idea");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        let note_path = Path::new("/notes/projects/new-idea.md");
        assert_eq!(test.fs.load(note_path).await.unwrap(), "");
        assert_eq!(active_editor_path(&test, cx), note_path);

        test.cx.simulate_keystrokes("x");
        assert_eq!(
            active_editor(&test, cx).read_with(cx, |editor, cx| editor.text(cx)),
            "x"
        );
    }

    #[test]
    fn test_markdown_note_filename_appends_only_when_missing() {
        assert_eq!(markdown_note_filename("note"), "note.md");
        assert_eq!(markdown_note_filename("nested/note.md"), "nested/note.md");
        assert_eq!(
            markdown_note_filename("nested/note.markdown"),
            "nested/note.markdown"
        );
    }

    #[gpui::test]
    async fn test_explorer_note_and_directory_lifecycle_actions(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        focus_project_path(&mut test, "", cx);
        test.cx.simulate_keystrokes("a");
        test.cx.simulate_input("inbox");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new("/notes/inbox.md")).await.unwrap(),
            ""
        );

        focus_project_path(&mut test, "", cx);
        test.cx.simulate_keystrokes("shift-a");
        test.cx.simulate_input("archive");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert!(test.fs.is_dir(Path::new("/notes/archive")).await);

        focus_project_path(&mut test, "archive", cx);
        test.cx.simulate_keystrokes("r");
        test.cx.simulate_input("old-archive");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert!(!test.fs.is_dir(Path::new("/notes/archive")).await);
        assert!(test.fs.is_dir(Path::new("/notes/old-archive")).await);

        focus_project_path(&mut test, "inbox.md", cx);
        test.cx.dispatch_action(Duplicate);
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("escape");
        assert_eq!(
            test.fs
                .load(Path::new("/notes/inbox copy.md"))
                .await
                .unwrap(),
            ""
        );

        focus_project_path(&mut test, "old-archive", cx);
        test.cx.simulate_keystrokes("d");
        cx.simulate_prompt_answer("Trash");
        test.cx.run_until_parked();
        assert!(!test.fs.is_dir(Path::new("/notes/old-archive")).await);
        assert!(
            test.fs
                .trashed_paths()
                .contains(&PathBuf::from("/notes/old-archive"))
        );
    }

    #[gpui::test]
    async fn test_rename_preserves_open_editor_buffer_undo_and_recent_path(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;
        let editor = test.editor.clone();
        test.cx.simulate_keystrokes("i a b c escape");

        focus_project_path(&mut test, "spike.md", cx);
        test.cx.simulate_keystrokes("r");
        test.cx.simulate_input("renamed");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/renamed.md")
        );
        assert_eq!(active_editor(&test, cx), editor);
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("abc{TEST_NOTE}")
        );
        assert!(!test.fs.is_file(Path::new(TEST_NOTE_PATH)).await);
        assert!(test.fs.is_file(Path::new("/notes/renamed.md")).await);

        let workspace = workspace(&test, cx);
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace
                .recent_navigation_history_iter(cx)
                .any(|(_, abs_path)| abs_path == Some(PathBuf::from("/notes/renamed.md")))
        }));

        test.cx.update(|window, cx| {
            editor.focus_handle(cx).focus(window, cx);
        });
        test.cx.simulate_keystrokes("u");
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            TEST_NOTE
        );
    }

    #[gpui::test]
    async fn test_delete_trashes_open_note_and_closes_its_tab(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        focus_project_path(&mut test, "spike.md", cx);
        test.cx.simulate_keystrokes("d");
        cx.simulate_prompt_answer("Trash");
        test.cx.run_until_parked();

        assert!(!test.fs.is_file(Path::new(TEST_NOTE_PATH)).await);
        assert!(
            test.fs
                .trashed_paths()
                .contains(&PathBuf::from(TEST_NOTE_PATH))
        );
        assert_eq!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_pane()
                .read(cx)
                .items_len()),
            0
        );
    }

    #[gpui::test]
    async fn test_note_lifecycle_actions_are_registered_for_the_palette(cx: &mut TestAppContext) {
        let _test = test_window(cx).await;
        for action_name in [
            "note::New",
            "note::NewDirectory",
            "note::Rename",
            "note::Delete",
            "note::Duplicate",
        ] {
            assert!(
                cx.update(|cx| cx.all_action_names().contains(&action_name)),
                "{action_name} should be registered"
            );
        }
    }

    #[gpui::test]
    async fn test_palette_executes_note_new(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        execute_palette_command(&mut test, "note: new", cx);
        test.cx.simulate_input("from-palette");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs
                .load(Path::new("/notes/from-palette.md"))
                .await
                .unwrap(),
            ""
        );
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/from-palette.md")
        );
    }

    #[gpui::test]
    async fn test_vim_write_saves_the_editor_buffer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("i x escape");
        assert!(
            test.cx
                .update(|_, cx| cx.has_global::<GlobalCommandPaletteInterceptor>()),
            "Vim should install its command-line interceptor"
        );
        test.cx.simulate_keystrokes(":");
        let has_command_palette = test.cx.update(|window, cx| {
            Workspace::for_window(window, cx)
                .expect("the Vim command line needs a workspace")
                .read(cx)
                .active_modal::<command_palette::CommandPalette>(cx)
                .is_some()
        });
        assert!(
            has_command_palette,
            "colon should open the Vim command line"
        );
        test.cx.simulate_keystrokes("w");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        let has_command_palette = test.cx.update(|window, cx| {
            Workspace::for_window(window, cx)
                .expect("the Vim command line needs a workspace")
                .read(cx)
                .active_modal::<command_palette::CommandPalette>(cx)
                .is_some()
        });
        assert!(!has_command_palette, "enter should submit the Vim command");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_vim_builtin_edit_quit_force_quit_write_quit_and_exit_commands(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "beta.md": "# Beta\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        execute_vim_command(&mut test, "e", cx);
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        execute_vim_command(&mut test, "q", cx);
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("i x escape");
        execute_vim_command(&mut test, "q!", cx);
        assert_eq!(
            test.fs.load(Path::new("/notes/beta.md")).await.unwrap(),
            "# Beta\n"
        );

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("i y escape");
        execute_vim_command(&mut test, "wq", cx);
        assert_eq!(
            test.fs.load(Path::new("/notes/beta.md")).await.unwrap(),
            "y# Beta\n"
        );

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("i z escape");
        execute_vim_command(&mut test, "x", cx);
        assert_eq!(
            test.fs.load(Path::new("/notes/beta.md")).await.unwrap(),
            "zy# Beta\n"
        );
    }

    #[test]
    fn test_notes_vim_leader_maps_every_requested_action() {
        let bindings = notes_vim_key_bindings("space")
            .into_iter()
            .map(|binding| {
                let keystrokes = binding
                    .keystrokes()
                    .iter()
                    .map(|keystroke| keystroke.unparse())
                    .collect::<Vec<_>>()
                    .join(" ");
                (keystrokes, binding.action().name().to_owned())
            })
            .collect::<Vec<_>>();

        for (keystrokes, action_name) in [
            ("space f f", "file_finder::Toggle"),
            ("space f g", "workspace::Search"),
            ("space f n", "note::New"),
            ("space f r", "note::Open Recent"),
            ("space p", "command_palette::Toggle"),
            ("space e", "view::ToggleExplorer"),
            ("space v", "note::TogglePreview"),
            ("space b n", "pane::ActivateNextItem"),
            ("space b p", "pane::ActivatePreviousItem"),
            ("space b d", "pane::CloseActiveItem"),
            ("space w h", "workspace::ActivatePaneLeft"),
            ("space w j", "workspace::ActivatePaneDown"),
            ("space w k", "workspace::ActivatePaneUp"),
            ("space w l", "workspace::ActivatePaneRight"),
            ("space w v", "pane::SplitVertical"),
            ("space w s", "pane::SplitHorizontal"),
            ("space s f", "buffer_search::Deploy"),
            ("space s p", "workspace::Search"),
        ] {
            assert!(
                bindings
                    .iter()
                    .any(|binding| binding == &(keystrokes.to_owned(), action_name.to_owned())),
                "{keystrokes} should dispatch {action_name}"
            );
        }
    }

    #[gpui::test]
    async fn test_vim_leader_ff_opens_quick_open(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("space f f");
        test.cx.run_until_parked();

        assert!(workspace(&test, cx).read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()
        }));
    }

    #[gpui::test]
    async fn test_vim_leader_fg_searches_all_notes(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("space f g");
        test.cx.run_until_parked();

        active_project_search(&test, cx);
    }

    #[gpui::test]
    async fn test_vim_leader_e_toggles_explorer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let workspace = workspace(&test, cx);
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        test.cx.simulate_keystrokes("space e");

        assert!(!workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
    }

    #[gpui::test]
    async fn test_vim_leader_v_toggles_preview(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("space v");
        test.cx.run_until_parked();

        assert_eq!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );
    }

    #[gpui::test]
    async fn test_vim_leader_is_configurable_and_insert_mode_still_types_space(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;
        let workspace = workspace(&test, cx);

        test.cx.simulate_keystrokes("i space escape");
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!(" {TEST_NOTE}")
        );

        let mut settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        settings.vim.leader = "ctrl-space".to_owned();
        cx.update(|cx| apply_notes_settings(settings, cx));
        test.cx.run_until_parked();

        test.cx.simulate_keystrokes("space e");
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        test.cx.simulate_keystrokes("ctrl-space e");
        assert!(!workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
    }

    #[gpui::test]
    async fn test_vim_leader_splits_and_moves_focus_between_note_panes(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "beta.md": "# Beta\n",
            }),
            "/notes/alpha.md",
        )
        .await;
        let workspace = workspace(&test, cx);
        test.cx.simulate_keystrokes("space e");
        let original_pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());

        test.cx.simulate_keystrokes("space w v");
        test.cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            2
        );
        test.cx.simulate_resize(size(px(1025.), px(700.)));
        test.cx.run_until_parked();
        let split_pane = workspace.read_with(cx, |workspace, _| workspace.active_pane().clone());
        assert_ne!(original_pane.entity_id(), split_pane.entity_id());
        let focused_pane_id = |test: &mut TestWindow| {
            test.cx.update(|window, cx| {
                [&original_pane, &split_pane]
                    .into_iter()
                    .find(|pane| pane.focus_handle(cx).contains_focused(window, cx))
                    .map(|pane| pane.entity_id())
                    .expect("one notes pane should contain focus")
            })
        };
        let focused_before_navigation = focused_pane_id(&mut test);

        test.cx.simulate_keystrokes("space w h");
        let focused_after_left = focused_pane_id(&mut test);
        if focused_after_left == focused_before_navigation {
            test.cx.simulate_keystrokes("space w l");
            assert_ne!(focused_pane_id(&mut test), focused_before_navigation);
        } else {
            assert_ne!(focused_after_left, focused_before_navigation);
        }

        test.cx.simulate_keystrokes("space f f");
        test.cx.simulate_input("beta");
        test.cx.executor().advance_clock(Duration::from_millis(100));
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        let pane_paths = workspace.read_with(cx, |workspace, cx| {
            workspace
                .panes()
                .iter()
                .filter_map(|pane| pane.read(cx).active_item())
                .filter_map(|item| item.project_path(cx))
                .filter_map(|project_path| {
                    workspace
                        .project()
                        .read(cx)
                        .absolute_path(&project_path, cx)
                })
                .collect::<Vec<_>>()
        });
        assert!(pane_paths.contains(&PathBuf::from("/notes/alpha.md")));
        assert!(pane_paths.contains(&PathBuf::from("/notes/beta.md")));

        test.cx.simulate_keystrokes("space w s");
        test.cx.run_until_parked();
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            3
        );
    }

    #[gpui::test]
    async fn test_vim_app_command_aliases_dispatch_notes_actions(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let workspace = workspace(&test, cx);

        execute_vim_command(&mut test, "preview", cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );

        execute_vim_command(&mut test, "explorer", cx);
        assert!(!workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        execute_vim_command(&mut test, "notes", cx);
        assert!(workspace.read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()
        }));
        test.cx.simulate_keystrokes("escape");
        test.cx.run_until_parked();

        execute_vim_command(&mut test, "search", cx);
        active_project_search(&test, cx);
    }

    #[test]
    fn test_parses_explorer_settings_from_jsonc() {
        let settings = parse_notes_settings(
            r#"
            {
                // Non-Markdown files are useful in mixed vaults.
                "explorer": {
                    "markdown_only": false,
                    "show_hidden": true,
                },
                "files": {
                    "exclude": ["drafts", "**/*.tmp"],
                },
            }
            "#,
        )
        .expect("settings should parse")
        .explorer;

        assert_eq!(
            settings,
            ExplorerSettings {
                width: DEFAULT_EXPLORER_WIDTH,
                markdown_only: false,
                show_hidden: true,
                excluded_paths: vec!["drafts".to_owned(), "**/*.tmp".to_owned()],
            }
        );
    }

    #[test]
    fn test_parses_preview_settings_from_jsonc() {
        let settings = parse_notes_settings(
            r#"
            {
                "preview": {
                    "max_width": 680,
                    "allow_remote_images": true,
                    "max_file_size_bytes": 2048,
                },
            }
            "#,
        )
        .expect("settings should parse")
        .preview;

        assert_eq!(
            settings,
            PreviewSettings {
                max_width: 680,
                allow_remote_images: true,
                max_file_size_bytes: Some(2048),
            }
        );
    }

    #[gpui::test]
    async fn test_preview_defaults_are_readable_offline_and_guard_large_files(
        cx: &mut TestAppContext,
    ) {
        let _test = test_window(cx).await;

        let settings = cx.update(|cx| {
            *markdown_preview::markdown_preview_settings::MarkdownPreviewSettings::get_global(cx)
        });
        assert_eq!(settings.max_width, Some(px(760.)));
        assert!(!settings.allow_remote_images);
        assert_eq!(
            settings.max_file_size_bytes,
            Some(DEFAULT_PREVIEW_MAX_FILE_SIZE_BYTES)
        );
    }

    #[gpui::test]
    async fn test_cmd_shift_v_toggles_editor_preview_split(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-shift-v");
        test.cx.run_until_parked();

        let workspace = test
            .window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .expect("failed to read the notes window");
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            2
        );
        workspace.read_with(cx, |workspace, cx| {
            for pane in workspace.panes() {
                let toolbar = pane.read(cx).toolbar().read(cx);
                assert!(toolbar.item_of_type::<BufferSearchBar>().is_some());
                assert!(toolbar.item_of_type::<ProjectSearchBar>().is_some());
            }
        });
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );
        test.cx.update(|window, cx| {
            assert!(test.editor.read(cx).focus_handle(cx).is_focused(window));
        });

        test.cx.simulate_keystrokes("cmd-shift-v");
        test.cx.run_until_parked();

        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            1
        );
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            0
        );
    }

    #[gpui::test]
    async fn test_open_preview_opens_preview_only_in_the_editor_pane(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.dispatch_action(OpenPreview);
        test.cx.run_until_parked();

        test.window
            .read_with(cx, |multi_workspace, cx| {
                let workspace = multi_workspace.workspace().read(cx);
                assert_eq!(workspace.panes().len(), 1);
                assert!(
                    workspace
                        .active_item_as::<MarkdownPreviewView>(cx)
                        .is_some()
                );
            })
            .expect("failed to read the notes window");
    }

    #[gpui::test]
    async fn test_cmd_o_prompts_for_one_directory(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.fs
            .insert_tree("/other-notes", json!({ "other.md": "# Other\n" }))
            .await;

        test.cx.simulate_keystrokes("cmd-o");
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(!options.files);
            assert!(options.directories);
            assert!(!options.multiple);
            Some(vec![PathBuf::from("/other-notes")])
        });
        test.cx.run_until_parked();

        let root_path = test
            .window
            .read_with(cx, |multi_workspace, cx| {
                multi_workspace
                    .workspace()
                    .read(cx)
                    .project()
                    .read(cx)
                    .visible_worktrees(cx)
                    .next()
                    .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            })
            .expect("failed to read the notes window")
            .expect("the selected folder should become a visible worktree");
        assert_eq!(root_path, PathBuf::from("/other-notes"));
    }

    #[gpui::test]
    async fn test_explorer_filters_non_markdown_files_by_default(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "beta.txt": "not a note\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        focus_project_path(&mut test, "alpha.md", cx);
        test.cx.simulate_keystrokes("j enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );
    }

    #[gpui::test]
    async fn test_markdown_only_false_shows_other_files(cx: &mut TestAppContext) {
        let mut explorer_settings = ExplorerSettings::default();
        explorer_settings.markdown_only = false;
        let mut test = test_window_with(
            cx,
            explorer_settings,
            json!({
                "alpha.md": "# Alpha\n",
                "beta.txt": "not a note\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        focus_project_path(&mut test, "alpha.md", cx);
        test.cx.simulate_keystrokes("j enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/beta.txt")
        );
    }

    #[gpui::test]
    async fn test_toggle_hidden_files_updates_the_explorer(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                ".hidden.md": "# Hidden\n",
                "alpha.md": "# Alpha\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        focus_project_path(&mut test, "", cx);
        test.cx.simulate_keystrokes("j enter");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );

        test.cx.dispatch_action(ToggleHiddenFiles);
        test.cx.run_until_parked();
        focus_project_path(&mut test, "", cx);
        test.cx.simulate_keystrokes("j enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/.hidden.md")
        );
    }

    #[gpui::test]
    async fn test_explorer_keeps_directories_and_excludes_configured_paths(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "nested": {
                    "beta.markdown": "# Beta\n",
                },
                "node_modules": {
                    "ignored.md": "# Ignored\n",
                },
                "target": {
                    "ignored.md": "# Ignored\n",
                },
            }),
            "/notes/alpha.md",
        )
        .await;

        focus_project_path(&mut test, "", cx);
        test.cx.simulate_keystrokes("j l");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("j enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/nested/beta.markdown")
        );
    }

    #[gpui::test]
    async fn test_explorer_vim_bindings_open_tabs_and_return_focus_to_editor(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "beta.md": "# Beta\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("o");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/beta.md")
        );
        assert_eq!(
            test.window
                .read_with(cx, |multi_workspace, cx| multi_workspace
                    .workspace()
                    .read(cx)
                    .active_pane()
                    .read(cx)
                    .items_len())
                .expect("failed to read the notes window"),
            2
        );

        test.cx.simulate_keystrokes("cmd-{");
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );
        test.cx.simulate_keystrokes("cmd-}");
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/beta.md")
        );

        focus_project_path(&mut test, "beta.md", cx);
        test.cx.simulate_keystrokes("q");
        test.cx.update(|window, cx| {
            let editor = Workspace::for_window(window, cx)
                .expect("test window should contain a workspace")
                .read(cx)
                .active_item_as::<Editor>(cx)
                .expect("an editor should be active");
            assert!(
                editor
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            );
        });

        test.cx.simulate_keystrokes("cmd-w");
        test.cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );
    }

    #[gpui::test]
    async fn test_cmd_b_toggles_the_left_explorer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let workspace = test
            .window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .expect("failed to read the notes window");
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        test.cx.simulate_keystrokes("cmd-b");

        assert!(!workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
    }

    #[gpui::test]
    async fn test_cmd_p_opens_markdown_notes_and_restores_editor_focus(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "nested": {
                    "project-plan.markdown": "# Project plan\n",
                },
            }),
            "/notes/alpha.md",
        )
        .await;
        let workspace = workspace(&test, cx);

        test.cx.simulate_keystrokes("cmd-p");
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()),
            "Cmd+P should open quick open"
        );
        test.cx.simulate_input("prjpln");
        test.cx.executor().advance_clock(Duration::from_millis(100));
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/nested/project-plan.markdown")
        );
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_none()),
            "opening a note should dismiss quick open"
        );

        test.cx.simulate_keystrokes("cmd-p");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("escape");
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_none()),
            "Escape should dismiss quick open"
        );
        test.cx.update(|window, cx| {
            let editor = workspace
                .read(cx)
                .active_item_as::<Editor>(cx)
                .expect("the opened note should be active");
            assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        });
    }

    #[gpui::test]
    async fn test_cmd_click_opens_relative_markdown_link(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "target.md": "# Target\n",
                "nested": {
                    "source.md": "[Target](../target.md)\n",
                },
            }),
            "/notes/nested/source.md",
        )
        .await;
        let editor = active_editor(&test, cx);
        test.cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections
                        .select_display_ranges([DisplayPoint::new(DisplayRow(0), 3)
                            ..DisplayPoint::new(DisplayRow(0), 3)]);
                });
            });
        });
        test.cx.run_until_parked();

        let click_position = workspace(&test, cx)
            .read_with(cx, |workspace, cx| {
                workspace
                    .active_pane()
                    .read(cx)
                    .pixel_position_of_cursor(cx)
            })
            .expect("the link cursor should have a screen position");
        test.cx
            .simulate_mouse_move(click_position, None, Modifiers::secondary_key());
        test.cx
            .simulate_click(click_position, Modifiers::secondary_key());
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/target.md")
        );
    }

    #[gpui::test]
    async fn test_enter_opens_relative_markdown_link_in_vim_normal_mode(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "source.md": "Read [Target](nested/target.md)\n",
                "nested": {
                    "target.md": "# Target\n",
                },
            }),
            "/notes/source.md",
        )
        .await;
        let editor = active_editor(&test, cx);
        test.cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections
                        .select_display_ranges([DisplayPoint::new(DisplayRow(0), 8)
                            ..DisplayPoint::new(DisplayRow(0), 8)]);
                });
            });
        });

        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/nested/target.md")
        );
    }

    #[gpui::test]
    async fn test_enter_keeps_normal_vim_motion_when_cursor_is_not_on_link(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({ "source.md": "first\n  second\n" }),
            "/notes/source.md",
        )
        .await;

        test.cx.simulate_keystrokes("enter");

        let editor = active_editor(&test, cx);
        let selections = test.cx.update(|_, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .selections
                    .display_ranges(&editor.display_snapshot(cx))
            })
        });
        assert_eq!(
            selections,
            vec![DisplayPoint::new(DisplayRow(1), 2)..DisplayPoint::new(DisplayRow(1), 2)]
        );
    }

    #[gpui::test]
    async fn test_missing_markdown_link_shows_notification_without_navigating(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({ "source.md": "[Missing](missing.md)\n" }),
            "/notes/source.md",
        )
        .await;
        let editor = active_editor(&test, cx);
        test.cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections
                        .select_display_ranges([DisplayPoint::new(DisplayRow(0), 3)
                            ..DisplayPoint::new(DisplayRow(0), 3)]);
                });
            });
        });

        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/source.md")
        );
        assert!(workspace(&test, cx).read_with(cx, |workspace, _| {
            workspace
                .notification_ids()
                .contains(&NotificationId::Named("notes-missing-link".into()))
        }));
    }

    #[gpui::test]
    async fn test_cmd_click_missing_markdown_link_shows_notification(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({ "source.md": "[Missing](missing.md)\n" }),
            "/notes/source.md",
        )
        .await;
        let editor = active_editor(&test, cx);
        test.cx.update(|window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections
                        .select_display_ranges([DisplayPoint::new(DisplayRow(0), 3)
                            ..DisplayPoint::new(DisplayRow(0), 3)]);
                });
            });
        });
        test.cx.run_until_parked();

        let click_position = workspace(&test, cx)
            .read_with(cx, |workspace, cx| {
                workspace
                    .active_pane()
                    .read(cx)
                    .pixel_position_of_cursor(cx)
            })
            .expect("the missing link cursor should have a screen position");
        test.cx
            .simulate_mouse_move(click_position, None, Modifiers::secondary_key());
        test.cx
            .simulate_click(click_position, Modifiers::secondary_key());
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/source.md")
        );
        assert!(workspace(&test, cx).read_with(cx, |workspace, _| {
            workspace
                .notification_ids()
                .contains(&NotificationId::Named("missing-file-link".into()))
        }));
    }

    #[gpui::test]
    async fn test_link_history_restores_selection_and_scroll_across_files(cx: &mut TestAppContext) {
        let mut source = (0..50)
            .map(|line| format!("source line {line}\n"))
            .collect::<String>();
        source.push_str("[Target](target.md)\n");
        source.extend((51..90).map(|line| format!("source line {line}\n")));
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "source.md": source,
                "target.md": "# Target\n",
            }),
            "/notes/source.md",
        )
        .await;
        let source_editor = active_editor(&test, cx);
        let expected_scroll = test.cx.update(|window, cx| {
            source_editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections.select_display_ranges([DisplayPoint::new(DisplayRow(50), 10)
                        ..DisplayPoint::new(DisplayRow(50), 15)]);
                });
                editor.set_scroll_position(point(0., 25.5), window, cx);
                editor.scroll_position(cx)
            })
        });

        test.cx.run_until_parked();
        let click_position = workspace(&test, cx)
            .read_with(cx, |workspace, cx| {
                workspace
                    .active_pane()
                    .read(cx)
                    .pixel_position_of_cursor(cx)
            })
            .expect("the link cursor should have a screen position");
        test.cx
            .simulate_mouse_move(click_position, None, Modifiers::secondary_key());
        test.cx
            .simulate_click(click_position, Modifiers::secondary_key());
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/target.md")
        );

        test.cx.simulate_keystrokes("ctrl--");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/source.md")
        );
        let restored_editor = active_editor(&test, cx);
        let (restored_selections, restored_scroll) = test.cx.update(|_, cx| {
            restored_editor.update(cx, |editor, cx| {
                (
                    editor
                        .selections
                        .display_ranges(&editor.display_snapshot(cx)),
                    editor.scroll_position(cx),
                )
            })
        });
        assert_eq!(
            restored_selections,
            vec![DisplayPoint::new(DisplayRow(50), 10)..DisplayPoint::new(DisplayRow(50), 15)]
        );
        assert_eq!(restored_scroll, expected_scroll);

        test.cx.simulate_keystrokes("ctrl-_");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/target.md")
        );
    }

    #[gpui::test]
    async fn test_quick_open_excludes_non_markdown_files(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "private.txt": "not a note\n",
            }),
            "/notes/alpha.md",
        )
        .await;
        let workspace = workspace(&test, cx);

        test.cx.simulate_keystrokes("cmd-p");
        test.cx.simulate_input("private.txt");
        test.cx.executor().advance_clock(Duration::from_millis(100));
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );
        assert!(
            workspace.read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()),
            "a non-Markdown query should have no selectable result"
        );
        test.cx.simulate_keystrokes("escape");
    }

    #[gpui::test]
    async fn test_external_create_and_delete_refresh_explorer_and_quick_open(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "removed.md": "# Removed\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        test.fs
            .insert_file("/notes/external.md", b"# External\n".to_vec())
            .await;
        test.cx.run_until_parked();
        focus_project_path(&mut test, "external.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/external.md")
        );

        focus_project_path(&mut test, "alpha.md", cx);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        let worktree = test.project.read_with(cx, |project, cx| {
            project.visible_worktrees(cx).next().unwrap()
        });
        test.fs
            .remove_file(Path::new("/notes/removed.md"), Default::default())
            .await
            .unwrap();
        worktree.next_event(cx).await;
        test.cx.executor().advance_clock(Duration::from_millis(100));
        test.cx.run_until_parked();
        let removed_path = project_path(&test.project, "removed.md", cx);
        assert!(test.project.read_with(cx, |project, cx| {
            project.entry_for_path(&removed_path, cx).is_none()
        }));

        test.cx.simulate_keystrokes("cmd-p");
        test.cx.simulate_input("removed.md");
        test.cx.executor().advance_clock(Duration::from_millis(100));
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );
        assert!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<file_finder::FileFinder>(cx)
                .is_some()),
            "the deleted note must not remain selectable in quick open"
        );
        test.cx.simulate_keystrokes("escape");
    }

    #[gpui::test]
    async fn test_command_palette_executes_all_major_notes_actions(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let workspace = workspace(&test, cx);

        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
        execute_palette_command(&mut test, "view: toggle explorer", cx);
        assert!(!workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));
        test.cx.simulate_keystrokes("cmd-b");
        assert!(workspace.read_with(cx, |workspace, cx| workspace.left_dock().read(cx).is_open()));

        execute_palette_command(&mut test, "note: toggle preview", cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );
        test.cx.simulate_keystrokes("cmd-shift-v");
        test.cx.run_until_parked();

        test.cx.simulate_keystrokes("i x escape");
        execute_palette_command(&mut test, "workspace: save", cx);
        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );

        execute_palette_command(&mut test, "workspace: open folder", cx);
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|options| {
            assert!(!options.files);
            assert!(options.directories);
            assert!(!options.multiple);
            None
        });
        test.cx.run_until_parked();

        execute_palette_command(&mut test, "workspace: search", cx);
        active_project_search(&test, cx);
    }

    #[gpui::test]
    async fn test_workspace_search_defaults_to_markdown_files(cx: &mut TestAppContext) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n\nneedle\n",
                "ignored.txt": "needle\n",
                "nested": {
                    "beta.markdown": "# Beta\n\nneedle\n",
                },
            }),
            "/notes/alpha.md",
        )
        .await;

        test.cx.simulate_keystrokes("cmd-shift-f");
        let search = active_project_search(&test, cx);
        test.cx.simulate_input("needle");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            2
        );
    }

    #[gpui::test]
    async fn test_workspace_search_finds_saved_text_and_enter_opens_exact_match(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "alpha.md": "# Alpha\n",
                "ignored.txt": "fresh result\n",
            }),
            "/notes/alpha.md",
        )
        .await;

        test.cx
            .simulate_keystrokes("i f r e s h space r e s u l t enter escape cmd-s");
        test.cx.run_until_parked();
        test.cx.simulate_keystrokes("cmd-shift-f");
        let search = active_project_search(&test, cx);
        test.cx.simulate_input("fresh result");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            1
        );

        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            active_editor_path(&test, cx),
            PathBuf::from("/notes/alpha.md")
        );

        test.cx.simulate_keystrokes("i x escape cmd-s");
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new("/notes/alpha.md")).await.unwrap(),
            "xfresh result\n# Alpha\n"
        );
    }

    #[gpui::test]
    async fn test_workspace_search_toggles_and_directory_filter_narrow_results(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with(
            cx,
            ExplorerSettings::default(),
            json!({
                "root.md": "Needle\nneedle\nneedleish\n",
                "nested": {
                    "note.md": "needle\n",
                },
            }),
            "/notes/root.md",
        )
        .await;

        test.cx.simulate_keystrokes("cmd-shift-f");
        let search = active_project_search(&test, cx);
        test.cx.simulate_input("needle");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            4
        );

        test.cx.dispatch_action(search::ToggleCaseSensitive);
        test.cx.dispatch_action(Search);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            3
        );

        test.cx.dispatch_action(search::ToggleWholeWord);
        test.cx.dispatch_action(Search);
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            2
        );

        test.cx.dispatch_action(Search);
        test.cx.simulate_keystrokes("tab cmd-a");
        test.cx.simulate_input("nested/**");
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();
        assert_eq!(
            search
                .read_with(cx, |search, cx| search.get_matches(cx))
                .len(),
            1
        );
    }

    #[gpui::test]
    async fn test_cmd_f_find_replace_and_vim_slash_search_remain_independent(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-f");
        test.cx.simulate_input("fast");
        test.cx.run_until_parked();
        let search_bar = buffer_search_bar(&test, cx);
        assert!(!search_bar.read_with(cx, |search_bar, _| search_bar.is_dismissed()));
        assert_eq!(
            search_bar.read_with(cx, |search_bar, cx| search_bar.query(cx)),
            "fast"
        );
        assert!(test.cx.update(|window, cx| {
            search_bar.update(cx, |search_bar, cx| search_bar.match_exists(window, cx))
        }));

        test.cx.dispatch_action(search::ToggleCaseSensitive);
        test.cx.dispatch_action(search::ToggleWholeWord);
        test.cx.dispatch_action(search::ToggleRegex);
        search_bar.update(cx, |search_bar, _cx| {
            assert!(search_bar.has_search_option(search::SearchOptions::CASE_SENSITIVE));
            assert!(search_bar.has_search_option(search::SearchOptions::WHOLE_WORD));
            assert!(search_bar.has_search_option(search::SearchOptions::REGEX));
        });

        test.cx.dispatch_action(search::ToggleReplace);
        test.cx.simulate_input("quick");
        test.cx.dispatch_action(search::ReplaceAll);
        test.cx.run_until_parked();
        assert_eq!(
            active_editor(&test, cx).read_with(cx, |editor, cx| editor.text(cx)),
            "# Spike\n\n- **quick** editing\n"
        );

        test.cx.simulate_keystrokes("escape escape /");
        test.cx.simulate_input("Spike");
        test.cx.simulate_keystrokes("enter i x escape cmd-s");
        test.cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            "# xSpike\n\n- **quick** editing\n"
        );
    }

    #[gpui::test]
    async fn test_cmd_w_closes_the_active_tab_without_closing_the_window(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-w");
        test.cx.run_until_parked();

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(
            test.window
                .read_with(cx, |multi_workspace, cx| multi_workspace
                    .workspace()
                    .read(cx)
                    .active_pane()
                    .read(cx)
                    .items_len())
                .expect("failed to read the notes window"),
            0
        );
    }
}
