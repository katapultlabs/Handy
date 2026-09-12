//! Dictionary commands. Thin wrappers over `dictionary_store::DictionaryManager`.

use crate::dictionary::{learn_pairs, CaseMode};
use crate::dictionary_store::{DictionaryManager, DictionaryRow, LearnReport};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub fn list_dictionary_entries(
    manager: State<'_, Arc<DictionaryManager>>,
) -> Result<Vec<DictionaryRow>, String> {
    manager.list().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn add_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    wrong: String,
    right: String,
) -> Result<DictionaryRow, String> {
    manager
        .add_manual(&wrong, &right)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn update_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
    wrong: String,
    right: String,
    case_mode: CaseMode,
    active: bool,
) -> Result<DictionaryRow, String> {
    manager
        .update(id, &wrong, &right, case_mode, active)
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn delete_dictionary_entry(
    manager: State<'_, Arc<DictionaryManager>>,
    id: i64,
) -> Result<(), String> {
    manager.delete(id).map(|_| ()).map_err(|e| e.to_string())
}

/// Diff the text Handy pasted against the user's edit (History screen),
/// then store what the learn gates accept.
#[tauri::command]
#[specta::specta]
pub fn learn_dictionary_from_edit(
    manager: State<'_, Arc<DictionaryManager>>,
    original: String,
    corrected: String,
) -> Result<LearnReport, String> {
    let pairs = learn_pairs(&original, &corrected);
    if pairs.is_empty() {
        return Ok(LearnReport {
            added: Vec::new(),
            known: Vec::new(),
        });
    }
    manager.learn(&pairs, "history").map_err(|e| e.to_string())
}
