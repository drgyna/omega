//! Ronda 4 · punto 1 — dos formas de vaciar un alcance por inferencia.
//!
//! Las dos aparecieron reproduciendo `CALC-SUM-calidad-MXN`, y las dos hacían
//! que la respuesta pareciera «el acervo no tiene esos datos» cuando en
//! realidad era «pedí una condición imposible»:
//!
//! 1. Una palabra que la pregunta escribe como NOMBRE de campo («area=…»,
//!    «moneda=…») se reutilizaba como VALOR inferido de otro campo. En un
//!    acervo cuya carátula de dos columnas dejó los propios nombres de campo
//!    como valores del concepto «Documento», eso añadía filtros
//!    («Documento = Área») que el usuario nunca pidió.
//! 2. Varias grafías OCR del mismo rótulo («Moneda», «Monede») se reconocían
//!    todas y se aplicaban **en conjunción**: ningún documento tiene los dos
//!    campos, así que el alcance quedaba vacío siempre.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// «area=Norte» filtra por Área, no además por el campo cuyo valor es la
/// palabra «Área».
#[test]
fn a_word_written_as_a_field_name_is_not_reused_as_an_inferred_value() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // Carátula de dos columnas: el nombre del campo quedó como VALOR.
    fs::write(root.join("01_caratula.md"), "Documento: Área\nFolio: A-1\n").unwrap();
    fs::write(root.join("02_registro.md"), "Área: Norte\nFolio: A-2\n").unwrap();
    fs::write(root.join("03_registro.md"), "Área: Norte\nFolio: A-3\n").unwrap();

    let engine = index(root, "inferencia-1");
    let answer = engine.ask("¿Cuántos documentos hay para area=Norte?").unwrap();

    assert!(
        answer.text.starts_with("2 documentos"),
        "el alcance es el de «Área = Norte», sin el filtro inventado: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Documento = Área"),
        "no puede añadirse un filtro que el usuario no escribió: {}",
        answer.text
    );
}

/// Dos grafías del mismo rótulo son un campo, no dos condiciones que haya que
/// cumplir a la vez.
#[test]
fn two_ocr_spellings_of_one_label_do_not_empty_the_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("01_pedido.md"), "Folio: P-1\nMoneda: MXN\n").unwrap();
    fs::write(root.join("02_pedido.md"), "Folio: P-2\nMoneda: MXN\n").unwrap();
    fs::write(root.join("03_pedido.md"), "Folio: P-3\nMoneda: MXN\n").unwrap();
    // Misma etiqueta, leída por OCR con una letra distinta.
    fs::write(root.join("04_pedido.md"), "Folio: P-4\nMonede: MXN\n").unwrap();

    let engine = index(root, "inferencia-2");
    let answer = engine.ask("¿Cuántos documentos hay con moneda MXN?").unwrap();

    assert!(
        answer.text.starts_with("3 documentos"),
        "se conserva la grafía dominante, no la conjunción imposible de las dos: {}",
        answer.text
    );
}

/// La tolerancia anterior no puede fusionar dos campos realmente distintos:
/// «Importe» e «Importe total» siguen siendo dos condiciones.
#[test]
fn two_genuinely_different_fields_are_never_collapsed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("01_pedido.md"),
        "Folio: P-1\nImporte: Pendiente\nImporte total: Pendiente\n",
    )
    .unwrap();
    fs::write(root.join("02_pedido.md"), "Folio: P-2\nImporte: Pendiente\n").unwrap();

    let engine = index(root, "inferencia-3");
    let answer = engine
        .ask("¿Cuántos documentos hay con Importe: Pendiente y Importe total: Pendiente?")
        .unwrap();

    assert!(
        answer.text.starts_with("1 documento"),
        "los dos filtros siguen siendo dos: {}",
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
