//! Ronda 4 · punto 1 — cobertura declarada en sumas de dinero.
//!
//! Antes, una suma cuyo campo no tuviera ni un valor en el alcance se
//! contestaba sólo con «No encontré valores de «X»…»: una negativa correcta
//! pero ciega, que no distinguía «el alcance no tiene nada monetario» de «el
//! alcance sí tiene un campo monetario en casi todos sus documentos, sólo que
//! no se llama como tú lo llamaste». Ahora, cuando cada documento determina
//! **por sí mismo** un único valor de la misma categoría, se calcula sobre
//! ésos y la respuesta declara cuántos de cuántos cubrió y por qué quedó fuera
//! cada uno de los demás.
//!
//! Se comprueban las dos direcciones: el caso que se arregla y —sobre todo—
//! los casos normales que NO deben cambiar (el campo pedido sí está; ningún
//! documento determina un único valor).

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// El campo pedido existe en el acervo pero no en el alcance, y cada documento
/// del alcance nombra su propio campo monetario: se suma sobre los que lo
/// determinan y se declara la cobertura, en vez de negarse sin más.
#[test]
fn a_sum_over_a_field_absent_from_the_scope_declares_what_it_covered() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("calidad")).unwrap();
    fs::create_dir_all(root.join("ventas")).unwrap();
    fs::write(
        root.join("calidad/01_hallazgo.md"),
        "Folio: CAL-1\nCosto de revisión: $100.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("calidad/02_hallazgo.md"),
        "Folio: CAL-2\nCosto de auditoría: $200.00 MXN\n",
    )
    .unwrap();
    // Sin ningún campo monetario.
    fs::write(root.join("calidad/03_hallazgo.md"), "Folio: CAL-3\nEstado: Abierto\n").unwrap();
    // Dos campos monetarios: el documento no dice cuál es el principal.
    fs::write(
        root.join("calidad/04_hallazgo.md"),
        "Folio: CAL-4\nCosto de revisión: $50.00 MXN\nCosto de auditoría: $25.00 MXN\n",
    )
    .unwrap();
    // «Importe total» existe en el acervo, pero en otra carpeta.
    fs::write(
        root.join("ventas/05_pedido.md"),
        "Folio: VEN-5\nImporte total: $999.00 MXN\n",
    )
    .unwrap();

    let engine = index(root, "cobertura-1");
    let answer = engine
        .ask("¿Cuál es la suma total de importe total en la carpeta calidad?")
        .unwrap();

    assert!(
        answer.text.contains("No encontré ningún valor de «Importe total»"),
        "la primera línea tiene que decir que el campo pedido no está: {}",
        answer.text
    );
    assert!(
        answer.text.contains("$300.00 MXN"),
        "suma de los dos documentos que sí determinan su campo monetario: {}",
        answer.text
    );
    assert!(
        answer.text.contains("Cobertura: 2 de 4 documentos del alcance"),
        "la cobertura va declarada con las dos cifras: {}",
        answer.text
    );
    assert!(
        answer.text.contains("1 sin ningún campo de esa clase"),
        "{}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("1 con más de uno, sin que el documento diga cuál es el principal"),
        "un documento con dos importes se excluye y se dice por qué: {}",
        answer.text
    );
    assert!(
        answer.text.contains("«Costo de revisión»") && answer.text.contains("«Costo de auditoría»"),
        "los campos realmente usados se nombran: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "la cifra es de campos distintos del pedido: nunca puede darse por verificada"
    );
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("cubre 2 de 4")),
        "el aviso repite la cobertura para quien sólo lee el aviso: {:?}",
        answer.warning
    );
    let scope = answer.scope.expect("un cálculo declara su alcance");
    assert_eq!(scope.document_count, Some(4));
    assert_eq!(scope.value_count, Some(2));
    assert_eq!(scope.excluded_count, Some(2));
}

/// El campo pedido sí está en el alcance: la ruta normal no cambia en nada y
/// no aparece ninguna sustitución por categoría.
#[test]
fn a_sum_over_a_field_present_in_the_scope_is_untouched() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    fs::write(
        root.join("ventas/01_pedido.md"),
        "Folio: VEN-1\nImporte total: $100.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("ventas/02_pedido.md"),
        "Folio: VEN-2\nImporte total: $250.00 MXN\n",
    )
    .unwrap();

    let engine = index(root, "cobertura-2");
    let answer = engine
        .ask("¿Cuál es la suma total de importe total en la carpeta ventas?")
        .unwrap();

    assert!(
        answer.text.starts_with("Suma de «Importe total»"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("$350.00 MXN"), "{}", answer.text);
    assert!(
        !answer.text.contains("Cobertura:"),
        "la ruta por categoría no debe activarse cuando el campo pedido está: {}",
        answer.text
    );
    assert!(answer.verified);
}

/// Ningún documento del alcance determina un único campo monetario: la
/// negativa sigue siendo la respuesta correcta y no se inventa una cobertura.
#[test]
fn a_scope_where_no_document_determines_one_money_field_still_refuses() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("calidad")).unwrap();
    fs::create_dir_all(root.join("ventas")).unwrap();
    fs::write(
        root.join("calidad/01_hallazgo.md"),
        "Folio: CAL-1\nCosto de revisión: $10.00 MXN\nCosto de auditoría: $20.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("calidad/02_hallazgo.md"),
        "Folio: CAL-2\nCosto de revisión: $30.00 MXN\nCosto de auditoría: $40.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("ventas/03_pedido.md"),
        "Folio: VEN-3\nImporte total: $999.00 MXN\n",
    )
    .unwrap();

    let engine = index(root, "cobertura-3");
    let answer = engine
        .ask("¿Cuál es la suma total de importe total en la carpeta calidad?")
        .unwrap();

    assert!(
        answer.text.starts_with("No encontré valores de «Importe total»"),
        "sin ningún documento que determine su campo monetario, la negativa original se conserva: {}",
        answer.text
    );
    assert!(!answer.verified);
    assert!(answer.citations.is_empty());
}

/// La moneda escrita en la pregunta acota el cálculo y lo que queda fuera por
/// ese motivo se declara aparte, no se mezcla con «documentos sin el campo».
#[test]
fn a_currency_written_in_the_question_narrows_the_sum_and_declares_the_rest() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    fs::write(
        root.join("ventas/01_pedido.md"),
        "Folio: VEN-1\nImporte total: $100.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("ventas/02_pedido.md"),
        "Folio: VEN-2\nImporte total: $250.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("ventas/03_pedido.md"),
        "Folio: VEN-3\nImporte total: $70.00 USD\n",
    )
    .unwrap();

    let engine = index(root, "cobertura-4");
    let answer = engine
        .ask("¿Cuál es la suma total de importe total en la carpeta ventas en MXN?")
        .unwrap();

    assert!(answer.text.contains("$350.00 MXN"), "{}", answer.text);
    assert!(
        !answer.text.contains("USD"),
        "la moneda pedida acota el cálculo: {}",
        answer.text
    );
    assert!(
        answer.text.contains("1 documento en otra moneda"),
        "lo excluido por moneda se declara por su propio motivo: {}",
        answer.text
    );
    let scope = answer.scope.expect("un cálculo declara su alcance");
    assert_eq!(scope.currency.as_deref(), Some("MXN"));
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
