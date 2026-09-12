use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context as _;
use editor::Editor;
use fs::MTime;
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, Focusable, Render, ScrollStrategy, Subscription,
    Task, UniformListScrollHandle, WeakEntity, actions, uniform_list,
};
use language::{BufferSnapshot, ToPoint};
use markdown::wiki_links::{self, NoteLinks};
use menu::{Confirm, SelectNext, SelectPrevious};
use project::{
    Project, ProjectPath,
    note_links::{is_note, resolve_path},
};
use ui::prelude::*;
use util::ResultExt as _;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
    notifications::NotifyTaskExt,
};

actions!(
    markdown,
    [
        /// Toggles the backlinks panel for the active Markdown note.
        ToggleBacklinks,
    ]
);

#[derive(Clone)]
struct CachedNote {
    stamp: (Option<MTime>, u64),
    text: Arc<str>,
    links: NoteLinks,
}

struct ScanInput {
    files: Vec<NoteInput>,
    target: PathBuf,
    previous: Arc<BTreeMap<PathBuf, CachedNote>>,
    file_system: Arc<dyn fs::Fs>,
}

struct NoteInput {
    path: PathBuf,
    absolute_path: PathBuf,
    stamp: (Option<MTime>, u64),
    buffer: Option<BufferSnapshot>,
}

#[derive(Clone, Debug)]
struct Backlink {
    path: PathBuf,
    range: Range<usize>,
    row: usize,
    excerpt: String,
}

pub struct BacklinksPanel {
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    focus_handle: FocusHandle,
    source: Option<ProjectPath>,
    notes: Arc<BTreeMap<PathBuf, CachedNote>>,
    backlinks: Vec<Backlink>,
    selected: Option<usize>,
    scroll_handle: UniformListScrollHandle,
    error: Option<String>,
    loading: bool,
    position: DockPosition,
    pending_refresh: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

pub fn register(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let handle = cx.entity();
    let project = workspace.project().clone();
    let panel = cx.new(|cx| BacklinksPanel::new(handle.downgrade(), project, window, cx));
    workspace.add_panel(panel.clone(), window, cx);
    workspace.register_action(|workspace, _: &ToggleBacklinks, window, cx| {
        workspace.toggle_panel_focus::<BacklinksPanel>(window, cx);
    });
    cx.defer(move |cx| panel.update(cx, |panel, cx| panel.follow_active_note(cx)));
}

impl BacklinksPanel {
    fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(workspace) = workspace.upgrade() {
            subscriptions.push(
                cx.subscribe_in(&workspace, window, |this, _, event, _, cx| {
                    if matches!(
                        event,
                        workspace::Event::ActiveItemChanged
                            | workspace::Event::ItemAdded { .. }
                            | workspace::Event::UserSavedItem { .. }
                    ) {
                        this.follow_active_note(cx);
                    }
                }),
            );
        }
        subscriptions.push(cx.subscribe(&project, |this, _, event, cx| match event {
            project::Event::BufferEdited { .. }
            | project::Event::WorktreeUpdatedEntries(..)
            | project::Event::WorktreeRemoved(..)
            | project::Event::EntryRenamed { .. } => this.follow_active_note(cx),
            _ => {}
        }));
        Self {
            workspace,
            project,
            focus_handle: cx.focus_handle(),
            source: None,
            notes: Arc::new(BTreeMap::new()),
            backlinks: Vec::new(),
            selected: None,
            scroll_handle: UniformListScrollHandle::new(),
            error: None,
            loading: false,
            position: DockPosition::Right,
            pending_refresh: None,
            _subscriptions: subscriptions,
        }
    }

