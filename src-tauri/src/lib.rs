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

    // J14 : interface servie depuis %LOCALAPPDATA%\orchestrator\ui (version
    // active valide), sinon l'interface embarquée, en bloc (décision 15). Le
    // watchdog de démarrage (15 s) s'arme au premier chargement de
    // `/index.html` ; celui de la bascule (5 s) à chaque activation.
    let mut context = tauri::generate_context!();
    let ui = ui::UiHost::new(
        ui::UiHost::default_root()
            .unwrap_or_else(|| std::env::temp_dir().join("orchestrator").join("ui")),
    );
    // Verrous de build laissés par un arrêt brutal : aucun build ne tourne.
    self_update::clear_orphan_build_locks(ui.root());
    ui.install_assets(&mut context);

    let app = tauri::Builder::default()
        .manage(AppState::with_launch(launch))
        .manage(ui)
        .manage(selfupdate::SelfUpdate::default())
        .setup(|app| {
            // Clone de l'app en tâche de fond, cycle d'amélioration géré.
            self_update::setup(app.handle());
            Ok(())
        })
        // Rechargement de la webview (J14, décision 15) : les sessions sont
        // détachées, pas fermées ; la nouvelle page s'y rattache par
        // `attach_sessions`, sinon elles sont fermées après 60 s. Appel
        // direct (pas de tâche) : la génération de page avance avant que la
        // nouvelle page puisse appeler `attach_sessions`.
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Started {
                // Thread de la webview : contexte du runtime pour la
                // fermeture différée (`tokio::spawn`).
                let rt = tauri::async_runtime::handle();
                let _enter = rt.inner().enter();
                webview.app_handle().state::<AppState>().detach_all();
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
        .build(context)
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
