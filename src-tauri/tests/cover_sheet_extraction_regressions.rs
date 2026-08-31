//! Ronda 10 — Un archivo que es sólo carátula no tiene tabla que encabezar.
//!
//! El encabezado se elegía como «la fila con más celdas con contenido». Con
//! una tabla real debajo el criterio acierta: la tabla gana el máximo por
//! tener más columnas. Sin tabla, todas las filas miden lo mismo, gana la
//! primera por serlo, y sus dos celdas pasan a nombrar los campos de todo el
//! archivo: el rótulo real de cada fila de abajo quedaba convertido en un
//! *valor* de una columna inventada.
//!
//! Fixtures de giros distintos al del acervo de evaluación —una clínica
//! veterinaria, una panadería, un taller mecánico— para que lo que se prueba
//! sea la forma de la fila y no un vocabulario.

use std::path::Path;

use omega_core::{Clock, OmegaEngine};

#[path = "support/mod.rs"]
mod support;

use support::SheetCell;

const TODAY: &str = "2026-08-25";

/// Una carátula pura en XLSX: todos sus campos se extraen con su nombre real.
#[test]
fn a_pure_cover_sheet_indexes_every_row_as_a_field() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("historia.xlsx"),
        "Datos",
        &[
            // Título suelto en A, como lo escriben las plantillas reales.
            vec![SheetCell::text("Historia Clínica Veterinaria HC-2024-0071")],
            vec![
                SheetCell::text("Clínica"),
                SheetCell::text("Patitas Felices, S.C."),
            ],
            vec![SheetCell::text("Especialidad"), SheetCell::text("Cirugía de tejidos blandos")],
            vec![SheetCell::text("Paciente"), SheetCell::text("Rocky (MASC-2021-0042)")],
            vec![SheetCell::text("Fecha"), SheetCell::text("12 de marzo de 2024")],
            vec![
                SheetCell::text("Médico responsable"),
                SheetCell::text("MVZ Aurora Peñaloza (VET-2019-0018)"),
            ],
            vec![SheetCell::text("Peso"), SheetCell::text("18.4 kg")],
        ],
    );
    let engine = index(root);

    // El campo que antes se perdía: su rótulo quedaba como valor de la
    // columna inventada con el nombre de la clínica.
    for (field, value) in [
        ("Clínica", "Patitas Felices, S.C."),
        ("Especialidad", "Cirugía de tejidos blandos"),
        ("Paciente", "Rocky (MASC-2021-0042)"),
        ("Fecha", "12 de marzo de 2024"),
        ("Médico responsable", "MVZ Aurora Peñaloza (VET-2019-0018)"),
        ("Peso", "18.4 kg"),
    ] {
        let values = values_of(&engine, field);
        assert!(
            values.iter().any(|item| item.contains(value)),
            "el campo {field} debe indexarse con su nombre real y su valor {value}: {values:?}"
        );
    }

    // Y ninguna de las dos celdas de la primera fila se convirtió en columna.
    assert!(
        !concept_exists(&engine, "Patitas Felices, S.C."),
        "el valor de la carátula no puede volverse el nombre de un campo"
    );
    // El título suelto no es un par y no inventa ningún campo.
    assert!(
        !concept_exists(&engine, "Historia Clínica Veterinaria HC-2024-0071"),
        "un título suelto no es un campo"
    );
}

/// La misma carátula en CSV: la ruta de CSV comparte el defecto y la cura.
#[test]
fn a_pure_cover_csv_indexes_every_row_as_a_field() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::write(
        root.join("orden.csv"),
        "Taller,Frenos y Suspensión del Centro\n\
         Servicio,Afinación mayor\n\
         Vehículo,Nissan Versa 2018 (PLC-VTR-4471)\n\
         Fecha de ingreso,03 de febrero de 2025\n\
         Mecánico,Ignacio Robles Tapia (MEC-2020-0009)\n\
         Costo,$8,450.00 MXN\n",
    )
    .unwrap();
    let engine = index(root);

    for (field, value) in [
        ("Taller", "Frenos y Suspensión del Centro"),
        ("Servicio", "Afinación mayor"),
        ("Vehículo", "Nissan Versa 2018 (PLC-VTR-4471)"),
        ("Mecánico", "Ignacio Robles Tapia (MEC-2020-0009)"),
    ] {
        let values = values_of(&engine, field);
        assert!(
            values.iter().any(|item| item.contains(value)),
            "el campo {field} debe indexarse con su nombre real: {values:?}"
        );
    }
    assert!(
        !concept_exists(&engine, "Frenos y Suspensión del Centro"),
        "el valor de la carátula no puede volverse el nombre de un campo"
    );
}

