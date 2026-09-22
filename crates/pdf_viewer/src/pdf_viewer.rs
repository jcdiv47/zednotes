use std::{
    collections::BTreeMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, ensure};
use gpui::{
    App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Render, RenderImage,
    ScrollHandle, Size, Task, Window, img, point, px, size,
};
use kkpdf_zed::{PdfDocument, PdfiumEngine, RasterizerOptions};
use project::{Project, ProjectEntryId, ProjectPath};
use ui::{Button, prelude::*};
use util::ResultExt;
use workspace::{
    Pane,
    item::{Item, ItemBufferKind, ProjectItem},
};

const MAX_RASTER_DIMENSION: f32 = 4096.0;
const PAGE_GAP: f32 = 16.0;

#[derive(Default)]
struct ContinuousLayout {
    pages: Vec<gpui::Bounds<gpui::Pixels>>,
    size: Size<gpui::Pixels>,
    zoom: f32,
}

impl ContinuousLayout {
    fn new(dimensions: &[(f32, f32)], viewport_width: f32, zoom: f32, fit_width: bool) -> Self {
        let widest = dimensions
            .iter()
            .map(|(width, _)| *width)
            .fold(1.0, f32::max);
        let zoom = if fit_width {
            (viewport_width - PAGE_GAP * 2.0).max(1.0) / widest
        } else {
            zoom
        };
        let width = viewport_width.max(widest * zoom + PAGE_GAP * 2.0);
        let mut top = PAGE_GAP;
        let pages = dimensions
            .iter()
            .map(|(page_width, page_height)| {
                let page_width = page_width * zoom;
                let page_height = page_height * zoom;
                let bounds = gpui::Bounds::new(
                    point(px((width - page_width) / 2.0), px(top)),
                    size(px(page_width), px(page_height)),
                );
                top += page_height + PAGE_GAP;
                bounds
            })
            .collect();
        Self {
            pages,
            size: size(px(width), px(top)),
            zoom,
        }
    }

    fn page_at(&self, offset: gpui::Pixels) -> usize {
        self.pages
            .partition_point(|page| page.bottom() <= offset + px(PAGE_GAP))
            .min(self.pages.len().saturating_sub(1))
    }

    fn visible_pages(&self, offset: gpui::Pixels, height: gpui::Pixels) -> Range<usize> {
        let first = self.pages.partition_point(|page| page.bottom() <= offset);
        let end = self
            .pages
            .partition_point(|page| page.top() < offset + height);
        first.saturating_sub(1)..end.saturating_add(1).min(self.pages.len())
    }
}

#[derive(Default)]
struct ContinuousPage {
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    task: Option<Task<()>>,
}

pub fn init(cx: &mut App) {
    workspace::register_project_item::<PdfView>(cx);
}

pub struct PdfItem {
    project: Entity<Project>,
    project_path: ProjectPath,
}

