//! zednotes' application layer.
//!
//! Everything notes-specific lives in this crate rather than in the generic Zed
//! crates of the fork, so that merges from `upstream` stay cheap.

use std::{
    cell::Cell,
    cmp::Reverse,
    io::ErrorKind,
    ops::Range,
    path::{Component, Path, PathBuf},
    rc::Rc,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use assets::Assets;
use chrono::{Local, NaiveDate, SecondsFormat, format::Item, format::StrftimeItems};
use client::{Client, UserStore};
use db::kvp::KeyValueStore;
use editor::{Editor, EditorEvent, SelectionEffects, scroll::Autoscroll};
use fs::{Fs, PathEventKind, RealFs};
use futures::{FutureExt as _, StreamExt as _};
use git::GitHostingProviderRegistry;
use git_ui::git_panel::GitPanel;
use git_ui_core::worktree_picker::WorktreePicker;
#[cfg(test)]
use gpui::WindowHandle;
use gpui::{
    Action as _, AnyElement, App, AppContext as _, AsyncApp, BorrowAppContext as _, Bounds, Entity,
    Focusable as _, Global, KeyBinding, Keystroke, Menu, MenuItem, OsAction, PathPromptOptions,
    Render, SharedString, Subscription, SystemMenuType, Task, TaskExt as _, TitlebarOptions,
    WeakEntity, Window, WindowBounds, WindowOptions, actions, div, px, size,
};
use http_client::BlockedHttpClient;
use image_viewer::ImageViewToolbarControls;
use language::{
    Buffer, BufferEvent, Capability, DiskState, LanguageConfig, LanguageMatcher, LanguageQueries,
    LanguageRegistry, LoadedLanguage,
};
use markdown_preview::markdown_preview_view::MarkdownPreviewView;
use node_runtime::NodeRuntime;
use outline_panel::OutlinePanel;
use percent_encoding::percent_decode_str;
use platform_title_bar::PlatformTitleBar;
use project::{
    Project,
    git_store::{GitStoreEvent, Repository},
    linked_worktree_short_name,
};
use project_panel::{
    EntryFilter, Event as ProjectPanelEvent, ProjectPanel, ProjectPanelOptions,
    project_panel_settings::ProjectPanelSettings,
};
use search::{BufferSearchBar, ProjectSearchView, project_search::ProjectSearchBar};
use semver::Version;
use serde::{Deserialize, Serialize};
use session::{AppSession, Session};
use settings::{
    AutosaveSetting, BufferLineHeight, CodeLens, CommandAliasTarget, DockSide, FontFamilyName,
    KeybindSource, KeymapFile, KeymapFileLoadResult, RelativeLineNumbers, ScrollbarDiagnostics,
    Settings as _, SettingsStore, ShowDiagnostics, ShowMinimap, SoftWrap, ThemeAppearanceMode,
    ThemeName, ThemeSelection, WordsCompletionMode,
};
use theme::{ActiveTheme as _, GlobalTheme, LoadThemes};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, IconSize, Label, LabelSize, PopoverMenu, TintColor,
    Tooltip, prelude::*,
};
use util::ResultExt as _;
use vim::ModeIndicator;
use workspace as zed_workspace;
#[cfg(test)]
use zed_workspace::dock::PanelButtons;
use zed_workspace::{
    AppState, CloseIntent, Event as WorkspaceEvent, ItemHandle as _, MultiWorkspace, OpenOptions,
    OpenVisible, Pane, SaveIntent, SessionWorkspace, SplitDirection, StatusBarSettings,
    StatusItemView, TabBarSettings, Toast, Workspace, WorkspaceSettings, WorkspaceStore,
    dock::DockPosition,
    item::SaveOptions,
    notifications::{DetachAndPromptErr as _, NotificationId},
    pane::Event as PaneEvent,
};

const APP_NAME: &str = "zednotes";
const MAX_PROJECT_NAME_LENGTH: usize = 40;
const MAX_BRANCH_NAME_LENGTH: usize = 40;
const MAX_SHORT_SHA_LENGTH: usize = 8;
const DEFAULT_MARKDOWN_SEARCH_FILTER: &str = "**/*.md, **/*.markdown";
const DEFAULT_EXCLUDED_PATHS: &[&str] = &[".git", ".DS_Store", "node_modules", "target"];
const DEFAULT_PREVIEW_MAX_WIDTH: u32 = 760;
const DEFAULT_PREVIEW_MAX_FILE_SIZE_BYTES: u64 = 5 * 1024 * 1024;
const DEFAULT_AUTOSAVE_DELAY_MS: u64 = 750;
const DEFAULT_EXPLORER_WIDTH: u32 = 240;
const DEFAULT_EDITOR_FONT_FAMILY: &str = ".ZedMono";
const DEFAULT_EDITOR_FONT_SIZE: f32 = 15.0;
const DEFAULT_EDITOR_LINE_HEIGHT: f32 = 1.55;
const DEFAULT_UI_FONT_FAMILY: &str = ".SystemUIFont";
const DEFAULT_UI_FONT_SIZE: f32 = 14.0;
const DEFAULT_PREVIEW_FONT_FAMILY: &str = ".SystemUIFont";
const DEFAULT_PREVIEW_FONT_SIZE: f32 = 16.0;
const DEFAULT_VIM_LEADER: &str = "space";
const DEFAULT_WHICH_KEY_DELAY_MS: u64 = 500;
const DEFAULT_DAILY_NOTES_DIRECTORY: &str = "journal";
const DEFAULT_DAILY_NOTES_FILENAME_FORMAT: &str = "%Y-%m-%d.md";
const DEFAULT_DAILY_NOTE_TEMPLATE: &str =
    "---\ntitle: \"{{title}}\"\ndate: {{datetime}}\nlastUpdated: {{datetime}}\n---\n\n";
const RECENT_NOTES_KEY: &str = "notes_recent_usage";
const MAX_RECENT_NOTES: usize = 512;
const INITIAL_SETTINGS_CONTENT: &str = r#"{
  // Standard Zed settings. These are also editable through Cmd+,.
  "theme": {
    "mode": "system",
    "light": "One Light",
    "dark": "One Dark",
  },
  "vim_mode": true,
  "relative_line_numbers": "disabled",

  "vim": {
    "toggle_relative_line_numbers": false,
  },

  "which_key": {
    "enabled": true,
    "delay_ms": 500,
  },

  "ui_font_family": ".SystemUIFont",
  "ui_font_size": 14,
  "buffer_font_family": ".ZedMono",
  "buffer_font_size": 15,
  "buffer_line_height": {
    "custom": 1.55,
  },
  "gutter": {
    "line_numbers": false,
  },
  "soft_wrap": "editor_width",

  "autosave": {
    "after_delay": {
      "milliseconds": 750,
    },
  },

  "project_panel": {
    "default_width": 240,
    "hide_hidden": true,
  },
  "file_scan_exclusions": [
    "**/.git",
    "**/.DS_Store",
    "**/node_modules",
    "**/target",
  ],

  "markdown_preview_font_family": ".SystemUIFont",
  "markdown_preview_font_size": 16,
  "markdown_preview": {
    "limit_content_width": true,
    "max_width": 760,
    "allow_remote_images": false,
    "max_file_size_bytes": 5242880,
  },

  // Zednotes-only policy. Upstream Zed settings stay at the root above.
  "zednotes": {
    "vim": {
      "leader": "space",
    },
    "explorer": {
      "markdown_only": true,
    },
    "daily_notes": {
      "directory": "journal",
      "filename_format": "%Y-%m-%d.md",
      // Optional path relative to the notes folder. The template may use
      // {{date}}, {{datetime}}, and {{title}} placeholders.
      // "template": "templates/daily.md",
    },
  },
}
"#;

actions!(
    notes,
    [
        /// Opens the user-owned zednotes settings file.
        OpenSettingsFile,
        /// Quits zednotes.
        Quit,
        /// Hides zednotes.
        Hide,
        /// Hides all applications except zednotes.
        HideOthers,
        /// Shows all applications.
        ShowAll,
        /// Minimizes the active notes window.
        Minimize,
        /// Zooms the active notes window.
        Zoom,
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
        /// Opens today's note, creating it from the daily template when absent.
        #[action(name = "Open Today")]
        OpenToday,
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
        /// Reports word and character counts for the active note.
        Statistics,
        /// Saves the active note and returns its editor to Vim Normal mode.
        SaveAndReturnToNormal,
    ]
);

actions!(
    view,
    [
        /// Toggles the notes explorer.
        ToggleExplorer,
        /// Hides surrounding UI and centers the active editor.
        ToggleFocusMode,
        /// Follows the macOS appearance.
        UseSystemTheme,
        /// Uses the light notes theme.
        UseLightTheme,
        /// Uses the dark notes theme.
        UseDarkTheme,
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
struct NotesUiSettings {
    font_family: String,
    font_size: f32,
}

impl Default for NotesUiSettings {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_UI_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_UI_FONT_SIZE,
        }
    }
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

#[derive(Clone, Debug, PartialEq)]
struct PreviewSettings {
    font_family: String,
    font_size: f32,
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
    toggle_relative_line_numbers: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NotesWhichKeySettings {
    enabled: bool,
    delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DailyNotesSettings {
    directory: String,
    filename_format: String,
    template: Option<String>,
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
            toggle_relative_line_numbers: false,
        }
    }
}

impl Default for NotesWhichKeySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: DEFAULT_WHICH_KEY_DELAY_MS,
        }
    }
}

impl Default for DailyNotesSettings {
    fn default() -> Self {
        Self {
            directory: DEFAULT_DAILY_NOTES_DIRECTORY.to_owned(),
            filename_format: DEFAULT_DAILY_NOTES_FILENAME_FORMAT.to_owned(),
            template: None,
        }
    }
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_PREVIEW_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_PREVIEW_FONT_SIZE,
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
    relative_line_numbers: RelativeLineNumbers,
    vim: NotesVimSettings,
    which_key: NotesWhichKeySettings,
    ui: NotesUiSettings,
    editor: NotesEditorSettings,
    autosave: AutosaveSettings,
    explorer: ExplorerSettings,
    preview: PreviewSettings,
    daily_notes: DailyNotesSettings,
}

