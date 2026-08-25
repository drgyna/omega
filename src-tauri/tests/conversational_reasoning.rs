//! Conversación estructurada, cálculos verificables, relaciones y
//! contradicciones.
//!
//! Las fixtures se escriben aquí mismo con campos genéricos de oficina —folio,
//! estado, importe, ciudad, fecha—; no proceden de ningún corpus del
//! repositorio ni describen un giro de negocio concreto. El reloj se fija en
//! todas las pruebas: ninguna depende del día en que se ejecuta.

use std::{collections::BTreeSet, fs, path::Path};

use omega_core::{Answer, Clock, OmegaEngine};

const TODAY: &str = "2026-08-24";

// --- 1. Continuación contextual -------------------------------------------

/// El caso central: una pregunta establece un conjunto y la siguiente calcula
/// **sólo sobre ese conjunto**, no sobre todo el acervo.
#[test]
fn a_continuation_computes_only_over_the_previous_result() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let first = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    assert!(first.text.contains('3'), "conteo inicial: {}", first.text);
    assert!(!first.used_context);

    let second = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(
        second.text.contains("$6,000.00 MXN"),
        "suma del conjunto anterior: {}",
        second.text
    );
    // El total de TODOS los importes es $7,200.00: heredar mal el conjunto se
    // vería exactamente aquí.
    assert!(!second.text.contains("$7,200.00"));
    assert!(second.used_context, "la respuesta debe declarar el contexto");
    assert!(second.verified);
    let scope = second.scope.clone().expect("alcance declarado");
    assert!(scope.inherited);
    assert_eq!(scope.document_count, Some(3));
    assert_eq!(scope.value_count, Some(3));
    assert_eq!(scope.concept.as_deref(), Some("Importe"));
    assert_eq!(scope.filters.len(), 1);
    assert_eq!(scope.filters[0].equals, "Vencida");
    assert!(
        second
            .citations
            .iter()
            .any(|citation| citation.match_kind == "cálculo"),
        "la suma es un cálculo local y debe citarse como tal"
    );
}

/// La tercera pregunta de la secuencia: agrupar el mismo conjunto y ordenar.
#[test]
fn a_ranking_continues_over_the_same_set() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    let answer = engine
        .ask_in_conversation("c1", "¿Cuál Cliente debe más?")
        .unwrap();

    assert!(
        answer.text.contains("Industrias del Norte") && answer.text.contains("$4,000.00 MXN"),
        "ranking: {}",
        answer.text
    );
    assert!(answer.used_context);
    // Comercial Sur suma $2,000.00 dentro del conjunto vencido; si el conjunto
    // se perdiera, sumaría $2,700.00 con la factura pagada.
    assert!(answer.text.contains("$2,000.00 MXN"));
    assert!(!answer.text.contains("$2,700.00"));
}

// --- 2. Conversación nueva -------------------------------------------------

#[test]
fn a_new_conversation_starts_without_any_context() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    engine.reset_conversation("c1");

    let after_reset = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(!after_reset.used_context);
    assert!(!after_reset.text.contains("$6,000.00"));
    assert!(
        after_reset.clarification.is_some(),
        "sin contexto la continuación debe pedir aclaración: {}",
        after_reset.text
    );

    // Otra conversación jamás ve el contexto de la primera.
    engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    let other = engine.ask_in_conversation("c3", "¿Cuánto suman?").unwrap();
    assert!(!other.text.contains("$6,000.00"));
}

// --- 3. Referencia ambigua -------------------------------------------------

#[test]
fn an_ambiguous_reference_asks_instead_of_guessing() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let answer = engine
        .ask_in_conversation("nueva", "¿Cuánto suman esos?")
        .unwrap();
    let clarification = answer.clarification.expect("debe pedir aclaración");
    assert_eq!(clarification.reason, "referencia_sin_contexto");
    assert!(!answer.verified);
    assert!(answer.citations.is_empty());
}

// --- 4. Suma, promedio, máximo y mínimo con citas --------------------------

