//! Operaciones aritméticas universales: multiplicación y división entre dos
//! campos del mismo documento, y procedencia de filtros en comparaciones.
//!
//! Las fixtures son genéricas —folio, zona, cantidad, precio, montos— y se
//! escriben en un directorio temporal (`tempfile`). No proceden de ningún
//! corpus del repositorio y no describen ningún giro de negocio concreto. El
//! reloj se fija para que ninguna prueba dependa del día en que se ejecuta.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Los importes negativos son hechos numéricos, no texto decorativo. El
/// signo puede ir antes o después del símbolo monetario y debe conservarse
/// hasta la suma exacta; omitirlo convertía una corrección en evidencia no
/// calculable y podía inflar un total.
#[test]
fn negative_money_is_indexed_and_summed_with_its_real_sign() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "ajuste.md",
        &[("Folio", "NEG-01"), ("Importe", "-$1,200.50 MXN")],
    );
    write_record(
        root,
        "abono.md",
        &[("Folio", "NEG-02"), ("Importe", "$200.25 MXN")],
    );
    let engine = index(root);

    let answer = engine.ask_in_conversation("c1", "Suma el campo Importe.").unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.contains("-$1,000.25 MXN"), "{}", answer.text);
    assert!(!answer.text.contains("$1,400.75 MXN"), "{}", answer.text);
    let scope = answer.scope.expect("alcance declarado");
    assert_eq!(scope.document_count, Some(2));
    assert_eq!(scope.value_count, Some(2));
    assert_eq!(scope.excluded_count, Some(0));
}

/// Un campo numérico que la pregunta ya nombra dice por sí mismo de qué se
/// totaliza, así que pedir su total es pedir una suma. Las palabras genéricas
/// de dinero o cantidad («importe», «monto», «cantidad», «unidades») existen
/// para el caso contrario —una pregunta que no nombra ningún campo— y exigirlas
/// también aquí dejaba sin sumar la forma más directa de pedir una suma: la
/// pregunta caía en la búsqueda del campo y contestaba «— N valores», una frase
/// sin ninguna cifra.
#[test]
fn a_total_of_a_named_numeric_field_is_summed_without_a_generic_money_word() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "uno.md",
        &[("Folio", "TOT-01"), ("Total de piezas", "120")],
    );
    write_record(
        root,
        "dos.md",
        &[("Folio", "TOT-02"), ("Total de piezas", "45")],
    );
    let engine = index(root);

    let summed = engine
        .ask_in_conversation("c1", "¿A cuánto asciende el total de piezas?")
        .unwrap();
    assert!(summed.text.contains("165"), "{}", summed.text);
    let scope = summed.scope.expect("alcance declarado");
    assert_eq!(scope.value_count, Some(2));

    // Y «total» sin ningún campo nombrado se sigue leyendo como cierre de
    // frase, no como suma: eso es un conteo de documentos, no una cifra.
    let counted = engine
        .ask_in_conversation("c2", "¿Cuántos documentos hay en total?")
        .unwrap();
    assert!(!counted.text.contains("165"), "{}", counted.text);
    assert!(counted.text.contains("documento"), "{}", counted.text);
}

#[test]
fn an_explicit_euro_symbol_is_indexed_and_rendered_as_eur() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "euros.md",
        &[("Folio", "EUR-01"), ("Importe", "€500.00 EUR")],
    );
    let engine = index(root);

    let answer = engine.ask_in_conversation("c1", "Suma el campo Importe.").unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.contains("€500.00 EUR"), "{}", answer.text);
    assert!(!answer.text.contains("$500.00"), "{}", answer.text);
}