impl Default for NotesSettings {
    fn default() -> Self {
        Self {
            theme: NotesTheme::System,
            vim_mode: true,
            relative_line_numbers: RelativeLineNumbers::Disabled,
            vim: NotesVimSettings::default(),
            which_key: NotesWhichKeySettings::default(),
            ui: NotesUiSettings::default(),
            editor: NotesEditorSettings::default(),
            autosave: AutosaveSettings::default(),
            explorer: ExplorerSettings::default(),
            preview: PreviewSettings::default(),
            daily_notes: DailyNotesSettings::default(),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NoteStatistics {
    words: usize,
    characters: usize,
}

fn note_statistics(text: &str) -> NoteStatistics {
    NoteStatistics {
        words: text
            .split_whitespace()
            .filter(|token| token.chars().any(char::is_alphanumeric))
            .count(),
        characters: text.chars().count(),
    }
}

fn note_statistics_message(text: &str) -> String {
    let statistics = note_statistics(text);
    format!(
        "{} words · {} characters",
        statistics.words, statistics.characters
    )
}

/// The subset of Zed's title bar that is useful in the local notes app.
///
/// Keeping this app-scoped avoids pulling collaboration, account, and updater
/// controls into zednotes while preserving Zed's project and Git workflows.
struct NotesTitleBar {
    platform_title_bar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    _subscriptions: Vec<Subscription>,
}

impl NotesTitleBar {
    fn new(
        workspace: &Workspace,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let mut subscriptions = vec![cx.observe(&project, |_, _, cx| cx.notify())];
        subscriptions.push(cx.subscribe(&git_store, |_, _, event, cx| {
            if matches!(
                event,
                GitStoreEvent::ActiveRepositoryChanged(_)
                    | GitStoreEvent::RepositoryUpdated(_, _, true)
            ) {
                cx.notify();
            }
        }));
        if let Some(workspace_entity) = workspace.weak_handle().upgrade() {
            subscriptions.push(cx.observe(&workspace_entity, |_, _, cx| cx.notify()));
        }

        // zednotes' dock panels start below the title bar. Do not pass the
        // multi-workspace through here: PlatformTitleBar would otherwise treat
        // an open left dock as occupying the traffic-light area and remove the
        // inset from the project switcher.
        let platform_title_bar = cx.new(|cx| PlatformTitleBar::new("notes-title-bar", cx));

        Self {
            platform_title_bar,
            project,
            workspace: workspace.weak_handle(),
            multi_workspace,
            _subscriptions: subscriptions,
        }
    }

    fn effective_active_worktree(&self, cx: &App) -> Option<Entity<project::Worktree>> {
        let project = self.project.read(cx);
        if let Some(repository) = project.active_repository(cx) {
            let repository_path = &repository.read(cx).work_directory_abs_path;
            if let Some(worktree) = project.visible_worktrees(cx).find(|worktree| {
                let worktree_path = worktree.read(cx).abs_path();
                worktree_path == *repository_path
                    || worktree_path.starts_with(repository_path.as_ref())
            }) {
                return Some(worktree);
            }
        }
        project.visible_worktrees(cx).next()
    }

    fn repository_for_worktree(
        &self,
        worktree: &Entity<project::Worktree>,
        cx: &App,
    ) -> Option<Entity<Repository>> {
        let project = self.project.read(cx);
        let git_store = project.git_store().read(cx);
        let worktree_path = worktree.read(cx).abs_path();
        git_store
            .repositories()
            .values()
            .filter(|repository| {
                let repository_path = &repository.read(cx).work_directory_abs_path;
                worktree_path == *repository_path
                    || worktree_path.starts_with(repository_path.as_ref())
            })
            .max_by_key(|repository| {
                repository
                    .read(cx)
                    .work_directory_abs_path
                    .as_os_str()
                    .len()
            })
            .cloned()
    }

    fn worktree_count(&self, cx: &App) -> usize {
        self.project.read(cx).visible_worktrees(cx).count()
    }

    fn render_project_name(
        &self,
        project_name: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let workspace = self.workspace.clone();
        let focus_handle = workspace
            .upgrade()
            .map(|workspace| workspace.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());
        let project_groups = self
            .multi_workspace
            .as_ref()
            .and_then(WeakEntity::upgrade)
            .map(|multi_workspace| multi_workspace.read(cx).project_group_keys())
            .unwrap_or_default();
        let project_is_open = project_name.is_some();
        let display_name = project_name
            .as_ref()
            .map(|name| util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH))
            .unwrap_or_else(|| "Open Recent Project".to_owned());

        PopoverMenu::new("notes-recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    project_groups.clone(),
                    Some(false),
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("notes-project-name-trigger", display_name)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |button| {
                        button.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .when(!project_is_open, |button| button.color(Color::Muted)),
                |_window, cx| {
                    Tooltip::for_action("Switch Project", &zed_actions::OpenRecent::default(), cx)
                },
            )
            .anchor(gpui::Anchor::TopLeft)
            .into_any_element()
    }

    fn render_worktree_and_branch(
        &self,
        repository: Entity<Repository>,
        linked_worktree_name: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let workspace = self.workspace.upgrade()?;
        let (branch_name, icon_info, is_detached_head) = {
            let repository = repository.read(cx);
            let is_detached_head = repository.branch.is_none();
            let branch_name = repository
                .branch
                .as_ref()
                .map(|branch| util::truncate_and_trailoff(branch.name(), MAX_BRANCH_NAME_LENGTH))
                .or_else(|| {
                    repository.head_commit.as_ref().map(|commit| {
                        commit
                            .sha
                            .chars()
                            .take(MAX_SHORT_SHA_LENGTH)
                            .collect::<String>()
                    })
                });
            let status = repository.status_summary();
            let tracked = status.index + status.worktree;
            let icon_info = if status.conflict > 0 {
                (IconName::Warning, Color::VersionControlConflict)
            } else if tracked.modified > 0 {
                (IconName::SquareDot, Color::VersionControlModified)
            } else if tracked.added > 0 || status.untracked > 0 {
                (IconName::SquarePlus, Color::VersionControlAdded)
            } else if tracked.deleted > 0 {
                (IconName::SquareMinus, Color::VersionControlDeleted)
            } else {
                (IconName::GitBranch, Color::Muted)
            };
            (branch_name, icon_info, is_detached_head)
        };

        let worktree_label = linked_worktree_name.unwrap_or_else(|| "main".into());
        let (creation_in_progress, is_switch) = {
            let creation = workspace.read(cx).active_worktree_creation();
            (creation.label.clone(), creation.is_switch)
        };
        let is_creating = creation_in_progress.is_some();
        let display_label: SharedString = match creation_in_progress {
            Some(name) if is_switch => format!("Loading {name}…").into(),
            Some(name) => format!("Creating {name}…").into(),
            None => worktree_label.clone(),
        };

        let project = self.project.clone();
        let workspace_handle = workspace.downgrade();
        let worktree_button = PopoverMenu::new("notes-worktree-picker-menu")
            .menu(move |window, cx| {
                Some(cx.new(|cx| {
                    WorktreePicker::new(project.clone(), workspace_handle.clone(), window, cx)
                }))
            })
            .trigger_with_tooltip(
                Button::new("notes-worktree-picker-trigger", display_label)
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .label_size(LabelSize::Small)
                    .color(Color::Muted)
                    .tab_index(0isize)
                    .loading(is_creating)
                    .start_icon(
                        Icon::new(IconName::GitWorktree)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    ),
                move |_window, cx| {
                    Tooltip::with_meta(
                        "Worktree",
                        Some(&zed_actions::git::Worktree),
                        format!("Currently In Use: {worktree_label}"),
                        cx,
                    )
                },
            )
            .anchor(gpui::Anchor::TopLeft);

        let effective_repository = Some(repository);
        let branch_picker = branch_name.map(|branch_name| {
            let tooltip_branch_name = branch_name.clone();
            let (branch_icon, branch_icon_color) = icon_info;
            let trigger = if is_detached_head {
                Button::new("notes-project-branch-trigger", "Create Branch")
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .start_icon(
                        Icon::new(IconName::GitBranchPlus)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
            } else {
                Button::new("notes-project-branch-trigger", branch_name)
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                    .label_size(LabelSize::Small)
                    .color(Color::Muted)
                    .tab_index(0isize)
                    .start_icon(
                        Icon::new(branch_icon)
                            .size(IconSize::XSmall)
                            .color(branch_icon_color),
                    )
            };
            PopoverMenu::new("notes-branch-menu")
                .menu(move |window, cx| {
                    git_ui_core::build_branch_picker(
                        workspace.downgrade(),
                        effective_repository.clone(),
                        window,
                        cx,
                    )
                })
                .trigger_with_tooltip(trigger, move |_window, cx| {
                    let meta = if is_detached_head {
                        format!("Detached HEAD: {tooltip_branch_name}")
                    } else {
                        format!("Currently Checked Out: {tooltip_branch_name}")
                    };
                    Tooltip::with_meta("Branch & Stash", Some(&zed_actions::git::Branch), meta, cx)
                })
                .anchor(gpui::Anchor::TopLeft)
        });

        Some(
            h_flex()
                .gap_px()
                .child(worktree_button)
                .when(branch_picker.is_some(), |row| {
                    row.child(
                        Label::new("/")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .alpha(0.25),
                    )
                })
                .children(branch_picker)
                .into_any_element(),
        )
    }
}

impl Render for NotesTitleBar {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if self.multi_workspace.is_none()
            && let Some(multi_workspace) = self
                .workspace
                .upgrade()
                .and_then(|workspace| workspace.read(cx).multi_workspace().cloned())
        {
            self.multi_workspace = Some(multi_workspace);
        }

        let active_worktree = self.effective_active_worktree(cx);
        let project_name = active_worktree.as_ref().and_then(|worktree| {
            worktree
                .read(cx)
                .root_name()
                .file_name()
                .map(|name| SharedString::from(name.to_string()))
        });
        let repository = active_worktree
            .as_ref()
            .and_then(|worktree| self.repository_for_worktree(worktree, cx));
        let linked_worktree_name = repository.as_ref().and_then(|repository| {
            let repository = repository.read(cx);
            repository
                .main_worktree_abs_path()
                .and_then(|main_worktree_path| {
                    linked_worktree_short_name(
                        main_worktree_path,
                        repository.work_directory_abs_path.as_ref(),
                    )
                })
                .or_else(|| {
                    repository
                        .is_linked_worktree()
                        .then_some(project_name.clone())
                        .flatten()
                })
        });

        let controls = h_flex()
            .h_full()
            .gap_0p5()
            .child(
                div()
                    .debug_selector(|| "notes-project-switcher".into())
                    .child(self.render_project_name(project_name, cx)),
            )
            .children(repository.and_then(|repository| {
                self.render_worktree_and_branch(repository, linked_worktree_name, cx)
            }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element();
        self.platform_title_bar.update(cx, |title_bar, _| {
            title_bar.set_children([controls]);
        });
        self.platform_title_bar.clone()
    }
}

fn init_notes_title_bar(cx: &mut App) {
    PlatformTitleBar::init(cx);
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let title_bar =
            cx.new(|cx| NotesTitleBar::new(workspace, workspace.multi_workspace().cloned(), cx));
        workspace.set_titlebar_item(title_bar.into(), window, cx);
    })
    .detach();
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum NoteSaveState {
    #[default]
    Saved,
    Modified,
}

impl NoteSaveState {
    fn label(self) -> &'static str {
        match self {
            Self::Saved => "Saved",
            Self::Modified => "Modified",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NotesStatusSnapshot {
    language: String,
    statistics: NoteStatistics,
    line: usize,
    column: usize,
    save_state: NoteSaveState,
}

fn status_editor(
    active_pane_item: Option<&dyn zed_workspace::ItemHandle>,
    cx: &App,
) -> Option<Entity<Editor>> {
    let active_pane_item = active_pane_item?;
    active_pane_item.act_as::<Editor>(cx).or_else(|| {
        active_pane_item
            .downcast::<MarkdownPreviewView>()
            .and_then(|preview| preview.read(cx).source_editor())
    })
}

fn notes_status_snapshot(editor: &mut Editor, cx: &mut App) -> NotesStatusSnapshot {
    let cursor = editor.newest_selection(cx).head();
    let text = editor.text(cx);
    let cursor_offset = editor
        .buffer()
        .read(cx)
        .as_singleton()
        .map(|buffer| buffer.read(cx).snapshot().point_to_offset(cursor))
        .unwrap_or_default()
        .min(text.len());
    let column = text
        .get(..cursor_offset)
        .and_then(|text| text.rsplit_once('\n').map(|(_, line)| line).or(Some(text)))
        .map_or(1, |line| line.chars().count().saturating_add(1));
    let language = editor
        .active_buffer(cx)
        .and_then(|buffer| buffer.read(cx).language().map(|language| language.name()))
        .map_or_else(|| "Markdown".to_owned(), |language| language.to_string());

    NotesStatusSnapshot {
        language,
        statistics: note_statistics(&text),
        line: cursor.row as usize + 1,
        column,
        save_state: if editor.buffer().read(cx).is_dirty(cx) {
            NoteSaveState::Modified
        } else {
            NoteSaveState::Saved
        },
    }
}

struct NotesStatusDetails {
    snapshot: Option<NotesStatusSnapshot>,
    update_task: Task<()>,
    _active_editor_subscription: Option<Subscription>,
}

impl NotesStatusDetails {
    fn new() -> Self {
        Self {
            snapshot: None,
            update_task: Task::ready(()),
            _active_editor_subscription: None,
        }
    }

    fn update_snapshot(
        &mut self,
        editor: &Entity<Editor>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let editor = editor.downgrade();
        self.update_task = cx.spawn_in(window, async move |this, cx| {
            let Ok(snapshot) = editor.update(cx, |editor, cx| notes_status_snapshot(editor, cx))
            else {
                return;
            };
            this.update(cx, |this, cx| {
                this.snapshot = Some(snapshot);
                cx.notify();
            })
            .ok();
        });
    }
}

impl Render for NotesStatusDetails {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let text = self.snapshot.as_ref().map(|snapshot| {
            format!(
                "{}   {} words · {} chars   {}",
                snapshot.save_state.label(),
                snapshot.statistics.words,
                snapshot.statistics.characters,
                snapshot.language,
            )
        });
        div()
            .whitespace_nowrap()
            .text_size(px(11.))
            .text_color(cx.theme().colors().text_muted)
            .child(text.unwrap_or_default())
    }
}

impl StatusItemView for NotesStatusDetails {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn zed_workspace::ItemHandle>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(editor) = status_editor(active_pane_item, cx) else {
            self.snapshot = None;
            self._active_editor_subscription = None;
            cx.notify();
            return;
        };
        self._active_editor_subscription =
            Some(
                cx.subscribe_in(&editor, window, |this, editor, event, window, cx| {
                    if matches!(
                        event,
                        EditorEvent::BufferEdited
                            | EditorEvent::Edited { .. }
                            | EditorEvent::SelectionsChanged { .. }
                            | EditorEvent::DirtyChanged
                            | EditorEvent::Saved
                            | EditorEvent::FileHandleChanged
                            | EditorEvent::Reparsed(_)
                    ) {
                        this.update_snapshot(editor, window, cx);
                    }
                }),
            );
        self.update_snapshot(&editor, window, cx);
    }

    fn hide_setting(&self, _: &App) -> Option<zed_workspace::HideStatusItem> {
        None
    }
}

struct NotesCursorStatus {
    details: Entity<NotesStatusDetails>,
    _details_subscription: Subscription,
}

impl NotesCursorStatus {
    fn new(details: Entity<NotesStatusDetails>, cx: &mut gpui::Context<Self>) -> Self {
        let _details_subscription = cx.observe(&details, |_, _, cx| cx.notify());
        Self {
            details,
            _details_subscription,
        }
    }
}

impl Render for NotesCursorStatus {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let text = self
            .details
            .read(cx)
            .snapshot
            .as_ref()
            .map(|snapshot| format!("Ln {}, Col {}", snapshot.line, snapshot.column));
        div()
            .whitespace_nowrap()
            .pr_1()
            .text_size(px(11.))
            .text_color(cx.theme().colors().text_muted)
            .child(text.unwrap_or_default())
    }
}

impl StatusItemView for NotesCursorStatus {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn zed_workspace::ItemHandle>,
        _: &mut Window,
        _: &mut gpui::Context<Self>,
    ) {
    }

    fn hide_setting(&self, _: &App) -> Option<zed_workspace::HideStatusItem> {
        None
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

#[derive(Debug, Default, Deserialize)]
struct LegacyNotesSettingsContent {
    theme: Option<NotesTheme>,
    vim_mode: Option<bool>,
    relative_line_numbers: Option<RelativeLineNumbers>,
    vim: Option<NotesVimSettingsContent>,
    which_key: Option<NotesWhichKeySettingsContent>,
    ui: Option<NotesUiSettingsContent>,
    editor: Option<NotesEditorSettingsContent>,
    autosave: Option<AutosaveSettingsContent>,
    explorer: Option<ExplorerSettingsContent>,
    files: Option<FileSettingsContent>,
    preview: Option<PreviewSettingsContent>,
    daily_notes: Option<DailyNotesSettingsContent>,
}

#[derive(Debug, Default, Deserialize)]
struct NotesVimSettingsContent {
    leader: Option<String>,
    toggle_relative_line_numbers: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct NotesWhichKeySettingsContent {
    enabled: Option<bool>,
    delay_ms: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct NotesUiSettingsContent {
    font_family: Option<String>,
    font_size: Option<f32>,
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
    font_family: Option<String>,
    font_size: Option<f32>,
    max_width: Option<u32>,
    allow_remote_images: Option<bool>,
    max_file_size_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct DailyNotesSettingsContent {
    directory: Option<String>,
    filename_format: Option<String>,
    template: Option<String>,
}

fn validate_vault_relative_path(value: &str, setting_name: &str, allow_empty: bool) -> Result<()> {
    anyhow::ensure!(
        allow_empty || !value.trim().is_empty(),
        "{setting_name} cannot be empty"
    );
    let path = Path::new(value);
    anyhow::ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "{setting_name} must stay inside the notes folder"
    );
    Ok(())
}

fn validate_daily_notes_settings(settings: &DailyNotesSettings) -> Result<()> {
    validate_vault_relative_path(&settings.directory, "zednotes.daily_notes.directory", true)?;
    anyhow::ensure!(
        !settings.filename_format.trim().is_empty(),
        "zednotes.daily_notes.filename_format cannot be empty"
    );
    anyhow::ensure!(
        !StrftimeItems::new(&settings.filename_format).any(|item| matches!(item, Item::Error)),
        "zednotes.daily_notes.filename_format contains an invalid date directive"
    );
    let validation_date = NaiveDate::from_ymd_opt(2000, 1, 2)
        .context("failed to construct the daily notes validation date")?;
    let example_filename = validation_date
        .format(&settings.filename_format)
        .to_string();
    validate_vault_relative_path(
        &example_filename,
        "zednotes.daily_notes.filename_format",
        false,
    )?;
    anyhow::ensure!(
        Path::new(&example_filename).components().count() == 1,
        "zednotes.daily_notes.filename_format must produce one filename"
    );
    anyhow::ensure!(
        is_markdown_path(Path::new(&example_filename)),
        "zednotes.daily_notes.filename_format must produce a Markdown filename"
    );
    if let Some(template) = &settings.template {
        validate_vault_relative_path(template, "zednotes.daily_notes.template", false)?;
    }
    Ok(())
}

fn daily_note_relative_path(settings: &DailyNotesSettings, date: NaiveDate) -> Result<PathBuf> {
    validate_daily_notes_settings(settings)?;
    let filename = date.format(&settings.filename_format).to_string();
    Ok(Path::new(&settings.directory).join(filename))
}

fn render_daily_note_template(
    template: &str,
    date: NaiveDate,
    datetime: &str,
    note_path: &Path,
) -> String {
    let date = date.format("%Y-%m-%d").to_string();
    let title = note_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&date);
    template
        .replace("{{date}}", &date)
        .replace("{{datetime}}", datetime)
        .replace("{{title}}", title)
}

fn parse_legacy_notes_settings(content: &str) -> Result<NotesSettings> {
    let content = settings::parse_json_with_comments::<LegacyNotesSettingsContent>(content)
        .context("failed to parse legacy zednotes settings")?;
    let mut settings = NotesSettings::default();
    settings.theme = content.theme.unwrap_or(settings.theme);
    settings.vim_mode = content.vim_mode.unwrap_or(settings.vim_mode);
    settings.relative_line_numbers = content
        .relative_line_numbers
        .unwrap_or(settings.relative_line_numbers);
    if let Some(vim) = content.vim {
        settings.vim.leader = vim.leader.unwrap_or(settings.vim.leader);
        settings.vim.toggle_relative_line_numbers = vim
            .toggle_relative_line_numbers
            .unwrap_or(settings.vim.toggle_relative_line_numbers);
    }
    if let Some(which_key) = content.which_key {
        settings.which_key.enabled = which_key.enabled.unwrap_or(settings.which_key.enabled);
        settings.which_key.delay_ms = which_key.delay_ms.unwrap_or(settings.which_key.delay_ms);
    }
    if let Some(ui) = content.ui {
        settings.ui.font_family = ui.font_family.unwrap_or(settings.ui.font_family);
        settings.ui.font_size = ui.font_size.unwrap_or(settings.ui.font_size);
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
        settings.preview.font_family = preview.font_family.unwrap_or(settings.preview.font_family);
        settings.preview.font_size = preview.font_size.unwrap_or(settings.preview.font_size);
        settings.preview.max_width = preview.max_width.unwrap_or(settings.preview.max_width);
        settings.preview.allow_remote_images = preview
            .allow_remote_images
            .unwrap_or(settings.preview.allow_remote_images);
        settings.preview.max_file_size_bytes = preview
            .max_file_size_bytes
            .or(settings.preview.max_file_size_bytes);
    }
    if let Some(daily_notes) = content.daily_notes {
        settings.daily_notes.directory = daily_notes
            .directory
            .unwrap_or(settings.daily_notes.directory);
        settings.daily_notes.filename_format = daily_notes
            .filename_format
            .unwrap_or(settings.daily_notes.filename_format);
        settings.daily_notes.template = daily_notes.template;
    }

    anyhow::ensure!(
        !settings.ui.font_family.trim().is_empty(),
        "ui.font_family cannot be empty"
    );
    anyhow::ensure!(
        settings.ui.font_size > 0.0,
        "ui.font_size must be greater than zero"
    );
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
        !settings.preview.font_family.trim().is_empty(),
        "preview.font_family cannot be empty"
    );
    anyhow::ensure!(
        settings.preview.font_size > 0.0,
        "preview.font_size must be greater than zero"
    );
    anyhow::ensure!(
        settings.preview.max_width > 0,
        "preview.max_width must be positive"
    );
    validate_daily_notes_settings(&settings.daily_notes)?;
    Ok(settings)
}

#[derive(Debug, Default, Deserialize)]
struct ZednotesSettingsRoot {
    zednotes: Option<ZednotesSettingsContent>,
}

#[derive(Debug, Default, Deserialize)]
struct ZednotesSettingsContent {
    vim: Option<ZednotesVimSettingsContent>,
    explorer: Option<ZednotesExplorerSettingsContent>,
    daily_notes: Option<DailyNotesSettingsContent>,
}

#[derive(Debug, Default, Deserialize)]
struct ZednotesVimSettingsContent {
    leader: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ZednotesExplorerSettingsContent {
    markdown_only: Option<bool>,
}

fn validate_notes_settings(settings: &NotesSettings) -> Result<()> {
    anyhow::ensure!(
        !settings.vim.leader.is_empty() && !settings.vim.leader.chars().any(char::is_whitespace),
        "zednotes.vim.leader must be one GPUI keystroke"
    );
    Keystroke::parse(&settings.vim.leader)
        .with_context(|| format!("invalid zednotes.vim.leader {:?}", settings.vim.leader))?;
    validate_daily_notes_settings(&settings.daily_notes)?;
    Ok(())
}

fn parse_notes_settings(content: &str) -> Result<NotesSettings> {
    let document = settings::parse_json_with_comments::<serde_json::Value>(content)
        .context("failed to parse zednotes settings")?;
    let root = document
        .as_object()
        .context("zednotes settings must be a JSON object")?;
    if is_legacy_settings(root) {
        return parse_legacy_notes_settings(content);
    }

    let content = settings::parse_json_with_comments::<ZednotesSettingsRoot>(content)
        .context("failed to parse zednotes settings")?;
    let mut settings = NotesSettings::default();
    if let Some(zednotes) = content.zednotes {
        if let Some(vim) = zednotes.vim {
            settings.vim.leader = vim.leader.unwrap_or(settings.vim.leader);
        }
        if let Some(explorer) = zednotes.explorer {
            settings.explorer.markdown_only = explorer
                .markdown_only
                .unwrap_or(settings.explorer.markdown_only);
        }
        if let Some(daily_notes) = zednotes.daily_notes {
            settings.daily_notes.directory = daily_notes
                .directory
                .unwrap_or(settings.daily_notes.directory);
            settings.daily_notes.filename_format = daily_notes
                .filename_format
                .unwrap_or(settings.daily_notes.filename_format);
            settings.daily_notes.template = daily_notes.template;
        }
    }
    validate_notes_settings(&settings)?;
    Ok(settings)
}

fn is_legacy_settings(root: &serde_json::Map<String, serde_json::Value>) -> bool {
    root.get("theme").is_some_and(|theme| {
        theme
            .as_str()
            .is_some_and(|theme| matches!(theme, "system" | "light" | "dark"))
    }) || [
        "ui",
        "editor",
        "explorer",
        "files",
        "preview",
        "daily_notes",
    ]
    .iter()
    .any(|key| root.contains_key(*key))
        || root.get("autosave").is_some_and(|autosave| {
            autosave.as_object().is_some_and(|autosave| {
                autosave.contains_key("enabled") || autosave.contains_key("delay_ms")
            })
        })
        || root.get("vim").is_some_and(|vim| {
            vim.as_object()
                .is_some_and(|vim| vim.contains_key("leader"))
        })
}

fn nested_object<'a>(
    object: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !value.is_object() {
        *value = serde_json::Value::Object(serde_json::Map::new());
    }
    value
        .as_object_mut()
        .expect("nested settings value was replaced with an object")
}

fn rounded_setting_number(value: f32) -> f64 {
    (f64::from(value) * 100.0).round() / 100.0
}

fn migrate_legacy_settings_content(content: &str) -> Result<Option<String>> {
    let mut document = settings::parse_json_with_comments::<serde_json::Value>(content)
        .context("failed to parse legacy zednotes settings")?;
    let root = document
        .as_object_mut()
        .context("zednotes settings must be a JSON object")?;
    if !is_legacy_settings(root) {
        return Ok(None);
    }

    let settings = parse_legacy_notes_settings(content)?;
    for legacy_key in [
        "ui",
        "editor",
        "explorer",
        "files",
        "preview",
        "daily_notes",
    ] {
        root.remove(legacy_key);
    }

    let theme_mode = match settings.theme {
        NotesTheme::System => "system",
        NotesTheme::Light => "light",
        NotesTheme::Dark => "dark",
    };
    root.insert(
        "theme".to_owned(),
        serde_json::json!({
            "mode": theme_mode,
            "light": "One Light",
            "dark": "One Dark",
        }),
    );
    root.insert("vim_mode".to_owned(), settings.vim_mode.into());
    root.insert(
        "relative_line_numbers".to_owned(),
        serde_json::to_value(settings.relative_line_numbers)?,
    );
    let vim = nested_object(root, "vim");
    vim.remove("leader");
    vim.insert(
        "toggle_relative_line_numbers".to_owned(),
        settings.vim.toggle_relative_line_numbers.into(),
    );
    root.insert(
        "which_key".to_owned(),
        serde_json::json!({
            "enabled": settings.which_key.enabled,
            "delay_ms": settings.which_key.delay_ms,
        }),
    );
    root.insert(
        "ui_font_family".to_owned(),
        settings.ui.font_family.clone().into(),
    );
    root.insert(
        "ui_font_size".to_owned(),
        rounded_setting_number(settings.ui.font_size).into(),
    );
    root.insert(
        "buffer_font_family".to_owned(),
        settings.editor.font_family.clone().into(),
    );
    root.insert(
        "buffer_font_size".to_owned(),
        rounded_setting_number(settings.editor.font_size).into(),
    );
    root.insert(
        "buffer_line_height".to_owned(),
        serde_json::json!({
            "custom": rounded_setting_number(settings.editor.line_height),
        }),
    );
    root.insert(
        "gutter".to_owned(),
        serde_json::json!({ "line_numbers": settings.editor.line_numbers }),
    );
    root.insert(
        "soft_wrap".to_owned(),
        if settings.editor.soft_wrap {
            "editor_width"
        } else {
            "none"
        }
        .into(),
    );
    root.insert(
        "autosave".to_owned(),
        if settings.autosave.enabled {
            serde_json::json!({
                "after_delay": { "milliseconds": settings.autosave.delay_ms },
            })
        } else {
            "off".into()
        },
    );
    root.insert(
        "project_panel".to_owned(),
        serde_json::json!({
            "default_width": settings.explorer.width,
            "hide_hidden": !settings.explorer.show_hidden,
        }),
    );
    root.insert(
        "file_scan_exclusions".to_owned(),
        settings
            .explorer
            .excluded_paths
            .iter()
            .map(|path| exclusion_glob(path))
            .collect::<Vec<_>>()
            .into(),
    );
    root.insert(
        "markdown_preview_font_family".to_owned(),
        settings.preview.font_family.clone().into(),
    );
    root.insert(
        "markdown_preview_font_size".to_owned(),
        rounded_setting_number(settings.preview.font_size).into(),
    );
    root.insert(
        "markdown_preview".to_owned(),
        serde_json::json!({
            "limit_content_width": true,
            "max_width": settings.preview.max_width,
            "allow_remote_images": settings.preview.allow_remote_images,
            "max_file_size_bytes": settings.preview.max_file_size_bytes,
        }),
    );
    let zednotes = nested_object(root, "zednotes");
    nested_object(zednotes, "vim").insert("leader".to_owned(), settings.vim.leader.clone().into());
    nested_object(zednotes, "explorer").insert(
        "markdown_only".to_owned(),
        settings.explorer.markdown_only.into(),
    );
    let daily_notes = nested_object(zednotes, "daily_notes");
    daily_notes.insert(
        "directory".to_owned(),
        settings.daily_notes.directory.clone().into(),
    );
    daily_notes.insert(
        "filename_format".to_owned(),
        settings.daily_notes.filename_format.clone().into(),
    );
    if let Some(template) = &settings.daily_notes.template {
        daily_notes.insert("template".to_owned(), template.clone().into());
    }

    let mut migrated = serde_json::to_string_pretty(&document)?;
    migrated.push('\n');
    Ok(Some(migrated))
}

fn native_settings_document(content: &str) -> Result<serde_json::Value> {
    let migrated = migrate_legacy_settings_content(content)?;
    settings::parse_json_with_comments(migrated.as_deref().unwrap_or(content))
        .context("failed to parse settings document")
}

fn merge_settings_documents(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, overlay_value) in overlay {
                if let Some(base_value) = base.get_mut(&key) {
                    merge_settings_documents(base_value, overlay_value);
                } else {
                    base.insert(key, overlay_value);
                }
            }
        }
        (base, overlay) => {
            *base = overlay;
        }
    }
}

fn merge_split_settings_contents(
    canonical_content: Option<&str>,
    split_content: &str,
    split_wins: bool,
) -> Result<String> {
    let canonical = canonical_content
        .map(native_settings_document)
        .transpose()?
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let split = native_settings_document(split_content)?;
    let (mut merged, overlay) = if split_wins {
        (canonical, split)
    } else {
        (split, canonical)
    };
    merge_settings_documents(&mut merged, overlay);
    let mut content = serde_json::to_string_pretty(&merged)?;
    content.push('\n');
    settings::parse_json_with_comments::<settings::UserSettingsContent>(&content)
        .context("merged settings are not valid native Zed settings")?;
    parse_notes_settings(&content).context("merged zednotes settings are invalid")?;
    Ok(content)
}

pub fn default_config_dir() -> PathBuf {
    paths::home_dir().join(".config").join(APP_NAME)
}

fn config_dir() -> PathBuf {
    paths::config_dir().clone()
}

pub fn application_support_dir() -> PathBuf {
    paths::home_dir()
        .join("Library")
        .join("Application Support")
        .join(APP_NAME)
}

pub fn logs_dir() -> &'static PathBuf {
    static LOGS_DIR: OnceLock<PathBuf> = OnceLock::new();
    LOGS_DIR.get_or_init(|| {
        if cfg!(target_os = "macos") {
            paths::home_dir().join("Library/Logs").join(APP_NAME)
        } else {
            application_support_dir().join("logs")
        }
    })
}

