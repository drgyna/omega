//! Ronda 7 — operaciones sobre un documento señalado por su clave.
//!
//! Dos defectos distintos, con la misma raíz: la pregunta señalaba **un**
//! documento (o dos) por su clave interna y el motor no acotaba nada a él.
//!
//!  1. «Para el documento doc_id=D#####, ¿cuál es el resultado de dividir el
//!     importe entre la cantidad registrada?» se contestaba «El campo
//!     «Cantidad» es 0 piezas», con cita y marcada como verificada: no se
//!     dividía, y el valor del divisor se presentaba como si fuera el
//!     resultado. La ruta de operación entre campos existía, pero exigía que
//!     los dos operandos fueran nombres de campo del acervo, y «el importe» no
//!     lo es — los campos se llaman «Importe del pedido», «Importe pagado»,
//!     «Costo estimado de no conformidad».
//!  2. «Los documentos D##### y D##### … reportan el campo "X". ¿Coinciden?»
//!     se contestaba «No encontré evidencia local suficiente», con los dos
//!     documentos localizables y el campo pedido a la vista.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// Un divisor de cero deja la operación indeterminada, y se dice.
#[test]
fn dividing_by_a_zero_quantity_is_declared_indeterminate_not_answered_with_the_divisor() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("01147_bitacora.md"),
        "Folio: MTTO-1\nImporte del pedido: $12,000.00 MXN\nCantidad: 0 piezas\n",
    )
    .unwrap();

    let engine = index(root, "operacion-1");
    let answer = engine
        .ask("Para el documento doc_id=D01147, ¿cuál es el resultado de dividir el importe entre la cantidad registrada?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("la operación indeterminada se declara: {}", answer.text));
    assert_eq!(clarification.reason, "operacion_indeterminada");
    assert!(
        clarification.question.contains("no se puede dividir entre cero"),
        "{}",
        clarification.question
    );
    assert!(
        clarification.question.contains("0 piezas"),
        "el valor exacto con el que se topó queda a la vista: {}",
        clarification.question
    );
}

/// Un divisor que no es una cifra tampoco se convierte en respuesta.
#[test]
fn a_non_numeric_quantity_is_declared_indeterminate() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("02082_no_conformidad.md"),
        "Folio: NC-1\nCosto estimado: $221,781.53 MXN\nCantidad: N/D piezas\n",
    )
    .unwrap();

    let engine = index(root, "operacion-2");
    let answer = engine
        .ask("Para el documento doc_id=D02082, ¿cuál es el resultado de dividir el importe entre la cantidad registrada?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("{}", answer.text));
    assert_eq!(clarification.reason, "operacion_indeterminada");
    assert!(
        clarification.question.contains("N/D piezas"),
        "{}",
        clarification.question
    );
}

/// Cuando los dos operandos sí son cifras, se divide y se muestra la fórmula.
#[test]
fn a_computable_division_gives_the_quotient_with_both_citations() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("04899_pago.md"),
        "Folio: FAC-1\nImporte pagado: $1,000.00 MXN\nCantidad: 4 piezas\n",
    )
    .unwrap();

    let engine = index(root, "operacion-3");
    let answer = engine
        .ask("Para el documento doc_id=D04899, ¿cuál es el resultado de dividir el importe entre la cantidad registrada?")
        .unwrap();

    assert!(answer.clarification.is_none(), "{}", answer.text);
    assert!(
        answer.text.contains("250"),
        "1.000 entre 4 son 250: {}",
        answer.text
    );
    assert!(
        answer.text.contains("4 piezas"),
        "la unidad del divisor se conserva tal como el documento la escribió: {}",
        answer.text
    );
    assert_eq!(
        answer.citations.len(),
        2,
        "una cita por operando: {:?}",
        answer.citations
    );
}

/// Con varios valores del mismo operando en un documento, se pregunta.
#[test]
fn several_values_of_the_same_operand_are_a_question_not_a_guess() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("00084_pedido.md"),
        "Folio: PED-1\nImporte del pedido: $100.00 MXN\nImporte unitario: $25.00 MXN\nCantidad: 4 piezas\n",
    )
    .unwrap();

    let engine = index(root, "operacion-4");
    let answer = engine
        .ask("Para el documento doc_id=D00084, ¿cuál es el resultado de dividir el importe entre la cantidad registrada?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("{}", answer.text));
    assert_eq!(clarification.reason, "operando_multiple_en_documento");
    assert_eq!(clarification.options.len(), 2, "{:?}", clarification.options);
}

/// Dos documentos, un campo: si difieren, se dice, y no se decide cuál vale.
#[test]
fn two_documents_that_disagree_get_both_values_and_a_question() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("05351_orden_compra.md"),
        "Folio: OC-1\nImporte de la orden: $444,954.93 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("04899_orden_compra.md"),
        "Folio: OC-2\nImporte de la orden: $414,834.92 MXN\n",
    )
    .unwrap();

    let engine = index(root, "operacion-5");
    let answer = engine
        .ask("Los documentos D05351 y D04899 pertenecen al expediente CASE-0093 y reportan el campo \"Importe de la orden\". ¿Coinciden los valores registrados?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("{}", answer.text));
    assert_eq!(clarification.reason, "valores_discrepantes");
    assert!(
        clarification.question.contains("444,954.93")
            && clarification.question.contains("414,834.92"),
        "los dos valores citados van en la respuesta: {}",
        clarification.question
    );
    assert_eq!(answer.citations.len(), 2, "{:?}", answer.citations);
}

/// Si coinciden, es una afirmación con evidencia, no una aclaración.
#[test]
fn two_documents_that_agree_get_a_plain_answer() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("05351_orden_compra.md"),
        "Folio: OC-1\nImporte de la orden: $444,954.93 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("04899_orden_compra.md"),
        "Folio: OC-2\nImporte de la orden: $444,954.93 MXN\n",
    )
    .unwrap();

    let engine = index(root, "operacion-6");
    let answer = engine
        .ask("Los documentos D05351 y D04899 reportan el campo \"Importe de la orden\". ¿Coinciden los valores registrados?")
        .unwrap();

    assert!(answer.clarification.is_none(), "{}", answer.text);
    assert!(answer.text.starts_with("Sí coinciden"), "{}", answer.text);
    assert_eq!(answer.citations.len(), 2);
}

/// Si el segundo documento no registra ese campo, se dice — nunca se compara
/// contra «el otro campo parecido» como si fuera el mismo.
#[test]
fn a_document_without_the_named_field_is_said_not_substituted() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("05351_orden_compra.md"),
        "Folio: OC-1\nImporte de la orden: $444,954.93 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("04899_pago.md"),
        "Folio: FAC-1\nImporte pagado: $414,834.92 MXN\n",
    )
    .unwrap();

    let engine = index(root, "operacion-7");
    let answer = engine
        .ask("Los documentos D05351 y D04899 pertenecen al expediente CASE-0093 y reportan el campo \"Importe de la orden\". ¿Coinciden los valores registrados?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("{}", answer.text));
    assert_eq!(clarification.reason, "campo_ausente_en_un_documento");
    assert!(
        !clarification.question.contains("No coinciden"),
        "dos campos con nombres distintos no son una contradicción: {}",
        clarification.question
    );
    assert!(
        clarification
            .options
            .iter()
            .any(|option| option == "Importe pagado"),
        "lo que ese documento sí registra se ofrece como opción: {:?}",
        clarification.options
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