impl project::ProjectItem for PdfItem {
    fn try_open(
        project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<Result<Entity<Self>>>> {
        let absolute_path = project.read(cx).absolute_path(path, cx);
        if !is_pdf(path.path.extension(), absolute_path.as_deref()) {
            return None;
        }
        let project = project.clone();
        let project_path = path.clone();
        Some(Task::ready(Ok(cx.new(|_| Self {
            project,
            project_path,
        }))))
    }

    fn entry_id(&self, cx: &App) -> Option<ProjectEntryId> {
        self.project
            .read(cx)
            .entry_for_path(&self.project_path, cx)
            .map(|entry| entry.id)
    }

    fn project_path(&self, _: &App) -> Option<ProjectPath> {
        Some(self.project_path.clone())
    }

    fn is_dirty(&self) -> bool {
        false
    }
}

fn is_pdf(extension: Option<&str>, absolute_path: Option<&Path>) -> bool {
    // Single-file worktrees have an empty relative path; the filename belongs to the root.
    extension
        .or_else(|| absolute_path?.extension()?.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

struct LoadedPdf {
    engine: PdfiumEngine,
    bytes: Vec<u8>,
    document: PdfDocument,
}

impl LoadedPdf {
    fn new(bytes: Vec<u8>, path: PathBuf) -> Result<Self> {
        let mut engine = PdfiumEngine::new();
        // Never let a development environment variable substitute fake pages for a document.
        engine.set_allow_mock_fallback(false);
        ensure!(
            engine.is_native_available(),
            "PDF rendering requires Pdfium. Run script/install-pdfium, then restart Zednotes."
        );
        let document = engine.load_document_from_bytes(&bytes, Some(path))?;
        ensure!(!document.is_empty(), "This PDF contains no pages");
        for page in 0..document.total_pages() {
            let dimensions = document
                .page_size(page)
                .context("PDF page does not exist")?;
            ensure!(
                dimensions.width.is_finite()
                    && dimensions.height.is_finite()
                    && dimensions.width > 0.0
                    && dimensions.height > 0.0,
                "Invalid PDF page dimensions"
            );
        }
        Ok(Self {
            engine,
            bytes,
            document,
        })
    }

    fn render_page(&self, page: usize, zoom: f32, scale_factor: f32) -> Result<Arc<RenderImage>> {
        let dimensions = self
            .document
            .page_size(page)
            .context("PDF page does not exist")?;
        ensure!(
            dimensions.width.is_finite()
                && dimensions.height.is_finite()
                && dimensions.width > 0.0
                && dimensions.height > 0.0,
            "Invalid PDF page dimensions"
        );
        let raster_scale = (zoom * scale_factor)
            .min(MAX_RASTER_DIMENSION / dimensions.width.max(dimensions.height));
        let rendered = self.engine.render_page_from_bytes(
            &self.bytes,
            page,
            RasterizerOptions {
                target_dpi: 72.0,
                zoom_factor: raster_scale,
                ..Default::default()
            },
        )?;
        let mut pixels = rendered.rgba_buffer.as_ref().clone();
        // GPUI uploads BGRA pixels, whereas Pdfium returns RGBA.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let buffer = image::RgbaImage::from_raw(rendered.width, rendered.height, pixels)
            .context("Pdfium returned an invalid bitmap")?;
        Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }
}

pub struct PdfView {
    item: Entity<PdfItem>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    document: Option<Arc<LoadedPdf>>,
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    page: usize,
    zoom: f32,
    fit_page: bool,
    continuous: bool,
    continuous_layout: ContinuousLayout,
    continuous_pages: BTreeMap<usize, ContinuousPage>,
    continuous_scroll_offset: Option<gpui::Pixels>,
    viewport_size: Size<gpui::Pixels>,
    loading: bool,
    task: Option<Task<()>>,
}

impl PdfView {
    fn new(item: Entity<PdfItem>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.on_release_in(window, |this, window, _| {
            this.clear_image(window);
            this.clear_continuous_pages(window);
        })
        .detach();
        let mut view = Self {
            item,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            document: None,
            image: None,
            error: None,
            page: 0,
            zoom: 1.0,
            fit_page: true,
            continuous: false,
            continuous_layout: ContinuousLayout::default(),
            continuous_pages: BTreeMap::new(),
            continuous_scroll_offset: None,
            viewport_size: Size::default(),
            loading: false,
            task: None,
        };
        view.load(window, cx);
        view
    }

    fn clear_image(&mut self, window: &mut Window) {
        if let Some(image) = self.image.take() {
            window.drop_image(image).log_err();
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.task = None;
        self.clear_image(window);
        self.clear_continuous_pages(window);
        self.continuous_layout = ContinuousLayout::default();
        self.document = None;
        self.error = None;
        let item = self.item.read(cx);
        let project = item.project.read(cx);
        if !project.is_local() {
            self.loading = false;
            self.error = Some("PDF viewing currently supports local files only.".into());
            cx.notify();
            return;
        }
        let Some(path) = project.absolute_path(&item.project_path, cx) else {
            self.loading = false;
            self.error = Some("The PDF file is no longer available.".into());
            cx.notify();
            return;
        };
        let fs = project.fs().clone();
        self.loading = true;
        let task = cx.background_spawn(async move {
            let bytes = fs.load_bytes(&path).await?;
            LoadedPdf::new(bytes, path).map(Arc::new)
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(document) => {
                        this.page = this
                            .page
                            .min(document.document.total_pages().saturating_sub(1));
                        this.document = Some(document);
                        this.render_page(window, cx);
                    }
                    Err(error) => this.error = Some(format!("Unable to open PDF: {error:#}")),
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.continuous {
            self.reflow_continuous(window, cx);
            return;
        }
        let Some(document) = self.document.clone() else {
            return;
        };
        self.clear_image(window);
        self.error = None;
        self.loading = true;
        let page = self.page;
        let zoom = if self.fit_page { 1.5 } else { self.zoom };
        let scale_factor = window.scale_factor();
        let task =
            cx.background_spawn(async move { document.render_page(page, zoom, scale_factor) });
        // Replacing the task prevents an obsolete page or zoom result from updating this view.
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, _, cx| {
                this.loading = false;
                match result {
                    Ok(image) => this.image = Some(image),
                    Err(error) => this.error = Some(format!("Unable to render PDF: {error:#}")),
                }
                cx.notify();
            })
            .log_err();
        }));
        cx.notify();
    }

    fn change_page(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let total = self
            .document
            .as_ref()
            .map_or(0, |document| document.document.total_pages());
        let page = if forward {
            self.page.saturating_add(1).min(total.saturating_sub(1))
        } else {
            self.page.saturating_sub(1)
        };
        if page != self.page {
            self.page = page;
            if self.continuous {
                self.scroll_to_page();
                self.update_continuous_pages(window, cx);
                cx.notify();
            } else {
                self.scroll_handle.set_offset(gpui::Point::default());
                self.render_page(window, cx);
            }
        }
    }

    fn change_zoom(&mut self, increase: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.continuous && self.fit_page {
            self.zoom = self.continuous_layout.zoom;
        }
        self.fit_page = false;
        self.zoom = (self.zoom * if increase { 1.25 } else { 0.8 }).clamp(0.25, 4.0);
        self.render_page(window, cx);
    }

    fn clear_continuous_pages(&mut self, window: &mut Window) {
        for (_, page) in std::mem::take(&mut self.continuous_pages) {
            if let Some(image) = page.image {
                window.drop_image(image).log_err();
            }
        }
    }

    fn toggle_continuous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.continuous = !self.continuous;
        self.task = None;
        self.clear_image(window);
        self.clear_continuous_pages(window);
        self.scroll_handle.set_offset(gpui::Point::default());
        self.render_page(window, cx);
    }

    fn scroll_to_page(&mut self) {
        self.continuous_scroll_offset = None;
        if let Some(bounds) = self.continuous_layout.pages.get(self.page) {
            self.scroll_handle.set_offset(point(
                self.scroll_handle.offset().x,
                -(bounds.top() - px(PAGE_GAP)),
            ));
        }
    }

    fn reflow_continuous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = &self.document else {
            return;
        };
        let dimensions = (0..document.document.total_pages())
            .filter_map(|page| document.document.page_size(page))
            .map(|dimensions| (dimensions.width, dimensions.height))
            .collect::<Vec<_>>();
        self.continuous_layout = ContinuousLayout::new(
            &dimensions,
            f32::from(self.viewport_size.width),
            self.zoom,
            self.fit_page,
        );
        self.clear_continuous_pages(window);
        self.error = None;
        self.loading = false;
        self.scroll_to_page();
        self.update_continuous_pages(window, cx);
        cx.notify();
    }

