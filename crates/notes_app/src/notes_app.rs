//! zednotes' application layer.
//!
//! Everything notes-specific lives in this crate rather than in the generic Zed
//! crates of the fork, so that merges from `upstream` stay cheap. At this stage
//! the crate only owns the application shell: the window, its lifecycle, and the
//! actions that drive them.

use anyhow::Result;
use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions,
    Window, WindowBounds, WindowHandle, WindowOptions, actions, div, prelude::*, px, rgb, size,
};

const APP_NAME: &str = "zednotes";

actions!(
    notes,
    [
        /// Quits zednotes.
        Quit,
        /// Closes the focused window.
        CloseWindow,
    ]
);

/// The root view of a notes window. It is empty until the editor lands; its job
/// for now is to own the window's focus and key context.
pub struct NotesWindow {
    focus_handle: FocusHandle,
}

impl NotesWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self { focus_handle }
    }
}

impl Focusable for NotesWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NotesWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .size_full()
            .bg(rgb(0x1c1c1c))
    }
}

/// Call this once, before opening any window.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
    ]);
    cx.set_menus([Menu::new(APP_NAME).items([MenuItem::action("Quit", Quit)])]);
    cx.on_window_closed(|cx, _window_id| {
        // zednotes has no dock-resident or menu-bar-only mode, so once the last
        // window is gone there is nothing left to return to.
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// `appears_transparent: false` keeps the titlebar — and therefore the traffic
/// lights — drawn by macOS rather than by us.
fn notes_window_options(cx: &mut App) -> WindowOptions {
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

pub fn open_notes_window(cx: &mut App) -> Result<WindowHandle<NotesWindow>> {
    let options = notes_window_options(cx);
    cx.open_window(options, |window, cx| {
        window.set_window_title(APP_NAME);
        cx.new(|cx| NotesWindow::new(window, cx))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    fn open_window(cx: &mut TestAppContext) -> VisualTestContext {
        let window = cx
            .update(open_notes_window)
            .expect("opening the notes window failed");
        VisualTestContext::from_window(window.into(), cx)
    }

    #[gpui::test]
    fn test_notes_windows_keep_the_native_macos_titlebar(cx: &mut TestAppContext) {
        let titlebar = cx
            .update(notes_window_options)
            .titlebar
            .expect("a notes window must have a titlebar");

        assert!(
            !titlebar.appears_transparent,
            "the traffic lights must be drawn by macOS, not by us"
        );
    }

    #[gpui::test]
    fn test_opening_a_notes_window(cx: &mut TestAppContext) {
        cx.update(init);

        let mut window_cx = open_window(cx);

        assert_eq!(cx.windows().len(), 1);
        assert_eq!(window_cx.window_title().as_deref(), Some(APP_NAME));
    }

    #[gpui::test]
    fn test_close_window_action_closes_the_window(cx: &mut TestAppContext) {
        cx.update(init);
        let mut window_cx = open_window(cx);

        window_cx.dispatch_action(CloseWindow);
        window_cx.run_until_parked();

        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn test_cmd_w_closes_the_window(cx: &mut TestAppContext) {
        cx.update(init);
        let mut window_cx = open_window(cx);

        window_cx.simulate_keystrokes("cmd-w");

        assert!(cx.windows().is_empty());
    }

    #[gpui::test]
    fn test_closing_one_of_two_windows_leaves_the_other_open(cx: &mut TestAppContext) {
        cx.update(init);
        let mut first_window_cx = open_window(cx);
        let mut second_window_cx = open_window(cx);

        first_window_cx.dispatch_action(CloseWindow);
        first_window_cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);

        second_window_cx.dispatch_action(CloseWindow);
        second_window_cx.run_until_parked();
        assert!(cx.windows().is_empty());
    }
}
