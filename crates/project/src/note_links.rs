use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use gpui::{App, AppContext as _, Context, Entity, Task};
use language::{Buffer, CodeLabel, ToOffset, ToPointUtf16};
use markdown::wiki_links::{self, WikiLink};
use util::rel_path::RelPath;

use crate::{Completion, CompletionResponse, CompletionSource, Location, Project, ProjectPath};

pub fn is_note(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

pub fn link_at(buffer: &Entity<Buffer>, position: impl ToPointUtf16, cx: &App) -> Option<WikiLink> {
    let buffer = buffer.read(cx);
    if !is_note(buffer.file()?.path().as_std_path()) {
        return None;
    }
    let offset = position.to_point_utf16(buffer).to_offset(buffer);
    wiki_links::parse(&buffer.text()).link_at(offset).cloned()
}

/// Resolve within one worktree (vault). Ambiguous short names must never silently
/// point navigation and backlinks at an arbitrary file.
pub fn resolve_path(source: &Path, target: &str, paths: &BTreeSet<PathBuf>) -> Result<PathBuf> {
    let (target, fragment) = wiki_links::split_target(target);
    if target.is_empty() {
        anyhow::ensure!(
            fragment.is_some_and(|fragment| !fragment.is_empty()),
            "Empty note link"
        );
        return Ok(source.to_path_buf());
    }
    let target = target.replace('\\', "/");
    let target = Path::new(&target);
    let explicit_relative = target.starts_with(".") || target.starts_with("..");
    let root_relative = target.has_root();
    let relative = normalize(source.parent().unwrap_or(Path::new("")).join(target));
    let root = normalize(target.strip_prefix("/").unwrap_or(target).to_path_buf());
    let candidates = |path: &Path| {
        let mut candidates = vec![path.to_path_buf()];
        if !is_note(path) {
            let mut name = path.as_os_str().to_os_string();
            name.push(".md");
            candidates.push(PathBuf::from(name));
            let mut name = path.as_os_str().to_os_string();
            name.push(".markdown");
            candidates.push(PathBuf::from(name));
        }
        candidates
    };
    for path in [
        (!root_relative).then_some(relative).flatten(),
        (!explicit_relative).then_some(root.clone()).flatten(),
    ]
    .into_iter()
    .flatten()
    {
        let matches = candidates(&path)
            .into_iter()
            .filter(|path| paths.contains(path))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [path] => return Ok(path.clone()),
            [] => {}
            _ => bail!("Ambiguous note link: {}", target.display()),
        }
    }
    if !explicit_relative
        && !root_relative
        && let Some(root) = root
    {
        let suffixes = candidates(&root);
        let matches = paths
            .iter()
            .filter(|path| suffixes.iter().any(|suffix| path.ends_with(suffix)))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [path] => return Ok((*path).clone()),
            [] => {}
            _ => bail!(
                "Ambiguous note link: {}. Use a folder-qualified path.",
                target.display()
            ),
        }
    }
    bail!("Note not found: {}", target.display())
}

fn normalize(path: PathBuf) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

impl Project {
    pub fn note_paths(&self, source: &ProjectPath, cx: &App) -> BTreeSet<PathBuf> {
        let mut paths = BTreeSet::new();
        if let Some(worktree) = self.worktree_for_id(source.worktree_id, cx) {
            paths.extend(
                worktree
                    .read(cx)
                    .files(false, 0)
                    .filter(|entry| is_note(entry.path.as_std_path()))
                    .map(|entry| entry.path.as_std_path().to_path_buf()),
            );
        }
        paths.extend(self.buffer_store().read(cx).buffers().filter_map(|buffer| {
            let buffer = buffer.read(cx);
            let file = buffer.file()?;
            let path = ProjectPath::from_file(file.as_ref(), cx);
            (path.worktree_id == source.worktree_id
                && is_note(path.path.as_std_path())
                && !file.disk_state().is_deleted())
            .then(|| path.path.as_std_path().to_path_buf())
        }));
        paths
    }

    pub fn resolve_note_link(
        &mut self,
        source: &ProjectPath,
        target: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<Location>> {
        let path = match resolve_path(
            source.path.as_std_path(),
            target,
            &self.note_paths(source, cx),
        ) {
            Ok(path) => path,
            Err(error) => return Task::ready(Err(error)),
        };
        let path = match RelPath::new(&path, util::paths::PathStyle::local()) {
            Ok(path) => path.into(),
            Err(error) => return Task::ready(Err(error.into())),
        };
        let buffer = self.open_buffer(
            ProjectPath {
                worktree_id: source.worktree_id,
                path,
            },
            cx,
        );
        let fragment = wiki_links::split_target(target).1.map(str::to_owned);
        cx.spawn(async move |_, cx| {
            let buffer = buffer.await?;
            let snapshot = buffer.read_with(cx, |buffer, _| buffer.snapshot());
            let source = snapshot.text();
            let offset = if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
                cx.background_spawn(async move {
                    wiki_links::parse(&source)
                        .heading_offset(&fragment)
                        .with_context(|| format!("Heading not found: {fragment}"))
                })
                .await?
            } else {
                0
            };
            Ok(Location {
                buffer,
                range: snapshot.anchor_before(offset)..snapshot.anchor_after(offset),
            })
        })
    }

