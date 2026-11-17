//! Cœur de l'application : pilote `claude.exe` en stream-json et relaie des
//! `CoreEvent` normalisés à l'interface Svelte.

pub mod api;
pub mod build_info;
pub mod colors;
pub mod errors;
pub mod logging;
pub mod process;
pub mod protocol;
pub mod selfupdate;
pub mod sessions;
pub mod state;
pub mod ui;

use api::commands::{self, AppState};
use api::self_update;
use tauri::Manager;

pub fn run() {
    // Journal dans %LOCALAPPDATA%\orchestrator\logs (plan 1.9) ; l'app
    // continue sans journal s'il est indisponible.
    logging::init();

    // `<app> .` : le premier argument devient le cwd de claude.exe ; sans
    // argument, l'interface ouvre le sélecteur des projets (décision 11).
    let launch = sessions::launch_dir_from_args(std::env::args());
    if let Some(e) = &launch.error {
        tracing::warn!("dossier de projet en argument inutilisable : {e}");
    }

    let app = tauri::Builder::default()
        .manage(AppState::with_launch(launch))
        // J14, étape 0 : états de l'auto-amélioration, sans effet tant que
        // l'étape 5 ne les branche pas (`set_assets`, watchdog, clone).
        .manage(ui::UiHost::new(
            ui::UiHost::default_root().unwrap_or_default(),
        ))
        .manage(selfupdate::SelfUpdate::default())
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
            commands::list_projects,
            commands::list_sessions,
            commands::load_history,
            commands::stop_session,
            commands::get_diagnostics,
            commands::attach_sessions,
            self_update::ui_ready,
            self_update::get_ui_info,
            self_update::improve_start,
            self_update::improve_cancel,
            self_update::ui_rollback,
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