/// «Cantidad × Precio unitario», documento por documento, sumando después
/// los resultados. Nunca debe multiplicar el total global de Cantidad por el
/// total global de Precio unitario: esa sería una cifra distinta (9 × $285 =
/// $2,565) de la que realmente se pidió ($830, la suma de los tres
/// productos).
#[test]
fn multiplying_two_fields_computes_row_by_row_not_totals_against_totals() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, cantidad, precio) in [
        ("doc-01.md", "4", "$125.00 MXN"),
        ("doc-02.md", "2", "$150.00 MXN"),
        ("doc-03.md", "3", "$10.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Cantidad", cantidad),
                ("Precio unitario", precio),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Cantidad multiplicada por Precio unitario?")
        .unwrap();

    assert!(answer.text.contains('×'), "{}", answer.text);
    assert!(
        answer.text.contains("$830.00 MXN"),
        "500 + 300 + 30 = 830, nunca 9 × 285 = 2,565: {}",
        answer.text
    );
    assert!(!answer.text.contains("2,565"), "{}", answer.text);
    assert!(answer.verified, "{}", answer.text);
    let scope = answer.scope.clone().expect("alcance declarado");
    assert_eq!(scope.document_count, Some(3));
    assert_eq!(scope.value_count, Some(3));
}

/// División entre dos campos: el divisor es dinero y el dividendo es una
/// cantidad simple, así que el resultado conserva la moneda (dinero por
/// unidad). Un documento con divisor cero se excluye explícitamente — nunca
/// produce infinito ni un cero fabricado — y le quita a la respuesta el
/// derecho a declararse totalmente verificada.
#[test]
fn dividing_by_zero_is_excluded_and_reported_never_infinite_or_zero() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, monto, cantidad) in [
        ("doc-01.md", "$400.00 MXN", "4"),
        ("doc-02.md", "$300.00 MXN", "0"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Monto principal", monto),
                ("Cantidad", cantidad),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Monto principal dividido entre Cantidad?")
        .unwrap();

    assert!(
        answer.text.contains("$100.00 MXN"),
        "400 / 4 = 100, el único documento válido: {}",
        answer.text
    );
    assert!(!answer.text.contains("inf"), "{}", answer.text);
    assert!(
        answer.text.contains("1 documento no se calculó porque dividía entre cero"),
        "un solo documento excluido debe concordar en singular: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "un documento excluido por dividir entre cero no puede declararse verificado: {}",
        answer.text
    );
}

/// Resta entre dos campos del mismo documento: exige la misma moneda. Un
/// documento con monedas distintas en los dos campos se excluye —nunca se
/// resta a la fuerza— y también quita el derecho a la verificación total.
#[test]
fn subtracting_two_fields_excludes_incompatible_currencies() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[
            ("Folio", "RG-01"),
            ("Monto A", "$500.00 MXN"),
            ("Monto B", "$100.00 MXN"),
        ],
    );
    write_record(
        root,
        "doc-02.md",
        &[
            ("Folio", "RG-02"),
            ("Monto A", "$700.00 MXN"),
            ("Monto B", "$50.00 USD"),
        ],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es la diferencia entre Monto A y Monto B?")
        .unwrap();

    assert!(answer.text.contains("$400.00 MXN"), "{}", answer.text);
    assert!(!answer.text.contains("$650.00"), "{}", answer.text);
    assert!(
        answer.text.contains("unidades incompatibles"),
        "{}",
        answer.text
    );
    assert!(!answer.verified, "{}", answer.text);
}

