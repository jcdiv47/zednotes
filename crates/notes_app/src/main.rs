use gpui_platform::application;

fn main() {
    application().run(|cx| {
        cx.activate(true);
        notes_app::init(cx);
        notes_app::open_notes_window(cx)
            .expect("failed to open the notes window; there is nothing to fall back to");
    });
}