#[test]
fn arithmetic_operations_report_values_used_and_cite_the_calculation() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    for (question, expected) in [
        ("¿Cuál es el promedio del Importe?", "$1,440.00 MXN"),
        ("¿Cuál es el Importe máximo?", "$3,000.00 MXN"),
        ("¿Cuál es el Importe mínimo?", "$500.00 MXN"),
    ] {
        let answer = engine.ask_in_conversation(question, question).unwrap();
        assert!(
            answer.text.contains(expected),
            "«{question}» debía dar {expected}: {}",
            answer.text
        );
        assert!(answer.text.contains("5 valores"), "{}", answer.text);
        assert!(answer.verified);
        assert!(
            answer
                .citations
                .iter()
                .any(|citation| citation.match_kind == "cálculo")
        );
        assert!(
            answer.citations.len() > 1,
            "el cálculo debe citar también sus operandos"
        );
    }
}

// --- 5. Comparación entre dos grupos ---------------------------------------

#[test]
fn two_groups_are_compared_with_difference_and_variation() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let answer = engine
        .ask_in_conversation(
            "c1",
            "Compara el Importe por Ciudad entre Monterrey y Guadalajara.",
        )
        .unwrap();
    assert!(answer.text.contains("$4,500.00 MXN"), "{}", answer.text);
    assert!(answer.text.contains("$2,700.00 MXN"), "{}", answer.text);
    assert!(answer.text.contains("Diferencia"), "{}", answer.text);
    assert!(answer.text.contains("-40 %"), "variación: {}", answer.text);
    assert!(answer.verified);
}

// --- 6. Comparación entre periodos -----------------------------------------

#[test]
fn periods_are_compared_against_an_injected_clock() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let first = engine
        .ask_in_conversation("c1", "¿Cuánto suma el Importe en marzo de 2026?")
        .unwrap();
    assert!(first.text.contains("$4,500.00 MXN"), "{}", first.text);
    let scope = first.scope.clone().expect("alcance");
    let date = scope.date.expect("rango de fechas anclado a un campo");
    assert_eq!(date.concept, "Fecha de registro");
    assert_eq!(date.from, "2026-03-01");
    assert_eq!(date.to, "2026-03-31");

    let compared = engine
        .ask_in_conversation("c1", "Compáralo con el mes anterior.")
        .unwrap();
    assert!(
        compared.text.contains("2026-02-01 a 2026-02-28"),
        "{}",
        compared.text
    );
    assert!(compared.text.contains("$2,700.00 MXN"), "{}", compared.text);
    assert!(compared.text.contains("$4,500.00 MXN"), "{}", compared.text);
    assert!(
        compared.text.contains("+66.6667 %") || compared.text.contains("+66.67 %"),
        "variación entre periodos: {}",
        compared.text
    );
    assert!(compared.used_context);
}

// --- 7. Porcentajes con cero, ausencia y monedas distintas -----------------

