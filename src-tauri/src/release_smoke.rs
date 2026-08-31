use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{OmegaEngine, OmegaError, Result, SearchHit};

#[derive(Debug, Serialize)]
pub struct ReleaseSmokeReport {
    pub database: PathBuf,
    pub fixture: PathBuf,
    pub indexed_documents: usize,
    pub initial_hits: usize,
    pub reopened_hits: usize,
    pub citation_path: String,
    pub citation_location: String,
    pub ocr_status: String,
    pub reliable: bool,
}

/// Smoke test ejecutado por el propio binario empaquetado. Sólo se activa con
/// `--release-smoke`; el arranque gráfico normal no cambia.
pub fn run_release_smoke(
    database_path: impl AsRef<Path>,
    fixture_path: impl AsRef<Path>,
    query: &str,
) -> Result<ReleaseSmokeReport> {
    let database_path = database_path.as_ref().to_path_buf();
    let fixture_path = fixture_path.as_ref().canonicalize()?;
    if !fixture_path.is_file() {
        return Err(OmegaError::InvalidArguments(format!(
            "{} no es un fixture OCR",
            fixture_path.display()
        )));
    }
    let source_path = fixture_path
        .parent()
        .ok_or_else(|| OmegaError::InvalidArguments("el fixture no tiene carpeta padre".into()))?;

    let engine = OmegaEngine::open_recovering(&database_path)?;
    let source = engine.authorize_source(source_path)?;
    let indexed = engine.index_source(source)?;
    let initial_search = engine.search(query)?;
    let initial = matching_hits(&initial_search, &fixture_path);
    let first = require_reliable_ocr(&initial, &fixture_path)?;
    let citation_path = first.evidence.path.clone();
    let citation_location = first.evidence.location.clone();
    let ocr_status = first.evidence.ocr_status.clone().unwrap_or_default();
    let reliable = first.evidence.reliable;
    let initial_hits = initial.len();
    drop(engine);

    let reopened = OmegaEngine::open_recovering(&database_path)?;
    let reopened_search = reopened.search(query)?;
    let after_restart = matching_hits(&reopened_search, &fixture_path);
    let after = require_reliable_ocr(&after_restart, &fixture_path)?;
    if after.evidence.path != citation_path {
        return Err(OmegaError::Verification(
            "la cita OCR cambió de documento después de reabrir SQLite".into(),
        ));
    }

    Ok(ReleaseSmokeReport {
        database: database_path,
        fixture: fixture_path,
        indexed_documents: indexed.indexed,
        initial_hits,
        reopened_hits: after_restart.len(),
        citation_path,
        citation_location,
        ocr_status,
        reliable,
    })
}

fn matching_hits<'a>(hits: &'a [SearchHit], fixture: &Path) -> Vec<&'a SearchHit> {
    hits.iter()
        .filter(|hit| Path::new(&hit.evidence.path) == fixture)
        .collect()
}

fn require_reliable_ocr<'a>(hits: &'a [&SearchHit], fixture: &Path) -> Result<&'a SearchHit> {
    hits.iter()
        .copied()
        .find(|hit| {
            hit.evidence.reliable
                && hit.evidence.ocr_status.as_deref() == Some("complete")
                && hit.evidence.location.contains("OCR, página")
        })
        .ok_or_else(|| {
            OmegaError::Verification(format!(
                "{} no produjo una cita OCR completa, fiable y ubicada",
                fixture.display()
            ))
        })
}