pub fn log_file() -> &'static PathBuf {
    static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    LOG_FILE.get_or_init(|| logs_dir().join(format!("{APP_NAME}.log")))
}

pub fn old_log_file() -> &'static PathBuf {
    static OLD_LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
    OLD_LOG_FILE.get_or_init(|| logs_dir().join(format!("{APP_NAME}.log.old")))
}

fn notes_settings_path() -> PathBuf {
    paths::settings_file().clone()
}

fn notes_keymap_path() -> PathBuf {
    paths::keymap_file().clone()
}

fn write_settings_migration(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary_path = parent.join(format!(
        ".settings.json.{}.{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&temporary_path, content)
        .with_context(|| format!("failed to write {}", temporary_path.display()))?;
    if let Err(error) = std::fs::rename(&temporary_path, path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    Ok(())
}

fn migrate_legacy_settings_file() -> Result<bool> {
    let path = notes_settings_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let Some(migrated) = migrate_legacy_settings_content(&content)? else {
        return Ok(false);
    };
    write_settings_migration(&path, &migrated)?;
    Ok(true)
}

fn migrate_split_settings_file() -> Result<bool> {
    let split_path = application_support_dir().join("config/settings.json");
    let canonical_path = notes_settings_path();
    if split_path == canonical_path {
        return Ok(false);
    }
    let split_content = match std::fs::read_to_string(&split_path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", split_path.display()));
        }
    };
    let canonical_content = match std::fs::read_to_string(&canonical_path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read {}", canonical_path.display()));
        }
    };
    let split_modified = std::fs::metadata(&split_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    let canonical_modified = std::fs::metadata(&canonical_path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    let merged = merge_split_settings_contents(
        canonical_content.as_deref(),
        &split_content,
        split_modified >= canonical_modified,
    )?;
    write_settings_migration(&canonical_path, &merged)?;
    std::fs::remove_file(&split_path)
        .with_context(|| format!("failed to retire {}", split_path.display()))?;
    Ok(true)
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

fn frontmatter_last_updated_range(text: &str) -> Option<(Range<usize>, Option<char>)> {
    let mut lines = text.split_inclusive('\n');
    let first_line = lines.next()?;
    if first_line.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }

    let mut offset = first_line.len();
    let mut last_updated = None;
    for line in lines {
        let content = line.trim_end_matches(['\r', '\n']);
        if content == "---" {
            return last_updated;
        }

        if let Some(value) = content.strip_prefix("lastUpdated:") {
            if last_updated.is_some() {
                return None;
            }

            let leading_whitespace = value.len() - value.trim_start().len();
            let trailing_whitespace = value.len() - value.trim_end().len();
            let value_start = offset + "lastUpdated:".len() + leading_whitespace;
            let value_end = offset + content.len() - trailing_whitespace;
            let old_value = &text[value_start..value_end];
            let quote = match (old_value.chars().next(), old_value.chars().last()) {
                (Some(start), Some(end)) if start == end && matches!(start, '\'' | '"') => {
                    Some(start)
                }
                _ => None,
            };
            last_updated = Some((value_start..value_end, quote));
        }

        offset += line.len();
    }
    None
}

fn update_last_updated_before_save(buffer: &Entity<Buffer>, cx: &mut App) {
    let edit = {
        let buffer = buffer.read(cx);
        if !buffer.is_dirty() {
            return;
        }
        let Some(path) = buffer
            .file()
            .and_then(|file| file.as_local().map(|file| file.abs_path(cx)))
        else {
            return;
        };
        if !is_markdown_path(&path) {
            return;
        }

        frontmatter_last_updated_range(&buffer.snapshot().text())
    };
    let Some((range, quote)) = edit else {
        return;
    };

    let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    let replacement = quote
        .map(|quote| format!("{quote}{timestamp}{quote}"))
        .unwrap_or(timestamp);
    buffer.update(cx, |buffer, cx| {
        if !buffer.is_dirty() {
            return;
        }
        buffer.finalize_last_transaction();
        buffer.start_transaction();
        buffer.edit([(range, replacement)], None, cx);
        if let Some(transaction_id) = buffer.end_transaction(cx) {
            buffer.forget_transaction(transaction_id);
        }
    });
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

fn register_lazy_language(
    languages: &LanguageRegistry,
    config: LanguageConfig,
    load_queries: impl Fn() -> LanguageQueries + Send + Sync + 'static,
) {
    let load_queries = Arc::new(load_queries);
    languages.register_language(
        config.name.clone(),
        config.grammar.clone(),
        config.matcher.clone(),
        config.hidden,
        None,
        Arc::new(move || {
            let config = config.clone();
            let load_queries = load_queries.clone();
            async move {
                Ok(LoadedLanguage {
                    config,
                    queries: load_queries(),
                    context_provider: None,
                    toolchain_provider: None,
                    manifest_name: None,
                })
            }
            .boxed()
        }),
    );
}

fn register_bundled_preview_language(
    languages: &LanguageRegistry,
    name: &'static str,
    grammar_name: &'static str,
    fence_aliases: &[&str],
    grammar: tree_sitter::Language,
    highlights_query: &'static str,
) {
    languages.register_native_grammars([(grammar_name, grammar)]);
    register_lazy_language(
        languages,
        LanguageConfig {
            name: name.into(),
            code_fence_block_name: Some(name.to_lowercase().into()),
            grammar: Some(grammar_name.into()),
            matcher: LanguageMatcher {
                path_suffixes: fence_aliases
                    .iter()
                    .map(|alias| (*alias).to_owned())
                    .collect(),
                ..LanguageMatcher::default()
            }
            .into(),
            ..LanguageConfig::default()
        },
        move || LanguageQueries {
            highlights: Some(highlights_query.into()),
            ..LanguageQueries::default()
        },
    );
}

fn register_built_in_languages(languages: &LanguageRegistry) {
    let native_grammars = grammars::native_grammars();
    languages.register_native_grammars(
        native_grammars
            .iter()
            .map(|(name, grammar)| (*name, grammar.clone())),
    );
    for (name, _) in native_grammars {
        let config = grammars::load_config(name);
        register_lazy_language(languages, config, move || grammars::load_queries(name));
    }

    // JavaScript shares the TSX parser but has its own highlight queries and
    // file/fence aliases.
    register_lazy_language(languages, grammars::load_config("javascript"), || {
        grammars::load_queries("javascript")
    });
    register_bundled_preview_language(
        languages,
        "HTML",
        "html",
        &["html", "htm", "shtml"],
        tree_sitter_html::LANGUAGE.into(),
        tree_sitter_html::HIGHLIGHTS_QUERY,
    );
    register_bundled_preview_language(
        languages,
        "SQL",
        "sql",
        &["sql", "postgres", "postgresql", "mysql", "sqlite", "plsql"],
        tree_sitter_sequel::LANGUAGE.into(),
        tree_sitter_sequel::HIGHLIGHTS_QUERY,
    );
    register_bundled_preview_language(
        languages,
        "XML",
        "xml",
        &["xml", "xsd", "xsl", "xslt", "svg"],
        tree_sitter_xml::LANGUAGE_XML.into(),
        tree_sitter_xml::XML_HIGHLIGHT_QUERY,
    );
    register_bundled_preview_language(
        languages,
        "DTD",
        "dtd",
        &["dtd"],
        tree_sitter_xml::LANGUAGE_DTD.into(),
        tree_sitter_xml::DTD_HIGHLIGHT_QUERY,
    );
    register_bundled_preview_language(
        languages,
        "TOML",
        "toml",
        &["toml"],
        tree_sitter_toml_ng::LANGUAGE.into(),
        tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
    );
}

fn markdown_languages(cx: &App) -> Arc<LanguageRegistry> {
    let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
    register_built_in_languages(&languages);
    languages.set_theme(cx.theme().clone());
    languages
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
        KeyBinding::new(&keys("n d"), OpenToday, CONTEXT),
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
        KeyBinding::new(&keys("b b"), tab_switcher::ToggleAll, CONTEXT),
        KeyBinding::new(&keys("o"), zed_actions::outline::ToggleOutline, CONTEXT),
        KeyBinding::new(&keys("m m"), editor::actions::ToggleBookmark, CONTEXT),
        KeyBinding::new(
            &keys("m l"),
            editor::actions::ToggleBookmarkWithLabel,
            CONTEXT,
        ),
        KeyBinding::new(&keys("m e"), editor::actions::EditBookmark, CONTEXT),
        KeyBinding::new(&keys("m n"), editor::actions::GoToNextBookmark, CONTEXT),
        KeyBinding::new(&keys("m p"), editor::actions::GoToPreviousBookmark, CONTEXT),
        KeyBinding::new(&keys("m v"), editor::actions::ViewBookmarks, CONTEXT),
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
        KeyBinding::new("cmd-,", zed_actions::OpenSettings, Some("Workspace")),
        KeyBinding::new("cmd-alt-,", OpenSettingsFile, Some("Workspace")),
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("Editor")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("MarkdownPreview")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("ProjectPanel")),
        KeyBinding::new("cmd-e", OpenRecentNote, Some("Workspace")),
        KeyBinding::new("cmd-n", New, Some("Workspace")),
        KeyBinding::new("cmd-shift-j", OpenToday, Some("Workspace")),
        KeyBinding::new("cmd-b", ToggleExplorer, Some("Workspace")),
        KeyBinding::new("cmd-shift-enter", ToggleFocusMode, Some("Workspace")),
        KeyBinding::new("cmd-shift-f", Search, Some("Pane")),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new(
            "cmd-shift-v",
            OpenPreview,
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
            "cmd-s",
            SaveAndReturnToNormal,
            Some("Editor && vim_mode == insert"),
        ),
        KeyBinding::new(
            "j k",
            SaveAndReturnToNormal,
            Some("Editor && vim_mode == insert"),
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
    keymap_editor::KeymapEventChannel::trigger_keymap_changed(cx);
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

fn notes_menus() -> Vec<Menu> {
    let mut application_items = vec![
        MenuItem::action("Settings…", zed_actions::OpenSettings),
        MenuItem::action("Keyboard Shortcuts…", zed_actions::OpenKeymap),
        MenuItem::action("Open Settings File…", OpenSettingsFile),
        MenuItem::action("Open Keymap File…", zed_actions::OpenKeymapFile),
        MenuItem::separator(),
    ];
    #[cfg(target_os = "macos")]
    application_items.push(MenuItem::os_submenu("Services", SystemMenuType::Services));
    #[cfg(target_os = "macos")]
    application_items.push(MenuItem::separator());
    application_items.extend([
        MenuItem::action("Hide zednotes", Hide),
        MenuItem::action("Hide Others", HideOthers),
        MenuItem::action("Show All", ShowAll),
        MenuItem::separator(),
        MenuItem::action("Quit zednotes", Quit),
    ]);

    vec![
        Menu::new(APP_NAME).items(application_items),
        Menu::new("File").items([
            MenuItem::action("New Note…", New),
            MenuItem::action("Open Today", OpenToday),
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Switch Project…", OpenRecentWorkspace),
            MenuItem::separator(),
            MenuItem::action("Save", zed_workspace::Save { save_intent: None }),
            MenuItem::action("Save All", zed_workspace::SaveAll { save_intent: None }),
            MenuItem::separator(),
            MenuItem::action("Statistics", Statistics),
            MenuItem::separator(),
            MenuItem::action("Close Note", zed_workspace::CloseActiveItem::default()),
            MenuItem::action("Close Window", zed_workspace::CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", editor::actions::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", editor::actions::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", editor::actions::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", editor::actions::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", editor::actions::Paste, OsAction::Paste),
            MenuItem::os_action(
                "Select All",
                editor::actions::SelectAll,
                OsAction::SelectAll,
            ),
            MenuItem::separator(),
            MenuItem::action("Find in Note…", zed_actions::buffer_search::Deploy::find()),
            MenuItem::action("Find in Workspace…", Search),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Explorer", ToggleExplorer),
            MenuItem::action("Toggle Git Panel", zed_actions::git_panel::ToggleFocus),
            MenuItem::action("Toggle Preview", TogglePreview),
            MenuItem::action("Toggle Focus Mode", ToggleFocusMode),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Theme").items([
                MenuItem::action("System", UseSystemTheme),
                MenuItem::action("Light", UseLightTheme),
                MenuItem::action("Dark", UseDarkTheme),
            ])),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Quick Open…", zed_workspace::ToggleFileFinder::default()),
            MenuItem::action("Open Recent Note…", OpenRecentNote),
            MenuItem::action("Switch Open Note…", tab_switcher::ToggleAll),
            MenuItem::action("Outline…", zed_actions::outline::ToggleOutline),
            MenuItem::action("View Bookmarks", editor::actions::ViewBookmarks),
            MenuItem::separator(),
            MenuItem::action("Back", zed_workspace::GoBack),
            MenuItem::action("Forward", zed_workspace::GoForward),
            MenuItem::separator(),
            MenuItem::action("Command Palette…", zed_actions::command_palette::Toggle),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Close Window", zed_workspace::CloseWindow),
        ]),
    ]
}

fn init_editor_subsystems(cx: &mut App) -> Result<()> {
    zed_actions::init();
    command_palette::init(cx);
    editor::init(cx);
    editor::register_before_save_buffer_hook(update_last_updated_before_save, cx);
    language_model::init(cx);
    file_finder::init_with_file_filter_file_creation_and_history_ranker(
        Arc::new(is_markdown_path),
        false,
        Arc::new(recent_note_score),
        cx,
    );
    image_viewer::init(cx);
    GitHostingProviderRegistry::default_global(cx);
    git_hosting_providers::init(cx);
    git_ui::init(cx);
    markdown_preview::init(cx);
    settings_ui::init_with_options(
        settings_ui::SettingsUiOptions::new("Zednotes — Settings").with_visible_pages([
            settings_ui::SettingsPageKind::General,
            settings_ui::SettingsPageKind::Appearance,
            settings_ui::SettingsPageKind::Keymap,
            settings_ui::SettingsPageKind::Editor,
            settings_ui::SettingsPageKind::LanguagesAndTools,
            settings_ui::SettingsPageKind::SearchAndFiles,
            settings_ui::SettingsPageKind::WindowAndLayout,
            settings_ui::SettingsPageKind::Panels,
            settings_ui::SettingsPageKind::VersionControl,
        ]),
        cx,
    );
    keymap_editor::init(cx);
    outline::init(cx);
    outline_panel::init(cx);
    project_panel::init(cx);
    search::init(cx);
    tab_switcher::init(cx);
    init_notes_title_bar(cx);
    vim::init(cx);
    which_key::init(cx);
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
    cx.on_action(hide);
    cx.on_action(hide_others);
    cx.on_action(show_all);
    observe_external_file_changes(cx);
    cx.set_menus(notes_menus());
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

fn apply_editor_settings(
    settings: &NotesEditorSettings,
    relative_line_numbers: RelativeLineNumbers,
    cx: &mut App,
) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.theme.buffer_font_family =
                Some(FontFamilyName::from(settings.font_family.clone()));
            content.theme.markdown_preview_code_font_family =
                Some(FontFamilyName::from(settings.font_family.clone()));
            content.theme.buffer_font_size = Some(settings.font_size.into());
            content.theme.buffer_line_height = Some(BufferLineHeight::Custom(settings.line_height));
            content.editor.relative_line_numbers = Some(relative_line_numbers);
            content.editor.gutter.get_or_insert_default().line_numbers =
                Some(settings.line_numbers);
            let gutter = content.editor.gutter.get_or_insert_default();
            gutter.runnables = Some(false);
            gutter.breakpoints = Some(false);
            gutter.bookmarks = Some(false);
            gutter.folds = Some(false);
            let minimap = content.editor.minimap.get_or_insert_default();
            minimap.show = Some(ShowMinimap::Never);
            let scrollbar = content.editor.scrollbar.get_or_insert_default();
            scrollbar.git_diff = Some(true);
            scrollbar.diagnostics = Some(ScrollbarDiagnostics::None);
            let toolbar = content.editor.toolbar.get_or_insert_default();
            toolbar.breadcrumbs = Some(false);
            toolbar.quick_actions = Some(false);
            toolbar.selections_menu = Some(false);
            toolbar.agent_review = Some(false);
            toolbar.code_actions = Some(false);
            content.editor.hover_popover_enabled = Some(false);
            content.editor.auto_signature_help = Some(false);
            content.editor.show_signature_help_after_edits = Some(false);
            content.editor.inline_code_actions = Some(false);
            content.editor.code_lens = Some(CodeLens::Off);
            content.editor.lsp_document_links = Some(false);
            content.project.all_languages.defaults.soft_wrap = Some(if settings.soft_wrap {
                SoftWrap::EditorWidth
            } else {
                SoftWrap::None
            });
            content
                .project
                .all_languages
                .defaults
                .enable_language_server = Some(false);
            content
                .project
                .all_languages
                .defaults
                .show_completions_on_input = Some(false);
            content.project.all_languages.defaults.show_edit_predictions = Some(false);
            let completions = content
                .project
                .all_languages
                .defaults
                .completions
                .get_or_insert_default();
            completions.words = Some(WordsCompletionMode::Disabled);
            completions.lsp = Some(false);
            content
                .project
                .all_languages
                .defaults
                .inlay_hints
                .get_or_insert_default()
                .enabled = Some(false);
            let tabs = content.tabs.get_or_insert_default();
            tabs.git_status = Some(true);
            tabs.show_diagnostics = Some(ShowDiagnostics::Off);
            let project_panel = content.project_panel.get_or_insert_default();
            project_panel.git_status = Some(true);
            project_panel.show_diagnostics = Some(ShowDiagnostics::Off);
            content.git_panel.get_or_insert_default().dock = Some(DockPosition::Left.into());
            let outline_panel = content.outline_panel.get_or_insert_default();
            outline_panel.button = Some(true);
            outline_panel.dock = Some(DockSide::Left);
            let status_bar = content.status_bar.get_or_insert_default();
            status_bar.show_active_file = Some(false);
            status_bar.active_language_button = Some(false);
            status_bar.cursor_position_button = Some(false);
            status_bar.line_endings_button = Some(false);
            content
                .git
                .get_or_insert_default()
                .enabled
                .get_or_insert_default()
                .disable_git = Some(false);
        });
    });
}

