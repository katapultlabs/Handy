//! Dictionary commands. Thin wrappers over `dictionary_store::DictionaryManager`.

use crate::dictionary::CaseMode;
use crate::dictionary_store::{DictionaryManager, DictionaryRow, LearnReport};
use std::sync::Arc;
use tauri::{AppHandle, State};

#[tauri::command]
#[specta::specta]
pub async fn list_dictionary_entries(
    manager: State<'_, Arc<DictionaryManager>>,
) -> Result<Vec<DictionaryRow>, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || manager.list())
        .await
        .map_err(|err| format!("Dictionary list task panicked: {err}"))?
        .map_err(|err| err.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn add_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    wrong: String,
    right: String,
) -> Result<DictionaryRow, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || manager.add_manual(&wrong, &right))
        .await
        .map_err(|err| format!("Dictionary add task panicked: {err}"))?
        .map_err(|err| err.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn update_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
    wrong: String,
    right: String,
    case_mode: CaseMode,
    active: bool,
) -> Result<DictionaryRow, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || {
        manager.update(id, &wrong, &right, case_mode, active)
    })
    .await
    .map_err(|err| format!("Dictionary update task panicked: {err}"))?
    .map_err(|err| err.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn confirm_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
) -> Result<DictionaryRow, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || manager.confirm_dictionary_entry(id))
        .await
        .map_err(|err| format!("Dictionary confirmation task panicked: {err}"))?
        .map_err(|err| err.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn reject_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
) -> Result<DictionaryRow, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || manager.reject_dictionary_entry(id))
        .await
        .map_err(|err| format!("Dictionary rejection task panicked: {err}"))?
        .map_err(|err| err.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
) -> Result<(), String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || manager.delete(id))
        .await
        .map_err(|err| format!("Dictionary delete task panicked: {err}"))?
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Diff the text Handy pasted against the user's edit (History screen),
/// then store what the learn gates accept.
#[tauri::command]
#[specta::specta]
pub async fn learn_dictionary_from_edit(
    app: AppHandle,
    manager: State<'_, Arc<DictionaryManager>>,
    original: String,
    corrected: String,
) -> Result<LearnReport, String> {
    let manager = Arc::clone(&manager);
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<LearnReport> {
        let settings = crate::settings::get_settings(&app);
        if !settings.experimental_enabled || !settings.dictionary_enabled {
            return Ok(LearnReport {
                added: Vec::new(),
                known: Vec::new(),
            });
        }
        manager.learn_from_edit(&original, &corrected, "history")
    })
    .await
    .map_err(|err| format!("Dictionary learning task panicked: {err}"))?
    .map_err(|err| err.to_string())
}
