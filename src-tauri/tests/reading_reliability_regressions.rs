//! Ronda 4 · punto 5 — Omega responde por la fiabilidad de su propia lectura.
//!
//! `documents.ocr_status` y `documents.ocr_confidence` existían desde la
//! auditoría original y el motor ya los usaba para decidir si una respuesta
//! puede declararse verificada. Lo que no sabía era **contestar la pregunta
//! directa**: «¿con qué confianza leíste este documento y puedo citar su
//! texto?». Las 115 preguntas de esa forma del banco caían en «no encontré
//! evidencia», y sólo acertaban las 22 en las que eso resultaba ser correcto.
//!
//! El motor OCR se inyecta —igual que en `ocr_state_regressions.rs`— para que
//! el estado bajo prueba sea un dato del caso y no dependa del equipo.

use std::{path::Path, sync::Arc};

use omega_core::{
    Clock, LocalDocumentParser, OcrEngine, OcrOutcome, OmegaEngine, RecognizedLine,
    ocr::outcome_from_lines,
};

const TODAY: &str = "2026-08-29";

const PREGUNTA: &str = "El documento D0{} es un PDF escaneado (requiere OCR). ¿Cuál es su nivel de confianza de reconocimiento y se puede citar texto de forma confiable?";

/// Un escaneo leído por encima del umbral: se dice y su texto se puede citar.
#[test]
fn a_high_confidence_scan_reports_high_confidence_and_is_citable() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("01958_alta.png"), b"imagen").unwrap();

    let engine = index(root);
    let answer = engine.ask(&pregunta("1958")).unwrap();

    assert!(
        answer.text.starts_with("Confianza alta"),
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("se puede citar"),
        "la pregunta pide las dos cosas: nivel y si se puede citar: {}",
        answer.text
    );
    assert!(answer.verified, "{:?}", answer.warning);
    assert_eq!(answer.citations.len(), 1);
    assert_eq!(
        answer.citations[0].field.as_deref(),
        Some("estado de reconocimiento (OCR)")
    );
    assert!(
        answer.citations[0].value.is_some(),
        "la cita es un metadato CON valor: por eso sostiene la respuesta"
    );
}

/// Un escaneo por debajo del umbral: se dice, y la respuesta no se declara
/// verificada — el mismo invariante de P0-1, sin ninguna regla nueva.
#[test]
fn a_low_confidence_scan_says_so_and_is_never_verified() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("03564_baja.png"), b"imagen").unwrap();

    let engine = index(root);
    let answer = engine.ask(&pregunta("3564")).unwrap();

    assert!(
        answer.text.starts_with("Confianza baja"),
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains("no se puede citar de forma confiable")
            || answer.text.contains("**no** se puede citar de forma confiable"),
        "{}",
        answer.text
    );
    assert!(!answer.verified);
    assert!(answer.warning.is_some());
}

/// Un escaneo del que no salió nada: sin evidencia que citar, y se dice por
/// qué en vez de con el mensaje genérico.
#[test]
fn a_scan_with_no_recoverable_text_answers_without_evidence() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("04578_vacio.png"), b"imagen").unwrap();
    // Un documento legible cualquiera, para que el índice no quede vacío.
    std::fs::write(root.join("00001_nota.md"), "Folio: N-1\nEstado: Abierto\n").unwrap();

    let engine = index(root);
    let answer = engine.ask(&pregunta("4578")).unwrap();

    assert!(answer.citations.is_empty(), "{:?}", answer.citations);
    assert!(!answer.verified);
    assert!(
        answer.text.contains("Sin texto recuperable")
            || answer.text.contains("no encontré evidencia")
            || answer.text.contains("No encontré evidencia"),
        "{}",
        answer.text
    );
}

/// Un documento que nunca necesitó OCR responde por lo que es, no por un
/// reconocimiento que no ocurrió.
#[test]
fn a_document_that_never_needed_ocr_says_exactly_that() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("00042_nota.md"), "Folio: N-42\nEstado: Abierto\n").unwrap();

    let engine = index(root);
    let answer = engine.ask(&pregunta("0042")).unwrap();

    assert!(
        answer.text.contains("no necesitó reconocimiento óptico"),
        "{}",
        answer.text
    );
    assert!(answer.verified);
}

/// La ruta es estrecha a propósito: una pregunta por el CONTENIDO del mismo
/// documento escaneado no se desvía aquí.
#[test]
fn a_question_about_the_content_of_the_same_document_is_not_captured() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("01958_alta.png"), b"imagen").unwrap();

    let engine = index(root);
    let answer = engine
        .ask("¿Cuál es el Importe del documento D01958?")
        .unwrap();

    assert!(
        !answer.text.starts_with("Confianza"),
        "una pregunta de contenido no puede contestarse con el estado de OCR: {}",
        answer.text
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn pregunta(numero: &str) -> String {
    PREGUNTA.replace("{}", numero)
}

/// Motor OCR guionizado por el nombre del archivo, para que cada estado sea un
/// dato del caso.
struct ScriptedOcr;

impl OcrEngine for ScriptedOcr {
    fn recognize(&self, path: &Path) -> OcrOutcome {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned();
        if name.contains("alta") {
            outcome_from_lines(vec![line("Folio: FAC-ALTA", 0.94), line("Importe: $100.00 MXN", 0.94)])
        } else if name.contains("baja") {
            outcome_from_lines(vec![line("Folio: FAC-BAJA", 0.30), line("Importe: $200.00 MXN", 0.30)])
        } else {
            outcome_from_lines(vec![])
        }
    }
}

fn line(text: &str, confidence: f64) -> RecognizedLine {
    RecognizedLine {
        page: 1,
        text: text.to_owned(),
        confidence,
        x: 0.1,
        y: 0.2,
        width: 0.5,
        height: 0.05,
    }
}

fn index(root: &Path) -> OmegaEngine {
    let engine = OmegaEngine::open_with_clock(
        root.join("omega-lectura.db"),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap()
    .with_parser(Arc::new(LocalDocumentParser::with_ocr(Arc::new(ScriptedOcr))));
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();
    engine
}
