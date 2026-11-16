//! Cœur de l'application : pilote `claude.exe` en stream-json et relaie des
//! `CoreEvent` normalisés à l'interface Svelte.

pub mod api;
pub mod colors;
pub mod errors;
pub mod logging;
pub mod process;
pub mod protocol;
pub mod state;

use api::commands::{self, AppState};
use tauri::Manager;

pub fn run() {
    // Journal dans %LOCALAPPDATA%\orchestrator\logs (plan 1.9) ; l'app
    // continue sans journal s'il est indisponible.
    logging::init();

    // `<app> .` : le premier argument devient le cwd de claude.exe.
    let cwd = process::locate::launch_cwd_from_args(std::env::args())
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| std::path::PathBuf::from("."));

    let app = tauri::Builder::default()
        .manage(AppState::new(cwd))
        // Rechargement de la webview : les canaux des sessions sont perdus,
        // ces sessions orphelines sont fermées (plan J6).
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                let app = webview.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    app.state::<AppState>().close_orphans().await;
                });
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_launch_info,
            commands::start_session,
            commands::send_message,
            commands::interrupt,
            commands::set_model,
            commands::answer_question,
            commands::skip_question,
            commands::list_sessions,
            commands::load_history,
            commands::stop_session,
            commands::get_diagnostics,
        ])
        .build(tauri::generate_context!())
        .expect("échec de la construction de l'app Tauri");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Fermer stdin, attendre 3 s, puis fermer le Job Object (plan 1.9).
            // Si l'app est tuée brutalement, le noyau ferme le Job : même effet.
            let state = handle.state::<AppState>();
            tauri::async_runtime::block_on(state.stop_all());
        }
    });
}