#[test]
fn a_zero_base_makes_the_variation_undefined_instead_of_infinite() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Folio", "RG-2026-001"),
        ("Ciudad", "Norte"),
        ("Importe", "$0.00 MXN"),
    ]);
    write_record(root, "b.md", &[
        ("Folio", "RG-2026-002"),
        ("Ciudad", "Sur"),
        ("Importe", "$500.00 MXN"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Compara el Importe por Ciudad entre Norte y Sur.")
        .unwrap();
    assert!(
        answer.text.contains("no está definida"),
        "una base cero no puede producir un porcentaje: {}",
        answer.text
    );
}

#[test]
fn a_missing_side_is_explained_instead_of_treated_as_zero() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Folio", "RG-2026-001"),
        ("Ciudad", "Norte"),
        ("Importe", "$500.00 MXN"),
    ]);
    write_record(root, "b.md", &[("Folio", "RG-2026-002"), ("Ciudad", "Sur")]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Compara el Importe por Ciudad entre Norte y Sur.")
        .unwrap();
    assert!(
        answer.text.contains("No puedo calcular la diferencia"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("Sin datos"), "{}", answer.text);
}

#[test]
fn two_currencies_are_never_subtracted_from_each_other() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Folio", "RG-2026-001"),
        ("Ciudad", "Norte"),
        ("Importe", "$500.00 MXN"),
    ]);
    write_record(root, "b.md", &[
        ("Folio", "RG-2026-002"),
        ("Ciudad", "Sur"),
        ("Importe", "$500.00 USD"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Compara el Importe por Ciudad entre Norte y Sur.")
        .unwrap();
    assert!(
        answer.text.contains("monedas distintas") || answer.text.contains("No puedo restar"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("MXN") && answer.text.contains("USD"));
}

/// Una suma sobre un conjunto con dos monedas devuelve dos cifras separadas.
#[test]
fn a_mixed_currency_sum_reports_one_total_per_currency() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[("Folio", "RG-1"), ("Importe", "$100.00 MXN")]);
    write_record(root, "b.md", &[("Folio", "RG-2"), ("Importe", "$200.00 MXN")]);
    write_record(root, "c.md", &[("Folio", "RG-3"), ("Importe", "$40.00 USD")]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es el promedio del Importe?")
        .unwrap();
    assert!(answer.text.contains("$150.00 MXN"), "{}", answer.text);
    assert!(answer.text.contains("$40.00 USD"), "{}", answer.text);
    assert!(answer.text.contains("no pueden combinarse"), "{}", answer.text);
}

// --- 8 y 9. Relaciones ------------------------------------------------------

#[test]
fn documents_are_related_by_an_exact_identifier() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "factura.md", &[
        ("Folio", "FA-2026-001"),
        ("Cliente", "Industrias del Norte"),
        ("Importe", "$1,000.00 MXN"),
    ]);
    write_record(root, "pago.md", &[
        ("Folio referido", "FA-2026-001"),
        ("Medio", "Transferencia"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Cuáles son todos los documentos relacionados con FA-2026-001?",
        )
        .unwrap();
    assert!(answer.text.contains("factura.md"), "{}", answer.text);
    assert!(answer.text.contains("pago.md"), "{}", answer.text);
    assert!(
        answer.text.contains("Folio referido"),
        "debe decir qué campo creó el vínculo: {}",
        answer.text
    );
    assert!(answer.verified);
    assert_eq!(answer.citations.len(), 2);
}

#[test]
fn similar_names_never_create_a_relation() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "uno.md", &[
        ("Folio", "FA-2026-001"),
        ("Cliente", "Comercial Álamo S.A."),
    ]);
    write_record(root, "dos.md", &[
        ("Folio", "FA-2026-002"),
        ("Cliente", "Comercial Álamos S.A."),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Qué documentos están relacionados con Comercial Álamo S.A.?",
        )
        .unwrap();
    assert!(
        answer.text.contains("no produce una clave estable"),
        "{}",
        answer.text
    );
    assert!(
        !answer.verified,
        "una relación sin clave no puede darse por verificada"
    );
    // La cita, si la hay, es una mención literal: nunca los dos documentos
    // como si fueran el mismo cliente.
    assert!(
        answer.citations.len() <= 1,
        "no puede unir «Álamo» con «Álamos»: {:?}",
        answer
            .citations
            .iter()
            .map(|citation| citation.value.clone())
            .collect::<Vec<_>>()
    );
}

// --- 10. Contradicciones ----------------------------------------------------

#[test]
fn contradictions_show_both_values_without_choosing_one() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "acta-a.md", &[
        ("Expediente", "EXP-2026-041"),
        ("Estado", "Abierto"),
        ("Importe", "$1,000.00 MXN"),
    ]);
    write_record(root, "acta-b.md", &[
        ("Expediente", "EXP-2026-041"),
        ("Estado", "Cerrado"),
        ("Importe", "$1,200.00 MXN"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Hay documentos contradictorios?")
        .unwrap();
    assert!(answer.text.contains("EXP-2026-041"), "{}", answer.text);
    assert!(answer.text.contains("Expediente"), "{}", answer.text);
    assert!(answer.text.contains("$1,000.00 MXN"), "{}", answer.text);
    assert!(answer.text.contains("$1,200.00 MXN"), "{}", answer.text);
    assert!(answer.text.contains("Abierto") && answer.text.contains("Cerrado"));
    assert!(
        !answer.text.contains("correcto es"),
        "Omega no decide cuál documento tiene razón"
    );
    assert!(answer.citations.len() >= 4);
}

#[test]
fn a_consistent_acervo_reports_no_contradiction() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let answer = engine
        .ask_in_conversation("c1", "¿Hay documentos contradictorios?")
        .unwrap();
    assert!(answer.text.contains("No encontré"), "{}", answer.text);
    assert!(answer.citations.is_empty());
}

// --- 11. Resumen de expediente ---------------------------------------------

#[test]
fn a_dossier_gathers_linked_documents_conflicts_and_gaps() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "acta-a.md", &[
        ("Expediente", "EXP-2026-041"),
        ("Cliente", "Industrias del Norte"),
        ("Importe", "$1,000.00 MXN"),
    ]);
    write_record(root, "acta-b.md", &[
        ("Expediente", "EXP-2026-041"),
        ("Importe", "$1,200.00 MXN"),
        ("Estado", "Cerrado"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Dame un resumen del expediente EXP-2026-041.")
        .unwrap();
    assert!(answer.text.contains("EXP-2026-041"), "{}", answer.text);
    assert!(answer.text.contains("2 documentos"), "{}", answer.text);
    assert!(answer.text.contains("Industrias del Norte"), "{}", answer.text);
    assert!(
        answer.text.contains("conflicto"),
        "el importe difiere y debe marcarse: {}",
        answer.text
    );
    assert!(
        answer.text.contains("ausentes"),
        "cliente y estado faltan en un documento: {}",
        answer.text
    );
    assert!(answer.verified);
    assert!(answer.citations.len() >= 4);
}

// --- 12. Documentos que respaldan un total ---------------------------------

#[test]
fn the_documents_behind_a_total_can_be_requested() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    let total = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(total.text.contains("$6,000.00 MXN"));

    let support = engine
        .ask_in_conversation("c1", "¿Qué documentos respaldan ese total?")
        .unwrap();
    assert!(support.used_context);
    assert!(support.text.contains("$6,000.00 MXN"), "{}", support.text);
    let cited = support
        .citations
        .iter()
        .map(|citation| file_name(&citation.path))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        cited,
        ["doc-01.md", "doc-02.md", "doc-03.md"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>(),
        "sólo los tres documentos vencidos respaldan el total"
    );
}

// --- 13. Reindexación -------------------------------------------------------

#[test]
fn reindexing_updates_the_context_without_contaminating_it() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_ledger(root);
    let engine = OmegaEngine::open_with_clock(
        root.join("omega-test.db"),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    let before = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(before.text.contains("$6,000.00 MXN"));

    // Un documento vencido más y una reindexación completa: los identificadores
    // internos de fila se reasignan por completo.
    write_record(root, "doc-06.md", &[
        ("Folio", "FA-2026-006"),
        ("Estado", "Vencida"),
        ("Cliente", "Comercial Sur"),
        ("Ciudad", "Guadalajara"),
        ("Fecha de registro", "2026-03-28"),
        ("Importe", "$1,500.00 MXN"),
    ]);
    engine.index_source(source).unwrap();

    let after = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(
        after.text.contains("$7,500.00 MXN"),
        "el conjunto se reevalúa contra el índice nuevo: {}",
        after.text
    );
    assert!(after.used_context);
    assert_eq!(after.scope.clone().unwrap().document_count, Some(4));
    for citation in &after.citations {
        if citation.match_kind == "cálculo" {
            continue;
        }
        assert!(
            Path::new(&citation.path).exists(),
            "ninguna cita puede apuntar a evidencia fantasma: {}",
            citation.path
        );
    }
}

// --- 14. Ausencia de evidencia ---------------------------------------------

#[test]
fn an_empty_scope_never_produces_a_number() {
    let fixture = tempfile::tempdir().unwrap();
    write_ledger(fixture.path());
    let engine = index(fixture.path());

    let first = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Cancelada?")
        .unwrap();
    assert!(!first.verified, "{}", first.text);

    let second = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert!(!second.verified, "{}", second.text);
    assert!(
        second
            .citations
            .iter()
            .all(|citation| citation.match_kind != "cálculo"),
        "sin operandos no puede haber cálculo"
    );
    assert!(!second.text.contains('$'));
}

// --- 15. El motor no conoce el vocabulario de los corpus -------------------

/// El motor de producción no puede contener el vocabulario de ningún corpus de
/// control de calidad. La prueba lee el propio código fuente: si alguien añade
/// una regla dependiente de un giro concreto, falla aquí.
#[test]
fn the_production_engine_contains_no_corpus_vocabulary() {
    let engine_sources = [
        "src/planner.rs",
        "src/tools.rs",
        "src/agent.rs",
        "src/answer.rs",
        "src/calc.rs",
        "src/dates.rs",
        "src/conversation.rs",
        "src/relations.rs",
        "src/report.rs",
        "src/extract.rs",
        "src/normalize.rs",
        "src/indexer.rs",
    ];
    // Palabras propias de los corpus sintéticos del repositorio. No incluye
    // términos genéricos de oficina (documento, expediente, folio) que
    // pertenecen a cualquier acervo.
    let forbidden = [
        "yate", "yacht", "charter", "notaria", "notarial", "ferreteria", "restaurante",
        "poliza", "asegurado", "menu", "platillo", "embarcacion", "eslora",
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for source in engine_sources {
        let path = root.join(source);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("no se pudo leer {source}: {error}"));
        let normalized = text
            .to_lowercase()
            .replace(['á', 'é', 'í', 'ó', 'ú'], "?");
        for word in forbidden {
            assert!(
                !normalized.contains(word),
                "{source} contiene vocabulario de corpus: «{word}»"
            );
        }
    }
}

// --- 16. El valor escrito no se recorta -------------------------------------

/// Un par «Campo: valor» exige el valor completo. Recortarlo para encontrar
/// algo parecido cambia la pregunta sin avisar.
#[test]
fn an_explicit_value_is_never_shortened_to_a_partial_match() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[("Folio", "RG-1"), ("Estado", "Pendiente")]);
    write_record(root, "b.md", &[("Folio", "RG-2"), ("Estado", "Pendiente")]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Pendiente de revisión?")
        .unwrap();
    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.citations.is_empty());
    assert!(
        answer.text.contains("Pendiente de revisión"),
        "debe nombrar el valor pedido: {}",
        answer.text
    );
    assert!(!answer.text.contains("2 documentos"), "{}", answer.text);

    // El valor exacto sí responde.
    let exact = engine
        .ask_in_conversation("c2", "¿Cuántos documentos tienen Estado: Pendiente?")
        .unwrap();
    assert!(exact.verified && exact.text.contains('2'), "{}", exact.text);
}

