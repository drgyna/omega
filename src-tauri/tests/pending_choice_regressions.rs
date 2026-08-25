//! Regresión obligatoria: qué pasa cuando el usuario responde a una
//! aclaración pendiente con algo que no es una de las opciones ofrecidas.
//!
//! Las fixtures son genéricas —folio, estado, cantidad, precio, sucursal— y
//! se escriben en un directorio temporal (`tempfile`). No proceden de ningún
//! corpus del repositorio y no describen ningún giro de negocio concreto: el
//! motor debe comportarse igual con estos documentos que con los de
//! cualquier otro acervo autorizado. El reloj se fija para que ninguna
//! prueba dependa del día en que se ejecuta.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Secuencia obligatoria: preguntar por un subconjunto, pedir una suma
/// ambigua, responder «ponme todos» y después nombrar un campo concreto.
///
/// «ponme todos» no es una de las opciones ofrecidas, así que nunca debe
/// convertirse en una consulta global sobre todo el acervo: debe calcular
/// los campos numéricos ofrecidos sobre el mismo conjunto filtrado, y la
/// aclaración debe seguir disponible para que «Monto principal» la resuelva
/// después sin perder el filtro `Estado: Pendiente`.
#[test]
fn ponme_todos_keeps_the_filtered_set_and_never_sums_the_whole_archive() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, estado, cantidad, precio, monto) in [
        ("doc-01.md", "Pendiente", "4", "$125.00 MXN", "$500.00 MXN"),
        ("doc-02.md", "Pendiente", "2", "$150.00 MXN", "$300.00 MXN"),
        // Documento pagado: si el filtro se perdiera, esta cifra se colaría
        // en cualquiera de las sumas y lo delataría.
        ("doc-03.md", "Pagada", "10", "$999.00 MXN", "$9,990.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Estado", estado),
                ("Cantidad", cantidad),
                ("Precio unitario", precio),
                ("Monto principal", monto),
            ],
        );
    }
    let engine = index(root);

    let filtered = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Pendiente?")
        .unwrap();
    assert!(filtered.text.contains('2'), "{}", filtered.text);

    let ambiguous = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    let clarification = ambiguous
        .clarification
        .clone()
        .expect("varios campos numéricos en el alcance filtrado");
    assert_eq!(clarification.reason, "campo_ambiguo");
    assert!(clarification.options.contains(&"Cantidad".to_string()));
    assert!(clarification.options.contains(&"Precio unitario".to_string()));
    assert!(clarification.options.contains(&"Monto principal".to_string()));

    // «ponme todos» no es ninguna de las opciones. No debe destruir el
    // contexto ni convertirse en una consulta sobre todo el acervo.
    let all = engine.ask_in_conversation("c1", "ponme todos").unwrap();
    assert!(all.verified, "las tres sumas se calcularon sobre evidencia real: {}", all.text);
    assert!(all.used_context, "hereda el conjunto de la pregunta original: {}", all.text);
    assert!(all.text.contains("Cantidad"), "{}", all.text);
    assert!(all.text.contains("Precio unitario"), "{}", all.text);
    assert!(all.text.contains("Monto principal"), "{}", all.text);
    assert!(
        all.text.contains("$800.00 MXN"),
        "suma de Monto principal restringida a los dos pendientes: {}",
        all.text
    );
    assert!(
        all.text.contains("$275.00 MXN"),
        "suma de Precio unitario restringida a los dos pendientes: {}",
        all.text
    );
    // Ninguna cifra del documento pagado debe aparecer: ni sola ni sumada.
    assert!(!all.text.contains("9,990"), "{}", all.text);
    assert!(!all.text.contains("10,790"), "{}", all.text);
    assert!(!all.text.contains("1,124"), "{}", all.text);
    let scope = all.scope.clone().expect("alcance declarado");
    assert_eq!(scope.document_count, Some(2));

    // El usuario, tras ver la tabla, nombra un campo concreto: la aclaración
    // seguía pendiente y debe replanificar la pregunta original sobre el
    // mismo conjunto, exactamente como si lo hubiera elegido de entrada.
    let chosen = engine
        .ask_in_conversation("c1", "Monto principal")
        .unwrap();
    assert!(
        chosen.text.contains("$800.00 MXN"),
        "Monto principal no debe sumar todo el acervo: {}",
        chosen.text
    );
    assert!(!chosen.text.contains("9,990"), "{}", chosen.text);
    assert!(!chosen.text.contains("10,490"), "{}", chosen.text);
    assert!(chosen.verified, "el conjunto es correcto y debe declararse verificado");
    assert!(chosen.used_context);
    let scope = chosen.scope.clone().expect("alcance declarado");
    assert_eq!(
        scope.document_count,
        Some(2),
        "el filtro Estado: Pendiente se conserva"
    );
    assert_eq!(scope.value_count, Some(2), "cantidad de valores usados");
}