    fn update_continuous_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.document.clone() else {
            return;
        };
        if self.viewport_size.width <= px(0.0) || self.viewport_size.height <= px(0.0) {
            return;
        }
        let offset = -self.scroll_handle.offset().y;
        let range = self
            .continuous_layout
            .visible_pages(offset, self.viewport_size.height);
        let mut changed = false;
        self.continuous_pages.retain(|page, rendered| {
            if range.contains(page) {
                true
            } else {
                if let Some(image) = rendered.image.take() {
                    window.drop_image(image).log_err();
                }
                changed = true;
                false
            }
        });
        for page in range {
            if self.continuous_pages.contains_key(&page) {
                continue;
            }
            let zoom = self.continuous_layout.zoom;
            let scale_factor = window.scale_factor();
            let task = cx.background_spawn({
                let document = document.clone();
                async move { document.render_page(page, zoom, scale_factor) }
            });
            let task = cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                this.update_in(cx, |this, _, cx| {
                    if let Some(rendered) = this.continuous_pages.get_mut(&page) {
                        match result {
                            Ok(image) => rendered.image = Some(image),
                            Err(error) => {
                                rendered.error = Some(format!("Unable to render PDF: {error:#}"))
                            }
                        }
                        rendered.task = None;
                        cx.notify();
                    }
                })
                .log_err();
            });
            self.continuous_pages.insert(
                page,
                ContinuousPage {
                    task: Some(task),
                    ..Default::default()
                },
            );
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    fn update_continuous_viewport(
        &mut self,
        viewport_size: Size<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.continuous {
            return;
        }
        if self.viewport_size != viewport_size {
            self.viewport_size = viewport_size;
            self.reflow_continuous(window, cx);
            return;
        }
        let offset = -self.scroll_handle.offset().y;
        // Navigation can be clamped when a page is shorter than the viewport.
        // Keep the selected page until the user scrolls.
        if self
            .continuous_scroll_offset
            .is_some_and(|previous| previous != offset)
        {
            let page = if offset > px(0.0)
                && offset + self.viewport_size.height >= self.continuous_layout.size.height
            {
                self.continuous_layout.pages.len().saturating_sub(1)
            } else {
                self.continuous_layout.page_at(offset)
            };
            if self.page != page {
                self.page = page;
                cx.notify();
            }
        }
        self.continuous_scroll_offset = Some(offset);
        self.update_continuous_pages(window, cx);
    }

    fn render_continuous(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut pages = div()
            .relative()
            .flex_shrink_0()
            .w(self.continuous_layout.size.width)
            .h(self.continuous_layout.size.height);
        for (index, rendered) in &self.continuous_pages {
            let Some(bounds) = self.continuous_layout.pages.get(*index) else {
                continue;
            };
            let mut page = div()
                .absolute()
                .left(bounds.left())
                .top(bounds.top())
                .w(bounds.size.width)
                .h(bounds.size.height)
                .bg(gpui::white());
            if let Some(image) = &rendered.image {
                page = page.child(img(image.clone()).size_full());
            } else if let Some(error) = &rendered.error {
                page = page.child(Label::new(error.clone()).color(Color::Error));
            } else {
                page = page
                    .child(Label::new(format!("Loading page {}…", index + 1)).color(Color::Muted));
            }
            pages = pages.child(page);
        }
        let view = cx.weak_entity();
        div()
            .id("pdf-continuous")
            .debug_selector(|| "pdf-continuous".into())
            .relative()
            .size_full()
            .overflow_scroll()
            .track_scroll(&self.scroll_handle)
            .child(pages)
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        window.defer(cx, move |window, cx| {
                            view.update(cx, |view, cx| {
                                view.update_continuous_viewport(bounds.size, window, cx);
                            })
                            .log_err();
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
    }
}

impl EventEmitter<()> for PdfView {}

impl Focusable for PdfView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PdfView {
    type Event = ();

    fn tab_content_text(&self, _: usize, cx: &App) -> SharedString {
        let item = self.item.read(cx);
        let path = item.project.read(cx).absolute_path(&item.project_path, cx);
        path.as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .or_else(|| item.project_path.path.file_name().map(str::to_owned))
            .unwrap_or_else(|| "PDF".to_owned())
            .into()
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        let item = self.item.read(cx);
        Some(
            item.project
                .read(cx)
                .absolute_path(&item.project_path, cx)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| item.project_path.path.to_string())
                .into(),
        )
    }

    fn for_each_project_item(
        &self,
        cx: &App,
        callback: &mut dyn FnMut(EntityId, &dyn project::ProjectItem),
    ) {
        callback(self.item.entity_id(), self.item.read(cx));
    }

    fn buffer_kind(&self, _: &App) -> ItemBufferKind {
        ItemBufferKind::Singleton
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        _: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>> {
        Task::ready(Some(cx.new(|cx| Self::new(self.item.clone(), window, cx))))
    }
}

impl ProjectItem for PdfView {
    type Item = PdfItem;

    fn for_project_item(
        _: Entity<Project>,
        _: Option<&Pane>,
        item: Entity<PdfItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(item, window, cx)
    }
}

impl Render for PdfView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self
            .document
            .as_ref()
            .map_or(0, |document| document.document.total_pages());
        let mut content = div()
            .id("pdf-page")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .when(!self.continuous, |content| {
                content
                    .overflow_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p_4()
            });
        if let Some(error) = &self.error {
            content = content.child(Label::new(error.clone()).color(Color::Error));
        } else if self.loading {
            content = content.child(Label::new("Loading PDF…"));
        } else if self.continuous {
            content = content.child(self.render_continuous(cx));
        } else if let Some(image) = &self.image {
            let page = img(image.clone());
            if self.fit_page {
                let image = image.clone();
                content = content.child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let image_bounds =
                                gpui::ObjectFit::Contain.get_bounds(bounds, image.size(0));
                            window
                                .paint_image(
                                    bounds,
                                    image_bounds,
                                    Default::default(),
                                    image.clone(),
                                    0,
                                    false,
                                )
                                .log_err();
                        },
                    )
                    .absolute()
                    .inset_4(),
                );
            } else if let Some(dimensions) = self
                .document
                .as_ref()
                .and_then(|document| document.document.page_size(self.page))
            {
                content = content.child(
                    page.w(px(dimensions.width * self.zoom))
                        .h(px(dimensions.height * self.zoom)),
                );
            }
        }
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().colors().editor_background)
            .child(
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        Button::new("previous-page", "Previous")
                            .disabled(self.page == 0 || total == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.change_page(false, window, cx)
                            })),
                    )
                    .child(Label::new(if total == 0 {
                        "— / —".into()
                    } else {
                        format!("{} / {total}", self.page + 1)
                    }))
                    .child(
                        div().debug_selector(|| "pdf-next-page".into()).child(
                            Button::new("next-page", "Next")
                                .disabled(total == 0 || self.page + 1 >= total)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.change_page(true, window, cx)
                                })),
                        ),
                    )
                    .child(Button::new("zoom-out", "−").disabled(total == 0).on_click(
                        cx.listener(|this, _, window, cx| this.change_zoom(false, window, cx)),
                    ))
                    .child(Label::new(if self.fit_page {
                        if self.continuous {
                            "Fit width"
                        } else {
                            "Fit page"
                        }
                        .into()
                    } else {
                        format!("{:.0}%", self.zoom * 100.0)
                    }))
                    .child(Button::new("zoom-in", "+").disabled(total == 0).on_click(
                        cx.listener(|this, _, window, cx| this.change_zoom(true, window, cx)),
                    ))
                    .child(
                        Button::new(
                            "fit-page",
                            if self.continuous {
                                "Fit width"
                            } else {
                                "Fit page"
                            },
                        )
                        .disabled(total == 0)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.fit_page = true;
                            this.scroll_handle.set_offset(gpui::Point::default());
                            this.render_page(window, cx);
                        })),
                    )
                    .child(
                        Button::new("continuous-scrolling", "Continuous")
                            .toggle_state(self.continuous)
                            .disabled(total == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_continuous(window, cx);
                            })),
                    )
                    .child(
                        Button::new("reload-pdf", "Reload")
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                    ),
            )
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires native Pdfium; run script/install-pdfium first"]
    fn renders_real_pdf_pages() -> Result<()> {
        let document = LoadedPdf::new(
            include_bytes!("../test_data/two-pages.pdf").to_vec(),
            PathBuf::from("two-pages.pdf"),
        )?;
        assert_eq!(document.document.total_pages(), 2);
        let portrait = document.render_page(0, 1.0, 1.0)?;
        assert_eq!(portrait.size(0).width.0, 200);
        assert_eq!(portrait.size(0).height.0, 300);
        assert_eq!(
            portrait.as_bytes(0).and_then(|bytes| bytes.get(..4)),
            Some([0, 0, 255, 255].as_slice())
        );
        let landscape = document.render_page(1, 2.0, 1.0)?;
        assert_eq!(landscape.size(0).width.0, 600);
        assert_eq!(landscape.size(0).height.0, 400);
        assert_eq!(
            landscape.as_bytes(0).and_then(|bytes| bytes.get(..4)),
            Some([255, 0, 0, 255].as_slice())
        );
        let bounded = document.render_page(0, 100.0, 2.0)?;
        assert!(bounded.size(0).width.0 <= MAX_RASTER_DIMENSION as i32);
        assert!(bounded.size(0).height.0 <= MAX_RASTER_DIMENSION as i32);
        assert!(document.render_page(2, 1.0, 1.0).is_err());
        assert!(LoadedPdf::new(b"%PDF-broken".to_vec(), PathBuf::from("broken.pdf")).is_err());
        Ok(())
    }

    #[gpui::test]
    #[ignore = "requires native Pdfium; run script/install-pdfium first"]
    async fn opens_and_navigates_single_file_pdf(cx: &mut gpui::TestAppContext) {
        use fs::{FakeFs, Fs as _};
        use project::ProjectItem as _;
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
        let fs = FakeFs::new(cx.executor());
        let path = Path::new("/documents/report.PDF");
        fs.create_dir(Path::new("/documents"))
            .await
            .expect("create test directory");
        fs.insert_file(path, include_bytes!("../test_data/two-pages.pdf").to_vec())
            .await;
        let project = Project::test(fs, [path], cx).await;
        let item = cx
            .update(|cx| {
                let worktree = project
                    .read(cx)
                    .worktrees(cx)
                    .next()
                    .expect("single-file worktree");
                let worktree = worktree.read(cx);
                assert!(worktree.is_single_file());
                let project_path = ProjectPath {
                    worktree_id: worktree.id(),
                    path: worktree.root_entry().expect("root entry").path.clone(),
                };
                assert!(project_path.path.extension().is_none());
                PdfItem::try_open(&project, &project_path, cx)
                    .expect("PDF opener handles single files")
            })
            .await
            .expect("open PDF item");
        let (view, cx) = cx.add_window_view(|window, cx| PdfView::new(item, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(view.read(cx).tab_content_text(0, cx), "report.PDF");
            assert!(view.read(cx).image.is_some(), "{:?}", view.read(cx).error);
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let bounds = cx
            .debug_bounds("pdf-next-page")
            .expect("Next button bounds");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(view.read(cx).page, 1, "Next button must navigate");
            assert!(view.read(cx).image.is_some(), "{:?}", view.read(cx).error);
            view.update(cx, |view, cx| view.change_zoom(true, window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(!view.read(cx).fit_page);
            assert_eq!(view.read(cx).zoom, 1.25);
            view.update(cx, |view, cx| view.load(window, cx));
        });
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(view.read(cx).page, 1, "Reload preserves page");
            assert_eq!(view.read(cx).zoom, 1.25, "Reload preserves zoom");
            assert!(view.read(cx).image.is_some(), "{:?}", view.read(cx).error);
        });
        view.update_in(cx, |view, window, cx| {
            view.zoom = 4.0;
            view.toggle_continuous(window, cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let viewer = view.read(cx);
            assert!(viewer.continuous);
            assert!(viewer.image.is_none());
            assert_eq!(viewer.page, 1, "Changing mode preserves the page");
            assert_eq!(viewer.continuous_pages.len(), 2);
            assert!(
                viewer
                    .continuous_pages
                    .values()
                    .all(|page| page.image.is_some())
            );
            view.update(cx, |view, cx| view.change_page(false, window, cx));
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(view.read(cx).page, 0);
            assert_eq!(view.read(cx).scroll_handle.offset().y, px(0.0));
        });
        let bounds = cx
            .debug_bounds("pdf-continuous")
            .expect("continuous viewport");
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-1232.0))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(view.read(cx).page, 1, "Scrolling updates the page counter");
            assert!(view.read(cx).scroll_handle.offset().y < px(0.0));
            view.update(cx, |view, cx| view.load(window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(view.read(cx).continuous, "Reload preserves the mode");
            assert_eq!(view.read(cx).page, 1);
            view.update(cx, |view, cx| {
                view.fit_page = true;
                view.render_page(window, cx);
            });
            window.resize(size(px(800.0), px(600.0)));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let viewer = view.read(cx);
            assert_eq!(viewer.page, 1, "Resizing preserves the selected page");
            let widest = viewer.continuous_layout.pages.get(1).expect("second page");
            assert_eq!(
                widest.size.width + px(PAGE_GAP * 2.0),
                viewer.viewport_size.width
            );
            view.update(cx, |view, cx| {
                view.fit_page = false;
                view.zoom = 0.25;
                view.render_page(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert_eq!(
                view.read(cx).page,
                1,
                "Short pages preserve explicit navigation"
            );
            assert_eq!(view.read(cx).scroll_handle.offset().y, px(0.0));
            view.update(cx, |view, cx| view.toggle_continuous(window, cx));
        });
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(!view.read(cx).continuous);
            assert_eq!(view.read(cx).page, 1);
            assert!(view.read(cx).continuous_pages.is_empty());
            assert!(view.read(cx).image.is_some());
        });
    }

    #[test]
    fn recognizes_single_file_worktrees() {
        assert!(is_pdf(None, Some(Path::new("/documents/report.PDF"))));
        assert!(!is_pdf(None, Some(Path::new("/documents/report.txt"))));
    }

    #[test]
    fn recognizes_pdf_extensions_only() {
        assert!(is_pdf(Some("pdf"), None));
        assert!(is_pdf(Some("PDF"), None));
        assert!(!is_pdf(Some("pdf.bak"), None));
        assert!(!is_pdf(None, None));
    }

    #[test]
    fn continuous_layout_fits_and_centers_mixed_page_sizes() {
        let layout = ContinuousLayout::new(&[(200.0, 300.0), (300.0, 200.0)], 632.0, 1.0, true);
        assert_eq!(layout.zoom, 2.0);
        assert_eq!(layout.size, size(px(632.0), px(1048.0)));
        assert_eq!(
            layout.pages.first(),
            Some(&gpui::Bounds::new(
                point(px(116.0), px(16.0)),
                size(px(400.0), px(600.0))
            ))
        );
        assert_eq!(
            layout.pages.get(1),
            Some(&gpui::Bounds::new(
                point(px(16.0), px(632.0)),
                size(px(600.0), px(400.0))
            ))
        );
        assert_eq!(layout.page_at(px(0.0)), 0);
        assert_eq!(layout.page_at(px(616.0)), 1);
    }

    #[test]
    fn continuous_layout_limits_rendering_to_visible_and_neighboring_pages() {
        let dimensions = vec![(200.0, 300.0); 100];
        let layout = ContinuousLayout::new(&dimensions, 232.0, 1.0, false);
        assert_eq!(layout.visible_pages(px(0.0), px(300.0)), 0..2);
        assert_eq!(layout.visible_pages(px(3160.0), px(300.0)), 9..12);
        assert_eq!(layout.visible_pages(px(31284.0), px(316.0)), 98..100);
        let zoomed = ContinuousLayout::new(&dimensions, 232.0, 2.0, false);
        assert_eq!(zoomed.size.width, px(432.0));
        assert_eq!(
            zoomed.pages.first().map(|page| page.size.height),
            Some(px(600.0))
        );
    }
}
