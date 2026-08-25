use std::path::PathBuf;

use omega_core::{Answer, OmegaEngine};

#[test]
fn universal_local_engine_answers_the_yacht_acceptance_matrix() {
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("corpus-prueba-agencia-yates");
    assert!(corpus.is_dir(), "falta el corpus de prueba versionado");
    let temporary = tempfile::tempdir().unwrap();
    let engine = OmegaEngine::open(temporary.path().join("omega-yates.db")).unwrap();
    let source = engine.authorize_source(&corpus).unwrap();
    let report = engine.index_source(source).unwrap();
    assert_eq!(report.indexed, 600);

    // A y B: inventario global y conteo por carpeta descubierta.
    let inventory = ask(
        &engine,
        "¿Cuántos documentos hay indexados y qué categorías contiene el acervo?",
    );
    assert_contains(&inventory, &["600", "11 categorías", "02_reservas_charter"]);
    assert_eq!(inventory.citations.len(), 11);
    let reservations = ask(
        &engine,
        "¿Cuántos documentos pertenecen a la carpeta 02_reservas_charter?",
    );
    assert_contains(&reservations, &["140", "02_reservas_charter"]);
    assert!(
        reservations
            .citations
            .iter()
            .all(|item| item.origin == "02_reservas_charter")
    );
    let incidents = ask(
        &engine,
        "¿Cuántos documentos pertenecen a la carpeta 08_incidentes?",
    );
    assert_contains(&incidents, &["30", "08_incidentes"]);

    // Búsquedas literales: un identificador completo permanece cerrado y un
    // prefijo o valor inexistente no se expande.
    for (question, expected_origin, expected_fragment) in [
        (
            "Encuentra exactamente el folio MAY-26-0001-AZM.",
            "01_ventas",
            "MAY-26-0001-AZM",
        ),
        (
            "Busca el permiso con número de control PERM-QUI-00535.",
            "06_permisos_cumplimiento",
            "PERM-QUI-00535",
        ),
    ] {
        let answer = ask(&engine, question);
        assert_eq!(distinct_documents(&answer), 1);
        assert!(
            answer
                .citations
                .iter()
                .all(|item| item.origin == expected_origin)
        );
        assert!(
            answer
                .citations
                .iter()
                .any(|item| item.excerpt.contains(expected_fragment))
        );
    }
    let nonexistent = ask(
        &engine,
        "Encuentra exactamente el identificador OMEGA-YATE-INEXISTENTE-999.",
    );
    assert!(nonexistent.citations.is_empty());

    // Conteos estructurados de un filtro. Cada cifra es de documentos
    // distintos; no del número de filas extraídas ni del límite de citas.
    for (question, expected) in [
        (
            "¿Cuántas reservas tienen Tipo de documento: Reserva de renta náutica?",
            "140 documentos",
        ),
        (
            "¿Cuántas reservas tienen Estado: Pendiente de pago?",
            "11 documentos",
        ),
        (
            "¿Cuántas facturas tienen Estado de la factura: Vencida?",
            "4 documentos",
        ),
        (
            "¿Cuántos permisos tienen Estado: Por renovar?",
            "4 documentos",
        ),
        (
            "¿Cuántos incidentes tienen Estado: Investigación abierta?",
            "5 documentos",
        ),
        (
            "¿Cuántos documentos registran Clase de embarcación: catamarán?",
            "120 documentos",
        ),
    ] {
        assert_contains(&ask(&engine, question), &[expected]);
    }

    // C: todos los filtros deben cumplirse en el mismo document_id.
    for (question, expected, origin) in [
        (
            "Muestra documentos con Ciudad base: Cancún y Tipo de documento: Reserva de renta náutica.",
            "11 documentos",
            "02_reservas_charter",
        ),
        (
            "Muestra los expedientes de mantenimiento cerrados de embarcaciones en Puerto Vallarta.",
            "5 documentos",
            "04_mantenimiento",
        ),
    ] {
        let answer = ask(&engine, question);
        assert_contains(&answer, &[expected]);
        assert!(answer.citations.len() <= 24);
        assert!(answer.citations.iter().all(|item| item.origin == origin));
    }

    // D: sumas, conteo de valores y agrupación son operaciones sobre valores,
    // no conteos de documentos ni sumas entre etiquetas monetarias distintas.
    for (question, expected) in [
        (
            "Suma el campo Total facturado de todas las facturas en MXN.",
            ["$5,585,748.00 MXN", "45 valores"],
        ),
        (
            "Suma el campo Precio de lista de todos los expedientes de venta en MXN.",
            ["$1,273,950,000.00 MXN", "100 valores"],
        ),
        (
            "Suma el campo Tarifa contratada de todas las reservas en MXN.",
            ["$15,089,550.00 MXN", "140 valores"],
        ),
        (
            "¿Cuántos valores tiene el campo Anticipo recibido?",
            ["140 valores", "con evidencia"],
        ),
    ] {
        let answer = ask(&engine, question);
        assert_contains(&answer, &expected);
        assert!(
            answer
                .citations
                .iter()
                .any(|item| item.id.starts_with("calc-"))
        );
        assert!(answer.citations.len() <= 18);
    }
    let grouped = ask(
        &engine,
        "Agrupa la suma de Total facturado por Ciudad base.",
    );
    assert_contains(&grouped, &["Total facturado", "Ciudad base", "45 valores"]);
    assert!(grouped.text.contains("| Grupo | Suma | Valores |"));

    // E: respuesta puramente extractiva y citas acotadas del origen adecuado.
    let cancellation = ask(
        &engine,
        "¿Qué reglas de cancelación se aplican a una renta de yate?",
    );
    assert!(!cancellation.citations.is_empty());
    assert!(cancellation.citations.len() <= 6);
    assert!(cancellation.citations.iter().any(|item| {
        item.origin == "02_reservas_charter" && item.excerpt.contains("siete días")
    }));
    for (question, expected_origin, expected_term) in [
        (
            "¿Quién tiene la autoridad final para cambiar una ruta durante un charter?",
            None,
            "capitán",
        ),
        (
            "¿Qué impide liberar una embarcación para renta después de mantenimiento?",
            Some("04_mantenimiento"),
            "propulsión",
        ),
        (
            "¿Qué controles se aplican a un pago inusual o realizado por un tercero?",
            None,
            "verific",
        ),
        (
            "¿Qué se debe hacer cuando un permiso vence o queda suspendido?",
            Some("06_permisos_cumplimiento"),
            "bloque",
        ),
        (
            "¿Qué datos no puede compartir el personal con terceros?",
            Some("05_personal"),
            "tercer",
        ),
        (
            "Busca incidentes relacionados con lesión leve atendida a bordo.",
            Some("08_incidentes"),
            "lesión leve",
        ),
    ] {
        let answer = ask(&engine, question);
        assert!(
            !answer.citations.is_empty(),
            "sin evidencia para {question}"
        );
        assert!(
            answer.citations.len() <= 12,
            "demasiadas citas ({}) para {question}: {}",
            answer.citations.len(),
            answer.text
        );
        if let Some(origin) = expected_origin {
            assert!(answer.citations.iter().all(|item| item.origin == origin));
        }
        assert!(
            answer
                .citations
                .iter()
                .any(|item| item.excerpt.to_lowercase().contains(expected_term)),
            "no apareció {expected_term:?} para {question}: {}",
            answer.text
        );
    }

    let person = ask(&engine, "Busca a Sofía Valdés Romero.");
    assert!(distinct_documents(&person) > 1);
    assert!(person.citations.len() <= 20);
    assert!(
        person
            .citations
            .iter()
            .any(|item| item.value.as_deref() == Some("Sofía Valdés Romero"))
    );
    let privacy = ask(
        &engine,
        "¿Existe documentación sobre protección de datos personales?",
    );
    assert!(!privacy.citations.is_empty());
    assert!(privacy.citations.len() <= 6);

    // F: cierre honesto, búsqueda exacta cerrada y advertencia legal.
    let absent = ask(
        &engine,
        "¿Cuántos documentos mencionan una flota de submarinos?",
    );
    assert!(!absent.verified);
    assert!(absent.citations.is_empty());
    let incomplete = ask(&engine, "Encuentra exactamente MAY.");
    assert!(incomplete.citations.is_empty());
    let fiscal = ask(
        &engine,
        "Encuentra exactamente el folio fiscal interno FAC-MAY-26-0481.",
    );
    assert_eq!(
        fiscal
            .citations
            .iter()
            .map(|item| item.document_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
    assert_eq!(fiscal.citations[0].origin, "07_facturas");
    assert!(fiscal.citations[0].excerpt.contains("FAC-MAY-26-0481"));
    let legal = ask(
        &engine,
        "Resume las obligaciones legales exactas de México para operar un yate comercial.",
    );
    assert!(legal.warning.is_some());
    assert!(legal.text.contains("no sustituye asesoría legal"));
}

fn ask(engine: &OmegaEngine, question: &str) -> Answer {
    engine
        .ask(question)
        .unwrap_or_else(|error| panic!("falló {question:?}: {error}"))
}

fn assert_contains(answer: &Answer, expected: &[&str]) {
    for value in expected {
        assert!(
            answer.text.contains(value),
            "faltó {value:?} en respuesta: {}",
            answer.text
        );
    }
}

fn distinct_documents(answer: &Answer) -> usize {
    answer
        .citations
        .iter()
        .map(|item| item.document_id)
        .collect::<std::collections::HashSet<_>>()
        .len()
}