/// El alcance que declara una operación entre dos campos («÷», «×», «−») es
/// el filtro original completo, no sólo los documentos que además tenían
/// ambos campos: un documento sin `Cantidad`, o uno que dividía entre cero,
/// seguía estando en el alcance de la pregunta. `document_count` (alcance),
/// `value_count` (usados) y `excluded_count` (excluidos) se leen por
/// separado, sin tener que restar unos de otros.
#[test]
fn a_division_scope_reports_the_full_filter_not_only_the_examined_documents() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, monto, cantidad) in [
        ("doc-01.md", "$400.00 MXN", "4"),
        ("doc-02.md", "$900.00 MXN", "3"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Monto principal", monto),
                ("Cantidad", cantidad),
            ],
        );
    }
    write_record(
        root,
        "doc-missing.md",
        &[("Folio", "RG-MISS"), ("Monto principal", "$500.00 MXN")],
    );
    write_record(
        root,
        "doc-zero.md",
        &[
            ("Folio", "RG-ZERO"),
            ("Monto principal", "$300.00 MXN"),
            ("Cantidad", "0"),
        ],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Monto principal dividido entre Cantidad?")
        .unwrap();

    let scope = answer.scope.clone().expect("alcance declarado");
    assert_eq!(
        scope.document_count,
        Some(4),
        "el alcance es el filtro original completo (4 documentos), no sólo los 3 examinados: {:?}",
        scope
    );
    assert_eq!(
        scope.value_count,
        Some(2),
        "sólo 2 documentos produjeron una cifra: {:?}",
        scope
    );
    assert_eq!(
        scope.excluded_count,
        Some(2),
        "1 documento sin Cantidad + 1 dividiendo entre cero: {:?}",
        scope
    );
}

/// Misma separación de alcance/usados/excluidos, ahora con las tres razones
/// de exclusión que aplican a una resta entre dos campos: campo faltante,
/// valor no numérico y moneda incompatible.
#[test]
fn a_subtraction_scope_separates_missing_invalid_and_currency_exclusions() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-ok-1.md",
        &[("Folio", "SB-OK1"), ("Monto A", "$500.00 MXN"), ("Monto B", "$100.00 MXN")],
    );
    write_record(
        root,
        "doc-ok-2.md",
        &[("Folio", "SB-OK2"), ("Monto A", "$700.00 MXN"), ("Monto B", "$200.00 MXN")],
    );
    write_record(root, "doc-missing.md", &[("Folio", "SB-MISS"), ("Monto A", "$600.00 MXN")]);
    write_record(
        root,
        "doc-invalid.md",
        &[("Folio", "SB-INV"), ("Monto A", "$600.00 MXN"), ("Monto B", "N/D")],
    );
    write_record(
        root,
        "doc-currency.md",
        &[("Folio", "SB-CUR"), ("Monto A", "$600.00 MXN"), ("Monto B", "$50.00 USD")],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es la diferencia entre Monto A y Monto B?")
        .unwrap();

    let scope = answer.scope.clone().expect("alcance declarado");
    assert_eq!(
        scope.document_count,
        Some(5),
        "el alcance es el filtro original completo (5 documentos): {:?}",
        scope
    );
    assert_eq!(
        scope.value_count,
        Some(2),
        "sólo 2 documentos produjeron una cifra: {:?}",
        scope
    );
    assert_eq!(
        scope.excluded_count,
        Some(3),
        "1 sin Monto B + 1 con valor no numérico + 1 en otra moneda: {:?}",
        scope
    );
    assert!(
        answer.text.contains("1 documento no se calculó por unidades incompatibles"),
        "un solo documento excluido por moneda debe concordar en singular: {}",
        answer.text
    );
    assert!(
        answer.text.contains("1 documento tenía los dos campos, pero con un valor que no es un número"),
        "un solo documento excluido por valor inválido debe concordar en singular: {}",
        answer.text
    );
    assert!(
        answer.text.contains("1 documento sólo tenía uno de los dos campos"),
        "un solo documento excluido por campo faltante debe concordar en singular: {}",
        answer.text
    );
}

