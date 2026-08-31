//! Ronda 4 · punto 4 — caché de OCR por contenido del archivo.
//!
//! El perfilado midió que el reconocimiento óptico es el 94,3 % del tiempo de
//! una indexación completa (703.760 ms de 746.043 sobre 10.000 documentos), y
//! que reindexar volvía a correrlo entero sobre imágenes idénticas. La caché
//! guarda el resultado bajo el SHA-256 del archivo.
//!
//! Aquí se comprueba el comportamiento, no la velocidad: que el motor **no se
//! vuelva a invocar** cuando el contenido no cambió, que **sí** se invoque
//! cuando cambió, y que el resultado reconstruido desde la caché sea el mismo.
//! La medición de rendimiento va en el informe, sobre el corpus real.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use omega_core::{
    Clock, LocalDocumentParser, OcrEngine, OcrOutcome, OmegaEngine, RecognizedLine,
    ocr::outcome_from_lines,
};

const TODAY: &str = "2026-08-29";

/// Motor OCR que cuenta cuántas veces lo llaman de verdad.
struct CountingOcr {
    calls: Arc<AtomicUsize>,
    text: Arc<std::sync::Mutex<String>>,
}

impl OcrEngine for CountingOcr {
    fn recognize(&self, _path: &Path) -> OcrOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.text.lock().unwrap().clone();
        outcome_from_lines(vec![RecognizedLine {
            page: 1,
            text,
            confidence: 0.93,
            x: 0.1,
            y: 0.2,
            width: 0.5,
            height: 0.05,
        }])
    }
}

/// Reindexar sin cambios no vuelve a correr el OCR, y la evidencia sigue
/// siendo la misma.
#[test]
fn reindexing_unchanged_files_does_not_run_ocr_again() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("escaneo.png"), b"imagen").unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let text = Arc::new(std::sync::Mutex::new("Folio: FAC-CACHE".to_owned()));
    let engine = engine_for(root, &calls, &text);
    let source = engine.authorize_source(root).unwrap();

    engine.index_source(source).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "la primera vez sí se reconoce");
    let first = engine.search("FAC-CACHE").unwrap();
    assert_eq!(first.len(), 1);

    engine.index_source(source).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "el contenido no cambió: el OCR no puede volver a correr"
    );
    let second = engine.search("FAC-CACHE").unwrap();
    assert_eq!(
        second.len(),
        1,
        "la evidencia reconstruida desde la caché es la misma"
    );
    assert_eq!(second[0].evidence.ocr_status.as_deref(), Some("complete"));
    assert!(second[0].evidence.reliable);
}

/// Si el contenido cambia, cambia el hash y el OCR vuelve a correr. La
/// invalidación es correcta por construcción, sin política aparte.
#[test]
fn changing_the_file_content_runs_ocr_again() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("escaneo.png"), b"imagen").unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let text = Arc::new(std::sync::Mutex::new("Folio: FAC-UNO".to_owned()));
    let engine = engine_for(root, &calls, &text);
    let source = engine.authorize_source(root).unwrap();

    engine.index_source(source).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    std::fs::write(root.join("escaneo.png"), b"imagen distinta").unwrap();
    *text.lock().unwrap() = "Folio: FAC-DOS".to_owned();
    engine.index_source(source).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "otro contenido es otra clave: hay que volver a reconocerlo"
    );
    assert_eq!(engine.search("FAC-DOS").unwrap().len(), 1);
    assert!(
        engine.search("FAC-UNO").unwrap().is_empty(),
        "el texto viejo no puede sobrevivir a la reindexación"
    );
}

/// Un escaneo del que no sale nada costó exactamente lo mismo de reconocer:
/// también se recuerda, para no repetirlo en cada reindexación.
#[test]
fn a_scan_that_yields_nothing_is_also_remembered() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("escaneo.png"), b"imagen").unwrap();
    std::fs::write(root.join("nota.md"), "Folio: N-1\n").unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let text = Arc::new(std::sync::Mutex::new(String::new()));
    let engine = engine_for(root, &calls, &text);
    let source = engine.authorize_source(root).unwrap();

    let first = engine.index_source(source).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.skipped, 1, "sin texto no se indexa, y se reporta");

    let second = engine.index_source(source).unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "un escaneo ilegible no puede volver a pagarse en cada reindexación"
    );
    assert_eq!(second.skipped, 1, "y sigue reportándose igual");
}

// ─────────────────────────────────────────────────────────────────────────

fn engine_for(
    root: &Path,
    calls: &Arc<AtomicUsize>,
    text: &Arc<std::sync::Mutex<String>>,
) -> OmegaEngine {
    let database = root.join("omega-cache.db");
    let engine = OmegaEngine::open_with_clock(&database, Clock::fixed(TODAY).unwrap()).unwrap();
    let cached = omega_core::ocr::CachedOcr::new(
        CountingOcr {
            calls: Arc::clone(calls),
            text: Arc::clone(text),
        },
        omega_core::Database::open(&database).unwrap(),
    );
    engine.with_parser(Arc::new(LocalDocumentParser::with_ocr(Arc::new(cached))))
}