// --- 17. La aclaración conserva el filtro ----------------------------------

#[test]
fn choosing_a_clarified_field_keeps_the_previous_filter() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, state, amount, discount) in [
        ("a.md", "Vencida", "$1,000.00 MXN", "$100.00 MXN"),
        ("b.md", "Vencida", "$2,000.00 MXN", "$200.00 MXN"),
        ("c.md", "Pagada", "$4,000.00 MXN", "$400.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Estado", state),
                ("Importe", amount),
                ("Descuento", discount),
            ],
        );
    }
    let engine = index(root);

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    let ambiguous = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    let clarification = ambiguous.clarification.clone().expect("dos campos numéricos");
    assert_eq!(clarification.reason, "campo_ambiguo");
    assert_eq!(clarification.options.len(), 2);

    let chosen = engine.ask_in_conversation("c1", "Importe").unwrap();
    assert!(
        chosen.text.contains("$3,000.00 MXN"),
        "sólo los dos vencidos: {}",
        chosen.text
    );
    assert!(!chosen.text.contains("$7,000.00"), "{}", chosen.text);
    assert!(chosen.used_context);
    let scope = chosen.scope.clone().expect("alcance");
    assert!(scope.inherited);
    assert_eq!(scope.document_count, Some(2));
    assert_eq!(scope.filters[0].equals, "Vencida");

    // Y la elección no deja la aclaración enganchada para el turno siguiente.
    let after = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Pagada?")
        .unwrap();
    assert!(after.clarification.is_none(), "{}", after.text);
}