/// Control: una tabla real de dos columnas sigue leyéndose como tabla. La
/// primera columna lleva datos —claves de producto—, no rótulos.
#[test]
fn a_real_two_column_table_still_uses_its_header_row() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("panaderia.xlsx"),
        "Produccion",
        &[
            vec![SheetCell::text("Clave"), SheetCell::text("Piezas horneadas")],
            vec![SheetCell::text("PAN-001"), SheetCell::number("240")],
            vec![SheetCell::text("PAN-002"), SheetCell::number("180")],
            vec![SheetCell::text("PAN-003"), SheetCell::number("95")],
        ],
    );
    let engine = index(root);

    let values = values_of(&engine, "Piezas horneadas");
    assert_eq!(
        values.len(),
        3,
        "las tres filas son valores de la columna, no campos sueltos: {values:?}"
    );
    assert!(!concept_exists(&engine, "PAN-001"), "una clave no es un campo");
}

/// Control: un cuadro vertical con encabezado —rótulos en A y una columna de
/// valores homogénea en B— también sigue siendo tabla. Es el caso donde la
/// primera columna sí tiene forma de rótulos y sólo la homogeneidad de la
/// columna de valores decide.
#[test]
fn a_vertical_ledger_with_a_real_header_is_still_a_table() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("estado.xlsx"),
        "Resultados",
        &[
            vec![SheetCell::text("Concepto"), SheetCell::text("Monto")],
            vec![SheetCell::text("Ingresos"), SheetCell::number("120000")],
            vec![SheetCell::text("Egresos"), SheetCell::number("48000")],
            vec![SheetCell::text("Utilidad"), SheetCell::number("72000")],
        ],
    );
    let engine = index(root);

    let values = values_of(&engine, "Monto");
    assert_eq!(
        values.len(),
        3,
        "la columna de valores conserva sus tres importes: {values:?}"
    );
    // La marca de que se leyó como tabla y no como carátula: los rótulos de
    // las filas no se convirtieron en campos con valor propio.
    for label in ["Ingresos", "Egresos", "Utilidad"] {
        assert!(
            !concept_exists(&engine, label),
            "{label} es una fila de la tabla, no un campo de carátula"
        );
    }
}

/// Control (ronda 8): una hoja de una sola columna no se rompe. No hay par
/// que formar, así que el criterio de carátula no aplica y la hoja se sigue
/// leyendo como antes.
#[test]
fn a_single_column_sheet_is_still_indexed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("asistencia.xlsx"),
        "Turno",
        &[
            vec![SheetCell::text("Panadero en turno")],
            vec![SheetCell::text("Remedios Salgado Ibarra")],
            vec![SheetCell::text("Faustino Lira Bermúdez")],
        ],
    );
    let (engine, report) = index_with_report(root);
    assert!(report.indexed > 0);

    let values = values_of(&engine, "Panadero en turno");
    assert_eq!(
        values.len(),
        2,
        "los dos nombres de la columna siguen indexados: {values:?}"
    );
}

/// Control: basta una fila de tres columnas para que el archivo tenga tabla y
/// la carátula de arriba se lea como carátula, exactamente como hasta ahora.
#[test]
fn a_cover_followed_by_a_real_table_keeps_both_readings() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("mixto.xlsx"),
        "Datos",
        &[
            vec![
                SheetCell::text("Taller"),
                SheetCell::text("Frenos y Suspensión del Centro"),
            ],
            vec![SheetCell::text("Encargado"), SheetCell::text("Ignacio Robles Tapia")],
            vec![
                SheetCell::text("Refacción"),
                SheetCell::text("Cantidad"),
                SheetCell::text("Costo unitario"),
            ],
            vec![
                SheetCell::text("Balata delantera"),
                SheetCell::number("4"),
                SheetCell::number("650"),
            ],
            vec![
                SheetCell::text("Amortiguador"),
                SheetCell::number("2"),
                SheetCell::number("1890"),
            ],
        ],
    );
    let engine = index(root);

    let taller = values_of(&engine, "Taller");
    assert!(
        taller
            .iter()
            .any(|value| value.contains("Frenos y Suspensión del Centro")),
        "la carátula de arriba sigue leyéndose como pares: {taller:?}"
    );
    let cantidad = values_of(&engine, "Cantidad");
    assert_eq!(
        cantidad.len(),
        2,
        "la tabla de abajo sigue leyéndose como tabla: {cantidad:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn values_of(engine: &OmegaEngine, concept: &str) -> Vec<String> {
    let tools =
        omega_core::ToolEngine::new(omega_core::Database::open(engine.database_path()).unwrap());
    tools.concept_values(concept).unwrap()
}

fn concept_exists(engine: &OmegaEngine, name: &str) -> bool {
    engine
        .concepts(Some(name))
        .unwrap()
        .into_iter()
        .any(|concept| concept.display_name == name)
}

fn index(root: &Path) -> OmegaEngine {
    index_with_report(root).0
}

fn index_with_report(root: &Path) -> (OmegaEngine, omega_core::IndexReport) {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-libro.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    (engine, report)
}
