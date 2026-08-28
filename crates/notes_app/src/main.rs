use assets::Assets;
use db::{AppDatabase, kvp::KeyValueStore};
use gpui_platform::application;
use session::Session;
use std::io::{self, IsTerminal as _};
use uuid::Uuid;

fn init_logging() {
    zlog::init();

    let file_logging_ready = match std::fs::create_dir_all(notes_app::logs_dir()) {
        Ok(()) => {
            match zlog::init_output_file(notes_app::log_file(), Some(notes_app::old_log_file())) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("Could not open zednotes log file: {error}");
                    false
                }
            }
        }
        Err(error) => {
            eprintln!("Could not create zednotes log directory: {error}");
            false
        }
    };

    if io::stdout().is_terminal() {
        zlog::init_output_stdout();
    } else if !file_logging_ready {
        zlog::init_output_stderr();
    }
}

fn main() {
    if std::env::args_os().any(|arg| arg == "--printenv") {
        util::shell_env::print_env();
        return;
    }

    init_logging();
    log::info!("starting zednotes {}", env!("CARGO_PKG_VERSION"));

    let data_dir = notes_app::application_support_dir();
    paths::set_custom_data_dir(data_dir.to_string_lossy().as_ref());
    let application = application().with_assets(Assets);
    application.on_reopen(notes_app::reopen);
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
        notes_app::start(cx);
    });
    zlog::flush();
}