// --- 18. El campo explícito manda sobre el contexto ------------------------

#[test]
fn a_named_field_replaces_the_one_the_context_was_using() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Folio", "RG-1"),
        ("Estado", "Vencida"),
        ("Importe", "$1,000.00 MXN"),
        ("Descuento", "$100.00 MXN"),
    ]);
    write_record(root, "b.md", &[
        ("Folio", "RG-2"),
        ("Estado", "Vencida"),
        ("Importe", "$2,000.00 MXN"),
        ("Descuento", "$200.00 MXN"),
    ]);
    let engine = index(root);

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Vencida?")
        .unwrap();
    let first = engine
        .ask_in_conversation("c1", "¿Cuánto suman en Importe?")
        .unwrap();
    assert!(first.text.contains("$3,000.00 MXN"), "{}", first.text);

    // Un campo distinto y existente reemplaza al anterior.
    let replaced = engine
        .ask_in_conversation("c1", "¿Cuánto suman en Descuento?")
        .unwrap();
    assert!(replaced.text.contains("$300.00 MXN"), "{}", replaced.text);
    assert!(!replaced.text.contains("$3,000.00"), "{}", replaced.text);
    assert_eq!(
        replaced.scope.clone().unwrap().concept.as_deref(),
        Some("Descuento")
    );

    // Un campo inexistente nunca se sustituye por el del contexto.
    for question in [
        "Suma el campo Valor declarado.",
        "¿Cuál es el promedio del campo Valor declarado?",
        "¿Cuál es el máximo del campo Valor declarado?",
    ] {
        let answer = engine.ask_in_conversation("c1", question).unwrap();
        assert!(!answer.verified, "«{question}»: {}", answer.text);
        assert!(
            !answer.text.contains("Descuento") && !answer.text.contains("Importe"),
            "«{question}» no puede responderse con otro campo: {}",
            answer.text
        );
        assert!(answer.text.contains("No encontré"), "«{question}»: {}", answer.text);
    }
}

