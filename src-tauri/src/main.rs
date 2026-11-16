// Pas de console pour l'app en release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    orchestrator_lib::run()
}
