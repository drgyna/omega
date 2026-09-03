mod agent;
mod answer;
mod app;
mod calc;
mod census;
mod conversation;
mod dates;
mod db;
mod error;
pub mod evaluation;
mod extract;
mod indexer;
mod lectura;
#[cfg(test)]
mod migration_tests;
mod model;
mod normalize;
pub mod ocr;
pub mod parser;
mod planner;
mod recovery;
#[cfg(test)]
mod recovery_tests;
mod release_smoke;
mod relations;
mod report;
mod tools;
mod trace;
mod verifier;
pub mod workbook;

pub use app::OmegaEngine;
pub use dates::{CivilDate, Clock};
pub use db::Database;
pub use error::{OmegaError, Result};
pub use model::*;
pub use ocr::{OcrEngine, OcrOutcome, RecognizedLine, SystemOcr};
pub use parser::{DocumentParser, LocalDocumentParser, OcrProvider};
pub use recovery::{BackupPolicy, RecoveryReport, RecoverySource};
pub use release_smoke::{ReleaseSmokeReport, run_release_smoke};
pub use tools::ToolEngine;
pub use verifier::{value_is_supported, verify_model_answer};

use std::{fs, path::PathBuf};
use tauri::{Manager, State};

#[tauri::command]
fn get_status(engine: State<'_, OmegaEngine>) -> Result<AppStatus> {
    engine.status()
}

#[tauri::command]
fn list_sources(engine: State<'_, OmegaEngine>) -> Result<Vec<SourceSummary>> {
    engine.sources()
}

#[tauri::command]
fn authorize_source(engine: State<'_, OmegaEngine>, path: String) -> Result<i64> {
    engine.authorize_source(std::path::Path::new(&path))
}

#[tauri::command]
fn index_source(engine: State<'_, OmegaEngine>, source_id: i64) -> Result<IndexReport> {
    engine.index_source(source_id)
}

#[tauri::command]
fn revoke_source(engine: State<'_, OmegaEngine>, source_id: i64) -> Result<()> {
    engine.revoke_source(source_id)
}

#[tauri::command]
fn list_concepts(
    engine: State<'_, OmegaEngine>,
    query: Option<String>,
) -> Result<Vec<ConceptSummary>> {
    engine.concepts(query.as_deref())
}

#[tauri::command]
fn search_documents(engine: State<'_, OmegaEngine>, query: String) -> Result<Vec<SearchHit>> {
    engine.search(&query)
}

#[tauri::command]
fn ask(engine: State<'_, OmegaEngine>, question: String) -> Result<Answer> {
    engine.ask(&question)
}

/// Pregunta dentro de una conversación. La interfaz decide la clave; el motor
/// nunca mezcla dos conversaciones ni persiste ninguna.
#[tauri::command]
fn ask_in_conversation(
    engine: State<'_, OmegaEngine>,
    conversation: String,
    question: String,
) -> Result<Answer> {
    engine.ask_in_conversation(&conversation, &question)
}

/// Inicia una conversación nueva borrando el contexto de la anterior.
#[tauri::command]
fn reset_conversation(engine: State<'_, OmegaEngine>, conversation: String) -> Result<()> {
    engine.reset_conversation(&conversation);
    Ok(())
}

#[tauri::command]
fn open_document(engine: State<'_, OmegaEngine>, path: String) -> Result<()> {
    engine.open_document(std::path::Path::new(&path))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let database_path: PathBuf = data_dir.join("omega.db");
            let engine = OmegaEngine::open_recovering(database_path)
                .map_err(|error| Box::<dyn std::error::Error>::from(error.to_string()))?;
            app.manage(engine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            list_sources,
            authorize_source,
            index_source,
            revoke_source,
            list_concepts,
            search_documents,
            ask,
            ask_in_conversation,
            reset_conversation,
            open_document,
        ])
        .run(tauri::generate_context!())
        .expect("error al ejecutar Omega");
}
