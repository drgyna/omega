mod agent;
mod app;
mod db;
mod error;
mod extract;
mod indexer;
mod model;
mod normalize;
mod ocr;
mod parser;
mod tools;

pub use app::OmegaEngine;
pub use db::Database;
pub use error::{OmegaError, Result};
pub use model::*;
pub use parser::OcrProvider;
pub use tools::ToolEngine;

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

#[tauri::command]
fn configure_ai(engine: State<'_, OmegaEngine>, enabled: bool, consent: bool) -> Result<()> {
    engine.configure_ai(enabled, consent)
}

#[tauri::command]
fn store_api_key(engine: State<'_, OmegaEngine>, api_key: String) -> Result<()> {
    engine.store_api_key(&api_key)
}

#[tauri::command]
fn clear_api_key(engine: State<'_, OmegaEngine>) -> Result<()> {
    engine.clear_api_key()
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
            let engine = OmegaEngine::open(database_path)
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
            configure_ai,
            store_api_key,
            clear_api_key,
            open_document,
        ])
        .run(tauri::generate_context!())
        .expect("error al ejecutar Omega");
}