    pub fn note_link_completions(
        &self,
        buffer: &Entity<Buffer>,
        offset: usize,
        cx: &App,
    ) -> Option<CompletionResponse> {
        let buffer = buffer.read(cx);
        let file = buffer.file()?;
        if !is_note(file.path().as_std_path()) {
            return None;
        }
        let source = buffer.text();
        let range = wiki_links::completion_range(&source, offset)?;
        let prefix = source.get(range.clone())?;
        if prefix.contains('#') {
            return None;
        }
        let source_path = ProjectPath::from_file(file.as_ref(), cx);
        let paths = self.note_paths(&source_path, cx);
        let mut names = BTreeMap::<String, BTreeSet<PathBuf>>::new();
        for path in &paths {
            if let Some(name) = path.file_stem() {
                names
                    .entry(name.to_string_lossy().into_owned())
                    .or_default()
                    .insert(path.clone());
            }
        }
        // Resolve each short name once, using only its candidates, so completion
        // does not scan the entire vault for every suggestion.
        let short_targets = names
            .into_iter()
            .map(|(name, candidates)| {
                let target = resolve_path(source_path.path.as_std_path(), &name, &candidates).ok();
                (name, target)
            })
            .collect::<BTreeMap<_, _>>();
        let suffix = source.get(offset..)?;
        let delimiter_offset = suffix
            .find([']', '|', '#', '\n'])
            .map_or(offset, |end| offset + end);
        let has_delimiter = source
            .get(delimiter_offset..)
            .is_some_and(|suffix| suffix.starts_with("]]") || suffix.starts_with(['|', '#']));
        let target_end = if has_delimiter {
            delimiter_offset
        } else {
            offset
        };
        let start = buffer.anchor_before(range.start);
        let end = buffer.anchor_after(target_end);
        let completions = paths
            .iter()
            .map(|path| {
                let short = path.file_stem().unwrap_or_default().to_string_lossy();
                let target =
                    if short_targets.get(short.as_ref()).and_then(Option::as_ref) == Some(path) {
                        short.into_owned()
                    } else {
                        format!("/{}", path.to_string_lossy().replace('\\', "/"))
                    };
                Completion {
                    replace_range: start..end,
                    new_text: if has_delimiter {
                        target.clone()
                    } else {
                        format!("{target}]]")
                    },
                    label: CodeLabel {
                        filter_range: 0..target.len(),
                        text: target,
                        runs: Vec::new(),
                    },
                    documentation: None,
                    source: CompletionSource::Custom,
                    icon_path: None,
                    icon_color: None,
                    match_start: Some(start),
                    snippet_deduplication_key: None,
                    insert_text_mode: None,
                    confirm: None,
                    group: None,
                }
            })
            .collect();
        Some(CompletionResponse {
            completions,
            display_options: Default::default(),
            is_incomplete: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_root_and_unique_suffixes() {
        let paths = [
            "notes/Start.md",
            "notes/Local.md",
            "math/向量.md",
            "math/v1.0.md",
            "root.md",
        ]
        .map(PathBuf::from)
        .into();
        let source = Path::new("notes/Start.md");
        for (link, expected) in [
            ("Local|label", None),
            ("Local", Some("notes/Local.md")),
            ("向量#定义", Some("math/向量.md")),
            ("../root", Some("root.md")),
            ("/math/向量", Some("math/向量.md")),
            ("v1.0", Some("math/v1.0.md")),
            ("#Heading", Some("notes/Start.md")),
            ("../../root", None),
        ] {
            assert_eq!(
                resolve_path(source, link, &paths).ok(),
                expected.map(PathBuf::from),
                "{link}"
            );
        }
    }

    #[test]
    fn refuses_ambiguous_or_unresolved_links() {
        let paths = ["a/Note.md", "b/Note.md"].map(PathBuf::from).into();
        assert!(resolve_path(Path::new("Start.md"), "Note", &paths).is_err());
        assert!(resolve_path(Path::new("Start.md"), "Missing", &paths).is_err());
        assert_eq!(
            resolve_path(Path::new("a/Start.md"), "Note", &paths).unwrap(),
            PathBuf::from("a/Note.md")
        );
    }
}
