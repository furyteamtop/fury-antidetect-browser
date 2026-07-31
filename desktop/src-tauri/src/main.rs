//! Fury desktop — the native shell.
//!
//! Thin by design. The shell owns the window, the connection to the server and
//! the session token; everything an operator sees is the React app under
//! `../src`, and everything that touches a profile will be the agent
//! (docs/01-architecture.md). What lives here is what a web page cannot do:
//! keep a credential out of the document, reach a self-hosted server without
//! asking it for CORS, and refuse to run twice.

// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod commands;
mod session;
mod settings;

use std::sync::Mutex;
use std::time::Duration;

use tauri::Manager;

use commands::AppState;
use session::Session;
use settings::Settings;

fn main() {
    tauri::Builder::default()
        // Registered first, because plugins run in the order they are added and
        // this one has to decide whether the process should exist at all.
        //
        // What it is for: one settings file and one keychain binding per
        // machine, and the behaviour people expect from a dock icon. What it is
        // *not* for is protecting the profile lock — that lock belongs to the
        // agent (docs/01-architecture.md), and the case it would have to prevent
        // is the same user on two machines, where a single-instance guard on one
        // of them cannot help. The lock has to defend itself server-side.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let settings = Settings::load(&config_dir);

            let session = Session::new();
            if let Some(url) = settings.server_url.as_deref() {
                session.bind(url);
            }

            let http = reqwest::Client::builder()
                // Long enough for a loaded self-hosted box, short enough that a
                // black-holed address reports itself rather than hanging the UI.
                .timeout(Duration::from_secs(20))
                .user_agent(concat!("fury-desktop/", env!("CARGO_PKG_VERSION")))
                .build()?;

            // Touch the credential store once, here, where a failure is
            // visible. keyring 4 marks its store "initialised" *before* it
            // builds one (v1.rs), so a single transient failure — a locked
            // keychain, a denied prompt — poisons the process: every later
            // Entry::new skips setup and fails against a store that was never
            // installed. Probing at startup turns that into one line in the log
            // at the moment it happens, instead of a session that silently
            // stops being remembered.
            if let Err(e) = keyring::Entry::new("dev.fury.desktop", "startup-probe") {
                eprintln!("keychain unavailable — sessions will not be remembered: {e}");
            }

            app.manage(AppState {
                http,
                settings: Mutex::new(settings),
                config_dir,
                session,
                locks: Default::default(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::shell_state,
            commands::set_server,
            commands::disconnect_server,
            commands::login,
            commands::logout,
            commands::me,
            commands::projects,
            commands::profiles,
            commands::launch,
            commands::stop,
            commands::personas,
            commands::preview,
            commands::proxies,
            commands::save_proxy,
            commands::delete_proxy,
            commands::save_profile,
            commands::delete_profile,
            commands::create_project,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Fury");
}
