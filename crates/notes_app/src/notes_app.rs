//! zednotes' application layer.
//!
//! Everything notes-specific lives in this crate rather than in the generic Zed
//! crates of the fork, so that merges from `upstream` stay cheap.

use std::{io::ErrorKind, path::Path, path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use assets::Assets;
use client::{Client, UserStore};
use editor::Editor;
use fs::{Fs, RealFs};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, Bounds, Focusable as _, KeyBinding, Menu,
    MenuItem, PathPromptOptions, TaskExt as _, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions, actions, px, size,
};
use http_client::BlockedHttpClient;
use language::{Language, LanguageRegistry};
use markdown_preview::markdown_preview_view::MarkdownPreviewView;
use node_runtime::NodeRuntime;
use project::{LocalProjectFlags, Project};
use project_panel::{
    EntryFilter, Event as ProjectPanelEvent, ProjectPanel, ProjectPanelOptions,
    project_panel_settings::ProjectPanelSettings,
};
use search::{BufferSearchBar, ProjectSearchView, project_search::ProjectSearchBar};
use semver::Version;
use serde::Deserialize;
use session::{AppSession, Session};
use settings::{DockSide, KeybindSource, KeymapFile, Settings as _, SettingsStore};
use theme::{ActiveTheme as _, LoadThemes};
use vim::ModeIndicator;
use workspace as zed_workspace;
use zed_workspace::{
    AppState, Event as WorkspaceEvent, MultiWorkspace, Pane, SaveIntent, Toast, Workspace,
    WorkspaceStore, dock::DockPosition, notifications::NotificationId,
};

const APP_NAME: &str = "zednotes";
const NOTE_FILE_NAME: &str = "spike.md";
const DEFAULT_MARKDOWN_SEARCH_FILTER: &str = "**/*.md, **/*.markdown";
const DEFAULT_EXCLUDED_PATHS: &[&str] = &[".git", ".DS_Store", "node_modules", "target"];
const DEFAULT_PREVIEW_MAX_WIDTH: u32 = 760;
const DEFAULT_PREVIEW_MAX_FILE_SIZE_BYTES: u64 = 5 * 1024 * 1024;

actions!(
    notes,
    [
        /// Quits zednotes.
        Quit,
    ]
);

actions!(
    workspace,
    [
        /// Opens a directory as the notes workspace.
        OpenFolder,
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
        /// Toggles a preview split for the active note.
        TogglePreview,
        /// Opens the active note as a preview-only tab.
        OpenPreview,
    ]
);