    fn follow_active_note(&mut self, cx: &mut Context<Self>) {
        let source = self.workspace.upgrade().and_then(|workspace| {
            let item = workspace.read(cx).active_item(cx)?;
            let editor = item.act_as::<Editor>(cx)?;
            let buffer = editor.read(cx).buffer().read(cx).as_singleton()?;
            let file = buffer.read(cx).file()?;
            is_note(file.path().as_std_path()).then(|| ProjectPath::from_file(file.as_ref(), cx))
        });
        if self.source.as_ref().map(|source| source.worktree_id)
            != source.as_ref().map(|source| source.worktree_id)
        {
            self.notes = Arc::new(BTreeMap::new());
        }
        if self.source != source {
            self.backlinks.clear();
            self.selected = None;
        }
        self.source = source;
        self.error = None;
        self.pending_refresh.take();
        self.loading = self.source.is_some();
        if self.source.is_none() {
            self.backlinks.clear();
            cx.notify();
            return;
        }
        self.pending_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let inputs = this.update(cx, |this, cx| this.inputs(cx));
            let ScanInput {
                files: inputs,
                target: source,
                previous,
                file_system,
            } = match inputs {
                Ok(Ok(inputs)) => inputs,
                Ok(Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.error = Some(error.to_string());
                        this.loading = false;
                        cx.notify();
                    })
                    .log_err();
                    return;
                }
                Err(error) => {
                    log::debug!("Backlinks panel closed: {error}");
                    return;
                }
            };
            let paths = inputs
                .iter()
                .map(|input| input.path.clone())
                .collect::<BTreeSet<_>>();
            let (notes, errors) = cx
                .background_spawn(async move {
                    let mut notes = BTreeMap::new();
                    let mut errors = Vec::new();
                    for input in inputs {
                        let previous = previous.get(&input.path);
                        let text = if let Some(buffer) = input.buffer {
                            buffer.text()
                        } else if let Some(previous) =
                            previous.filter(|previous| previous.stamp == input.stamp)
                        {
                            notes.insert(input.path, previous.clone());
                            continue;
                        } else {
                            match file_system.load(&input.absolute_path).await {
                                Ok(text) => text,
                                Err(error) => {
                                    errors.push(format!("{}: {error}", input.path.display()));
                                    continue;
                                }
                            }
                        };
                        let links = match previous.filter(|previous| previous.text.as_ref() == text)
                        {
                            Some(previous) => previous.links.clone(),
                            None => wiki_links::parse(&text),
                        };
                        notes.insert(
                            input.path,
                            CachedNote {
                                stamp: input.stamp,
                                text: text.into(),
                                links,
                            },
                        );
                    }
                    (notes, errors)
                })
                .await;
            let (notes, backlinks) = cx
                .background_spawn(async move {
                    let backlinks = collect_backlinks(&source, &notes, &paths);
                    (notes, backlinks)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notes = Arc::new(notes);
                this.backlinks = backlinks;
                this.selected = this.selected.filter(|index| *index < this.backlinks.len());
                this.error = (!errors.is_empty())
                    .then(|| format!("Some notes could not be read:\n{}", errors.join("\n")));
                this.loading = false;
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn inputs(&self, cx: &App) -> anyhow::Result<ScanInput> {
        let source = self.source.as_ref().context("No active note")?;
        let project = self.project.read(cx);
        anyhow::ensure!(
            project.is_local(),
            "Backlinks currently require a local notes folder"
        );
        let worktree = project
            .worktree_for_id(source.worktree_id, cx)
            .context("Notes folder is closed")?;
        let worktree = worktree.read(cx);
        let mut buffers = BTreeMap::new();
        for buffer in project.buffer_store().read(cx).buffers() {
            let buffer = buffer.read(cx);
            if let Some(file) = buffer.file() {
                let path = ProjectPath::from_file(file.as_ref(), cx);
                if path.worktree_id == source.worktree_id
                    && is_note(path.path.as_std_path())
                    && !file.disk_state().is_deleted()
                {
                    buffers.insert(path.path.as_std_path().to_path_buf(), buffer.snapshot());
                }
            }
        }
        let mut inputs = Vec::new();
        for entry in worktree
            .files(false, 0)
            .filter(|entry| is_note(entry.path.as_std_path()))
        {
            let path = entry.path.as_std_path().to_path_buf();
            inputs.push(NoteInput {
                absolute_path: worktree.abs_path().join(&path),
                buffer: buffers.remove(&path),
                path,
                stamp: (entry.mtime, entry.size),
            });
        }
        for (path, buffer) in buffers {
            inputs.push(NoteInput {
                absolute_path: worktree.abs_path().join(&path),
                path,
                buffer: Some(buffer),
                stamp: (None, 0),
            });
        }
        Ok(ScanInput {
            files: inputs,
            target: source.path.as_std_path().to_path_buf(),
            previous: self.notes.clone(),
            file_system: project.fs().clone(),
        })
    }

    fn select(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self.backlinks.is_empty() {
            return;
        }
        let index = self.selected.map_or(0, |index| {
            if previous {
                index.saturating_sub(1)
            } else {
                (index + 1).min(self.backlinks.len() - 1)
            }
        });
        self.selected = Some(index);
        self.scroll_handle
            .scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn open_backlink(&mut self, backlink: Backlink, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = &self.source else {
            return;
        };
        let path =
            match util::rel_path::RelPath::new(&backlink.path, util::paths::PathStyle::local()) {
                Ok(path) => path.into(),
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            };
        let path = ProjectPath {
            worktree_id: source.worktree_id,
            path,
        };
        let workspace = self.workspace.clone();
        let workspace_for_error = workspace.clone();
        let task = cx.spawn_in(window, async move |_, cx| {
            let item = workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.open_path(path, None, true, window, cx)
                })?
                .await?;
            if let Some(editor) = cx.update(|_, cx| item.act_as::<Editor>(cx))? {
                editor.update_in(cx, |editor, window, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                        let buffer = buffer.read(cx);
                        let start = buffer.clip_offset(
                            backlink.range.start.min(buffer.len()),
                            language::Bias::Left,
                        );
                        let end = buffer.clip_offset(
                            backlink.range.end.min(buffer.len()),
                            language::Bias::Right,
                        );
                        let range = start.to_point(buffer)..end.to_point(buffer);
                        editor.go_to_singleton_buffer_range(range, window, cx);
                    }
                })?;
            }
            anyhow::Ok(())
        });
        task.detach_and_notify_err(workspace_for_error, window, cx);
    }
}