fn apply_ui_settings(settings: &NotesUiSettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.theme.ui_font_family = Some(FontFamilyName::from(settings.font_family.clone()));
            content.theme.ui_font_size = Some(settings.font_size.into());
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
            let selection = ThemeSelection::Dynamic {
                mode,
                light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
                dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
            };
            content.theme.theme = Some(selection.clone());
            content.theme.markdown_preview_theme = Some(selection);
        });
    });
}

fn apply_preview_settings(settings: &PreviewSettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.theme.markdown_preview_font_family =
                Some(FontFamilyName::from(settings.font_family.clone()));
            content.theme.markdown_preview_font_size = Some(settings.font_size.into());
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

fn apply_which_key_settings(settings: NotesWhichKeySettings, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            let which_key = content.which_key.get_or_insert_default();
            which_key.enabled = Some(settings.enabled);
            which_key.delay_ms = Some(settings.delay_ms);
        });
    });
}

fn apply_notes_settings(settings: NotesSettings, cx: &mut App) {
    let leader_changed = cx
        .try_global::<CurrentNotesSettings>()
        .is_some_and(|current| current.0.vim.leader != settings.vim.leader);
    apply_theme_setting(settings.theme, cx);
    apply_ui_settings(&settings.ui, cx);
    apply_editor_settings(&settings.editor, settings.relative_line_numbers, cx);
    apply_explorer_settings(&settings.explorer, cx);
    apply_preview_settings(&settings.preview, cx);
    apply_autosave_settings(settings.autosave, cx);
    apply_which_key_settings(settings.which_key, cx);
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.vim_mode = Some(settings.vim_mode);
            content.helix_mode = Some(false);
            content
                .session
                .get_or_insert_default()
                .restore_unsaved_buffers = Some(true);
            content
                .vim
                .get_or_insert_default()
                .toggle_relative_line_numbers = Some(settings.vim.toggle_relative_line_numbers);
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
    !matches!(
        WorkspaceSettings::get_global(cx).autosave,
        AutosaveSetting::Off
    )
}

fn hide(_: &Hide, cx: &mut App) {
    cx.hide();
}

fn hide_others(_: &HideOthers, cx: &mut App) {
    cx.hide_other_apps();
}

