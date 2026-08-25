//! Regresiones comprobadas a mano sobre el corpus de yates (600 documentos).
//!
//! Cada bloque corresponde a un defecto real observado en la aplicación. El
//! corpus es material de control de calidad del repositorio y no se modifica:
//! las cifras esperadas se calcularon leyendo los propios archivos.
//!
//! El motor de producción no conoce este corpus; lo que se comprueba aquí son
//! reglas generales —no degradar un valor escrito, conservar el alcance,
//! respetar el campo que nombra el usuario— con datos reales de un acervo
//! grande.

use std::{collections::BTreeSet, path::PathBuf};

use omega_core::{Answer, Clock, OmegaEngine};

const TODAY: &str = "2026-08-24";

#[test]
fn the_yacht_corpus_survives_the_manual_regression_matrix() {
    let engine = index_yacht_corpus();

    explicit_values_are_never_degraded(&engine);
    a_clarification_keeps_the_previous_scope(&engine);
    an_explicit_field_beats_the_context(&engine);
    two_groups_are_really_compared(&engine);
    a_ranking_answers_with_the_group(&engine);
    a_capacity_never_links_documents(&engine);
    repeated_folios_with_different_states_are_not_a_list_of_folios(&engine);
    a_clarification_is_written_once(&engine);
}

/// Defecto 1: «Estado: Pendiente de emisión» se respondía con «Estado de pago =
/// Pendiente», recortando el valor pedido y marcando la respuesta como
/// verificada.
fn explicit_values_are_never_degraded(engine: &OmegaEngine) {
    let answer = ask(engine, "d1", "¿Cuántos documentos tienen Estado: Pendiente de emisión?");
    assert!(
        !answer.verified,
        "un valor que no existe no puede dar una respuesta verificada: {}",
        answer.text
    );
    assert!(answer.citations.is_empty());
    assert!(
        answer.text.contains("Pendiente de emisión"),
        "la respuesta debe nombrar el valor que se pidió: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Estado de pago"),
        "no puede sustituirse por otro campo: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("11 documentos"),
        "no puede contar los documentos de un valor más corto: {}",
        answer.text
    );

    // El mismo par, esta vez completo y existente, sigue funcionando.
    let exact = ask(engine, "d1", "¿Cuántos documentos tienen Estado de pago: Pendiente?");
    assert!(exact.verified);
    assert!(exact.text.contains("11 documentos"), "{}", exact.text);
    // Y no arrastra un segundo filtro inferido por parecido de palabras.
    assert!(
        !exact.text.contains("Estado = "),
        "sólo el filtro escrito: {}",
        exact.text
    );
}

