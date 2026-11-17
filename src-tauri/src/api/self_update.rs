//! Commandes Tauri de l'auto-amélioration (J14, spec décisions 15 à 19).
//! `attach_sessions` est dans `commands.rs` (sessions).
//!
//! Étape 0 : corps provisoires. `get_ui_info` et `ui_ready` répondent
//! (interface embarquée, rien d'attendu) ; `improve_start`,
//! `improve_cancel` et `ui_rollback` renvoient `NotImplemented`.
//! Propriétaire : l'intégration (étape 5).

use tauri::{AppHandle, State};

use super::types::{CoreError, ImproveStatus, UiInfo};
use crate::selfupdate::SelfUpdate;
use crate::ui::UiHost;

fn ui_info(ui: &UiHost, su: &SelfUpdate) -> UiInfo {
    let active = ui.active();
    UiInfo {
        source: active.source,
        active_hash: active.hash,
        previous_hash: active.previous,
        build_commit: crate::build_info::BUILD_COMMIT.into(),
        contract: crate::build_info::CONTRACT.into(),
        restart_required: su.restart_required.lock().unwrap().clone(),
        repo: su.repo.lock().unwrap().clone(),
        improve: su.current.lock().unwrap().clone(),
    }
}

/// Interface affichée, état du clone et de l'amélioration en cours.
#[tauri::command(rename_all = "snake_case")]
pub fn get_ui_info(ui: State<'_, UiHost>, su: State<'_, SelfUpdate>) -> UiInfo {
    ui_info(&ui, &su)
}

/// Signal « prêt » de l'interface, après son montage et `attach_sessions`.
/// `hash` : `__UI_HASH__` de l'interface (`None` : embarquée). Renvoie vrai
/// si ce signal était attendu par le watchdog.
#[tauri::command(rename_all = "snake_case")]
pub fn ui_ready(ui: State<'_, UiHost>, hash: Option<String>) -> bool {
    ui.ready(hash.as_deref())
}

/// Lance une amélioration (`/ameliorer <request>`) ; l'avancement arrive par
/// l'événement global `improve_status` (`IMPROVE_EVENT`).
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_start(request: String) -> Result<ImproveStatus, CoreError> {
    let _ = request;
    Err(CoreError::NotImplemented("improve_start".into()))
}

/// Annule l'amélioration `id` en cours.
#[tauri::command(rename_all = "snake_case")]
pub async fn improve_cancel(id: String) -> Result<(), CoreError> {
    let _ = id;
    Err(CoreError::NotImplemented("improve_cancel".into()))
}

/// Revient à la version précédente de l'interface (puis `git revert` par le
/// cycle d'amélioration).
#[tauri::command(rename_all = "snake_case")]
pub fn ui_rollback(
    app: AppHandle,
    ui: State<'_, UiHost>,
    su: State<'_, SelfUpdate>,
) -> Result<UiInfo, CoreError> {
    ui.rollback(&app)?;
    Ok(ui_info(&ui, &su))
}