fn show_all(_: &ShowAll, cx: &mut App) {
    cx.unhide_other_apps();
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

fn active_note_editor(workspace: &Workspace, cx: &App) -> Option<Entity<Editor>> {
    workspace.active_item_as::<Editor>(cx).or_else(|| {
        workspace
            .active_item_as::<MarkdownPreviewView>(cx)
            .and_then(|preview| preview.read(cx).source_editor())
    })
}

fn show_note_statistics(workspace: &mut Workspace, cx: &mut gpui::Context<Workspace>) {
    let Some(editor) = active_note_editor(workspace, cx) else {
        workspace.show_toast(
            Toast::new(
                NotificationId::Named("notes-statistics".into()),
                "No active note",
            )
            .autohide(),
            cx,
        );
        return;
    };
    let message = editor.read_with(cx, |editor, cx| note_statistics_message(&editor.text(cx)));
    workspace.show_toast(
        Toast::new(NotificationId::Named("notes-statistics".into()), message).autohide(),
        cx,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusModeLayout {
    open_docks: [bool; 3],
    centered_layout: bool,
    pane_maximized: bool,
    tab_bar_visible: bool,
    status_bar_visible: bool,
}

fn set_notes_chrome_visibility(tab_bar_visible: bool, status_bar_visible: bool, cx: &mut App) {
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.update_default_settings(cx, |content| {
            content.tab_bar.get_or_insert_default().show = Some(tab_bar_visible);
            content.status_bar.get_or_insert_default().show = Some(status_bar_visible);
        });
    });
}

fn set_dock_open(
    workspace: &Workspace,
    position: DockPosition,
    open: bool,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    workspace
        .dock_at_position(position)
        .update(cx, |dock, cx| dock.set_open(open, window, cx));
}

fn toggle_focus_mode(
    workspace: &mut Workspace,
    state: &Cell<Option<FocusModeLayout>>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    const DOCKS: [DockPosition; 3] = [
        DockPosition::Left,
        DockPosition::Bottom,
        DockPosition::Right,
    ];

    if let Some(layout) = state.take() {
        if workspace.is_pane_maximized() != layout.pane_maximized {
            workspace.toggle_editor_zoom(&zed_workspace::ToggleEditorZoom, window, cx);
        }
        workspace.centered_layout = layout.centered_layout;
        for (position, open) in DOCKS.into_iter().zip(layout.open_docks) {
            set_dock_open(workspace, position, open, window, cx);
        }
        set_notes_chrome_visibility(layout.tab_bar_visible, layout.status_bar_visible, cx);
        workspace.focus_center_pane(window, cx);
        cx.notify();
        return;
    }

    let layout = FocusModeLayout {
        open_docks: DOCKS.map(|position| workspace.dock_at_position(position).read(cx).is_open()),
        centered_layout: workspace.centered_layout,
        pane_maximized: workspace.is_pane_maximized(),
        tab_bar_visible: TabBarSettings::get_global(cx).show,
        status_bar_visible: StatusBarSettings::get_global(cx).show,
    };
    let Some(editor) = active_note_editor(workspace, cx) else {
        return;
    };
    workspace.activate_item(&editor, true, true, window, cx);
    for position in DOCKS {
        set_dock_open(workspace, position, false, window, cx);
    }
    if !workspace.is_pane_maximized() {
        workspace.toggle_editor_zoom(&zed_workspace::ToggleEditorZoom, window, cx);
    }
    workspace.centered_layout = true;
    set_notes_chrome_visibility(false, false, cx);
    editor.focus_handle(cx).focus(window, cx);
    state.set(Some(layout));
    cx.notify();
}

fn use_theme(theme: NotesTheme, cx: &mut App) {
    let mut settings = cx.global::<CurrentNotesSettings>().0.clone();
    settings.theme = theme;
    apply_notes_settings(settings, cx);

    let mode = match theme {
        NotesTheme::System => ThemeAppearanceMode::System,
        NotesTheme::Light => ThemeAppearanceMode::Light,
        NotesTheme::Dark => ThemeAppearanceMode::Dark,
    };
    settings::update_settings_file(<dyn Fs>::global(cx), cx, move |content, _| {
        let selection = ThemeSelection::Dynamic {
            mode,
            light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
            dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
        };
        content.theme.theme = Some(selection.clone());
        content.theme.markdown_preview_theme = Some(selection);
    });
}

fn open_preview(workspace: &mut Workspace, window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    let Some(editor) = MarkdownPreviewView::resolve_active_item_as_markdown_editor(workspace, cx)
    else {
        return;
    };
    let pane = workspace.active_pane().clone();
    MarkdownPreviewView::open_preview_in_pane(workspace, editor, pane, window, cx);
}

fn active_notes_root(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    let project = workspace.project().read(cx);
    let active_worktree = workspace
        .active_item(cx)
        .and_then(|item| item.project_path(cx))
        .and_then(|path| project.worktree_for_id(path.worktree_id, cx));
    active_worktree
        .or_else(|| project.visible_worktrees(cx).next())
        .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
}

fn open_today(workspace: &mut Workspace, window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    let Some(notes_root) = active_notes_root(workspace, cx) else {
        workspace.show_toast(
            Toast::new(
                NotificationId::Named("notes-daily-note-no-worktree".into()),
                "Open a notes folder before opening today's note",
            )
            .autohide(),
            cx,
        );
        return;
    };
    let settings = cx.global::<CurrentNotesSettings>().0.daily_notes.clone();
    let now = Local::now();
    let date = now.date_naive();
    let datetime = now.to_rfc3339_opts(SecondsFormat::Secs, false);
    let relative_path = match daily_note_relative_path(&settings, date) {
        Ok(path) => path,
        Err(error) => {
            log::error!("invalid daily notes settings: {error:#}");
            workspace.show_toast(
                Toast::new(
                    NotificationId::Named("notes-invalid-daily-note-settings".into()),
                    format!("Invalid daily notes settings: {error}"),
                )
                .autohide(),
                cx,
            );
            return;
        }
    };
    let note_path = notes_root.join(relative_path);
    let template_path = settings
        .template
        .as_deref()
        .map(|template| notes_root.join(template));
    let fs = workspace.app_state().fs.clone();

    cx.spawn_in(window, async move |workspace, cx| {
        let created = !fs.is_file(&note_path).await;
        if created {
            let template = if let Some(template_path) = template_path {
                fs.load(&template_path).await.with_context(|| {
                    format!(
                        "failed to read daily note template {}",
                        template_path.display()
                    )
                })?
            } else {
                DEFAULT_DAILY_NOTE_TEMPLATE.to_owned()
            };
            let content = render_daily_note_template(&template, date, &datetime, &note_path);
            let parent = note_path
                .parent()
                .context("the daily note path has no parent directory")?;
            fs.create_dir(parent)
                .await
                .with_context(|| format!("failed to create {}", parent.display()))?;
            fs.save(&note_path, &content.as_str().into(), Default::default())
                .await
                .with_context(|| format!("failed to create {}", note_path.display()))?;
        }

        let item = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    note_path,
                    OpenOptions {
                        visible: Some(OpenVisible::None),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })?
            .await?;

        if created && let Some(editor) = item.downcast::<Editor>() {
            editor.update_in(cx, |editor, window, cx| {
                let end = editor.buffer().read(cx).len(cx);
                editor.change_selections(
                    SelectionEffects::scroll(Autoscroll::center()),
                    window,
                    cx,
                    |selections| selections.select_ranges([end..end]),
                );
            })?;
            workspace.update_in(cx, |_, window, cx| {
                editor.focus_handle(cx).focus(window, cx);
                window.dispatch_action(vim::SwitchToInsertMode.boxed_clone(), cx);
            })?;
        }

        anyhow::Ok(())
    })
    .detach_and_prompt_err("Failed to open today's note", window, cx, |_, _, _| None);
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

fn save_and_return_to_normal(window: &mut Window, cx: &mut gpui::Context<Workspace>) {
    match cx.build_action("vim::NormalBefore", None) {
        Ok(action) => window.dispatch_action(action, cx),
        Err(error) => log::error!("failed to build vim::NormalBefore: {error:#}"),
    }
    cx.defer_in(window, |workspace, window, cx| {
        workspace
            .save_active_item(SaveIntent::Save, window, cx)
            .detach_and_prompt_err("Failed to save", window, cx, |_, _, _| None);
    });
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

fn open_recent_projects(
    workspace: &mut Workspace,
    create_new_window: Option<bool>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let project_groups = workspace
        .multi_workspace()
        .and_then(|multi_workspace| multi_workspace.upgrade())
        .map(|multi_workspace| multi_workspace.read(cx).project_group_keys())
        .unwrap_or_default();
    let focus_handle = workspace.focus_handle(cx);
    recent_projects::RecentProjects::open(
        workspace,
        create_new_window,
        project_groups,
        window,
        focus_handle,
        cx,
    );
}

fn open_config_file(
    workspace: &mut Workspace,
    path: PathBuf,
    initial_content: SharedString,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let config_dir = config_dir();
    let fs = workspace.app_state().fs.clone();
    let project = workspace.project().clone();
    cx.spawn_in(window, async move |workspace, cx| {
        if !fs.is_file(&path).await {
            fs.save(&path, &initial_content.as_ref().into(), Default::default())
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

fn open_settings_file(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    open_config_file(
        workspace,
        notes_settings_path(),
        INITIAL_SETTINGS_CONTENT.into(),
        window,
        cx,
    );
}

fn open_keymap_file(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    open_config_file(
        workspace,
        notes_keymap_path(),
        settings::initial_keymap_content().into_owned().into(),
        window,
        cx,
    );
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
        let focus_mode_state = Rc::new(Cell::new(None));

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
            open_recent_projects(workspace, Some(false), window, cx);
        });
        workspace.register_action(|workspace, action: &zed_actions::OpenRecent, window, cx| {
            open_recent_projects(workspace, action.create_new_window, window, cx);
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
            settings::update_settings_file(<dyn Fs>::global(cx), cx, move |content, _| {
                content.project_panel.get_or_insert_default().hide_hidden = Some(!hide_hidden);
            });
        });
        workspace.register_action(|workspace, _: &OpenSettingsFile, window, cx| {
            open_settings_file(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &zed_actions::OpenKeymapFile, window, cx| {
            open_keymap_file(workspace, window, cx);
        });
        workspace.register_action(|workspace, _: &FocusEditor, window, cx| {
            workspace.focus_center_pane(window, cx);
        });
        workspace.register_action(|workspace, _: &ToggleExplorer, window, cx| {
            workspace.toggle_dock(DockPosition::Left, window, cx);
        });
        workspace.register_action(move |workspace, _: &ToggleFocusMode, window, cx| {
            toggle_focus_mode(workspace, &focus_mode_state, window, cx);
        });
        workspace.register_action(|_, _: &UseSystemTheme, _, cx| {
            use_theme(NotesTheme::System, cx);
        });
        workspace.register_action(|_, _: &UseLightTheme, _, cx| {
            use_theme(NotesTheme::Light, cx);
        });
        workspace.register_action(|_, _: &UseDarkTheme, _, cx| {
            use_theme(NotesTheme::Dark, cx);
        });
        workspace.register_action(|_, _: &Minimize, window, _| {
            window.minimize_window();
        });
        workspace.register_action(|_, _: &Zoom, window, _| {
            window.zoom_window();
        });
        workspace.register_action(|_, _: &zed_actions::IncreaseBufferFontSize, _, cx| {
            theme_settings::increase_buffer_font_size(cx);
        });
        workspace.register_action(|_, _: &zed_actions::DecreaseBufferFontSize, _, cx| {
            theme_settings::decrease_buffer_font_size(cx);
        });
        workspace.register_action(|_, _: &zed_actions::ResetBufferFontSize, _, cx| {
            theme_settings::reset_buffer_font_size(cx);
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
        workspace.register_action(|workspace, _: &Statistics, _, cx| {
            show_note_statistics(workspace, cx);
        });
        workspace.register_action(|_, _: &SaveAndReturnToNormal, window, cx| {
            save_and_return_to_normal(window, cx);
        });
        workspace.register_action(|workspace, _: &New, window, cx| {
            dispatch_project_panel_action(workspace, "project_panel::NewFile", window, cx);
        });
        workspace.register_action(|workspace, _: &OpenToday, window, cx| {
            open_today(workspace, window, cx);
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
                let project_panel = ProjectPanel::load_with_options(
                    workspace.clone(),
                    ProjectPanelOptions {
                        entry_filter: Some(entry_filter),
                        new_file_name_transformer: Some(Arc::new(markdown_note_filename)),
                        post_create_action: Some(Box::new(vim::SwitchToInsertMode)),
                    },
                    cx.clone(),
                )
                .await?;
                let git_panel = GitPanel::load(workspace.clone(), cx.clone()).await?;
                let outline_panel = OutlinePanel::load(workspace.clone(), cx.clone()).await?;
                workspace.update_in(cx, |workspace, window, cx| {
                    cx.subscribe_in(&project_panel, window, |workspace, _, event, _, cx| {
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
                    workspace.add_panel(project_panel, window, cx);
                    workspace.add_panel(git_panel, window, cx);
                    workspace.add_panel(outline_panel, window, cx);
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
    let config_dir = config_dir();
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
                let content = match fs.load(&settings_path).await {
                    Ok(content) => content,
                    Err(error) => {
                        if fs.is_file(&settings_path).await {
                            log::error!(
                                "failed to read {}: {error:#}; using defaults",
                                settings_path.display()
                            );
                        }
                        "{}".to_owned()
                    }
                };
                let notes_settings = parse_notes_settings(&content).unwrap_or_else(|error| {
                    log::error!(
                        "failed to load {}: {error:#}; using defaults",
                        settings_path.display()
                    );
                    NotesSettings::default()
                });
                cx.update(|cx| {
                    let result = cx.update_global::<SettingsStore, _>(|store, cx| {
                        store.set_user_settings(&content, cx)
                    });
                    if let Err(error) = result.result() {
                        log::error!(
                            "failed to load native settings from {}: {error:#}",
                            settings_path.display()
                        );
                    }
                    apply_notes_settings(notes_settings, cx);
                });
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
    let version = Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("Cargo package version should be valid semver");
    release_channel::init(version, cx);
    gpui_tokio::init(cx);
    settings::init(cx);
    theme_settings::init(LoadThemes::All(Box::new(Assets)), cx);

    if let Err(error) = std::fs::create_dir_all(config_dir()) {
        log::error!("failed to create zednotes config directory: {error}");
    }
    match migrate_split_settings_file() {
        Ok(true) => log::info!(
            "merged the former Application Support settings into {}",
            notes_settings_path().display()
        ),
        Ok(false) => {}
        Err(error) => log::error!("failed to merge split zednotes settings: {error:#}"),
    }
    match migrate_legacy_settings_file() {
        Ok(true) => log::info!(
            "migrated {} to the native Zed settings schema",
            notes_settings_path().display()
        ),
        Ok(false) => {}
        Err(error) => log::error!("failed to migrate legacy zednotes settings: {error:#}"),
    }
    let notes_settings = load_notes_settings();
    apply_notes_settings(notes_settings.clone(), cx);

    let fs: Arc<dyn Fs> = Arc::new(RealFs::new(None, cx.background_executor().clone()));
    <dyn Fs>::set_global(fs.clone(), cx);
    cx.update_global::<SettingsStore, _>(|store, cx| {
        store.watch_settings_files(fs.clone(), cx, |settings_file, result, _| {
            if let Err(error) = result.result() {
                log::error!("failed to load {settings_file:?}: {error:#}");
            }
        });
    });
    cx.set_http_client(Arc::new(BlockedHttpClient::new()));
    load_recent_notes(cx);

    let client = Client::production(cx);
    Client::set_global(client.clone(), cx);
    client::init(&client, cx);
    Project::init(&client, cx);

    let languages = markdown_languages(cx);
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
        let details = cx.new(|_| NotesStatusDetails::new());
        let cursor = cx.new(|cx| NotesCursorStatus::new(details.clone(), cx));
        let mode_indicator = cx.new(|cx| ModeIndicator::new(window, cx));
        status_bar.add_right_item(mode_indicator, window, cx);
        status_bar.add_right_item(details, window, cx);
        status_bar.add_right_item(cursor, window, cx);
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
            let image_view_toolbar = cx.new(|_| ImageViewToolbarControls::new());
            toolbar.add_item(image_view_toolbar, window, cx);
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
                if let Some(active_path) = active_path.filter(|path| is_markdown_path(path)) {
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
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(9.), px(9.))),
        }),
        app_owns_titlebar_drag: true,
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
    use image_viewer::ImageView;
    use project::ProjectPath;
    use serde_json::{Value, json};
    use settings::SettingsStore;
    use std::{path::Path, time::Duration};
    use util::rel_path::rel_path;

    const TEST_NOTE_PATH: &str = "/notes/spike.md";
    const TEST_NOTE: &str = "# Spike\n\n- **fast** editing\n";
    const TEST_TRACKED_NOTE: &str = "---\ntitle: \"Spike\"\ndate: 2000-01-01T00:00:00+08:00\nlastUpdated: 2000-01-01T00:00:00+08:00\n---\n\n# Spike\n";

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
            release_channel::init(Version::new(0, 0, 0), cx);
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
        project.read_with(cx, |project, cx| {
            register_built_in_languages(project.languages());
            project.languages().set_theme(cx.theme().clone());
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

    fn active_buffer(test: &TestWindow, cx: &TestAppContext) -> gpui::Entity<Buffer> {
        active_editor(test, cx).read_with(cx, |editor, cx| {
            editor
                .buffer()
                .read(cx)
                .as_singleton()
                .expect("the active editor should have one buffer")
        })
    }

    fn append_to_active_buffer(test: &TestWindow, text: &str, cx: &mut TestAppContext) {
        active_buffer(test, cx).update(cx, |buffer, cx| {
            let end = buffer.snapshot().text().len();
            buffer.start_transaction();
            buffer.edit([(end..end, text)], None, cx);
            buffer.end_transaction(cx);
        });
    }

    fn last_updated_value(content: &str) -> &str {
        let (range, _) = frontmatter_last_updated_range(content)
            .expect("the note should have a lastUpdated front matter field");
        &content[range]
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

    #[test]
    fn test_note_statistics_count_words_and_unicode_characters() {
        let text = "# Hello, **world**\n\n你好 world\n";
        assert_eq!(
            note_statistics(text),
            NoteStatistics {
                words: 4,
                characters: text.chars().count(),
            }
        );
        assert_eq!(
            note_statistics_message(text),
            format!("4 words · {} characters", text.chars().count())
        );
    }

    #[test]
    fn test_notes_menu_bar_has_only_the_minimal_sections() {
        assert_eq!(
            notes_menus()
                .into_iter()
                .map(|menu| menu.name.to_string())
                .collect::<Vec<_>>(),
            [APP_NAME, "File", "Edit", "View", "Go", "Window"]
        );
    }

    #[gpui::test]
    fn test_notes_windows_embed_controls_in_the_macos_titlebar(cx: &mut TestAppContext) {
        let titlebar = cx
            .update(|cx| notes_window_options(None, cx))
            .titlebar
            .expect("a notes window must have a titlebar");

        assert!(
            titlebar.appears_transparent,
            "project and Git controls should share the macOS titlebar"
        );
        assert!(titlebar.title.is_none());
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
    fn test_logs_are_isolated_from_upstream_zed() {
        assert_eq!(
            logs_dir(),
            &paths::home_dir()
                .join("Library")
                .join("Logs")
                .join("zednotes")
        );
        assert_eq!(log_file(), &logs_dir().join("zednotes.log"));
        assert_eq!(old_log_file(), &logs_dir().join("zednotes.log.old"));
        assert!(!log_file().starts_with(paths::logs_dir()));
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
    async fn test_recent_note_and_project_pickers_open(cx: &mut TestAppContext) {
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
                .active_modal::<recent_projects::RecentProjects>(cx)
                .is_some()),
            "Workspace: Open Recent should open Zed's recent-projects picker"
        );
        test.cx.simulate_keystrokes("escape");
        test.cx.simulate_keystrokes("alt-cmd-o");
        test.cx.run_until_parked();
        assert!(
            workspace(&test, cx).read_with(cx, |workspace, cx| workspace
                .active_modal::<recent_projects::RecentProjects>(cx)
                .is_some()),
            "Zed's default project-switcher shortcut should remain available"
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
                    zed_workspace::OpenMode::Activate,
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
    async fn test_hot_exit_restores_unsaved_note_without_writing_it_to_disk(
        cx: &mut TestAppContext,
    ) {
        use session::Session;

        const NOTE_PATH: &str = "/hot-exit-notes/draft.md";
        const SAVED_NOTE: &str = "# Saved on disk\n";
        const UNSAVED_NOTE: &str = "# Unsaved recovery copy\n\nStill being written.\n";

        let test = test_window(cx).await;
        let app_state = cx.read(AppState::global);
        test.window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to remove the setup window");
        test.fs
            .insert_tree("/hot-exit-notes", json!({ "draft.md": SAVED_NOTE }))
            .await;
        cx.run_until_parked();

        let mut notes_settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        notes_settings.autosave.enabled = false;
        cx.update(|cx| apply_notes_settings(notes_settings, cx));

        let session_id = cx.read(|cx| app_state.session.read(cx).id().to_owned());
        let zed_workspace::OpenResult { window, .. } = cx
            .update(|cx| {
                Workspace::new_local(
                    vec![PathBuf::from("/hot-exit-notes")],
                    app_state.clone(),
                    None,
                    None,
                    None,
                    zed_workspace::OpenMode::Activate,
                    cx,
                )
            })
            .await
            .expect("failed to open the hot-exit workspace");
        let open_note = window
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.workspace().update(cx, |workspace, cx| {
                    workspace.open_abs_path(
                        PathBuf::from(NOTE_PATH),
                        OpenOptions::default(),
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to schedule the draft open");
        open_note.await.expect("failed to open the draft");
        cx.run_until_parked();

        window
            .update(cx, |multi_workspace, window, cx| {
                let editor = multi_workspace
                    .workspace()
                    .read(cx)
                    .active_item_as::<Editor>(cx)
                    .expect("the draft should be active");
                editor.update(cx, |editor, cx| {
                    editor.set_text(UNSAVED_NOTE, window, cx);
                });
            })
            .expect("failed to edit the draft");
        cx.run_until_parked();
        assert_eq!(
            test.fs.load(Path::new(NOTE_PATH)).await.unwrap(),
            SAVED_NOTE,
            "the recovery checkpoint must not become an implicit save"
        );

        cx.executor()
            .advance_clock(zed_workspace::SERIALIZATION_THROTTLE_TIME);
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("failed to simulate the terminated window");
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
                .expect("failed to restore the hot-exit session"),
            1
        );
        cx.run_until_parked();

        let restored_window = cx
            .read(|cx| {
                cx.windows()
                    .into_iter()
                    .find_map(|window| window.downcast::<MultiWorkspace>())
            })
            .expect("the restored hot-exit window should exist");
        restored_window
            .read_with(cx, |multi_workspace, cx| {
                let editor = multi_workspace
                    .workspace()
                    .read(cx)
                    .active_item_as::<Editor>(cx)
                    .expect("the recovered draft should be active");
                assert_eq!(editor.read(cx).text(cx), UNSAVED_NOTE);
                assert!(
                    editor
                        .read(cx)
                        .buffer()
                        .read(cx)
                        .as_singleton()
                        .is_some_and(|buffer| buffer.read(cx).is_dirty()),
                    "the recovered contents should remain visibly unsaved"
                );
            })
            .expect("failed to inspect the recovered draft");
        assert_eq!(
            test.fs.load(Path::new(NOTE_PATH)).await.unwrap(),
            SAVED_NOTE,
            "restoring the checkpoint must leave the filesystem authoritative"
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
    async fn test_relative_line_number_settings_follow_vim_mode(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let mut settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        settings.relative_line_numbers = RelativeLineNumbers::Enabled;
        settings.vim.toggle_relative_line_numbers = true;

        cx.update(|cx| apply_notes_settings(settings, cx));
        test.cx.run_until_parked();
        assert_eq!(
            test.editor
                .read_with(cx, |editor, cx| editor.relative_line_numbers(cx)),
            RelativeLineNumbers::Enabled
        );

        test.cx.simulate_keystrokes("i");
        assert_eq!(
            test.editor
                .read_with(cx, |editor, cx| editor.relative_line_numbers(cx)),
            RelativeLineNumbers::Disabled
        );

        test.cx.simulate_keystrokes("escape");
        assert_eq!(
            test.editor
                .read_with(cx, |editor, cx| editor.relative_line_numbers(cx)),
            RelativeLineNumbers::Enabled
        );
    }

    #[gpui::test]
    async fn test_notes_status_bar_tracks_counts_cursor_and_save_state(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.cx.run_until_parked();
        test.cx.update(|window, cx| window.draw(cx).clear(cx));
        let project_switcher_bounds = test
            .cx
            .debug_bounds("notes-project-switcher")
            .expect("the project switcher should be rendered in the title bar");
        assert!(
            project_switcher_bounds.origin.x >= px(ui::utils::TRAFFIC_LIGHT_PADDING),
            "the project switcher should start after the macOS traffic lights"
        );
        let workspace = workspace(&test, cx);
        workspace.read_with(cx, |workspace, cx| {
            assert!(
                workspace
                    .titlebar_item()
                    .is_some_and(|item| item.downcast::<NotesTitleBar>().is_ok()),
                "notes workspaces should install the project and Git title bar"
            );
            assert!(workspace.panel::<ProjectPanel>(cx).is_some());
            assert!(workspace.panel::<GitPanel>(cx).is_some());
            assert!(workspace.panel::<OutlinePanel>(cx).is_some());
            let left_dock = workspace.left_dock().read(cx);
            assert_eq!(left_dock.panel_index_for_type::<ProjectPanel>(), Some(0));
            assert_eq!(left_dock.panel_index_for_type::<GitPanel>(), Some(1));
            assert_eq!(left_dock.panel_index_for_type::<OutlinePanel>(), Some(2));
            assert!(
                workspace
                    .right_dock()
                    .read(cx)
                    .panel::<GitPanel>()
                    .is_none()
            );
            assert!(
                workspace
                    .right_dock()
                    .read(cx)
                    .panel::<OutlinePanel>()
                    .is_none()
            );
        });
        let status_bar = workspace.read_with(cx, |workspace, _| workspace.status_bar().clone());
        let details = status_bar.read_with(cx, |status_bar, _| {
            assert!(status_bar.item_of_type::<PanelButtons>().is_some());
            assert!(status_bar.position_of_item::<ModeIndicator>().is_some());
            assert!(
                status_bar
                    .position_of_item::<NotesStatusDetails>()
                    .is_some()
            );
            assert!(status_bar.position_of_item::<NotesCursorStatus>().is_some());
            status_bar
                .item_of_type::<NotesStatusDetails>()
                .expect("notes details should be on the right")
        });
        assert_eq!(
            details.read_with(cx, |details, _| details.snapshot.clone()),
            Some(NotesStatusSnapshot {
                language: "Markdown".to_owned(),
                statistics: note_statistics(TEST_NOTE),
                line: 1,
                column: 1,
                save_state: NoteSaveState::Saved,
            })
        );

        test.cx.simulate_keystrokes("i");
        test.cx.simulate_input("two words ");
        test.cx.simulate_keystrokes("escape");
        test.cx.run_until_parked();

        let snapshot = details
            .read_with(cx, |details, _| details.snapshot.clone())
            .expect("status should remain attached to the editor");
        assert_eq!(
            snapshot.statistics.words,
            note_statistics(TEST_NOTE).words + 2
        );
        assert_eq!(
            snapshot.statistics.characters,
            note_statistics(TEST_NOTE).characters + "two words ".chars().count()
        );
        assert_eq!(snapshot.line, 1);
        assert!(snapshot.column > 1);
        assert_eq!(snapshot.save_state, NoteSaveState::Modified);
    }

    #[gpui::test]
    async fn test_cmd_s_saves_and_returns_to_normal_mode(cx: &mut TestAppContext) {
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

        test.cx.simulate_keystrokes("i x cmd-s");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
        test.cx.simulate_keystrokes("x");
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            TEST_NOTE,
            "Cmd+S should return the editor to Normal mode"
        );
    }

    #[gpui::test]
    async fn test_jk_saves_without_persisting_the_pending_j_and_returns_to_normal_mode(
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

        test.cx.simulate_keystrokes("i x j k");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}"),
            "the pending j should be removed before saving"
        );
        test.cx.simulate_keystrokes("x");
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            TEST_NOTE,
            "jk should return the editor to Normal mode"
        );
    }

    #[test]
    fn test_legacy_parser_reads_autosave_settings_from_jsonc() {
        assert_eq!(
            parse_legacy_notes_settings("{}")
                .expect("default settings should parse")
                .autosave,
            AutosaveSettings {
                enabled: true,
                delay_ms: 750,
            }
        );

        let settings = parse_legacy_notes_settings(
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
    fn test_legacy_parser_reads_which_key_settings_from_jsonc() {
        assert_eq!(
            parse_legacy_notes_settings("{}")
                .expect("default settings should parse")
                .which_key,
            NotesWhichKeySettings {
                enabled: true,
                delay_ms: DEFAULT_WHICH_KEY_DELAY_MS,
            }
        );

        assert_eq!(
            parse_legacy_notes_settings(
                r#"{
                    "which_key": {
                        "enabled": false,
                        "delay_ms": 900,
                    },
                }"#,
            )
            .expect("which-key settings should parse")
            .which_key,
            NotesWhichKeySettings {
                enabled: false,
                delay_ms: 900,
            }
        );
    }

    #[test]
    fn test_legacy_parser_reads_editor_theme_vim_and_explorer_width_settings() {
        let settings = parse_legacy_notes_settings(
            r#"
            {
                "theme": "dark",
                "vim_mode": false,
                "relative_line_numbers": "wrapped",
                "vim": {
                    "leader": "ctrl-space",
                    "toggle_relative_line_numbers": true,
                },
                "ui": {
                    "font_family": "Test Sans",
                    "font_size": 13.5,
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
                "preview": {
                    "font_family": "Test Serif",
                    "font_size": 17,
                },
            }
            "#,
        )
        .expect("settings should parse");

        assert_eq!(settings.theme, NotesTheme::Dark);
        assert!(!settings.vim_mode);
        assert_eq!(settings.relative_line_numbers, RelativeLineNumbers::Wrapped);
        assert_eq!(settings.vim.leader, "ctrl-space");
        assert!(settings.vim.toggle_relative_line_numbers);
        assert_eq!(settings.ui.font_family, "Test Sans");
        assert_eq!(settings.ui.font_size, 13.5);
        assert_eq!(settings.editor.font_family, "Test Mono");
        assert_eq!(settings.editor.font_size, 18.5);
        assert_eq!(settings.editor.line_height, 1.4);
        assert!(settings.editor.line_numbers);
        assert!(!settings.editor.soft_wrap);
        assert_eq!(settings.explorer.width, 320);
        assert_eq!(settings.preview.font_family, "Test Serif");
        assert_eq!(settings.preview.font_size, 17.0);
        assert_eq!(settings.autosave, AutosaveSettings::default());
    }

    #[test]
    fn test_invalid_legacy_settings_are_rejected_for_migration() {
        assert!(parse_legacy_notes_settings("{ invalid").is_err());
        assert!(parse_legacy_notes_settings(r#"{ "ui": { "font_size": 0 } }"#).is_err());
        assert!(parse_legacy_notes_settings(r#"{ "editor": { "line_height": 0.5 } }"#).is_err());
        assert!(parse_legacy_notes_settings(r#"{ "preview": { "font_family": "" } }"#).is_err());
        assert!(parse_legacy_notes_settings(r#"{ "vim": { "leader": "" } }"#).is_err());
        assert!(parse_legacy_notes_settings(r#"{ "vim": { "leader": "g g" } }"#).is_err());
        assert!(parse_legacy_notes_settings(r#"{ "vim": { "leader": "ctrl-alt-x-y" } }"#).is_err());
    }

    #[test]
    fn test_reads_only_zednotes_policy_from_native_settings() {
        let settings = parse_notes_settings(
            r#"{
                "vim_mode": false,
                "buffer_font_size": 22,
                "zednotes": {
                    "vim": { "leader": "ctrl-space" },
                    "explorer": { "markdown_only": false },
                    "daily_notes": {
                        "directory": "log",
                        "filename_format": "%Y%m%d.md",
                        "template": "templates/log.md",
                    },
                },
            }"#,
        )
        .expect("native settings with zednotes policy should parse");

        assert_eq!(settings.vim.leader, "ctrl-space");
        assert!(!settings.explorer.markdown_only);
        assert_eq!(settings.daily_notes.directory, "log");
        assert_eq!(settings.daily_notes.filename_format, "%Y%m%d.md");
        assert_eq!(
            settings.daily_notes.template.as_deref(),
            Some("templates/log.md")
        );
        assert!(
            settings.vim_mode,
            "standard settings are owned by SettingsStore rather than the notes policy parser"
        );
        assert_eq!(settings.editor.font_size, DEFAULT_EDITOR_FONT_SIZE);
        assert!(parse_notes_settings(r#"{ "zednotes": { "vim": { "leader": "g g" } } }"#).is_err());
    }

    #[test]
    fn test_daily_note_path_and_template_placeholders() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 29).unwrap();
        let settings = DailyNotesSettings {
            directory: "journal".to_owned(),
            filename_format: "%Y-%m-%d.md".to_owned(),
            template: None,
        };
        let path = daily_note_relative_path(&settings, date).unwrap();

        assert_eq!(path, PathBuf::from("journal/2026-08-29.md"));
        assert_eq!(
            render_daily_note_template(
                "---\ntitle: \"{{title}}\"\ndate: {{datetime}}\nlastUpdated: {{datetime}}\n---\n\nDate: {{date}}\n",
                date,
                "2026-08-29T10:01:02+08:00",
                &path,
            ),
            "---\ntitle: \"2026-08-29\"\ndate: 2026-08-29T10:01:02+08:00\nlastUpdated: 2026-08-29T10:01:02+08:00\n---\n\nDate: 2026-08-29\n"
        );
    }

    #[test]
    fn test_daily_note_settings_cannot_escape_the_notes_folder() {
        for content in [
            r#"{ "zednotes": { "daily_notes": { "directory": "../outside" } } }"#,
            r#"{ "zednotes": { "daily_notes": { "filename_format": "%Y/%m/%d.md" } } }"#,
            r#"{ "zednotes": { "daily_notes": { "filename_format": "%Q.md" } } }"#,
            r#"{ "zednotes": { "daily_notes": { "filename_format": "%Y.txt" } } }"#,
            r#"{ "zednotes": { "daily_notes": { "template": "/tmp/daily.md" } } }"#,
        ] {
            assert!(
                parse_notes_settings(content).is_err(),
                "{content} should fail"
            );
        }
    }

    #[test]
    fn test_migrates_legacy_settings_to_native_schema_and_zednotes_namespace() {
        let migrated = migrate_legacy_settings_content(
            r#"{
                // Preserve unrelated user-owned keys during the one-time migration.
                "custom_unknown": { "value": 7 },
                "theme": "dark",
                "vim_mode": false,
                "relative_line_numbers": "wrapped",
                "vim": {
                    "leader": "ctrl-space",
                    "toggle_relative_line_numbers": true,
                },
                "which_key": { "enabled": false, "delay_ms": 900 },
                "ui": { "font_family": "Test Sans", "font_size": 13.5 },
                "editor": {
                    "font_family": "Test Mono",
                    "font_size": 18.5,
                    "line_height": 1.4,
                    "line_numbers": true,
                    "soft_wrap": false,
                },
                "autosave": { "enabled": true, "delay_ms": 1250 },
                "explorer": {
                    "width": 320,
                    "markdown_only": false,
                    "show_hidden": true,
                },
                "files": { "exclude": ["drafts", "**/*.tmp"] },
                "preview": {
                    "font_family": "Test Serif",
                    "font_size": 17,
                    "max_width": 680,
                    "allow_remote_images": true,
                    "max_file_size_bytes": 2048,
                },
                "daily_notes": {
                    "directory": "log",
                    "filename_format": "%Y%m%d.md",
                    "template": "templates/log.md",
                },
            }"#,
        )
        .expect("legacy settings should be migratable")
        .expect("legacy settings should require migration");
        let document: serde_json::Value = serde_json::from_str(&migrated).unwrap();

        assert_eq!(document["custom_unknown"]["value"], 7);
        assert_eq!(document["theme"]["mode"], "dark");
        assert_eq!(document["ui_font_family"], "Test Sans");
        assert_eq!(document["buffer_font_family"], "Test Mono");
        assert_eq!(document["buffer_line_height"]["custom"], 1.4);
        assert_eq!(document["gutter"]["line_numbers"], true);
        assert_eq!(document["soft_wrap"], "none");
        assert_eq!(document["autosave"]["after_delay"]["milliseconds"], 1250);
        assert_eq!(document["project_panel"]["default_width"], 320);
        assert_eq!(document["project_panel"]["hide_hidden"], false);
        assert_eq!(document["file_scan_exclusions"][0], "**/drafts");
        assert_eq!(document["file_scan_exclusions"][1], "**/*.tmp");
        assert_eq!(document["markdown_preview"]["max_width"], 680);
        assert_eq!(document["zednotes"]["vim"]["leader"], "ctrl-space");
        assert_eq!(document["zednotes"]["explorer"]["markdown_only"], false);
        assert_eq!(document["zednotes"]["daily_notes"]["directory"], "log");
        assert_eq!(
            document["zednotes"]["daily_notes"]["filename_format"],
            "%Y%m%d.md"
        );
        assert_eq!(
            document["zednotes"]["daily_notes"]["template"],
            "templates/log.md"
        );
        for legacy_key in [
            "ui",
            "editor",
            "explorer",
            "files",
            "preview",
            "daily_notes",
        ] {
            assert!(document.get(legacy_key).is_none());
        }
        settings::parse_json_with_comments::<settings::UserSettingsContent>(&migrated)
            .expect("migrated settings should be accepted by the native Zed settings parser");
    }

    #[test]
    fn test_merges_former_gui_settings_into_the_canonical_file() {
        let merged = merge_split_settings_contents(
            Some(
                r#"{
                    "editor": { "font_size": 15 },
                    "vim": { "leader": "ctrl-space" },
                    "explorer": { "markdown_only": false },
                }"#,
            ),
            r#"{
                "buffer_font_size": 22,
                "theme": {
                    "mode": "dark",
                    "light": "One Light",
                    "dark": "One Dark",
                },
            }"#,
            true,
        )
        .expect("split settings should merge");
        let document: serde_json::Value = serde_json::from_str(&merged).unwrap();

        assert_eq!(document["buffer_font_size"], 22);
        assert_eq!(document["theme"]["mode"], "dark");
        assert_eq!(document["zednotes"]["vim"]["leader"], "ctrl-space");
        assert_eq!(document["zednotes"]["explorer"]["markdown_only"], false);
        assert!(document.get("editor").is_none());
        assert!(document.get("explorer").is_none());
    }

    #[test]
    fn test_native_initial_settings_do_not_need_migration() {
        assert!(
            migrate_legacy_settings_content(INITIAL_SETTINGS_CONTENT)
                .expect("initial settings should parse")
                .is_none()
        );
        settings::parse_json_with_comments::<settings::UserSettingsContent>(
            INITIAL_SETTINGS_CONTENT,
        )
        .expect("initial settings should use the native Zed schema");
    }

    #[gpui::test]
    async fn test_applying_settings_updates_editor_theme_vim_and_explorer(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let settings = NotesSettings {
            theme: NotesTheme::Dark,
            vim_mode: false,
            relative_line_numbers: RelativeLineNumbers::Wrapped,
            vim: NotesVimSettings {
                toggle_relative_line_numbers: true,
                ..NotesVimSettings::default()
            },
            ui: NotesUiSettings {
                font_family: "Test Sans".to_owned(),
                font_size: 13.0,
            },
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
            preview: PreviewSettings {
                font_family: "Test Serif".to_owned(),
                font_size: 17.0,
                ..PreviewSettings::default()
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
            relative_line_numbers,
            toggle_relative_line_numbers,
            soft_wrap,
            explorer_width,
            ui_font_family,
            ui_font_size,
            preview_font_family,
            preview_font_size,
            minimap,
            automatic_completions,
            word_completions,
            lsp_completions,
            language_server,
            git_status,
            git_diff,
        ) = cx.update(|cx| {
            let theme = theme_settings::ThemeSettings::get_global(cx);
            (
                theme.buffer_font.family.to_string(),
                theme.buffer_font_size(cx),
                theme.buffer_line_height.value(),
                theme.theme.mode(),
                editor::EditorSettings::get_global(cx).gutter.line_numbers,
                editor::EditorSettings::get_global(cx).relative_line_numbers,
                cx.global::<SettingsStore>()
                    .raw_default_settings()
                    .vim
                    .as_ref()
                    .and_then(|vim| vim.toggle_relative_line_numbers),
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .soft_wrap,
                ProjectPanelSettings::get_global(cx).default_width,
                theme.ui_font.family.to_string(),
                theme.ui_font_size(cx),
                theme.markdown_preview_font_family().to_string(),
                theme.markdown_preview_font_size(cx),
                editor::EditorSettings::get_global(cx).minimap.show,
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .show_completions_on_input,
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .completions
                    .words,
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .completions
                    .lsp,
                language::language_settings::AllLanguageSettings::get_global(cx)
                    .defaults
                    .enable_language_server,
                project::project_settings::ProjectSettings::get_global(cx)
                    .git
                    .enabled
                    .status,
                project::project_settings::ProjectSettings::get_global(cx)
                    .git
                    .enabled
                    .diff,
            )
        });
        assert_eq!(font_family, "Test Mono");
        assert_eq!(font_size, px(18.0));
        assert_eq!(line_height, 1.4);
        assert_eq!(theme_mode, Some(ThemeAppearanceMode::Dark));
        assert!(line_numbers);
        assert_eq!(relative_line_numbers, RelativeLineNumbers::Wrapped);
        assert_eq!(toggle_relative_line_numbers, Some(true));
        assert_eq!(soft_wrap, SoftWrap::None);
        assert_eq!(explorer_width, px(320.0));
        assert_eq!(ui_font_family, "Test Sans");
        assert_eq!(ui_font_size, px(13.0));
        assert_eq!(preview_font_family, "Test Serif");
        assert_eq!(preview_font_size, px(17.0));
        assert_eq!(minimap, ShowMinimap::Never);
        assert!(!automatic_completions);
        assert_eq!(word_completions, WordsCompletionMode::Disabled);
        assert!(!lsp_completions);
        assert!(!language_server);
        assert!(git_status);
        assert!(git_diff);
        assert!(workspace(&test, cx).read_with(cx, |workspace, cx| {
            workspace.panel::<GitPanel>(cx).is_some()
        }));

        test.cx.simulate_keystrokes("x");
        assert_eq!(
            test.editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}"),
            "disabling Vim should make normal text input insert immediately"
        );
    }

    #[gpui::test]
    async fn test_editor_font_size_shortcuts(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let font_size = |cx: &mut TestAppContext| {
            cx.update(|cx| theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx))
        };
        let initial_font_size = font_size(cx);

        test.cx.simulate_keystrokes("cmd-+");
        assert_eq!(font_size(cx), initial_font_size + px(1.0));

        test.cx.simulate_keystrokes("cmd--");
        assert_eq!(font_size(cx), initial_font_size);

        test.cx.simulate_keystrokes("cmd-=");
        assert_eq!(font_size(cx), initial_font_size + px(1.0));

        test.cx.simulate_keystrokes("cmd-0");
        assert_eq!(font_size(cx), initial_font_size);
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
        let config_dir = config_dir();
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
                &r#"{ "gutter": { "line_numbers": true } }"#.into(),
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
    async fn test_cmd_comma_opens_settings_window(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-,");
        test.cx.run_until_parked();

        assert!(cx.read(|cx| {
            cx.windows()
                .iter()
                .any(|window| window.downcast::<settings_ui::SettingsWindow>().is_some())
        }));
        assert_eq!(active_editor_path(&test, cx), PathBuf::from(TEST_NOTE_PATH));
    }

    #[gpui::test]
    async fn test_cmd_k_cmd_s_opens_full_keymap_editor(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.update(|window, _| window.activate_window());
        test.cx.simulate_keystrokes("cmd-k cmd-s");
        test.cx.run_until_parked();

        let workspace = workspace(&test, cx);
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| {
                workspace
                    .active_item(cx)
                    .expect("keymap editor should be active")
                    .tab_content_text(0, cx)
            }),
            "Keymap Editor"
        );
    }

    #[gpui::test]
    async fn test_cmd_alt_comma_creates_and_opens_settings_file(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let settings_path = notes_settings_path();
        assert!(!test.fs.is_file(&settings_path).await);

        test.cx.simulate_keystrokes("cmd-alt-,");
        test.cx.run_until_parked();

        assert_eq!(active_editor_path(&test, cx), settings_path);
        assert_eq!(
            test.fs.load(&notes_settings_path()).await.unwrap(),
            INITIAL_SETTINGS_CONTENT
        );
    }

    #[gpui::test]
    async fn test_open_keymap_file_action_creates_the_canonical_keymap(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let keymap_path = notes_keymap_path();
        assert!(!test.fs.is_file(&keymap_path).await);

        test.cx.dispatch_action(zed_actions::OpenKeymapFile);
        test.cx.run_until_parked();

        assert_eq!(active_editor_path(&test, cx), keymap_path);
        assert_eq!(
            test.fs.load(&notes_keymap_path()).await.unwrap(),
            settings::initial_keymap_content()
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
            "note::Open Today",
            "note::NewDirectory",
            "note::Rename",
            "note::Delete",
            "note::Duplicate",
            "note::Statistics",
            "outline::Toggle",
            "tab_switcher::ToggleAll",
            "editor::ToggleBookmark",
            "editor::ToggleBookmarkWithLabel",
            "editor::EditBookmark",
            "editor::GoToNextBookmark",
            "editor::GoToPreviousBookmark",
            "editor::ViewBookmarks",
            "view::ToggleFocusMode",
            "view::UseSystemTheme",
            "view::UseLightTheme",
            "view::UseDarkTheme",
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
    async fn test_open_today_shortcut_creates_note_from_template_and_enters_insert_mode(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;
        test.fs
            .create_dir(Path::new("/notes/templates"))
            .await
            .unwrap();
        test.fs
            .save(
                Path::new("/notes/templates/daily.md"),
                &"---\ntitle: \"{{title}}\"\ndate: {{datetime}}\nlastUpdated: {{datetime}}\n---\n\nStart here: ".into(),
                Default::default(),
            )
            .await
            .unwrap();
        let mut settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        settings.daily_notes.filename_format = "today.md".to_owned();
        settings.daily_notes.template = Some("templates/daily.md".to_owned());
        cx.update(|cx| apply_notes_settings(settings, cx));

        test.cx.simulate_keystrokes("cmd-shift-j");
        test.cx.run_until_parked();

        let note_path = Path::new("/notes/journal/today.md");
        let content = test.fs.load(note_path).await.unwrap();
        let lines = content.lines().collect::<Vec<_>>();
        assert_eq!(lines.first().copied(), Some("---"));
        assert_eq!(lines.get(1).copied(), Some("title: \"today\""));
        let created_at = lines
            .get(2)
            .and_then(|line| line.strip_prefix("date: "))
            .expect("the template should render its creation timestamp");
        let last_updated = lines
            .get(3)
            .and_then(|line| line.strip_prefix("lastUpdated: "))
            .expect("the template should render its update timestamp");
        assert_eq!(created_at, last_updated);
        chrono::DateTime::parse_from_rfc3339(created_at)
            .expect("the rendered timestamp should include its UTC offset");
        assert_eq!(lines.get(4).copied(), Some("---"));
        assert!(content.ends_with("\n\nStart here: "));
        assert_eq!(active_editor_path(&test, cx), note_path);
        test.cx.simulate_keystrokes("x");
        assert_eq!(
            active_editor(&test, cx).read_with(cx, |editor, cx| editor.text(cx)),
            format!("{content}x")
        );
    }

    #[gpui::test]
    async fn test_open_today_does_not_overwrite_an_existing_note(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.fs
            .create_dir(Path::new("/notes/journal"))
            .await
            .unwrap();
        let note_path = Path::new("/notes/journal/today.md");
        test.fs
            .save(note_path, &"Existing entry\n".into(), Default::default())
            .await
            .unwrap();
        let mut settings = cx.update(|cx| cx.global::<CurrentNotesSettings>().0.clone());
        settings.daily_notes.filename_format = "today.md".to_owned();
        cx.update(|cx| apply_notes_settings(settings, cx));

        test.cx.dispatch_action(OpenToday);
        test.cx.run_until_parked();

        assert_eq!(test.fs.load(note_path).await.unwrap(), "Existing entry\n");
        assert_eq!(active_editor_path(&test, cx), note_path);
    }

    #[gpui::test]
    async fn test_save_updates_last_updated_when_note_content_changed(cx: &mut TestAppContext) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            },
            json!({ "spike.md": TEST_TRACKED_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        append_to_active_buffer(&test, "New thought\n", cx);
        test.cx.simulate_keystrokes("cmd-s");
        test.cx.run_until_parked();

        let saved = test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap();
        let updated_at = last_updated_value(&saved);
        assert_ne!(updated_at, "2000-01-01T00:00:00+08:00");
        chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("lastUpdated should remain an RFC 3339 timestamp");
        assert!(saved.ends_with("# Spike\nNew thought\n"));
    }

    #[gpui::test]
    async fn test_clean_save_does_not_update_last_updated(cx: &mut TestAppContext) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            },
            json!({ "spike.md": TEST_TRACKED_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        test.cx.simulate_keystrokes("cmd-s");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            TEST_TRACKED_NOTE
        );
    }

    #[gpui::test]
    async fn test_save_after_undoing_all_changes_does_not_update_last_updated(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: false,
                delay_ms: DEFAULT_AUTOSAVE_DELAY_MS,
            },
            json!({ "spike.md": TEST_TRACKED_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        append_to_active_buffer(&test, "Temporary thought\n", cx);
        active_buffer(&test, cx).update(cx, |buffer, cx| {
            buffer.undo(cx);
            assert!(!buffer.is_dirty());
        });
        test.cx.simulate_keystrokes("cmd-s");
        test.cx.run_until_parked();

        assert_eq!(
            test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            TEST_TRACKED_NOTE
        );
    }

    #[gpui::test]
    async fn test_autosave_updates_last_updated_when_note_content_changed(cx: &mut TestAppContext) {
        let test = test_window_with_settings(
            cx,
            ExplorerSettings::default(),
            AutosaveSettings {
                enabled: true,
                delay_ms: 2500,
            },
            json!({ "spike.md": TEST_TRACKED_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;

        append_to_active_buffer(&test, "Autosaved thought\n", cx);
        test.cx
            .executor()
            .advance_clock(Duration::from_millis(2500));
        test.cx.run_until_parked();

        let saved = test.fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap();
        let updated_at = last_updated_value(&saved);
        assert_ne!(updated_at, "2000-01-01T00:00:00+08:00");
        chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("lastUpdated should remain an RFC 3339 timestamp");
        assert!(saved.ends_with("# Spike\nAutosaved thought\n"));
    }

    #[test]
    fn test_last_updated_field_must_be_unique_and_top_level() {
        let quoted = "---\nlastUpdated: '2000-01-01T00:00:00+08:00'\n---\n";
        let (range, quote) = frontmatter_last_updated_range(quoted).unwrap();
        assert_eq!(&quoted[range], "'2000-01-01T00:00:00+08:00'");
        assert_eq!(quote, Some('\''));

        assert!(frontmatter_last_updated_range("# Note\nlastUpdated: old\n").is_none());
        assert!(frontmatter_last_updated_range("---\n  lastUpdated: nested\n---\n").is_none());
        assert!(
            frontmatter_last_updated_range("---\nlastUpdated: first\nlastUpdated: second\n---\n")
                .is_none()
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
            ("space n d", "note::Open Today"),
            ("space p", "command_palette::Toggle"),
            ("space e", "view::ToggleExplorer"),
            ("space v", "note::TogglePreview"),
            ("space b n", "pane::ActivateNextItem"),
            ("space b p", "pane::ActivatePreviousItem"),
            ("space b d", "pane::CloseActiveItem"),
            ("space b b", "tab_switcher::ToggleAll"),
            ("space o", "outline::Toggle"),
            ("space m m", "editor::ToggleBookmark"),
            ("space m l", "editor::ToggleBookmarkWithLabel"),
            ("space m e", "editor::EditBookmark"),
            ("space m n", "editor::GoToNextBookmark"),
            ("space m p", "editor::GoToPreviousBookmark"),
            ("space m v", "editor::ViewBookmarks"),
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
    async fn test_which_key_opens_for_an_incomplete_notes_leader_group(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        let workspace = workspace(&test, cx);

        test.cx.simulate_keystrokes("space m");
        test.cx
            .executor()
            .advance_clock(Duration::from_millis(DEFAULT_WHICH_KEY_DELAY_MS));
        test.cx.run_until_parked();

        assert!(test.cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| workspace.has_active_modal(window, cx))
        }));
    }

    #[gpui::test]
    async fn test_vim_leader_o_opens_outline_modal(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("space o");
        test.cx.run_until_parked();

        assert!(workspace(&test, cx).read_with(cx, |workspace, cx| {
            workspace.active_modal::<outline::OutlineView>(cx).is_some()
        }));
    }

    #[gpui::test]
    async fn test_vim_leader_bb_opens_global_tab_switcher(cx: &mut TestAppContext) {
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
        test.cx.simulate_keystrokes("enter");
        test.cx.run_until_parked();

        test.cx.simulate_keystrokes("space b b");
        test.cx.run_until_parked();

        assert!(workspace(&test, cx).read_with(cx, |workspace, cx| {
            workspace
                .active_modal::<tab_switcher::TabSwitcher>(cx)
                .is_some()
        }));
    }

    #[gpui::test]
    async fn test_vim_leader_mm_toggles_a_persistent_bookmark(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("space m m");
        test.cx.run_until_parked();

        let bookmark_store = test
            .project
            .read_with(cx, |project, _| project.bookmark_store());
        let bookmarks =
            bookmark_store.read_with(cx, |store, cx| store.all_serialized_bookmarks(cx));
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks.values().next().map(Vec::len), Some(1));
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
    fn test_legacy_parser_reads_explorer_settings_from_jsonc() {
        let settings = parse_legacy_notes_settings(
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
    fn test_legacy_parser_reads_preview_settings_from_jsonc() {
        let settings = parse_legacy_notes_settings(
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
                font_family: DEFAULT_PREVIEW_FONT_FAMILY.to_owned(),
                font_size: DEFAULT_PREVIEW_FONT_SIZE,
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
    async fn test_preview_registry_resolves_built_in_fenced_code_languages(
        cx: &mut TestAppContext,
    ) {
        let test = test_window(cx).await;
        let languages = test
            .project
            .read_with(cx, |project, _| project.languages().clone());

        for (fence_name, expected_language) in [
            ("rust", "Rust"),
            ("rs", "Rust"),
            ("python", "Python"),
            ("py", "Python"),
            ("javascript", "JavaScript"),
            ("js", "JavaScript"),
            ("typescript", "TypeScript"),
            ("ts", "TypeScript"),
            ("bash", "Shell Script"),
            ("json", "JSON"),
            ("yaml", "YAML"),
            ("go", "Go"),
            ("cpp", "C++"),
            ("html", "HTML"),
            ("sql", "SQL"),
            ("postgresql", "SQL"),
            ("xml", "XML"),
            ("xsd", "XML"),
            ("dtd", "DTD"),
            ("toml", "TOML"),
        ] {
            let language = languages
                .language_for_name_or_extension(fence_name)
                .await
                .unwrap_or_else(|error| {
                    panic!("failed to resolve {fence_name:?} code fence: {error}")
                });
            assert_eq!(language.name().as_ref(), expected_language);
            assert!(
                language.grammar().is_some(),
                "{fence_name:?} code fences need a grammar"
            );
        }

        let rust = languages
            .language_for_name_or_extension("rust")
            .await
            .expect("failed to load Rust highlighting");
        let source = "fn main() { let answer = 42; }";
        assert!(
            !rust
                .highlight_text(&language::Rope::from(source), 0..source.len())
                .is_empty(),
            "the bundled Rust queries should produce syntax highlights"
        );

        for (fence_name, source) in [
            ("html", "<main class=\"content\">Hello</main>"),
            ("sql", "SELECT name FROM users WHERE active = TRUE;"),
            ("xml", "<?xml version=\"1.0\"?><note id=\"1\">Hello</note>"),
            ("dtd", "<!ELEMENT note (#PCDATA)>"),
            ("toml", "[package]\nname = \"zednotes\""),
        ] {
            let language = languages
                .language_for_name_or_extension(fence_name)
                .await
                .unwrap_or_else(|error| {
                    panic!("failed to load {fence_name:?} highlighting: {error}")
                });
            assert!(
                !language
                    .highlight_text(&language::Rope::from(source), 0..source.len())
                    .is_empty(),
                "the bundled {fence_name:?} queries should produce syntax highlights"
            );
        }
    }

    #[gpui::test]
    async fn test_cmd_shift_v_opens_preview_in_the_editor_pane(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;

        test.cx.simulate_keystrokes("cmd-shift-v");
        test.cx.run_until_parked();

        let workspace = test
            .window
            .read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone())
            .expect("failed to read the notes window");
        assert_eq!(
            workspace.read_with(cx, |workspace, _| workspace.panes().len()),
            1
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
        workspace.read_with(cx, |workspace, cx| {
            assert!(
                workspace
                    .active_item_as::<MarkdownPreviewView>(cx)
                    .is_some()
            );
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
        test.cx.update(|window, cx| {
            assert!(test.editor.read(cx).focus_handle(cx).is_focused(window));
        });
    }

    #[gpui::test]
    async fn test_opens_png_and_jpeg_in_image_tabs_but_svg_as_text(cx: &mut TestAppContext) {
        let test = test_window_with(
            cx,
            ExplorerSettings {
                markdown_only: false,
                ..ExplorerSettings::default()
            },
            json!({ "spike.md": TEST_NOTE }),
            TEST_NOTE_PATH,
        )
        .await;
        test.fs
            .insert_file(
                "/notes/pixel.png",
                vec![
                    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
                    0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
                    0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44,
                    0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D,
                    0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42,
                    0x60, 0x82,
                ],
            )
            .await;
        test.fs
            .insert_file(
                "/notes/photo.jpg",
                include_bytes!("../../gpui/examples/image/exif-orientation-rotate-180.jpg")
                    .to_vec(),
            )
            .await;
        test.fs
            .insert_file(
                "/notes/diagram.svg",
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20"/></svg>"#
                    .to_vec(),
            )
            .await;
        cx.run_until_parked();

        for path in ["/notes/pixel.png", "/notes/photo.jpg"] {
            test.window
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
                .expect("failed to schedule image open")
                .await
                .unwrap_or_else(|error| panic!("failed to open {path}: {error:#}"));
            cx.run_until_parked();

            test.window
                .read_with(cx, |multi_workspace, cx| {
                    assert!(
                        multi_workspace
                            .workspace()
                            .read(cx)
                            .active_item_as::<ImageView>(cx)
                            .is_some(),
                        "{path} should open in an image tab"
                    );
                })
                .expect("failed to inspect image tab");
        }

        let svg_path = "/notes/diagram.svg";
        test.window
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.workspace().update(cx, |workspace, cx| {
                    workspace.open_abs_path(
                        PathBuf::from(svg_path),
                        OpenOptions::default(),
                        window,
                        cx,
                    )
                })
            })
            .expect("failed to schedule SVG open")
            .await
            .unwrap_or_else(|error| panic!("failed to open {svg_path}: {error:#}"));
        cx.run_until_parked();

        test.window
            .read_with(cx, |multi_workspace, cx| {
                let workspace = multi_workspace.workspace().read(cx);
                assert!(workspace.active_item_as::<ImageView>(cx).is_none());
                assert!(
                    workspace.active_item_as::<Editor>(cx).is_some(),
                    "SVG should open as text, matching Zed"
                );
            })
            .expect("failed to inspect SVG tab");
    }

    #[gpui::test]
    async fn test_focus_mode_restores_explorer_preview_and_chrome_layout(cx: &mut TestAppContext) {
        let mut test = test_window(cx).await;
        test.cx.simulate_keystrokes("cmd-shift-v");
        test.cx.run_until_parked();
        let workspace = workspace(&test, cx);
        let before = workspace.read_with(cx, |workspace, cx| FocusModeLayout {
            open_docks: [
                workspace.left_dock().read(cx).is_open(),
                workspace.bottom_dock().read(cx).is_open(),
                workspace.right_dock().read(cx).is_open(),
            ],
            centered_layout: workspace.centered_layout,
            pane_maximized: workspace.is_pane_maximized(),
            tab_bar_visible: TabBarSettings::get_global(cx).show,
            status_bar_visible: StatusBarSettings::get_global(cx).show,
        });
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );

        test.cx.dispatch_action(ToggleFocusMode);
        test.cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            assert!(workspace.centered_layout);
            assert!(workspace.is_pane_maximized());
            assert!(!workspace.left_dock().read(cx).is_open());
            assert!(!workspace.bottom_dock().read(cx).is_open());
            assert!(!workspace.right_dock().read(cx).is_open());
            assert_eq!(
                workspace.panes().len(),
                2,
                "the preview layout is preserved"
            );
            assert_eq!(
                workspace.items_of_type::<MarkdownPreviewView>(cx).count(),
                1
            );
            assert!(!TabBarSettings::get_global(cx).show);
            assert!(!StatusBarSettings::get_global(cx).show);
        });

        test.cx.dispatch_action(ToggleFocusMode);
        test.cx.run_until_parked();
        let after = workspace.read_with(cx, |workspace, cx| FocusModeLayout {
            open_docks: [
                workspace.left_dock().read(cx).is_open(),
                workspace.bottom_dock().read(cx).is_open(),
                workspace.right_dock().read(cx).is_open(),
            ],
            centered_layout: workspace.centered_layout,
            pane_maximized: workspace.is_pane_maximized(),
            tab_bar_visible: TabBarSettings::get_global(cx).show,
            status_bar_visible: StatusBarSettings::get_global(cx).show,
        });
        assert_eq!(after, before);
        assert_eq!(
            workspace.read_with(cx, |workspace, cx| workspace
                .items_of_type::<MarkdownPreviewView>(cx)
                .count()),
            1
        );
    }

    #[gpui::test]
    async fn test_theme_actions_keep_ui_preview_and_syntax_theme_modes_coordinated(
        cx: &mut TestAppContext,
    ) {
        let mut test = test_window(cx).await;
        for (action, expected) in [
            (UseDarkTheme.boxed_clone(), NotesTheme::Dark),
            (UseLightTheme.boxed_clone(), NotesTheme::Light),
            (UseSystemTheme.boxed_clone(), NotesTheme::System),
        ] {
            test.cx
                .update(|window, cx| window.dispatch_action(action, cx));
            test.cx.run_until_parked();
            cx.update(|cx| {
                let settings = cx.global::<CurrentNotesSettings>();
                let theme = theme_settings::ThemeSettings::get_global(cx);
                let expected_mode = match expected {
                    NotesTheme::System => ThemeAppearanceMode::System,
                    NotesTheme::Light => ThemeAppearanceMode::Light,
                    NotesTheme::Dark => ThemeAppearanceMode::Dark,
                };
                assert_eq!(settings.0.theme, expected);
                assert_eq!(theme.theme.mode(), Some(expected_mode));
                assert_eq!(
                    theme
                        .markdown_preview_theme
                        .as_ref()
                        .and_then(|theme| theme.mode()),
                    Some(expected_mode)
                );
            });
        }
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