/// Una respuesta que no es ninguna opción ni pide «todos» —una frase sin
/// relación con la aclaración, o una palabra como «ninguno»— tampoco puede
/// convertirse en una consulta global: la aclaración se repite y sigue
/// pendiente hasta que el usuario elija una opción real.
#[test]
fn an_unrelated_reply_reasks_instead_of_going_global() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, estado, cantidad, monto) in [
        ("doc-01.md", "Pendiente", "4", "$500.00 MXN"),
        ("doc-02.md", "Pendiente", "2", "$300.00 MXN"),
        ("doc-03.md", "Pagada", "10", "$9,990.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Estado", estado),
                ("Cantidad", cantidad),
                ("Monto principal", monto),
            ],
        );
    }
    let engine = index(root);

    engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Estado: Pendiente?")
        .unwrap();
    let ambiguous = engine.ask_in_conversation("c1", "¿Cuánto suman?").unwrap();
    assert_eq!(
        ambiguous.clarification.expect("aclaración").reason,
        "campo_ambiguo"
    );

    for reply in ["ninguno", "no sé", "una frase que no tiene relación con nada de esto"] {
        let answer = engine.ask_in_conversation("c1", reply).unwrap();
        assert!(
            !answer.verified,
            "una respuesta no reconocida no puede quedar verificada: {}",
            answer.text
        );
        assert!(!answer.text.contains("9,990"), "{}", answer.text);
        let clarification = answer
            .clarification
            .clone()
            .expect("la aclaración sigue pendiente");
        assert_eq!(clarification.reason, "campo_ambiguo");
        assert!(clarification.options.contains(&"Cantidad".to_string()));
        assert!(clarification.options.contains(&"Monto principal".to_string()));
    }

    // El contexto sigue intacto: elegir ahora un campo real calcula sobre el
    // conjunto original, no sobre todo el acervo.
    let chosen = engine
        .ask_in_conversation("c1", "Monto principal")
        .unwrap();
    assert!(chosen.text.contains("$800.00 MXN"), "{}", chosen.text);
    assert!(!chosen.text.contains("9,990"), "{}", chosen.text);
    assert!(chosen.verified);
}

/// Un filtro explícito escrito por el usuario debe seguir aplicándose en una
/// comparación entre dos grupos: no puede desaparecer silenciosamente sólo
/// porque la pregunta también compara.
#[test]
fn an_explicit_filter_survives_a_group_comparison() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, sucursal, estado, monto) in [
        ("doc-01.md", "Norte", "Pendiente", "$500.00 MXN"),
        ("doc-02.md", "Norte", "Pendiente", "$300.00 MXN"),
        // Si el filtro Estado: Pendiente se perdiera, esta cifra se sumaría.
        ("doc-03.md", "Norte", "Pagada", "$900.00 MXN"),
        ("doc-04.md", "Sur", "Pendiente", "$600.00 MXN"),
        ("doc-05.md", "Sur", "Pendiente", "$400.00 MXN"),
        ("doc-06.md", "Sur", "Pagada", "$750.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", &format!("RG-{name}")),
                ("Sucursal", sucursal),
                ("Estado", estado),
                ("Monto principal", monto),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "Compara el Monto principal por Sucursal entre Norte y Sur, con Estado: Pendiente.",
        )
        .unwrap();

    assert!(
        answer.text.contains("$800.00 MXN"),
        "Norte pendiente: {}",
        answer.text
    );
    assert!(
        answer.text.contains("$1,000.00 MXN"),
        "Sur pendiente: {}",
        answer.text
    );
    // Los totales sin filtrar (1,700 y 1,750) delatarían que el filtro
    // explícito se descartó.
    assert!(!answer.text.contains("1,700"), "{}", answer.text);
    assert!(!answer.text.contains("1,750"), "{}", answer.text);
    assert!(answer.verified, "{}", answer.text);
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
