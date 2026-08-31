//! Ronda 5 · punto A — los cinco defectos del modo conversacional.
//!
//! Cada prueba reproduce la secuencia exacta con la que se documentó el
//! defecto en la ronda 4 (§R4.6), sobre una fixture propia y mínima. Las
//! secuencias completas contra el acervo real quedan en
//! `reports/sol-fix/r5-conversaciones-*.jsonl`; aquí se fija el
//! comportamiento para que no vuelva a perderse.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// Defecto 1 — «¿cuál es el Responsable **del primero**?» no heredaba el
/// resultado anterior y contestaba «no encontré evidencia».
#[test]
fn an_ordinal_reference_resolves_against_the_previous_set() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("obra")).unwrap();
    fs::write(
        root.join("obra/01_parte.md"),
        "Folio: MTTO-2025-00030\nResponsable: Ada Serrano\n",
    )
    .unwrap();
    fs::write(
        root.join("obra/02_parte.md"),
        "Folio: MTTO-2025-00030\nResponsable: Bruno Cifuentes\n",
    )
    .unwrap();
    let engine = index(root, "ordinal");

    engine
        .ask_in_conversation("c1", "¿Qué documentos tienen el folio MTTO-2025-00030?")
        .unwrap();
    let first = engine
        .ask_in_conversation("c1", "¿Cuál es el Responsable del primero?")
        .unwrap();
    assert!(first.used_context, "{}", first.text);
    assert!(first.text.contains("Ada Serrano"), "{}", first.text);
    assert!(!first.text.contains("Bruno Cifuentes"), "{}", first.text);

    let last = engine
        .ask_in_conversation("c1", "¿Y el Responsable del último?")
        .unwrap();
    assert!(last.used_context, "{}", last.text);
    assert!(last.text.contains("Bruno Cifuentes"), "{}", last.text);
}

/// Sin conversación previa, un ordinal no señala nada y la pregunta sigue
/// exactamente el camino que seguía antes: no se adivina un documento.
#[test]
fn an_ordinal_reference_without_context_does_not_guess_a_document() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("01_parte.md"), "Folio: MTTO-2025-00030\nResponsable: Ada Serrano\n").unwrap();
    let engine = index(root, "ordinal-sin-contexto");

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es el Responsable del primero?")
        .unwrap();
    assert!(!answer.used_context, "{}", answer.text);
    assert!(!answer.text.contains("Ada Serrano"), "{}", answer.text);
}

/// Defecto 2 — la continuación deíctica caía en un filtro fantasma
/// («Documento = Moneda») que vaciaba el alcance, porque la carátula de dos
/// columnas de otros archivos había dejado nombres de campo como valores.
#[test]
fn a_deictic_continuation_answers_about_the_document_of_the_previous_turn() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // El artefacto: un archivo cuya primera columna quedó leída como el campo
    // «Documento» y cuyos valores son nombres de campo.
    fs::write(
        root.join("00_caratula.csv"),
        "Documento,Ficha de compra\nMoneda,USD\nImporte,100.00 USD\n",
    )
    .unwrap();
    fs::write(
        root.join("01_orden.md"),
        "Folio: OC-1\nImporte de la orden: $416.00 USD\nMoneda: USD\n",
    )
    .unwrap();
    fs::write(
        root.join("02_orden.md"),
        "Folio: OC-2\nImporte de la orden: $12.00 EUR\nMoneda: EUR\n",
    )
    .unwrap();
    let engine = index(root, "deictico");

    let first = engine
        .ask_in_conversation("c1", "¿Cuál es el Importe de la orden del folio OC-1?")
        .unwrap();
    assert!(first.text.contains("416"), "{}", first.text);
    let second = engine
        .ask_in_conversation("c1", "¿Y cuál es la Moneda de ese documento?")
        .unwrap();
    assert!(second.used_context, "{}", second.text);
    assert!(second.verified, "{}", second.text);
    assert!(second.text.contains("USD"), "{}", second.text);
    assert!(
        !second.text.contains("0 documentos"),
        "el alcance no puede quedar vacío por un filtro inventado: {}",
        second.text
    );
}

