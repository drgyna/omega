//! Un filtro «Campo: valor» compara valores, no su forma normalizada para
//! búsqueda difusa. Esa normalización borra puntuación (y aplica raíces), así
//! que un número y su porcentaje homónimo, o un valor citado y su versión sin
//! comillas, terminaban comparando iguales sólo porque compartían dígitos o
//! letras. El motor ahora conserva el tipo del valor de forma explícita al
//! comparar filtros — no lo vuelve a adivinar a partir de un texto que ya
//! perdió su puntuación.
//!
//! Las fixtures son genéricas (inventario, facturas, ubicaciones) y se
//! escriben en un directorio temporal (`tempfile`). No proceden de ningún
//! corpus del repositorio.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// `50` y `50%` son valores distintos: un filtro por uno no debe traer
/// documentos del otro sólo porque comparten los mismos dígitos.
#[test]
fn a_plain_number_and_its_percentage_of_the_same_digits_are_different_filters() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "doc-01.md", &[("Folio", "INV-01"), ("Descuento", "50")]);
    write_record(root, "doc-02.md", &[("Folio", "INV-02"), ("Descuento", "50%")]);
    let engine = index(root);

    let number = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Descuento: 50?")
        .unwrap();
    assert!(number.text.contains('1'), "{}", number.text);
    assert!(!number.text.contains("2 documentos"), "{}", number.text);

    let percent = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Descuento: 50%?")
        .unwrap();
    assert!(percent.text.contains('1'), "{}", percent.text);
    assert!(!percent.text.contains("2 documentos"), "{}", percent.text);
}

/// Lo mismo con separador de millares: `1,000` y `1,000%` no colapsan en el
/// mismo valor al quitarles la coma.
#[test]
fn a_grouped_thousand_and_its_percentage_are_different_filters() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "doc-01.md", &[("Folio", "INV-01"), ("Existencias", "1,000")]);
    write_record(root, "doc-02.md", &[("Folio", "INV-02"), ("Existencias", "1,000%")]);
    let engine = index(root);

    let number = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Existencias: 1,000?")
        .unwrap();
    assert!(number.text.contains('1'), "{}", number.text);
    assert!(!number.text.contains("2 documentos"), "{}", number.text);

    let percent = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Existencias: 1,000%?")
        .unwrap();
    assert!(percent.text.contains('1'), "{}", percent.text);
    assert!(!percent.text.contains("2 documentos"), "{}", percent.text);
}

/// Un valor citado entre comillas guillemet conserva su valor completo: no es
/// lo mismo que el mismo texto sin comillas, aunque ambos sean campos de
/// texto (no hay un tipo numérico que los distinga).
#[test]
fn a_value_quoted_in_guillemets_keeps_its_full_value_distinct_from_the_bare_word() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "doc-01.md", &[("Folio", "FAC-01"), ("Estado", "Pendiente")]);
    write_record(root, "doc-02.md", &[("Folio", "FAC-02"), ("Estado", "«Pendiente»")]);
    let engine = index(root);

    let bare = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Pendiente?")
        .unwrap();
    assert!(bare.text.contains('1'), "{}", bare.text);
    assert!(!bare.text.contains("2 documentos"), "{}", bare.text);

    let quoted = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Estado: «Pendiente»?")
        .unwrap();
    assert!(quoted.text.contains('1'), "{}", quoted.text);
    assert!(!quoted.text.contains("2 documentos"), "{}", quoted.text);
}

/// Un valor con dos puntos internos no se corta ni se confunde con un valor
/// parecido que use otro separador: «3:2» y «3-2» normalizan al mismo texto
/// difuso («3 2»), pero son valores distintos.
#[test]
fn a_value_with_a_colon_is_not_cut_or_confused_with_a_similar_separator() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "doc-01.md", &[("Folio", "PRY-01"), ("Marcador", "3-2")]);
    write_record(root, "doc-02.md", &[("Folio", "PRY-02"), ("Marcador", "3:2")]);
    let engine = index(root);

    let hyphen = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Marcador: 3-2?")
        .unwrap();
    assert!(hyphen.text.contains('1'), "{}", hyphen.text);
    assert!(!hyphen.text.contains("2 documentos"), "{}", hyphen.text);

    let colon = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Marcador: 3:2?")
        .unwrap();
    assert!(
        colon.text.contains('1'),
        "«3:2» no debe cortarse en «3»: {}",
        colon.text
    );
    assert!(!colon.text.contains("2 documentos"), "{}", colon.text);
}