/// Defecto 2: elegir una opción de la aclaración sumaba los 140 anticipos del
/// acervo en vez de los 11 documentos del resultado anterior.
fn a_clarification_keeps_the_previous_scope(engine: &OmegaEngine) {
    let first = ask(engine, "d2", "¿Cuántos documentos tienen Estado de pago: Pendiente?");
    assert!(first.text.contains("11 documentos"), "{}", first.text);

    let ambiguous = ask(engine, "d2", "¿Cuánto suman?");
    let clarification = ambiguous
        .clarification
        .clone()
        .expect("con cuatro campos numéricos debe preguntar");
    assert_eq!(clarification.reason, "campo_ambiguo");
    assert!(clarification.options.iter().any(|option| option == "Anticipo recibido"));

    let chosen = ask(engine, "d2", "Anticipo recibido");
    assert!(
        chosen.text.contains("$638,925.00 MXN"),
        "la suma debe ser la de los 11 pendientes: {}",
        chosen.text
    );
    assert!(
        !chosen.text.contains("$7,544,775.00"),
        "no puede sumar los 140 anticipos del acervo: {}",
        chosen.text
    );
    assert!(chosen.used_context, "la elección continúa el alcance anterior");
    let scope = chosen.scope.clone().expect("alcance declarado");
    assert!(scope.inherited);
    assert_eq!(scope.document_count, Some(11));
    assert_eq!(scope.value_count, Some(11));
    assert_eq!(scope.filters.len(), 1);
    assert_eq!(scope.filters[0].concept, "Estado de pago");
    assert_eq!(scope.filters[0].equals, "Pendiente");

    // Defecto 12: el alcance publicado coincide con los operandos usados.
    assert!(chosen.text.contains("11 valores en 11 documentos"), "{}", chosen.text);

    // La evidencia del total son esos mismos documentos y ninguno más.
    let support = ask(engine, "d2", "¿Qué documentos respaldan ese total?");
    assert!(support.used_context);
    let documents = support
        .citations
        .iter()
        .map(|citation| citation.document_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(documents.len(), 11, "sólo los once documentos del conjunto");
}

/// Defecto 3: tras calcular «Anticipo recibido», pedir un campo inexistente
/// devolvía el cálculo anterior como si fuera el pedido.
fn an_explicit_field_beats_the_context(engine: &OmegaEngine) {
    ask(engine, "d3", "¿Cuántos documentos tienen Estado de pago: Pendiente?");
    ask(engine, "d3", "¿Cuánto suman?");
    let previous = ask(engine, "d3", "Anticipo recibido");
    assert!(previous.text.contains("$638,925.00 MXN"));

    for question in [
        "Suma el campo Valor declarado.",
        "¿Cuál es el promedio del campo Valor declarado?",
        "¿Cuál es el máximo del campo Valor declarado?",
    ] {
        let answer = ask(engine, "d3", question);
        assert!(
            !answer.verified,
            "«{question}» no puede darse por verificada: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("Anticipo recibido"),
            "«{question}» no puede responderse con el campo anterior: {}",
            answer.text
        );
        assert!(
            answer.text.contains("No encontré") || answer.clarification.is_some(),
            "«{question}» debe decir que no lo encontró o preguntar: {}",
            answer.text
        );
    }

    // Un campo explícito que sí existe reemplaza al del contexto.
    let replaced = ask(engine, "d3", "Suma el campo Tarifa contratada.");
    assert!(
        replaced.text.contains("Tarifa contratada"),
        "el campo nuevo manda: {}",
        replaced.text
    );
    assert!(!replaced.text.contains("Anticipo recibido"), "{}", replaced.text);
}

/// Defecto 4: la comparación entre dos grupos devolvía un resumen global.
fn two_groups_are_really_compared(engine: &OmegaEngine) {
    let answer = ask(engine, "d4", "Compara el Importe total de Veracruz contra Cozumel.");
    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.contains("Ciudad base"), "agrupador: {}", answer.text);
    assert!(answer.text.contains("$745,200.00 MXN"), "Veracruz: {}", answer.text);
    assert!(answer.text.contains("$915,950.00 MXN"), "Cozumel: {}", answer.text);
    assert!(
        answer.text.contains("«Cozumel» es el mayor"),
        "debe decir cuál es mayor: {}",
        answer.text
    );
    assert!(
        answer.text.contains("$170,750.00 MXN"),
        "diferencia absoluta: {}",
        answer.text
    );
    assert!(answer.text.contains("22.9133 %"), "variación: {}", answer.text);
    // Evidencia de los dos grupos, no sólo del primero.
    let cited = answer
        .citations
        .iter()
        .filter(|citation| citation.match_kind != "cálculo")
        .map(|citation| citation.document_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(cited.len(), 13, "los seis y los siete documentos comparados");

    // Defecto 4, continuaciones: usan la comparación anterior sin recalcular.
    let difference = ask(engine, "d4", "¿Cuál es la diferencia?");
    assert!(difference.used_context);
    assert!(difference.text.contains("$170,750.00 MXN"), "{}", difference.text);
    let percentage = ask(engine, "d4", "¿Qué porcentaje representa la diferencia?");
    assert!(percentage.used_context);
    assert!(percentage.text.contains("22.9133 %"), "{}", percentage.text);
}

/// Defecto 5: «qué ciudad tiene el mayor anticipo» respondía con el anticipo
/// individual más alto en vez de con la ciudad.
fn a_ranking_answers_with_the_group(engine: &OmegaEngine) {
    let answer = ask(engine, "d5", "¿Qué ciudad tiene el mayor anticipo recibido?");
    assert!(answer.verified, "{}", answer.text);
    assert!(
        answer.text.contains("Ciudad base") && answer.text.contains("Veracruz"),
        "debe nombrar la ciudad ganadora: {}",
        answer.text
    );
    assert!(
        answer.text.contains("$689,325.00 MXN"),
        "el total del grupo, no el máximo individual: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("$87,150.00"),
        "$87,150.00 es el anticipo individual más alto, no la respuesta: {}",
        answer.text
    );
    // Defecto 12: el alcance son los 140 documentos con ese campo, no los 600.
    let scope = answer.scope.clone().expect("alcance");
    assert_eq!(scope.document_count, Some(140));
    assert_eq!(scope.group_by.as_deref(), Some("Ciudad base"));
    assert!(answer.text.contains("140 documentos"), "{}", answer.text);
    assert!(!answer.text.contains("600 documentos"), "{}", answer.text);
}

/// Defecto 6: «10 pasajeros» vinculaba inventario, contratos, personal y
/// facturas, y producía contradicciones inventadas.
fn a_capacity_never_links_documents(engine: &OmegaEngine) {
    let dossier = ask(engine, "d6", "Resume el expediente 10 pasajeros");
    assert!(!dossier.verified, "{}", dossier.text);
    assert!(
        dossier.text.contains("no produce una clave estable") || dossier.text.contains("No encontré"),
        "debe explicar que no hay vínculo, no fabricar un expediente: {}",
        dossier.text
    );
    assert!(
        !dossier.text.contains("Campos con valores en conflicto"),
        "no puede declarar conflictos de un expediente que no existe: {}",
        dossier.text
    );

    let related = ask(engine, "d6", "¿Qué documentos están relacionados con 10 pasajeros?");
    assert!(
        !related.verified,
        "una capacidad no es una clave estable: {}",
        related.text
    );

    let contradictions = ask(engine, "d6", "¿Hay documentos contradictorios?");
    assert!(
        !contradictions.verified && contradictions.citations.is_empty(),
        "el corpus no tiene claves repetidas: {}",
        contradictions.text
    );
    assert!(
        !contradictions.text.contains("Capacidad autorizada"),
        "una capacidad no puede aparecer como vínculo: {}",
        contradictions.text
    );
}

/// Defecto 7: «¿hay folios con estados diferentes?» listaba los 600 folios.
fn repeated_folios_with_different_states_are_not_a_list_of_folios(engine: &OmegaEngine) {
    let answer = ask(engine, "d7", "¿Hay folios con estados diferentes?");
    assert!(
        answer.text.contains("No encontré evidencia de contradicción"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("Folio") && answer.text.contains("Estado"), "{}", answer.text);
    assert!(
        !answer.text.contains("MAY-26-0001-AZM"),
        "no puede listar folios como sustituto: {}",
        answer.text
    );
    assert!(answer.citations.is_empty());
    assert!(!answer.verified);
}

/// Defecto 9: la pregunta de aclaración aparecía dos veces.
fn a_clarification_is_written_once(engine: &OmegaEngine) {
    ask(engine, "d9", "¿Cuántos documentos tienen Estado de pago: Pendiente?");
    let answer = ask(engine, "d9", "¿Cuánto suman?");
    let clarification = answer.clarification.clone().expect("aclaración");
    assert_eq!(
        answer.text.matches(&clarification.question).count(),
        1,
        "la pregunta de aclaración se escribe una sola vez: {}",
        answer.text
    );
    for option in &clarification.options {
        assert_eq!(
            answer.text.matches(option.as_str()).count(),
            1,
            "cada opción aparece una sola vez: {}",
            answer.text
        );
    }
}

fn ask(engine: &OmegaEngine, conversation: &str, question: &str) -> Answer {
    engine
        .ask_in_conversation(conversation, question)
        .unwrap_or_else(|error| panic!("«{question}» falló: {error}"))
}

fn index_yacht_corpus() -> OmegaEngine {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus-prueba-agencia-yates");
    assert!(corpus.is_dir(), "falta el corpus de prueba versionado");
    let temporary = tempfile::tempdir().expect("directorio temporal");
    let engine = OmegaEngine::open_with_clock(
        temporary.path().join("omega-regresiones.db"),
        Clock::fixed(TODAY).expect("fecha fija válida"),
    )
    .expect("motor");
    let source = engine.authorize_source(&corpus).expect("fuente autorizada");
    let report = engine.index_source(source).expect("indexación");
    assert_eq!(report.indexed, 600);
    // El directorio temporal debe sobrevivir a la prueba entera.
    std::mem::forget(temporary);
    engine
}