// --- 19. Claves de relación: negativos explícitos --------------------------

/// Ni una capacidad, ni una ciudad, ni un importe pueden vincular documentos:
/// no son identificadores, por mucho que dos documentos repitan la cifra.
#[test]
fn common_values_never_become_relation_keys() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "inventario.md", &[
        ("Clave de inventario", "INV-1"),
        ("Capacidad autorizada", "10 pasajeros"),
        ("Ciudad", "Zona 7"),
        ("Importe", "$1,000.00 MXN"),
        ("Estado", "Abierto"),
    ]);
    write_record(root, "contrato.md", &[
        ("Clave de contrato", "CTR-9"),
        ("Capacidad autorizada", "10 pasajeros"),
        ("Ciudad", "Zona 7"),
        ("Importe", "$1,000.00 MXN"),
        ("Estado", "Cerrado"),
    ]);
    let engine = index(root);

    let contradictions = engine
        .ask_in_conversation("c1", "¿Hay documentos contradictorios?")
        .unwrap();
    assert!(
        !contradictions.verified && contradictions.citations.is_empty(),
        "una capacidad, una ciudad o un importe compartidos no vinculan nada: {}",
        contradictions.text
    );

    for subject in ["10 pasajeros", "Zona 7", "$1,000.00 MXN"] {
        let answer = engine
            .ask_in_conversation("c2", &format!("Resume el expediente {subject}"))
            .unwrap();
        assert!(
            !answer.verified,
            "«{subject}» no puede abrir un expediente: {}",
            answer.text
        );
    }
}

/// Dos documentos con la misma clave pero campos distintos no se contradicen:
/// no hay nada que comparar.
#[test]
fn documents_with_different_fields_are_not_contradictory() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Expediente", "EXP-2026-090"),
        ("Cliente", "Industrias del Norte"),
    ]);
    write_record(root, "b.md", &[
        ("Expediente", "EXP-2026-090"),
        ("Estado", "Cerrado"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Hay documentos contradictorios?")
        .unwrap();
    assert!(
        !answer.verified,
        "campos distintos no son valores incompatibles: {}",
        answer.text
    );
    assert!(answer.citations.is_empty());
}

// --- 20. «Este mes» sale del reloj -----------------------------------------

#[test]
fn this_month_and_the_previous_one_come_from_the_injected_clock() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, date, amount) in [
        ("a.md", "2026-08-03", "$1,000.00 MXN"),
        ("b.md", "2026-08-19", "$2,000.00 MXN"),
        ("c.md", "2026-07-14", "$500.00 MXN"),
        ("d.md", "2026-06-02", "$9,000.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Fecha de registro", date),
                ("Importe", amount),
            ],
        );
    }
    // El reloj fijo de las pruebas es 2026-08-24.
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Compara los importes de este mes contra el mes anterior.")
        .unwrap();
    assert!(
        answer.text.contains("2026-08-01 a 2026-08-31"),
        "«este mes» sale del reloj: {}",
        answer.text
    );
    assert!(
        answer.text.contains("2026-07-01 a 2026-07-31"),
        "«el mes anterior»: {}",
        answer.text
    );
    assert!(answer.text.contains("$3,000.00 MXN"), "agosto: {}", answer.text);
    assert!(answer.text.contains("$500.00 MXN"), "julio: {}", answer.text);
    assert!(!answer.text.contains("$9,000.00"), "junio queda fuera: {}", answer.text);
    assert!(
        answer.text.contains("Fecha de registro"),
        "debe decir con qué campo de fecha: {}",
        answer.text
    );
    assert!(answer.verified);
}

