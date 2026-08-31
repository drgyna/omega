//! Ronda 3 — El nombre de un concepto no puede depender de qué archivo se
//! indexó primero, igual que ya no depende su tipo (ver
//! `concept_type_stability_regressions.rs`, P1-B.1).
//!
//! Un OCR reconoce por zona de página, no por línea lógica completa: un
//! rótulo puede llegar como un fragmento («y (EMP» en vez de «EMP», el caso
//! real encontrado en `operaciones/02797_orden_mantenimiento.pdf`). Si ese
//! fragmento se indexaba primero, `display_name` quedaba fijado a la
//! basura para siempre — incluido el `canonical_key` que cientos de
//! documentos de texto plano comparten con el campo real — y toda pregunta
//! por `campo "EMP"` dejaba de resolver, aunque el valor estuviera bien
//! indexado y el campo se pidiera con su nombre exacto entre comillas.
//!
//! Fixture genérica: un campo de dos letras, un OCR que lo reconoce mal y un
//! registro de texto plano que lo escribe bien.

use std::{path::Path, sync::Arc};

use omega_core::{
    Clock, LocalDocumentParser, OcrEngine, OcrOutcome, OmegaEngine, RecognizedLine,
    ocr::outcome_from_lines,
};

const TODAY: &str = "2026-08-25";

/// El archivo de OCR se indexa primero en orden alfabético. Antes de este
/// arreglo, su fragmento «y (EMP» se quedaba como nombre del concepto para
/// siempre.
#[test]
fn a_garbled_ocr_label_indexed_first_never_freezes_the_concept_name() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("a-escaneo.png"), b"imagen").unwrap();
    std::fs::write(
        root.join("b-registro.md"),
        "# Registro\n\nFolio: REG-001\nEMP: EMP-2023-0037\n",
    )
    .unwrap();

    let engine = index(root);
    let concept = engine
        .concepts(Some("EMP"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.key == "emp")
        .expect("el concepto emp debe existir");
    assert_eq!(
        concept.display_name, "EMP",
        "el rótulo de OCR no puede ganarle al rótulo real del campo"
    );
    assert_eq!(
        concept.occurrences, 2,
        "el valor de OCR sigue indexado; sólo el nombre mostrado cambia"
    );
}

/// Mismo acervo, orden inverso: el texto plano llega primero. El resultado
/// tiene que ser exactamente el mismo nombre — el orden de indexación no es
/// un dato.
#[test]
fn the_same_archive_indexed_in_reverse_order_names_the_concept_the_same_way() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(
        root.join("a-registro.md"),
        "# Registro\n\nFolio: REG-001\nEMP: EMP-2023-0037\n",
    )
    .unwrap();
    std::fs::write(root.join("b-escaneo.png"), b"imagen").unwrap();

    let engine = index(root);
    let concept = engine
        .concepts(Some("EMP"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.key == "emp")
        .expect("el concepto emp debe existir");
    assert_eq!(concept.display_name, "EMP");
    assert_eq!(concept.occurrences, 2);
}

/// Un campo que sólo existe en documentos de OCR (nunca hay un rótulo de
/// texto plano que lo corrija) conserva el rótulo de OCR: no hay nada mejor
/// con lo que reemplazarlo, así que no desaparece.
#[test]
fn a_field_seen_only_through_ocr_keeps_its_ocr_label() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(root.join("solo-escaneo.png"), b"imagen").unwrap();

    let engine = index(root);
    let concept = engine
        .concepts(Some("EMP"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.key == "emp")
        .expect("el concepto emp debe existir aunque su único origen sea OCR");
    assert_eq!(concept.display_name, "y (EMP");
    assert_eq!(concept.occurrences, 1);
}

// ─────────────────────────────────────────────────────────────────────────

/// Motor OCR de prueba: cualquier imagen produce el mismo fragmento
/// mal cortado que se encontró en el corpus real, con confianza alta para
/// aislar esta prueba del comportamiento de baja confianza (ya cubierto por
/// `ocr_state_regressions.rs`).
struct GarbledFieldOcr;

impl OcrEngine for GarbledFieldOcr {
    fn recognize(&self, _path: &Path) -> OcrOutcome {
        outcome_from_lines(vec![RecognizedLine {
            page: 1,
            text: "y (EMP:20170053) -".to_owned(),
            confidence: 0.90,
            x: 0.33,
            y: 0.83,
            width: 0.12,
            height: 0.02,
        }])
    }
}

fn index(root: &Path) -> OmegaEngine {
    let engine = OmegaEngine::open_with_clock(
        root.join("omega-nombres.db"),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap()
    .with_parser(Arc::new(LocalDocumentParser::with_ocr(Arc::new(
        GarbledFieldOcr,
    ))));
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
