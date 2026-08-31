//! P1-B.4 — Una tabla de Word con más de dos columnas es una tabla, no una
//! lista de pares.
//!
//! El indexador leía sólo las dos primeras celdas de cada fila: la tercera
//! columna en adelante desaparecía del acervo sin dejar rastro, y con ella
//! cualquier campo que viviera ahí. Los encabezados y la ubicación exacta de
//! cada celda tienen que conservarse.
//!
//! Fixtures genéricas: un contrato con una tabla de partidas y un proyecto con
//! una tabla de avance.

use std::path::Path;

use omega_core::{Clock, OmegaEngine};

#[path = "support/mod.rs"]
mod support;

const TODAY: &str = "2026-08-25";

/// Cinco columnas: ninguna se pierde y cada una conserva su encabezado.
#[test]
fn a_table_with_more_than_two_columns_keeps_every_column() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_docx_table(
        &root.join("contrato.docx"),
        &[
            vec!["Partida", "Descripción", "Cantidad", "Importe", "Estado"],
            vec![
                "PAR-001",
                "Servicio inicial",
                "3",
                "$1,500.00 MXN",
                "Cerrada",
            ],
            vec![
                "PAR-002",
                "Servicio adicional",
                "2",
                "$900.00 MXN",
                "Abierta",
            ],
        ],
    );
    let engine = index(root);

    for concept in ["Descripción", "Cantidad", "Importe", "Estado"] {
        assert!(
            engine
                .concepts(Some(concept))
                .unwrap()
                .iter()
                .any(|found| found.display_name == concept),
            "la columna «{concept}» desapareció del acervo"
        );
    }

    let values = values_of(&engine, "Estado");
    assert!(
        values.contains(&"Cerrada".to_owned()) && values.contains(&"Abierta".to_owned()),
        "la quinta columna debe estar completa: {values:?}"
    );

    let answer = engine
        .ask_in_conversation("docx", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(
        answer.text.contains("$2,400.00 MXN"),
        "1,500 + 900 = 2,400, con las dos filas de la cuarta columna: {}",
        answer.text
    );
    assert!(answer.verified, "{}", answer.text);
}

/// Cada celda conserva su ubicación exacta: tabla, fila y columna reales, no
/// una columna B inventada para todo.
#[test]
fn every_cell_keeps_its_own_location() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_docx_table(
        &root.join("proyecto.docx"),
        &[
            vec!["Clave", "Responsable", "Avance"],
            vec!["PRY-001", "Equipo Norte", "60%"],
        ],
    );
    let engine = index(root);

    let hits = engine.search("Equipo Norte").unwrap();
    let evidence = hits
        .iter()
        .map(|hit| &hit.evidence)
        .find(|evidence| evidence.location.starts_with("tabla "))
        .unwrap_or_else(|| panic!("sin evidencia de tabla: {hits:#?}"));
    assert!(
        evidence.location.contains("fila 2"),
        "la fila real es la 2: {}",
        evidence.location
    );
    assert!(
        evidence.location.contains("celda B2"),
        "«Responsable» es la columna B, no otra: {}",
        evidence.location
    );
    assert!(
        evidence.location.contains("Responsable"),
        "la ubicación nombra su encabezado: {}",
        evidence.location
    );

    let avance = engine.search("60%").unwrap();
    assert!(
        avance
            .iter()
            .any(|hit| hit.evidence.location.contains("celda C2")),
        "la tercera columna vive en C: {avance:#?}"
    );
}

/// Varias tablas en el mismo documento se distinguen entre sí.
#[test]
fn several_tables_in_one_document_are_told_apart() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_docx_tables(
        &root.join("contrato.docx"),
        &[
            vec![
                vec!["Partida", "Descripción", "Importe"],
                vec!["PAR-001", "Servicio inicial", "$100.00 MXN"],
            ],
            vec![
                vec!["Anexo", "Responsable", "Vigencia"],
                vec!["ANX-001", "Equipo Sur", "2026-01-31"],
            ],
        ],
    );
    let engine = index(root);

    let first = engine.search("Servicio inicial").unwrap();
    assert!(
        first
            .iter()
            .any(|hit| hit.evidence.location.starts_with("tabla 1")),
        "{first:#?}"
    );
    let second = engine.search("Equipo Sur").unwrap();
    assert!(
        second
            .iter()
            .any(|hit| hit.evidence.location.starts_with("tabla 2")),
        "la segunda tabla no puede llamarse «tabla 1»: {second:#?}"
    );
}

/// Control: una tabla de dos columnas sigue siendo una lista de pares
/// «campo / valor», que es lo que de verdad significa en un formulario.
#[test]
fn a_two_column_table_is_still_a_list_of_field_value_pairs() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_docx_table(
        &root.join("factura.docx"),
        &[
            vec!["Folio", "FAC-2026-0001"],
            vec!["Importe total", "$4,200.00 MXN"],
            vec!["Estado", "Pagada"],
        ],
    );
    let engine = index(root);

    assert_eq!(values_of(&engine, "Folio"), vec!["FAC-2026-0001".to_owned()]);
    assert_eq!(values_of(&engine, "Estado"), vec!["Pagada".to_owned()]);
    let answer = engine
        .ask_in_conversation("pares", "¿Cuánto suma el Importe total?")
        .unwrap();
    assert!(answer.text.contains("$4,200.00 MXN"), "{}", answer.text);
}

// ─────────────────────────────────────────────────────────────────────────

fn values_of(engine: &OmegaEngine, concept: &str) -> Vec<String> {
    let tools = omega_core::ToolEngine::new(
        omega_core::Database::open(engine.database_path()).unwrap(),
    );
    tools.concept_values(concept).unwrap()
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-docx.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