fn collect_backlinks(
    target: &std::path::Path,
    notes: &BTreeMap<PathBuf, CachedNote>,
    paths: &BTreeSet<PathBuf>,
) -> Vec<Backlink> {
    let mut backlinks = Vec::new();
    for (path, note) in notes {
        if path == target {
            continue;
        }
        for link in &note.links.links {
            if resolve_path(path, &link.target, paths).ok().as_deref() != Some(target) {
                continue;
            }
            let Some(prefix) = note.text.get(..link.range.start) else {
                continue;
            };
            let line_start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
            let Some(rest) = note.text.get(line_start..) else {
                continue;
            };
            backlinks.push(Backlink {
                path: path.clone(),
                range: link.range.clone(),
                row: prefix.bytes().filter(|byte| *byte == b'\n').count(),
                excerpt: rest.lines().next().unwrap_or_default().trim().to_owned(),
            });
        }
    }
    backlinks
}

impl Render for BacklinksPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("backlinks-panel")
            .key_context("BacklinksPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.select(false, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.select(true, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                if let Some(backlink) = this
                    .selected
                    .and_then(|index| this.backlinks.get(index))
                    .cloned()
                {
                    this.open_backlink(backlink, window, cx);
                }
            }))
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(
                v_flex()
                    .p_3()
                    .gap_1()
                    .child(Label::new(format!("Backlinks ({})", self.backlinks.len())))
                    .when_some(self.source.as_ref(), |this, source| {
                        this.child(
                            Label::new(source.path.to_string())
                                .color(Color::Muted)
                                .size(LabelSize::Small),
                        )
                    }),
            )
            .when(self.loading, |this| {
                this.child(Label::new("Updating backlinks…").color(Color::Muted))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().p_3().child(Label::new(error).color(Color::Error)))
            })
            .when(
                !self.loading && self.backlinks.is_empty() && self.error.is_none(),
                |this| {
                    this.child(
                        div().p_3().child(
                            Label::new(if self.source.is_some() {
                                "No notes link here yet"
                            } else {
                                "Open a Markdown note to see its backlinks"
                            })
                            .color(Color::Muted),
                        ),
                    )
                },
            )
            .child(
                uniform_list(
                    "backlinks",
                    self.backlinks.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| {
                                let backlink = this.backlinks.get(index)?.clone();
                                Some(
                                    v_flex()
                                        .id(index)
                                        .w_full()
                                        .px_3()
                                        .py_2()
                                        .gap_1()
                                        .cursor_pointer()
                                        .when(this.selected == Some(index), |this| {
                                            this.bg(cx.theme().colors().element_selected)
                                        })
                                        .hover(|style| style.bg(cx.theme().colors().element_hover))
                                        .child(
                                            Label::new(format!(
                                                "{}:{}",
                                                backlink.path.display(),
                                                backlink.row + 1
                                            ))
                                            .size(LabelSize::Small),
                                        )
                                        .child(
                                            Label::new(backlink.excerpt.clone())
                                                .color(Color::Muted)
                                                .size(LabelSize::Small)
                                                .truncate(),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.selected = Some(index);
                                            this.open_backlink(backlink.clone(), window, cx)
                                        })),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll_handle)
                .flex_1()
                .min_h_0(),
            )
    }
}

