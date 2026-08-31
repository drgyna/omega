//! Ronda 4 · punto 7 — detección de contradicciones sin depender de
//! `CASE-####`.
//!
//! La auditoría original pedía comprobar que el motor detecta valores
//! contradictorios entre documentos y los comunica **mostrando ambas
//! evidencias, sin decidir cuál es correcta**. Hasta ahora sólo se había
//! probado con preguntas que nombraban el código inventado `CASE-####`, que
//! no está escrito en ningún documento del acervo. Aquí se prueba nombrando
//! únicamente un identificador real, escrito dentro de los documentos.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// Dos documentos que comparten un folio real y declaran estados distintos:
/// se detecta, se muestran las dos evidencias y no se elige ninguna.
#[test]
fn two_documents_sharing_a_real_folio_with_different_values_are_reported() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("03566_orden_compra.md"),
        "OC: OC-2024-00001\nEstado: Autorizada\nResponsable: Equipo A\n",
    )
    .unwrap();
    fs::write(
        root.join("04507_recepcion.md"),
        "OC: OC-2024-00001\nEstado: Cancelada\nResponsable: Equipo A\n",
    )
    .unwrap();

    let engine = index(root, "contradiccion-1");
    let answer = engine.ask("¿Hay contradicciones en OC-2024-00001?").unwrap();

    assert!(
        answer.text.contains("Autorizada") && answer.text.contains("Cancelada"),
        "las dos evidencias tienen que verse, no una: {}",
        answer.text
    );
    assert!(
        answer.text.contains("no decide cuál valor es correcto"),
        "la respuesta tiene que decir explícitamente que no elige: {}",
        answer.text
    );
    assert!(
        answer.citations.len() >= 2,
        "una contradicción se cita por los dos lados: {:?}",
        answer.citations
    );
}

/// El campo en el que coinciden no puede reportarse como contradicción.
#[test]
fn a_field_both_documents_agree_on_is_not_a_contradiction() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("03566_orden_compra.md"),
        "OC: OC-2024-00002\nEstado: Autorizada\nResponsable: Equipo A\n",
    )
    .unwrap();
    fs::write(
        root.join("04507_recepcion.md"),
        "OC: OC-2024-00002\nEstado: Autorizada\nResponsable: Equipo A\n",
    )
    .unwrap();

    let engine = index(root, "contradiccion-2");
    let answer = engine.ask("¿Hay contradicciones en OC-2024-00002?").unwrap();

    assert!(
        answer.text.starts_with("No encontré evidencia de contradicción"),
        "{}",
        answer.text
    );
    assert!(!answer.verified);
}

/// Dos documentos que registran su importe bajo **campos distintos** no se
/// contradicen: es el caso que el generador del banco etiqueta como
/// «contradicción» y que Omega, correctamente, no reporta como tal.
#[test]
fn different_fields_with_different_values_are_not_a_contradiction() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("03566_orden_compra.md"),
        "OC: OC-2024-00003\nImporte de la orden: $416,731.90 USD\n",
    )
    .unwrap();
    fs::write(
        root.join("04507_recepcion.md"),
        "OC: OC-2024-00003\nCantidad recibida: $225,974.38 MXN\n",
    )
    .unwrap();

    let engine = index(root, "contradiccion-3");
    let answer = engine.ask("¿Hay contradicciones en OC-2024-00003?").unwrap();

    assert!(
        answer.text.starts_with("No encontré evidencia de contradicción"),
        "«lo pedido» y «lo recibido» son dos hechos distintos, no uno contradictorio: {}",
        answer.text
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn index(root: &Path, name: &str) -> OmegaEngine {
    let engine = OmegaEngine::open_with_clock(
        root.join(format!("omega-{name}.db")),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
