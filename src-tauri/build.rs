// Déclare les commandes de l'app : tauri-build génère pour chacune une
// permission `allow-<commande>` / `deny-<commande>` (kebab-case). Une commande
// absente de la capability de la fenêtre est alors refusée à l'appel.
const COMMANDS: &[&str] = &[
    "get_launch_info",
    "start_session",
    "send_message",
    "interrupt",
    "set_model",
    "answer_question",
    "skip_question",
    "list_sessions",
    "load_history",
    "stop_session",
    "get_diagnostics",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("échec de tauri-build");
}