/// Regresión del defecto real: una operación entre dos campos declaraba un
/// alcance de N documentos, calculaba unos pocos y no explicaba el resto,
/// presentándose además como verificada. Los documentos que no tienen
/// **ninguno** de los dos campos no aparecen en ninguna lista de operandos, y
/// por eso se escapaban de todas las categorías.
///
/// Aquí se reproduce la misma forma que en el corpus real (la mayoría del
/// alcance sin ninguno de los dos campos) y se exige el invariante completo:
/// alcance = calculados + todos los excluidos.
#[test]
fn documents_without_either_field_are_reported_instead_of_silently_dropped() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // 3 documentos con los dos campos: los únicos calculables.
    for (name, cantidad, precio) in [
        ("con-ambos-1.md", "4", "$100.00 MXN"),
        ("con-ambos-2.md", "2", "$200.00 MXN"),
        ("con-ambos-3.md", "3", "$300.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Cantidad", cantidad),
                ("Precio unitario", precio),
            ],
        );
    }
    // 7 documentos que no tienen ninguno de los dos campos: son los que antes
    // desaparecían de la respuesta sin dejar rastro.
    for index in 0..7 {
        write_record(
            root,
            &format!("sin-ninguno-{index}.md"),
            &[
                ("Folio", &format!("RG-OTRO-{index}")),
                ("Estado", "Vigente"),
                ("Ciudad", "Norte"),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Cantidad multiplicada por Precio unitario?")
        .unwrap();

    let scope = answer.scope.clone().expect("alcance declarado");
    assert_eq!(
        scope.document_count,
        Some(10),
        "el alcance son los 10 documentos del acervo: {scope:?}"
    );
    assert_eq!(
        scope.value_count,
        Some(3),
        "sólo 3 documentos tienen los dos campos: {scope:?}"
    );
    assert_eq!(
        scope.excluded_count,
        Some(7),
        "los 7 sin ninguno de los dos campos son exclusiones: {scope:?}"
    );
    assert_eq!(
        scope.value_count.unwrap() + scope.excluded_count.unwrap(),
        scope.document_count.unwrap(),
        "invariante: alcance = calculados + excluidos: {scope:?}"
    );
    assert!(
        !answer.verified,
        "con 7 documentos excluidos la respuesta no puede declararse verificada: {}",
        answer.text
    );
    assert!(
        answer
            .text
            .contains("7 documentos no tenían ninguno de los dos campos"),
        "la respuesta debe informar el conteo de esa exclusión: {}",
        answer.text
    );
}

/// Un documento sin ninguno de los dos campos se cuenta y se explica, pero no
/// se le fabrica evidencia: no existe ningún valor de «Cantidad» ni de
/// «Precio unitario» que citar en él. Las citas sólo pueden venir de
/// documentos que sí tenían los campos.
#[test]
fn documents_without_either_field_are_never_cited_with_invented_evidence() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, cantidad, precio) in [
        ("con-ambos-1.md", "4", "$100.00 MXN"),
        ("con-ambos-2.md", "2", "$200.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Cantidad", cantidad),
                ("Precio unitario", precio),
            ],
        );
    }
    for index in 0..4 {
        write_record(
            root,
            &format!("sin-ninguno-{index}.md"),
            &[("Folio", &format!("RG-OTRO-{index}")), ("Estado", "Vigente")],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Cantidad multiplicada por Precio unitario?")
        .unwrap();

    assert_eq!(answer.scope.as_ref().unwrap().excluded_count, Some(4));
    assert!(!answer.citations.is_empty(), "el cálculo real sí se cita");
    for citation in &answer.citations {
        assert!(
            !citation.path.contains("sin-ninguno"),
            "no puede citarse un documento que no tiene ninguno de los dos campos: {:?}",
            citation
        );
        // Cada cita apunta a un valor realmente indexado, no a un hueco.
        assert!(
            citation.value.is_some(),
            "una cita sin valor sería evidencia inventada: {citation:?}"
        );
        assert!(!citation.location.is_empty(), "{citation:?}");
    }
}

/// Todas las categorías a la vez, cada documento del alcance en exactamente
/// una: calculados, excluidos por dividir entre cero, excluidos por un valor
/// que no es un número, excluidos por tener sólo uno de los dos campos, y
/// excluidos por no tener ninguno.
#[test]
fn every_document_in_scope_falls_into_exactly_one_category() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // 2 calculables.
    write_record(root, "ok-1.md", &[("Folio", "C-1"), ("Monto principal", "$400.00 MXN"), ("Cantidad", "4")]);
    write_record(root, "ok-2.md", &[("Folio", "C-2"), ("Monto principal", "$900.00 MXN"), ("Cantidad", "3")]);
    // 1 dividiendo entre cero y 1 con un valor que no es un número: tienen
    // los dos campos, pero no producen cifra.
    write_record(root, "cero.md", &[("Folio", "C-3"), ("Monto principal", "$300.00 MXN"), ("Cantidad", "0")]);
    write_record(root, "invalido.md", &[("Folio", "C-4"), ("Monto principal", "$600.00 MXN"), ("Cantidad", "N/D")]);
    // 1 con sólo uno de los dos campos.
    write_record(root, "uno-solo.md", &[("Folio", "C-5"), ("Monto principal", "$900.00 MXN")]);
    // 2 sin ninguno de los dos campos.
    write_record(root, "ninguno-1.md", &[("Folio", "C-6"), ("Estado", "Vigente")]);
    write_record(root, "ninguno-2.md", &[("Folio", "C-7"), ("Estado", "Cerrado")]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuánto da Monto principal dividido entre Cantidad?")
        .unwrap();

    let scope = answer.scope.clone().expect("alcance declarado");
    assert_eq!(scope.document_count, Some(7), "{scope:?}");
    assert_eq!(scope.value_count, Some(2), "{scope:?}");
    assert_eq!(
        scope.excluded_count,
        Some(5),
        "1 entre cero + 1 inválido + 1 con un campo + 2 sin ninguno: {scope:?}"
    );
    assert_eq!(
        scope.value_count.unwrap() + scope.excluded_count.unwrap(),
        scope.document_count.unwrap(),
        "invariante: alcance = calculados + excluidos: {scope:?}"
    );
    assert!(!answer.verified, "{}", answer.text);

    // Cada categoría se informa con su propio conteo y su propio motivo.
    for expected in [
        "1 documento no se calculó porque dividía entre cero",
        "1 documento tenía los dos campos, pero con un valor que no es un número",
        "1 documento sólo tenía uno de los dos campos",
        "2 documentos no tenían ninguno de los dos campos",
    ] {
        assert!(
            answer.text.contains(expected),
            "falta «{expected}» en la respuesta: {}",
            answer.text
        );
    }
    assert!(
        answer.text.starts_with("División de «Monto principal» entre «Cantidad»"),
        "{}",
        answer.text
    );
}

/// Los tres encabezados nuevos, uno por operación: nunca «Suma de … por …».
/// Cada operación usa su propio acervo con dos campos inequívocos, para que
/// lo que se comprueba sea el encabezado y no la resolución de nombres.
#[test]
fn each_row_operation_is_named_by_its_own_verb() {
    for (fields, question, expected_prefix) in [
        (
            [("Cantidad", "4"), ("Precio unitario", "$125.00 MXN")],
            "¿Cuánto da Cantidad multiplicada por Precio unitario?",
            "Multiplicación de «Cantidad» por «Precio unitario»",
        ),
        (
            [("Monto principal", "$400.00 MXN"), ("Cantidad", "4")],
            "¿Cuánto da Monto principal dividido entre Cantidad?",
            "División de «Monto principal» entre «Cantidad»",
        ),
        (
            [("Monto A", "$500.00 MXN"), ("Monto B", "$100.00 MXN")],
            "¿Cuál es la diferencia entre Monto A y Monto B?",
            "Resta de «Monto A» menos «Monto B»",
        ),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        for index in 0..2 {
            let mut record = vec![("Folio", format!("RG-{index}"))];
            for (label, value) in &fields {
                record.push((*label, (*value).to_owned()));
            }
            let borrowed = record
                .iter()
                .map(|(label, value)| (*label, value.as_str()))
                .collect::<Vec<_>>();
            write_record(root, &format!("doc-{index}.md"), &borrowed);
        }
        let engine = index(root);

        let answer = engine.ask_in_conversation("c1", question).unwrap();
        assert!(
            answer.text.starts_with(expected_prefix),
            "«{question}» debía empezar por «{expected_prefix}»: {}",
            answer.text
        );
        assert!(
            !answer.text.starts_with("Suma de"),
            "ninguna operación fila por fila debe llamarse «Suma»: {}",
            answer.text
        );
    }
}

/// Un filtro explícito («Zona: Norte») debe conservarse en una comparación
/// aunque coincida exactamente con uno de los dos grupos comparados de esa
/// misma dimensión («Zona»). El lado que queda sin datos por ese filtro debe
/// explicarse, nunca borrarse en silencio ni completarse con una cifra
/// distinta a la que el filtro permite.
#[test]
fn an_explicit_filter_survives_even_when_it_matches_the_compared_dimension() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, zona, monto) in [
        ("doc-01.md", "Norte", "$500.00 MXN"),
        ("doc-02.md", "Norte", "$300.00 MXN"),
        ("doc-03.md", "Sur", "$600.00 MXN"),
        ("doc-04.md", "Sur", "$400.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Zona", zona),
                ("Monto principal", monto),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "Compara el Monto principal por Zona entre Norte y Sur, con Zona: Norte.",
        )
        .unwrap();

    // El filtro explícito restringe TODO el alcance a Zona: Norte, así que
    // el lado "Sur" se queda sin documentos: la pregunta original define un
    // alcance donde ese lado no existe, y el motor debe decirlo.
    assert!(answer.text.contains("$800.00 MXN"), "{}", answer.text);
    assert!(
        answer.text.contains("Sin datos") || answer.text.contains("sin datos"),
        "el lado Sur no puede tener evidencia bajo Zona: Norte: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("$1,000.00 MXN"),
        "600 + 400 delataría que el filtro explícito se descartó: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "una comparación con un lado vacío no puede declararse verificada: {}",
        answer.text
    );
}

