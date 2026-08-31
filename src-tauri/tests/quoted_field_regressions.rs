//! Ronda 7 — «eco de folio»: la pregunta entrecomilla un campo y la respuesta
//! devolvía el folio con el que se encontró el documento.
//!
//! «En el documento con folio INC-2025-00190 (…), ¿cuál es el valor del campo
//! "Fecha"?» se contestaba «El campo «INC» de EMP-2023-0066 es INC-2025-00190»,
//! con cita y marcada como verificada. El documento correcto estaba
//! localizado y sí registraba «Fecha»; lo que fallaba era la síntesis: como
//! todas las coincidencias de la búsqueda eran del campo por el que se
//! encontró el documento —el folio que la propia pregunta escribió—, el atajo
//! de «un solo grupo de evidencia» las daba por buenas sin comprobar que el
//! campo pedido fuera ése.
//!
//! El resultado era la peor forma de error posible en este motor: devolverle al
//! usuario, presentado como dato extraído y verificado, exactamente el texto
//! que él acababa de teclear.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// El campo entrecomillado manda sobre el campo por el que se encontró el
/// documento.
#[test]
fn the_quoted_field_wins_over_the_field_that_found_the_document() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    fs::write(
        root.join("rh/07532_incidencia_rh.md"),
        "Empresa: Grupo Nexo\nÁrea: Recursos humanos y capacitación\n\
         Fecha: 27 de diciembre de 2023\nINC: INC-2025-00190\nEMP: EMP-2023-0066\n",
    )
    .unwrap();

    let engine = index(root, "eco-1");
    let answer = engine
        .ask("En el documento con folio INC-2025-00190 (incidencia_rh, área Recursos humanos y capacitación), ¿cuál es el valor del campo \"Fecha\"?")
        .unwrap();

    assert!(
        answer.text.contains("27 de diciembre de 2023"),
        "se contesta el campo pedido: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("El campo «INC»"),
        "no se devuelve el folio que la pregunta ya traía: {}",
        answer.text
    );
}

/// Un campo pedido que el documento no registra sigue sin respuesta: el
/// arreglo no abre la puerta a contestar con cualquier otro campo suyo.
#[test]
fn a_quoted_field_the_document_does_not_have_stays_unanswered() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    fs::write(
        root.join("rh/07532_incidencia_rh.md"),
        "Empresa: Grupo Nexo\nINC: INC-2025-00190\nEMP: EMP-2023-0066\n",
    )
    .unwrap();

    let engine = index(root, "eco-2");
    let answer = engine
        .ask("En el documento con folio INC-2025-00190, ¿cuál es el valor del campo \"Kilometraje\"?")
        .unwrap();

    assert!(
        !answer.text.contains("EMP-2023-0066"),
        "un campo ausente no se sustituye por otro cualquiera: {}",
        answer.text
    );
}

/// Una búsqueda sin campo pedido sigue contestándose con la evidencia
/// encontrada, como siempre.
#[test]
fn a_plain_lookup_without_a_quoted_field_is_unchanged() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    fs::write(
        root.join("rh/07532_incidencia_rh.md"),
        "Empresa: Grupo Nexo\nFecha: 27 de diciembre de 2023\nINC: INC-2025-00190\n",
    )
    .unwrap();

    let engine = index(root, "eco-3");
    let answer = engine.ask("Encuentra INC-2025-00190").unwrap();

    assert!(
        answer.text.contains("INC-2025-00190"),
        "la búsqueda por identificador sigue igual: {}",
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