actions!(
    view,
    [
        /// Toggles the notes explorer.
        ToggleExplorer,
    ]
);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExplorerSettings {
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
            markdown_only: true,
            show_hidden: false,
            excluded_paths: DEFAULT_EXCLUDED_PATHS
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct NotesSettingsContent {
    explorer: Option<ExplorerSettingsContent>,
    files: Option<FileSettingsContent>,
    preview: Option<PreviewSettingsContent>,
}

#[derive(Debug, Default, Deserialize)]
struct ExplorerSettingsContent {
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

fn parse_explorer_settings(content: &str) -> Result<ExplorerSettings> {
    let content = settings::parse_json_with_comments::<NotesSettingsContent>(content)
        .context("failed to parse zednotes settings")?;
    let mut settings = ExplorerSettings::default();
    if let Some(explorer) = content.explorer {
        settings.markdown_only = explorer.markdown_only.unwrap_or(settings.markdown_only);
        settings.show_hidden = explorer.show_hidden.unwrap_or(settings.show_hidden);
    }
    if let Some(excluded_paths) = content.files.and_then(|files| files.exclude) {
        settings.excluded_paths = excluded_paths;
    }
    Ok(settings)
}

fn parse_preview_settings(content: &str) -> Result<PreviewSettings> {
    let content = settings::parse_json_with_comments::<NotesSettingsContent>(content)
        .context("failed to parse zednotes settings")?;
    let mut settings = PreviewSettings::default();
    if let Some(preview) = content.preview {
        settings.max_width = preview.max_width.unwrap_or(settings.max_width);
        settings.allow_remote_images = preview
            .allow_remote_images
            .unwrap_or(settings.allow_remote_images);
        settings.max_file_size_bytes = preview.max_file_size_bytes.or(settings.max_file_size_bytes);
    }
    Ok(settings)
}

fn explorer_settings_path() -> PathBuf {
    paths::home_dir()
        .join(".config")
        .join(APP_NAME)
        .join("settings.json")
}

fn load_explorer_settings() -> Result<ExplorerSettings> {
    match std::fs::read_to_string(explorer_settings_path()) {
        Ok(content) => parse_explorer_settings(&content),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(ExplorerSettings::default()),
        Err(error) => Err(error).context("failed to read zednotes settings"),
    }
}

fn load_preview_settings() -> Result<PreviewSettings> {
    match std::fs::read_to_string(explorer_settings_path()) {
        Ok(content) => parse_preview_settings(&content),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(PreviewSettings::default()),
        Err(error) => Err(error).context("failed to read zednotes settings"),
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

fn markdown_entry_filter(markdown_only: bool) -> EntryFilter {
    Arc::new(move |path: &Path, is_directory| {
        is_directory || !markdown_only || is_markdown_path(path)
    })
}

fn hard_coded_note_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(NOTE_FILE_NAME)
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

fn load_editor_keymaps(cx: &mut App) -> Result<()> {
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
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFolder, None),
        KeyBinding::new("cmd-n", New, None),
        KeyBinding::new("cmd-b", ToggleExplorer, None),
        KeyBinding::new("cmd-shift-f", Search, None),
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

fn init_editor_subsystems(cx: &mut App) -> Result<()> {
    zed_actions::init();
    command_palette::init(cx);
    editor::init(cx);
    file_finder::init_with_file_filter(Arc::new(is_markdown_path), cx);
    markdown_preview::init(cx);
    project_panel::init(cx);
    search::init(cx);
    vim::init(cx);
    // Vim activates its settings observer at the end of this effect cycle, so
    // apply our app default after that observer is live.
    cx.defer(|cx| {
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_default_settings(cx, |settings| {
                settings.vim_mode = Some(true);
                settings.helix_mode = Some(false);
            });
        });
    });
    load_editor_keymaps(cx)?;

    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-shift-v", TogglePreview, None),
        KeyBinding::new(
            "cmd-s",
            zed_workspace::Save { save_intent: None },
            Some("Editor"),
        ),
    ]);
    cx.set_menus([
        Menu::new(APP_NAME).items([MenuItem::action("Quit", Quit)]),
        Menu::new("File").items([
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Search Workspace", Search),
        ]),
        Menu::new("Note").items([
            MenuItem::action("New Note…", New),
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
        workspace.register_action(|_: &mut Workspace, _: &ToggleHiddenFiles, _, cx| {
            let hide_hidden = ProjectPanelSettings::get_global(cx).hide_hidden;
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store.update_default_settings(cx, |content| {
                    content.project_panel.get_or_insert_default().hide_hidden = Some(!hide_hidden);
                });
            });
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

/// Initializes the local-only services needed by the file-backed editor.
pub fn init(session: Session, cx: &mut App) -> Result<Arc<AppState>> {
    release_channel::init(Version::new(0, 1, 0), cx);
    gpui_tokio::init(cx);
    settings::init(cx);
    theme_settings::init(LoadThemes::All(Box::new(Assets)), cx);
    let explorer_settings = load_explorer_settings()?;
    let preview_settings = load_preview_settings()?;
    apply_explorer_settings(&explorer_settings, cx);
    apply_preview_settings(&preview_settings, cx);

    let fs: Arc<dyn Fs> = Arc::new(RealFs::new(None, cx.background_executor().clone()));
    <dyn Fs>::set_global(fs.clone(), cx);
    cx.set_http_client(Arc::new(BlockedHttpClient::new()));

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

    AppState::set_global(app_state.clone(), cx);
    zed_workspace::init(app_state.clone(), cx);
    init_editor_subsystems(cx)?;
    init_workspace_composition(app_state.clone(), explorer_settings, cx);
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
        |workspace, _, event, window, cx| {
            if let WorkspaceEvent::PaneAdded(pane) = event {
                configure_pane(workspace, pane, window, cx);
            }
        },
    )
    .detach();
    workspace.status_bar().update(cx, |status_bar, cx| {
        let mode_indicator = cx.new(|cx| ModeIndicator::new(window, cx));
        status_bar.add_right_item(mode_indicator, window, cx);
    });
}

fn configure_pane(
    workspace: &Workspace,
    pane: &gpui::Entity<Pane>,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let languages = workspace.project().read(cx).languages().clone();
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            let buffer_search_bar = cx.new(|cx| BufferSearchBar::new(Some(languages), window, cx));
            toolbar.add_item(buffer_search_bar, window, cx);
            let project_search_bar = cx.new(|_| ProjectSearchBar::new());
            toolbar.add_item(project_search_bar, window, cx);
        });
    });
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

pub fn open_notes_window(cx: &mut App) -> Result<WindowHandle<MultiWorkspace>> {
    let app_state = AppState::global(cx);
    let project = Project::local(
        app_state.client.clone(),
        app_state.node_runtime.clone(),
        app_state.user_store.clone(),
        app_state.languages.clone(),
        app_state.fs.clone(),
        None,
        LocalProjectFlags {
            init_worktree_trust: false,
            watch_global_configs: false,
        },
        cx,
    );
    open_notes_window_for_path(app_state, project, hard_coded_note_path(), cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_palette_hooks::GlobalCommandPaletteInterceptor;
    use fs::FakeFs;
    use gpui::{TestAppContext, VisualTestContext};
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

    async fn test_window_with(
        cx: &mut TestAppContext,
        explorer_settings: ExplorerSettings,
        tree: Value,
        initial_note_path: &str,
    ) -> TestWindow {
        let app_state = cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            let app_state = AppState::test(cx);
            AppState::set_global(app_state.clone(), cx);
            zed_workspace::init(app_state.clone(), cx);
            apply_explorer_settings(&explorer_settings, cx);
            apply_preview_settings(&PreviewSettings::default(), cx);
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

    #[test]
    fn test_parses_explorer_settings_from_jsonc() {
        let settings = parse_explorer_settings(
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
        .expect("settings should parse");

        assert_eq!(
            settings,
            ExplorerSettings {
                markdown_only: false,
                show_hidden: true,
                excluded_paths: vec!["drafts".to_owned(), "**/*.tmp".to_owned()],
            }
        );
    }

    #[test]
    fn test_parses_preview_settings_from_jsonc() {
        let settings = parse_preview_settings(
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
        .expect("settings should parse");

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
