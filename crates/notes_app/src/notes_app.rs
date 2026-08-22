//! zednotes' application layer.
//!
//! Everything notes-specific lives in this crate rather than in the generic Zed
//! crates of the fork, so that merges from `upstream` stay cheap.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context as _, Result};
use assets::Assets;
use client::{Client, UserStore};
use editor::Editor;
use fs::{Fs, RealFs};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, Bounds, KeyBinding, Menu, MenuItem, TaskExt as _,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, actions, px, size,
};
use http_client::BlockedHttpClient;
use language::{Language, LanguageRegistry};
use node_runtime::NodeRuntime;
use project::{LocalProjectFlags, Project};
use search::BufferSearchBar;
use semver::Version;
use session::{AppSession, Session};
use settings::{KeybindSource, KeymapFile, SettingsStore};
use theme::{ActiveTheme as _, LoadThemes};
use vim::ModeIndicator;
use workspace::{AppState, MultiWorkspace, Workspace, WorkspaceStore};

const APP_NAME: &str = "zednotes";
const NOTE_FILE_NAME: &str = "spike.md";

actions!(
    notes,
    [
        /// Quits zednotes.
        Quit,
    ]
);

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
    Ok(())
}

fn init_editor_subsystems(cx: &mut App) -> Result<()> {
    zed_actions::init();
    command_palette::init(cx);
    editor::init(cx);
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
        KeyBinding::new("cmd-w", workspace::CloseWindow, None),
        KeyBinding::new(
            "cmd-s",
            workspace::Save { save_intent: None },
            Some("Editor"),
        ),
    ]);
    cx.set_menus([Menu::new(APP_NAME).items([MenuItem::action("Quit", Quit)])]);
    cx.on_window_closed(|cx, _window_id| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();

    Ok(())
}

/// Initializes the local-only services needed by the file-backed editor.
pub fn init(session: Session, cx: &mut App) -> Result<Arc<AppState>> {
    release_channel::init(Version::new(0, 1, 0), cx);
    gpui_tokio::init(cx);
    settings::init(cx);
    theme_settings::init(LoadThemes::All(Box::new(Assets)), cx);

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
    workspace::init(app_state.clone(), cx);
    init_editor_subsystems(cx)?;
    Ok(app_state)
}

fn configure_workspace(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    workspace.active_pane().update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            let search_bar = cx.new(|cx| BufferSearchBar::new(None, window, cx));
            toolbar.add_item(search_bar, window, cx);
        });
    });
    workspace.status_bar().update(cx, |status_bar, cx| {
        let mode_indicator = cx.new(|cx| ModeIndicator::new(window, cx));
        status_bar.add_right_item(mode_indicator, window, cx);
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
    let buffer = project.update(cx, |project, cx| project.open_local_buffer(&path, cx));
    let options = notes_window_options(None, cx);
    let window_handle = cx.open_window(options, {
        let project = project.clone();
        move |window, cx| {
            window.set_window_title(APP_NAME);
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(None, project, app_state, window, cx);
                configure_workspace(&mut workspace, window, cx);
                workspace
            });
            cx.new(|cx| MultiWorkspace::new(workspace, window, cx))
        }
    })?;

    cx.spawn(async move |cx| {
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
    use serde_json::json;
    use settings::SettingsStore;
    use std::path::Path;

    const TEST_NOTE_PATH: &str = "/notes/spike.md";
    const TEST_NOTE: &str = "# Spike\n\n- **fast** editing\n";

    async fn test_window(
        cx: &mut TestAppContext,
    ) -> (Arc<FakeFs>, gpui::Entity<Editor>, VisualTestContext) {
        cx.update(|cx| {
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            theme_settings::init(LoadThemes::JustBase, cx);
            init_editor_subsystems(cx).expect("failed to initialize editor test subsystems");
        });

        let app_state = cx.update(AppState::test);
        let fs = app_state.fs.as_fake().clone();
        fs.insert_tree("/notes", json!({ "spike.md": TEST_NOTE }))
            .await;
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
                open_notes_window_for_path(app_state, project, PathBuf::from(TEST_NOTE_PATH), cx)
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
        (fs, editor, window_cx)
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
        let (_fs, editor, mut window_cx) = test_window(cx).await;

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

        window_cx.simulate_keystrokes("i x escape");
        assert_eq!(
            editor.read_with(cx, |editor, cx| editor.text(cx)),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_cmd_s_saves_the_editor_buffer(cx: &mut TestAppContext) {
        let (fs, _editor, mut window_cx) = test_window(cx).await;

        window_cx.simulate_keystrokes("i x escape cmd-s");
        window_cx.run_until_parked();

        assert_eq!(
            fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_vim_write_saves_the_editor_buffer(cx: &mut TestAppContext) {
        let (fs, _editor, mut window_cx) = test_window(cx).await;

        window_cx.simulate_keystrokes("i x escape");
        assert!(
            window_cx.update(|_, cx| cx.has_global::<GlobalCommandPaletteInterceptor>()),
            "Vim should install its command-line interceptor"
        );
        window_cx.simulate_keystrokes(":");
        let has_command_palette = window_cx.update(|window, cx| {
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
        window_cx.simulate_keystrokes("w");
        window_cx.run_until_parked();
        window_cx.simulate_keystrokes("enter");
        let has_command_palette = window_cx.update(|window, cx| {
            Workspace::for_window(window, cx)
                .expect("the Vim command line needs a workspace")
                .read(cx)
                .active_modal::<command_palette::CommandPalette>(cx)
                .is_some()
        });
        assert!(!has_command_palette, "enter should submit the Vim command");
        window_cx.run_until_parked();

        assert_eq!(
            fs.load(Path::new(TEST_NOTE_PATH)).await.unwrap(),
            format!("x{TEST_NOTE}")
        );
    }

    #[gpui::test]
    async fn test_cmd_w_closes_the_notes_window(cx: &mut TestAppContext) {
        let (_fs, _editor, mut window_cx) = test_window(cx).await;

        let close_bindings =
            window_cx.update(|window, _| window.bindings_for_action(&workspace::CloseWindow));
        assert!(
            !close_bindings.is_empty(),
            "the focused editor should expose the notes close binding"
        );
        window_cx.simulate_keystrokes("cmd-w");
        window_cx.run_until_parked();

        assert!(cx.windows().is_empty());
    }
}
