use assets::Assets;
use db::{AppDatabase, kvp::KeyValueStore};
use gpui_platform::application;
use session::Session;
use uuid::Uuid;

fn main() {
    if std::env::args_os().any(|arg| arg == "--printenv") {
        util::shell_env::print_env();
        return;
    }

    let application = application().with_assets(Assets);
    let database = AppDatabase::new();
    let session = application.background_executor().spawn(Session::new(
        Uuid::new_v4().to_string(),
        KeyValueStore::from_app_db(&database),
    ));

    application.run(move |cx| {
        cx.set_global(database);
        cx.activate(true);
        Assets
            .load_fonts(cx)
            .expect("failed to load zednotes' bundled fonts");
        let session = cx.foreground_executor().block_on(session);
        notes_app::init(session, cx).expect("failed to initialize the editor and Vim subsystems");
        notes_app::open_notes_window(cx)
            .expect("failed to open the notes window; there is nothing to fall back to");
    });
}