impl Focusable for BacklinksPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl EventEmitter<PanelEvent> for BacklinksPanel {}
impl Panel for BacklinksPanel {
    fn persistent_name() -> &'static str {
        "BacklinksPanel"
    }
    fn panel_key() -> &'static str {
        "backlinks"
    }
    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }
    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }
    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }
    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(320.)
    }
    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::Link)
    }
    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Backlinks")
    }
    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleBacklinks)
    }
    fn activation_priority(&self) -> u32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::{FakeFs, Fs};
    use gpui::TestAppContext;
    use language::Point;
    use serde_json::json;
    use util::{path, rel_path::rel_path};
    use workspace::{AppState, MultiWorkspace};

    #[gpui::test]
    async fn backlinks_follow_unsaved_edits_external_changes_and_navigation(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(db::AppDatabase::test_new());
            AppState::test(cx);
            editor::init(cx);
            crate::init(cx);
        });
        let file_system = FakeFs::new(cx.executor());
        file_system
            .insert_tree(
                path!("/notes"),
                json!({
                    "Target.md": "# Target\n\n## Heading\n",
                    "Source.md": "[[Target|label]]\n[[Target#Heading]]\n`[[Target]]`",
                    "Other.md": "no links"
                }),
            )
            .await;
        let project = Project::test(file_system.clone(), [path!("/notes").as_ref()], cx).await;
        let worktree_id = project.read_with(cx, |project, cx| {
            project.visible_worktrees(cx).next().unwrap().read(cx).id()
        });
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi_workspace.read_with(cx, |workspace, _| workspace.workspace().clone());
        let target = ProjectPath {
            worktree_id,
            path: rel_path("Target.md").into(),
        };
        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(target.clone(), None, true, window, cx)
            })
            .await
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        let panel = workspace.read_with(cx, |workspace, cx| {
            workspace.panel::<BacklinksPanel>(cx).unwrap()
        });
        panel.read_with(cx, |panel, _| {
            assert!(panel.error.is_none(), "{:?}", panel.error);
            assert!(!panel.loading);
            assert_eq!(panel.backlinks.len(), 2);
        });

        let source = project
            .update(cx, |project, cx| {
                project.open_buffer(
                    ProjectPath {
                        worktree_id,
                        path: rel_path("Source.md").into(),
                    },
                    cx,
                )
            })
            .await
            .unwrap();
        source.update(cx, |buffer, cx| {
            buffer.edit([(0..buffer.len(), "no links now")], None, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| assert!(panel.backlinks.is_empty()));

        file_system
            .write(
                path!("/notes/Other.md").as_ref(),
                b"intro\n[[Target#Heading|section]]",
            )
            .await
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        let backlink = panel.read_with(cx, |panel, _| {
            assert_eq!(panel.backlinks.len(), 1);
            panel.backlinks[0].clone()
        });
        panel.update_in(cx, |panel, window, cx| {
            panel.open_backlink(backlink, window, cx)
        });
        cx.run_until_parked();
        let editor = workspace.read_with(cx, |workspace, cx| {
            workspace.active_item_as::<Editor>(cx).unwrap()
        });
        editor.update_in(cx, |editor, window, cx| {
            assert_eq!(
                editor
                    .active_buffer(cx)
                    .unwrap()
                    .read(cx)
                    .file()
                    .unwrap()
                    .path()
                    .as_ref(),
                rel_path("Other.md")
            );
            let snapshot = editor.snapshot(window, cx);
            assert_eq!(
                editor.selections.newest::<Point>(&snapshot).start,
                Point::new(1, 0)
            );
        });

        workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(target, None, true, window, cx)
            })
            .await
            .unwrap();
        file_system
            .remove_file(path!("/notes/Other.md").as_ref(), Default::default())
            .await
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(300));
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| assert!(panel.backlinks.is_empty()));
    }

    #[test]
    fn backlinks_resolve_paths_and_keep_every_occurrence() {
        let mut notes = BTreeMap::new();
        for (path, text) in [
            ("Target.md", "# Heading\n[[Target]]"),
            (
                "a/Source.md",
                "[[Target|label]]\n[[Target#Heading]]\n`[[Target]]`",
            ),
            ("a/Duplicate.md", "[[Target]]"),
            ("b/Target.md", ""),
        ] {
            notes.insert(
                PathBuf::from(path),
                CachedNote {
                    stamp: (None, 0),
                    text: text.into(),
                    links: wiki_links::parse(text),
                },
            );
        }
        let backlinks = collect_backlinks(
            std::path::Path::new("Target.md"),
            &notes,
            &notes.keys().cloned().collect(),
        );
        assert_eq!(backlinks.len(), 3);
        assert_eq!(backlinks[1].row, 0);
        assert_eq!(backlinks[2].row, 1);
        notes.remove(&PathBuf::from("Target.md"));
        assert!(
            collect_backlinks(
                std::path::Path::new("Target.md"),
                &notes,
                &notes.keys().cloned().collect()
            )
            .is_empty()
        );
    }

    #[gpui::test]
    async fn wiki_link_completion_preserves_delimiters_aliases_and_headings(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            AppState::test(cx);
        });
        let file_system = FakeFs::new(cx.executor());
        file_system.insert_tree(path!("/notes"), json!({"Source.md": "", "Target.md": "", "a": {"Duplicate.md": ""}, "b": {"Duplicate.md": ""}})).await;
        let project = Project::test(file_system, [path!("/notes").as_ref()], cx).await;
        let worktree_id = project.read_with(cx, |project, cx| {
            project.visible_worktrees(cx).next().unwrap().read(cx).id()
        });
        let source = project
            .update(cx, |project, cx| {
                project.open_buffer(
                    ProjectPath {
                        worktree_id,
                        path: rel_path("Source.md").into(),
                    },
                    cx,
                )
            })
            .await
            .unwrap();
        for (text, offset, expected) in [
            ("[[Tar", 5, "[[Target]]"),
            ("[[Tar]]", 5, "[[Target]]"),
            ("[[Tar|label]]", 5, "[[Target|label]]"),
            ("[[Tar#Heading]]", 5, "[[Target#Heading]]"),
            ("[[Tar\nnext line", 5, "[[Target]]\nnext line"),
        ] {
            source.update(cx, |buffer, cx| {
                buffer.edit([(0..buffer.len(), text)], None, cx);
            });
            let response = project.read_with(cx, |project, cx| {
                project.note_link_completions(&source, offset, cx).unwrap()
            });
            let completion = response
                .completions
                .into_iter()
                .find(|completion| completion.label.text == "Target")
                .unwrap();
            source.update(cx, |buffer, cx| {
                buffer.edit([(completion.replace_range, completion.new_text)], None, cx);
                assert_eq!(buffer.text(), expected);
            });
        }
        source.update(cx, |buffer, cx| {
            buffer.edit([(0..buffer.len(), "[[")], None, cx);
        });
        let response = project.read_with(cx, |project, cx| {
            project.note_link_completions(&source, 2, cx).unwrap()
        });
        let duplicates = response
            .completions
            .iter()
            .filter(|completion| completion.label.text.contains("Duplicate"))
            .collect::<Vec<_>>();
        assert_eq!(duplicates.len(), 2);
        assert!(
            duplicates
                .iter()
                .all(|completion| completion.new_text.starts_with('/'))
        );
    }
}
