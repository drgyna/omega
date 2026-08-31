//! P1-B.3 — Una hoja de cálculo no guarda «15%»: guarda 0.15 y un formato de
//! celda que dice cómo leerlo. Ignorar el formato convertía un porcentaje en
//! un número suelto y un importe en una cifra sin moneda.
//!
//! Y una fórmula no es un valor: su resultado en caché puede faltar o estar
//! desactualizado. En ninguno de los dos casos el motor puede publicar una
//! cifra que nadie escribió.
//!
//! Fixtures genéricas: un inventario y un reporte de ventas.

use std::path::Path;

use omega_core::{Clock, OmegaEngine};

#[path = "support/mod.rs"]
mod support;

use support::SheetCell;

const TODAY: &str = "2026-08-25";

/// El formato de celda `0.00%` es lo único que distingue 0.15 de 15%.
#[test]
fn a_percentage_cell_format_keeps_its_percentage_meaning() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("ventas.xlsx"),
        "Ventas",
        &[
            vec![SheetCell::text("Folio"), SheetCell::text("Margen")],
            vec![
                SheetCell::text("VTA-001"),
                SheetCell::formatted("0.15", "0.00%"),
            ],
            vec![
                SheetCell::text("VTA-002"),
                SheetCell::formatted("0.075", "0.0%"),
            ],
        ],
    );
    let engine = index(root);

    assert_eq!(concept_type(&engine, "Margen"), "percentage");
    let values = values_of(&engine, "Margen");
    assert!(
        values.contains(&"15%".to_owned()),
        "0.15 con formato de porcentaje es 15%, no 0.15: {values:?}"
    );
    assert!(
        values.contains(&"7.5%".to_owned()),
        "0.075 con formato de porcentaje es 7.5%: {values:?}"
    );
    assert!(
        !values.iter().any(|value| value == "0.15"),
        "el valor no puede publicarse como si fuera un número suelto: {values:?}"
    );
}

/// El formato con código de moneda conserva la moneda escrita en la hoja.
#[test]
fn a_currency_cell_format_keeps_its_currency() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("inventario.xlsx"),
        "Inventario",
        &[
            vec![SheetCell::text("Clave"), SheetCell::text("Importe")],
            vec![
                SheetCell::text("INV-001"),
                SheetCell::formatted("1250", r#"#,##0.00" MXN""#),
            ],
            vec![
                SheetCell::text("INV-002"),
                SheetCell::formatted("750.5", r#"#,##0.00" MXN""#),
            ],
        ],
    );
    let engine = index(root);

    assert_eq!(concept_type(&engine, "Importe"), "money");
    let answer = engine
        .ask_in_conversation("libro", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(
        answer.text.contains("$2,000.50 MXN"),
        "1,250.00 + 750.50 = 2,000.50 MXN: {}",
        answer.text
    );
}

/// Un símbolo de moneda sin código no autoriza a inventar una moneda: es la
/// misma regla que ya rige para el texto plano.
#[test]
fn a_currency_symbol_without_a_code_never_invents_one() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("inventario.xlsx"),
        "Inventario",
        &[
            vec![SheetCell::text("Clave"), SheetCell::text("Importe")],
            vec![
                SheetCell::text("INV-001"),
                SheetCell::formatted("1250", r##""$"#,##0.00"##),
            ],
        ],
    );
    let engine = index(root);

    assert_eq!(concept_type(&engine, "Importe"), "money");
    let values = values_of(&engine, "Importe");
    assert!(
        values.iter().any(|value| value.contains("1,250.00")),
        "{values:?}"
    );
    assert!(
        !values.iter().any(|value| value.contains("MXN")),
        "la hoja no escribió ninguna moneda: no puede aparecer una: {values:?}"
    );
}