/// Con más de un campo de fecha, el motor pregunta cuál usar en vez de elegir.
#[test]
fn several_date_fields_produce_a_question_not_a_guess() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "a.md", &[
        ("Folio", "RG-1"),
        ("Fecha de registro", "2026-08-03"),
        ("Fecha de vencimiento", "2026-09-03"),
        ("Importe", "$1,000.00 MXN"),
    ]);
    write_record(root, "b.md", &[
        ("Folio", "RG-2"),
        ("Fecha de registro", "2026-07-03"),
        ("Fecha de vencimiento", "2026-08-03"),
        ("Importe", "$2,000.00 MXN"),
    ]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "Compara los importes de este mes contra el mes anterior.")
        .unwrap();
    let clarification = answer.clarification.clone().expect("debe preguntar");
    assert_eq!(clarification.reason, "campo_fecha_ambiguo");
    assert_eq!(clarification.options.len(), 2);
    // La aclaración se escribe una sola vez.
    assert_eq!(answer.text.matches(&clarification.question).count(), 1);

    // Y al elegir, resuelve la pregunta original con ese campo.
    let chosen = engine
        .ask_in_conversation("c1", "Fecha de registro")
        .unwrap();
    assert!(
        chosen.text.contains("2026-08-01 a 2026-08-31"),
        "{}",
        chosen.text
    );
    assert!(chosen.text.contains("Fecha de registro"), "{}", chosen.text);
}

// --- fixtures ---------------------------------------------------------------

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

/// Cinco registros con estado, cliente, ciudad, fecha e importe.
///
/// Vencidos: $1,000 + $2,000 + $3,000 = $6,000. Todos: $7,200.
/// Marzo: $1,000 + $3,000 + $500 = $4,500. Febrero: $2,000 + $700 = $2,700.
/// Monterrey: $4,500. Guadalajara: $2,700.
fn write_ledger(root: &Path) {
    let rows = [
        (
            "doc-01.md",
            "FA-2026-001",
            "Vencida",
            "Industrias del Norte",
            "Monterrey",
            "2026-03-05",
            "$1,000.00 MXN",
        ),
        (
            "doc-02.md",
            "FA-2026-002",
            "Vencida",
            "Comercial Sur",
            "Guadalajara",
            "2026-02-11",
            "$2,000.00 MXN",
        ),
        (
            "doc-03.md",
            "FA-2026-003",
            "Vencida",
            "Industrias del Norte",
            "Monterrey",
            "2026-03-19",
            "$3,000.00 MXN",
        ),
        (
            "doc-04.md",
            "FA-2026-004",
            "Pagada",
            "Industrias del Norte",
            "Monterrey",
            "2026-03-02",
            "$500.00 MXN",
        ),
        (
            "doc-05.md",
            "FA-2026-005",
            "Pagada",
            "Comercial Sur",
            "Guadalajara",
            "2026-02-22",
            "$700.00 MXN",
        ),
    ];
    for (name, folio, state, client, city, date, amount) in rows {
        write_record(
            root,
            name,
            &[
                ("Folio", folio),
                ("Estado", state),
                ("Cliente", client),
                ("Ciudad", city),
                ("Fecha de registro", date),
                ("Importe", amount),
            ],
        );
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

#[allow(dead_code)]
fn debug(answer: &Answer) -> String {
    format!("{}\n{:?}", answer.text, answer.scope)
}