/// Si la pregunta menciona «total» dos veces junto a una operación entre
/// campos, no queda claro si se pide documento por documento o entre los
/// totales ya agregados: Omega debe pedir aclaración en vez de elegir por su
/// cuenta la interpretación insegura (operar entre totales globales).
#[test]
fn an_ambiguous_totals_request_asks_instead_of_guessing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, cantidad, precio) in [
        ("doc-01.md", "4", "$125.00 MXN"),
        ("doc-02.md", "2", "$150.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Cantidad", cantidad),
                ("Precio unitario", precio),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Cuánto da el total de Cantidad multiplicado por el total de Precio unitario?",
        )
        .unwrap();

    assert!(answer.clarification.is_some(), "{}", answer.text);
    assert!(!answer.verified);
}

/// Un campo que el acervo no tiene en absoluto: la respuesta debe decirlo,
/// nunca sustituirlo por otro campo parecido ni inventar un total.
#[test]
fn summing_a_field_the_archive_does_not_have_says_so() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[("Folio", "RG-01"), ("Monto principal", "$500.00 MXN")],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es el promedio del campo Kilometraje?")
        .unwrap();

    assert!(
        answer.text.contains("no tiene un campo"),
        "{}",
        answer.text
    );
    assert!(!answer.verified);
}

/// Un campo que existe pero no es numérico (texto/estado) no puede sumarse:
/// la respuesta debe decir por qué, nunca tratarlo como si tuviera un valor.
#[test]
fn summing_a_non_numeric_field_is_rejected() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, estado, monto) in [
        ("doc-01.md", "Pendiente", "$500.00 MXN"),
        ("doc-02.md", "Pagada", "$300.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Estado", estado),
                ("Monto principal", monto),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuál es el promedio del Estado?")
        .unwrap();

    assert!(
        answer.text.contains("no es un campo numérico"),
        "{}",
        answer.text
    );
    assert!(!answer.verified);
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