/// Un filtro que un turno anterior dejó en el contexto conserva su tipo
/// original cuando un turno posterior lo reutiliza para otro cálculo: el
/// motor no vuelve a adivinar el tipo a partir de una forma degradada.
#[test]
fn a_filter_inherited_across_turns_keeps_its_original_value_kind() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[("Folio", "INV-01"), ("Descuento", "50"), ("Importe", "$100.00")],
    );
    write_record(
        root,
        "doc-02.md",
        &[("Folio", "INV-02"), ("Descuento", "50%"), ("Importe", "$900.00")],
    );
    let engine = index(root);

    let first = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Descuento: 50%?")
        .unwrap();
    assert!(first.text.contains('1'), "conteo inicial: {}", first.text);
    assert!(!first.text.contains("2 documentos"), "{}", first.text);

    // El segundo turno no vuelve a escribir «Descuento: 50%»: hereda el
    // filtro del turno anterior. El alcance tiene dos campos numéricos
    // («Descuento» e «Importe»), así que el motor pregunta cuál usar en vez
    // de adivinar — el mismo patrón que un campo ambiguo en cualquier otro
    // alcance.
    let ambiguous = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    let clarification = ambiguous
        .clarification
        .clone()
        .expect("con dos campos numéricos debe preguntar");
    assert!(clarification.options.iter().any(|option| option == "Importe"));

    // Elegir «Importe» continúa el alcance anterior: si el tipo del filtro
    // heredado se hubiera perdido («50%» degradado a «50»), la suma
    // incluiría también el documento con «Descuento: 50» (Importe $100.00) y
    // el total sería $1,000.00 en vez de $900.00.
    let chosen = engine.ask_in_conversation("c1", "Importe").unwrap();
    assert!(chosen.used_context, "debe heredar el alcance: {}", chosen.text);
    assert!(
        chosen.text.contains("900"),
        "debe sumar sólo el documento con 50%: {}",
        chosen.text
    );
    assert!(
        !chosen.text.contains("1,000"),
        "no debe mezclar el documento con «50» (sin %): {}",
        chosen.text
    );
    let scope = chosen.scope.clone().expect("alcance declarado");
    assert_eq!(scope.filters.len(), 1);
    assert_eq!(scope.filters[0].equals, "50%");
}

/// Campos y valores con acentos y otros caracteres Unicode siguen
/// resolviéndose igual que antes de este cambio: la comparación literal
/// pliega mayúsculas y acentos, sólo conserva la puntuación que distingue el
/// tipo del valor.
#[test]
fn unicode_fields_and_values_keep_matching_regardless_of_accents_or_case() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[("Folio", "RES-01"), ("Ubicación", "Bahía de Banderas")],
    );
    write_record(root, "doc-02.md", &[("Folio", "RES-02"), ("Ubicación", "Cancún")]);
    let engine = index(root);

    // Sin acentos y en minúsculas: debe seguir encontrando el documento.
    let answer = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Ubicación: bahia de banderas?")
        .unwrap();
    assert!(answer.text.contains('1'), "{}", answer.text);
    assert!(!answer.text.contains("2 documentos"), "{}", answer.text);

    // Con los acentos tal como están escritos, también.
    let accented = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Ubicación: Bahía de Banderas?")
        .unwrap();
    assert!(accented.text.contains('1'), "{}", accented.text);
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-test.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}

fn write_record(root: &Path, name: &str, fields: &[(&str, &str)]) {
    let body = fields
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(root.join(name), format!("# Registro\n\n{body}\n")).unwrap();
}
