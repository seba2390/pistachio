//! Pistachio: a lean desktop GUI for the pi coding agent.
//!
//! The GUI is a pure view over `pi --mode rpc`: every agent behaviour comes from pi
//! itself; this binary only renders pi's event stream and forwards user input.

// A GUI app: no console window on Windows. (Pipes still work, e.g. for ssh's askpass.)
#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod assets;
mod builtin_skills;
mod chat;
mod code_colors;
mod code_highlight;
mod config;
mod demo;
mod document_parsers;
mod hosts;
mod local_models;
mod model_store;
mod naming;
mod online;
mod panels;
mod perf;
mod persist;
mod pi_place;
mod relay;
mod remote_ui;
mod search_server;
mod security;
mod settings;
mod shell_env;
mod sidebar;
mod skills;
mod stt;
mod syntax;
mod theme;
mod ui;
mod ui_state;
mod updater;
mod updates;
mod whats_new;
mod window;

use gpui_kit::*;

fn main() {
    // Started by the deep search pi extension to run a web tool: do it and exit, no UI.
    if let Some(code) = deepsearch::cli::run_if_requested() {
        std::process::exit(code);
    }
    // Started by the chats' python tool to run the model's code in Monty.
    if let Some(code) = python_tool::cli::run_if_requested() {
        std::process::exit(code);
    }
    // Started by a chat, or by the pi extension of Code sessions, to read a document.
    if let Some(code) = documents::cli::run_if_requested() {
        std::process::exit(code);
    }
    // Started by the sandbox's pi extension (or the Security settings) to run a command in the
    // sandbox, or to say what it allows.
    if let Some(code) = sandbox::cli::run_if_requested() {
        std::process::exit(code);
    }
    // Started by ssh as its askpass program (a password or passphrase prompt): hand the
    // question to the running app and exit, before any UI starts.
    if let Some(code) = remote::askpass::run_client_if_requested() {
        std::process::exit(code);
    }
    // After the helper modes above: those runs are child processes of the app and must not
    // touch its PISTACHIO_PERF file.
    perf::init();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,pistachio=info"),
    )
    .init();
    // Settings saved before the rename from PI-gui move to the new folder, once.
    if let Some(agent) = pi_sessions::agent_dir()
        && let Err(error) = pi_sessions::migrate_legacy_app_dir(&agent)
    {
        log::warn!("could not move settings from the PI-gui folder: {error}");
    }

    // Probing the login shell takes ~50–500 ms depending on the user's shell setup. Nothing
    // needs it until the first pi or terminal starts, so it runs in the background and
    // startup never waits for it.
    let child_env = config::ChildEnv::probe();

    let app = application().with_assets(assets::AppAssets);
    app.on_reopen(window::reopen);
    app.run(move |cx| {
        perf::phase("app_run");
        let config = config::AppConfig::detect(child_env);
        // The proxy that lets sandboxed commands reach the sites the user allowed. It waits
        // on its sockets and costs nothing until a command connects.
        if sandbox::SUPPORTED
            && let Err(error) = sandbox::proxy::start(&config.sandbox_dir)
        {
            log::warn!("a limited network for sandboxed commands is unavailable: {error}");
        }
        init(cx);
        perf::phase("gpui_kit_init");
        cx.set_global(settings::AppSettings::load(
            settings::AppSettings::path().as_deref(),
        ));
        theme::init(cx);
        syntax::init();
        stt::init(cx);
        naming::init(cx);
        settings::init_text_size(cx);
        actions::init(cx);
        cx.set_global(config);
        search_server::init(cx);
        local_models::init(cx);
        settings::apply_pi_program(cx);
        cx.set_global(ui_state::UiState::load(
            ui_state::UiState::path().as_deref(),
        ));
        cx.set_global(chat::recall::Recall::load(
            chat::recall::Recall::path().as_deref(),
        ));
        cx.set_global(chat::thinking_notes::ThinkingNotes::load(
            chat::thinking_notes::ThinkingNotes::path().as_deref(),
        ));
        cx.set_global(chat::token_rate::TokenRates::load(
            chat::token_rate::TokenRates::path().as_deref(),
        ));
        window::init(cx);
        perf::phase("app_init");
        window::open_main_window(cx);
        perf::phase("window_opened");
        cx.activate(true);
    });
}