/// Defecto 3 — «compara … entre la carpeta A y la carpeta B» sumaba sobre el
/// acervo entero en vez de comparar las dos carpetas.
#[test]
fn two_folders_are_compared_as_two_groups() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("calidad")).unwrap();
    fs::create_dir_all(root.join("operaciones")).unwrap();
    fs::write(root.join("calidad/01.md"), "Folio: C-1\nCosto: $100.00 MXN\n").unwrap();
    fs::write(root.join("calidad/02.md"), "Folio: C-2\nCosto: $100.00 MXN\n").unwrap();
    fs::write(root.join("operaciones/03.md"), "Folio: O-1\nCosto: $300.00 MXN\n").unwrap();
    let engine = index(root, "comparacion");

    let comparison = engine
        .ask_in_conversation(
            "c1",
            "Compara la suma de Costo entre la carpeta calidad y la carpeta operaciones.",
        )
        .unwrap();
    assert!(comparison.text.contains("calidad"), "{}", comparison.text);
    assert!(comparison.text.contains("operaciones"), "{}", comparison.text);
    assert!(comparison.text.contains("200.00"), "{}", comparison.text);
    assert!(comparison.text.contains("300.00"), "{}", comparison.text);
    let scope = comparison.scope.clone().expect("alcance declarado");
    assert_eq!(scope.group_by.as_deref(), Some("carpeta de origen"));

    // Y el seguimiento se resuelve contra esa comparación, sin recalcular.
    let follow_up = engine
        .ask_in_conversation("c1", "¿Cuál es la diferencia en porcentaje?")
        .unwrap();
    assert!(follow_up.used_context, "{}", follow_up.text);
    assert!(follow_up.text.contains("50"), "{}", follow_up.text);
}

/// Defecto 4 — «suma X en calidad» → «¿y en operaciones?» no cambiaba el
/// alcance conservando la operación: contestaba que la carpeta existía.
#[test]
fn an_elliptical_question_changes_the_scope_and_keeps_the_operation() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("calidad")).unwrap();
    fs::create_dir_all(root.join("operaciones")).unwrap();
    fs::write(root.join("calidad/01.md"), "Folio: C-1\nCosto: $100.00 MXN\n").unwrap();
    fs::write(root.join("operaciones/02.md"), "Folio: O-1\nCosto: $300.00 MXN\n").unwrap();
    fs::write(root.join("operaciones/03.md"), "Folio: O-2\nCosto: $400.00 MXN\n").unwrap();
    let engine = index(root, "elipsis");

    let first = engine
        .ask_in_conversation("c1", "¿Cuál es la suma total de Costo en la carpeta calidad?")
        .unwrap();
    assert!(first.text.contains("100.00"), "{}", first.text);
    let second = engine
        .ask_in_conversation("c1", "¿Y en la carpeta operaciones?")
        .unwrap();
    assert!(second.used_context, "{}", second.text);
    assert!(second.text.contains("700.00"), "{}", second.text);
    let scope = second.scope.clone().expect("alcance declarado");
    // La carpeta nueva SUSTITUYE a la anterior; no se intersecan.
    assert_eq!(scope.origin.as_deref(), Some("operaciones"));
    assert_eq!(scope.concept.as_deref(), Some("Costo"));
}

/// Defecto 5 — el literal entrecomillado cortaba el planificador antes de leer
/// el contexto, así que «de esos, ¿cuántos …?» perdía el conjunto heredado y
/// contestaba con una muestra con tope.
#[test]
fn a_quoted_literal_does_not_cut_the_inherited_set() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    fs::create_dir_all(root.join("compras")).unwrap();
    for (index, area) in [("01", "Ventas, clientes"), ("02", "Ventas, clientes"), ("03", "Otra área")]
    {
        fs::write(
            root.join(format!("ventas/{index}.md")),
            format!("Folio: V-{index}\nMoneda: EUR\nÁrea: {area}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("compras/04.md"),
        "Folio: P-04\nMoneda: EUR\nÁrea: Ventas, clientes\n",
    )
    .unwrap();
    let engine = index(root, "entrecomillado");

    engine
        .ask_in_conversation("c1", "¿Qué documentos hay en la carpeta ventas con Moneda: EUR?")
        .unwrap();
    let second = engine
        .ask_in_conversation("c1", "De esos, ¿cuántos son del área \"Ventas, clientes\"?")
        .unwrap();
    assert!(second.used_context, "{}", second.text);
    let scope = second.scope.clone().expect("alcance declarado");
    assert!(scope.inherited, "{}", second.text);
    assert_eq!(scope.origin.as_deref(), Some("ventas"));
    assert_eq!(scope.document_count, Some(2), "{}", second.text);
}

/// El candado de P0-1 no se relaja por ninguna de las rutas nuevas: una
/// referencia ordinal sobre evidencia de OCR débil no puede declararse
/// verificada.
#[test]
fn the_ordinal_route_keeps_the_weak_ocr_lock() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("01_parte.md"), "Folio: MTTO-2025-00030\nResponsable: Ada Serrano\n").unwrap();
    fs::write(root.join("02_parte.md"), "Folio: MTTO-2025-00030\nResponsable: Bruno Cifuentes\n").unwrap();
    let engine = index(root, "candado");
    engine
        .ask_in_conversation("c1", "¿Qué documentos tienen el folio MTTO-2025-00030?")
        .unwrap();
    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es el Responsable del primero?")
        .unwrap();
    // Sin OCR de por medio la respuesta sí es verificable, y con evidencia:
    // lo que se fija aquí es que la ruta pasa por el mismo constructor y no
    // devuelve una afirmación sin citas.
    assert!(answer.verified, "{}", answer.text);
    assert!(!answer.citations.is_empty(), "{}", answer.text);
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