/// Una fórmula sin resultado en caché no tiene valor. El motor no puede
/// calcularla ni inventarla: la deja fuera y lo dice.
#[test]
fn a_formula_without_a_cached_value_never_invents_a_result() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("ventas.xlsx"),
        "Ventas",
        &[
            vec![SheetCell::text("Folio"), SheetCell::text("Importe")],
            vec![SheetCell::text("VTA-001"), SheetCell::number("100")],
            vec![SheetCell::text("VTA-002"), SheetCell::number("200")],
            vec![
                SheetCell::text("VTA-TOTAL"),
                SheetCell::formula_without_value("SUM(B2:B3)"),
            ],
        ],
    );
    let (engine, report) = index_with_report(root);

    let values = values_of(&engine, "Importe");
    assert_eq!(
        values.len(),
        2,
        "sólo los dos importes escritos son valores: {values:?}"
    );
    assert!(
        report.warnings.iter().any(|warning| {
            warning.contains("ventas.xlsx")
                && warning.contains("B4")
                && warning.contains("SUM(B2:B3)")
        }),
        "la celda de fórmula sin resultado debe quedar nombrada: {:?}",
        report.warnings
    );
    let answer = engine
        .ask_in_conversation("libro", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(
        answer.text.contains("$300.00") || answer.text.contains("300"),
        "100 + 200 = 300, sin fabricar el total de la fórmula: {}",
        answer.text
    );
}

/// Un libro marcado para recálculo completo declara que sus resultados en
/// caché ya no corresponden a sus fórmulas. Publicar esa cifra sería publicar
/// un número que la hoja misma desautoriza.
#[test]
fn a_workbook_flagged_for_recalculation_never_publishes_its_cached_formula_values() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_needing_recalculation(
        &root.join("ventas.xlsx"),
        "Ventas",
        &[
            vec![SheetCell::text("Folio"), SheetCell::text("Importe")],
            vec![SheetCell::text("VTA-001"), SheetCell::number("100")],
            vec![SheetCell::text("VTA-002"), SheetCell::number("200")],
            vec![
                SheetCell::text("VTA-TOTAL"),
                // El 999 en caché no corresponde a la fórmula.
                SheetCell::formula("SUM(B2:B3)", "999"),
            ],
        ],
    );
    let (engine, report) = index_with_report(root);

    let values = values_of(&engine, "Importe");
    assert!(
        !values.iter().any(|value| value.contains("999")),
        "un resultado en caché desautorizado por el propio libro no se publica: {values:?}"
    );
    assert_eq!(values.len(), 2, "{values:?}");
    assert!(
        report.warnings.iter().any(|warning| {
            warning.contains("ventas.xlsx") && warning.contains("recálculo")
        }),
        "el libro desactualizado debe informarse: {:?}",
        report.warnings
    );
}

/// Control: una fórmula con resultado en caché vigente sí es un valor.
#[test]
fn a_formula_with_a_fresh_cached_value_is_indexed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("ventas.xlsx"),
        "Ventas",
        &[
            vec![SheetCell::text("Folio"), SheetCell::text("Importe")],
            vec![SheetCell::text("VTA-001"), SheetCell::number("100")],
            vec![
                SheetCell::text("VTA-TOTAL"),
                SheetCell::formula("B2*2", "200"),
            ],
        ],
    );
    let engine = index(root);
    let values = values_of(&engine, "Importe");
    assert_eq!(values.len(), 2, "{values:?}");
    assert!(values.iter().any(|value| value.contains("200")), "{values:?}");
}

/// Control: una celda numérica sin formato especial sigue siendo un número.
#[test]
fn a_plain_number_cell_is_unaffected() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    support::write_xlsx_cells(
        &root.join("inventario.xlsx"),
        "Inventario",
        &[
            vec![SheetCell::text("Clave"), SheetCell::text("Cantidad")],
            vec![SheetCell::text("INV-001"), SheetCell::number("12")],
            vec![SheetCell::text("INV-002"), SheetCell::number("30")],
        ],
    );
    let engine = index(root);
    assert_eq!(concept_type(&engine, "Cantidad"), "number");
    let values = values_of(&engine, "Cantidad");
    assert!(values.contains(&"12".to_owned()), "{values:?}");
}

// ─────────────────────────────────────────────────────────────────────────

fn values_of(engine: &OmegaEngine, concept: &str) -> Vec<String> {
    let tools = omega_core::ToolEngine::new(
        omega_core::Database::open(engine.database_path()).unwrap(),
    );
    tools.concept_values(concept).unwrap()
}

fn concept_type(engine: &OmegaEngine, name: &str) -> String {
    engine
        .concepts(Some(name))
        .unwrap()
        .into_iter()
        .find(|concept| concept.display_name == name)
        .unwrap_or_else(|| panic!("el concepto {name} debe existir"))
        .value_type
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
