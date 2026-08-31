//! Regresiones P0: el alcance explícito no se sustituye, las sumas declaran
//! todas sus exclusiones y la procedencia OCR no se pierde al calcular.
//!
//! Todas las fixtures viven en directorios temporales. No usan el corpus real.

use std::{fs, path::Path};

use omega_core::{Clock, Database, OmegaEngine, ToolEngine};
use serde_json::json;

const TODAY: &str = "2026-08-26";

/// Las seis formas en que se escribe un origen ordinal. La matriz las cruza
/// con varias intenciones porque el defecto real sólo aparecía en algunas:
/// preguntar «¿cuántos documentos hay en 02 reportes?» se cortaba por otro
/// motivo, mientras que «suma el campo Importe en 02 reportes» sustituía en
/// silencio la carpeta por `01_reportes` y devolvía una respuesta verificada.
const ORIGIN_FORMATS: [&str; 6] = [
    "02_reportes",
    "02 reportes",
    "02-reportes",
    "“02 reportes”",
    "carpeta 02 reportes",
    "origen 02 reportes",
];

/// Las mismas seis formas, para una carpeta que sí existe.
const EXISTING_ORIGIN_FORMATS: [&str; 6] = [
    "01_reportes",
    "01 reportes",
    "01-reportes",
    "“01 reportes”",
    "carpeta 01 reportes",
    "origen 01 reportes",
];

fn origin_fixture() -> (tempfile::TempDir, OmegaEngine) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let reports = root.join("01_reportes");
    fs::create_dir(&reports).unwrap();
    for index in 0..3 {
        write_record(
            &reports,
            &format!("rep-{index}.md"),
            &[("Folio", &format!("REP-{index}")), ("Importe", "$10.00 MXN")],
        );
    }
    let contracts = root.join("05_contratos");
    fs::create_dir(&contracts).unwrap();
    write_record(
        &contracts,
        "con-0.md",
        &[("Folio", "CON-0"), ("Importe", "$99.00 MXN")],
    );
    let engine = index(root);
    (fixture, engine)
}

/// Un origen nombrado explícitamente que NO existe nunca puede resolverse por
/// coincidencia aproximada, en ninguno de los seis formatos y en ninguna de
/// las intenciones. La respuesta se queda sin evidencia, no verificada, y sin
/// mencionar la carpeta parecida.
#[test]
fn a_missing_explicit_origin_is_never_replaced_by_a_similar_folder() {
    let (_fixture, engine) = origin_fixture();

    for requested in ORIGIN_FORMATS {
        for question in [
            format!("¿Cuántos documentos hay en {requested}?"),
            format!("Suma el campo Importe en {requested}."),
            format!("¿Cuál es el promedio del Importe en {requested}?"),
            format!("Muéstrame los documentos de {requested}."),
        ] {
            let answer = engine.ask(&question).unwrap();

            assert!(!answer.verified, "«{question}»: {}", answer.text);
            assert!(
                answer.citations.is_empty(),
                "«{question}» no puede citar evidencia de otra carpeta: {:?}",
                answer.citations
            );
            assert!(
                !answer.text.contains("01_reportes") && !answer.text.contains("05_contratos"),
                "«{question}» no puede sustituirse por un origen parecido: {}",
                answer.text
            );
            assert!(
                answer
                    .scope
                    .as_ref()
                    .and_then(|scope| scope.origin.clone())
                    .is_none(),
                "«{question}» no puede fijar el alcance en una carpeta que no se pidió: {:?}",
                answer.scope
            );
        }
    }
}

/// La otra mitad del requisito: un origen que sí existe debe reconocerse en
/// los seis formatos y proteger el alcance, sin ampliarlo al resto del
/// acervo. La suma sólo puede ver los 3 documentos de `01_reportes`, nunca
/// los 4 del acervo completo.
#[test]
fn an_existing_explicit_origin_is_recognised_and_scopes_the_answer() {
    let (_fixture, engine) = origin_fixture();

    for requested in EXISTING_ORIGIN_FORMATS {
        let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());
        let question = format!("Suma el campo Importe en {requested}.");
        assert_eq!(
            tools.match_origin(&question).unwrap().as_deref(),
            Some("01_reportes"),
            "«{question}» debe reconocer la carpeta existente"
        );
        assert!(
            !tools.explicit_origin_is_missing(&question).unwrap(),
            "«{question}» nombra una carpeta que sí existe"
        );

        let answer = engine.ask(&question).unwrap();
        let scope = answer
            .scope
            .as_ref()
            .unwrap_or_else(|| panic!("«{question}» debe declarar alcance: {}", answer.text));
        assert_eq!(
            scope.origin.as_deref(),
            Some("01_reportes"),
            "«{question}» debe fijar el alcance en la carpeta pedida: {scope:?}"
        );
        assert_eq!(
            scope.document_count,
            Some(3),
            "«{question}» no puede salirse de la carpeta pedida: {scope:?}"
        );
        assert!(
            answer.text.contains("$30.00 MXN"),
            "«{question}» debe sumar sólo la carpeta pedida: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("$129.00"),
            "«{question}» nunca puede incluir la otra carpeta: {}",
            answer.text
        );
    }
}

/// Un ordinal sin cero a la izquierda dentro de una frase corriente («en 12
/// documentos») no es el nombre de una carpeta. Si se leyera como tal, la
/// pregunta se rechazaría por un origen que nadie nombró.
#[test]
fn a_plain_number_in_a_sentence_is_not_read_as_an_explicit_origin() {
    let (_fixture, engine) = origin_fixture();
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    for question in [
        "Suma el campo Importe en 12 documentos.",
        "¿Cuántos documentos tienen Importe de 10 pesos?",
        "Muéstrame los 3 primeros documentos.",
    ] {
        assert!(
            !tools.explicit_origin_is_missing(question).unwrap(),
            "«{question}» no nombra ninguna carpeta explícita"
        );
    }
}

#[test]
fn aggregate_values_reports_scope_exclusions_and_a_safe_decimal_result() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "valido.md", &[("Folio", "AGG-01"), ("Importe", "$0.10 MXN")]);
    write_record(root, "invalido.md", &[("Folio", "AGG-02"), ("Importe", "N/D")]);
    write_record(root, "ausente.md", &[("Folio", "AGG-03"), ("Estado", "Vigente")]);
    let engine = index(root);
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    let result = tools
        .execute(
            "aggregate_values",
            &json!({
                "concept": "Importe",
                "operation": "sum",
                "filters": [],
                "origin": null,
                "currency": null,
                "date_from": null,
                "date_to": null,
                "group_by": null
            }),
        )
        .unwrap();

    assert_eq!(result.data["document_count"].as_i64(), Some(3), "{}", result.data);
    assert_eq!(result.data["value_count"].as_i64(), Some(1), "{}", result.data);
    assert_eq!(result.data["excluded_count"].as_i64(), Some(2), "{}", result.data);
    assert_eq!(result.data["verified"].as_bool(), Some(false), "{}", result.data);
    assert!(
        result.data["warning"].as_str().is_some(),
        "la suma parcial debe advertirse: {}",
        result.data
    );
    assert_eq!(result.data["rows"][0]["value"].as_str(), Some("$0.10 MXN"));
}

#[test]
fn a_sum_reports_the_full_scope_invalid_and_missing_values_as_a_visible_partial_result() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "valido-1.md", &[("Folio", "SUM-01"), ("Importe", "$0.10 MXN")]);
    write_record(root, "valido-2.md", &[("Folio", "SUM-02"), ("Importe", "$0.20 MXN")]);
    write_record(root, "invalido.md", &[("Folio", "SUM-03"), ("Importe", "N/D")]);
    write_record(root, "ausente.md", &[("Folio", "SUM-04"), ("Estado", "Vigente")]);
    let engine = index(root);

    let answer = engine.ask("Suma el campo Importe.").unwrap();

    assert!(answer.text.contains("$0.30 MXN"), "{}", answer.text);
    let scope = answer.scope.expect("la suma debe declarar su alcance completo");
    assert_eq!(scope.document_count, Some(4), "{scope:?}");
    assert_eq!(scope.value_count, Some(2), "{scope:?}");
    assert_eq!(scope.excluded_count, Some(2), "{scope:?}");
    assert_eq!(
        scope.value_count.unwrap() + scope.excluded_count.unwrap(),
        scope.document_count.unwrap(),
        "alcance = calculados + excluidos: {scope:?}"
    );
    assert!(
        answer.text.contains("valor que no es un número")
            && answer.text.contains("sin ese campo"),
        "las dos exclusiones deben ser visibles: {}",
        answer.text
    );
    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.warning.is_some(), "{}", answer.text);
}

#[test]
fn low_confidence_ocr_is_inherited_by_calculation_evidence_and_blocks_verification() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "ocr.md", &[("Folio", "OCR-01"), ("Importe", "$25.00 MXN")]);
    let engine = index(root);

    let connection = rusqlite::Connection::open(engine.database_path()).unwrap();
    connection
        .execute(
            "UPDATE documents SET ocr_status = 'low_confidence', ocr_confidence = 0.10",
            [],
        )
        .unwrap();

    let answer = engine.ask("Suma el campo Importe.").unwrap();

    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.warning.is_some(), "{}", answer.text);
    assert!(
        answer
            .citations
            .iter()
            .any(|evidence| {
                !evidence.reliable
                    && evidence.ocr_status.as_deref() == Some("low_confidence")
                    && evidence.ocr_confidence == Some(0.10)
                    && evidence.confidence == Some(0.10)
            }),
        "los operandos deben conservar OCR 0.10: {:?}",
        answer.citations
    );
}

#[test]
fn ocr_after_the_visible_citation_limit_blocks_sum_row_calculation_and_classic_aggregate() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for index in 0..60 {
        write_record(
            root,
            &format!("doc-{index:02}.md"),
            &[
                ("Folio", &format!("OCR-{index:02}")),
                ("Importe", "$1.00 MXN"),
                ("Cantidad", "2"),
                ("Precio unitario", "$3.00 MXN"),
            ],
        );
    }
    let engine = index(root);
    let connection = rusqlite::Connection::open(engine.database_path()).unwrap();
    connection
        .execute(
            "UPDATE documents SET ocr_status = 'low_confidence', ocr_confidence = 0.10 WHERE path LIKE '%doc-59.md'",
            [],
        )
        .unwrap();

    let sum = engine.ask("Suma el campo Importe.").unwrap();
    assert!(!sum.verified, "{}", sum.text);
    assert!(sum.warning.is_some(), "{}", sum.text);
    assert!(
        !sum.citations.iter().any(|evidence| evidence.path.ends_with("doc-59.md")),
        "el OCR débil debe quedar fuera de la muestra visible para probar la señal agregada"
    );

    let row = engine
        .ask("¿Cuánto da Cantidad multiplicada por Precio unitario?")
        .unwrap();
    assert!(!row.verified, "{}", row.text);
    assert!(row.warning.is_some(), "{}", row.text);
    assert!(
        !row.citations.iter().any(|evidence| evidence.path.ends_with("doc-59.md")),
        "el OCR débil debe quedar fuera de la muestra visible para probar la señal de fila"
    );

    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());
    let classic = tools
        .execute(
            "aggregate_values",
            &json!({
                "concept": "Importe",
                "operation": "sum",
                "filters": [],
                "origin": null,
                "currency": null,
                "date_from": null,
                "date_to": null,
                "group_by": null
            }),
        )
        .unwrap();
    assert_eq!(classic.data["verified"].as_bool(), Some(false), "{}", classic.data);
    assert!(classic.data["warning"].as_str().is_some(), "{}", classic.data);
}

/// Acervo de 60 documentos donde el OCR débil está en el ÚLTIMO, para que
/// quede fuera de cualquier muestra de citas visible. `Zona` alterna para
/// poder comparar y ordenar dos grupos sobre el mismo acervo.
fn sixty_document_fixture_with_late_weak_ocr() -> (tempfile::TempDir, OmegaEngine) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for index in 0..60 {
        write_record(
            root,
            &format!("doc-{index:02}.md"),
            &[
                ("Folio", &format!("OCR-{index:02}")),
                ("Zona", if index % 2 == 0 { "Norte" } else { "Sur" }),
                ("Importe", "$1.00 MXN"),
                ("Cantidad", "2"),
                ("Precio unitario", "$3.00 MXN"),
            ],
        );
    }
    let engine = index(root);
    // Sólo el documento 59 queda marcado: es el último por orden de ruta, así
    // que ninguna muestra de citas lo alcanza.
    let connection = rusqlite::Connection::open(engine.database_path()).unwrap();
    let updated = connection
        .execute(
            "UPDATE documents SET ocr_status = 'low_confidence', ocr_confidence = 0.10
             WHERE path LIKE '%doc-59.md'",
            [],
        )
        .unwrap();
    assert_eq!(updated, 1, "debe marcarse exactamente un documento");
    (fixture, engine)
}

fn weak_ocr_is_outside_the_visible_citations(answer: &omega_core::Answer) -> bool {
    !answer
        .citations
        .iter()
        .any(|evidence| evidence.path.ends_with("doc-59.md"))
}

/// Una comparación entre dos grupos usa como operandos todos los valores del
/// alcance, no sólo los citados. Si el OCR débil cae en el lado que no se
/// muestra, la comparación tampoco puede declararse verificada.
#[test]
fn ocr_after_the_citation_limit_blocks_a_group_comparison_and_its_continuation() {
    let (_fixture, engine) = sixty_document_fixture_with_late_weak_ocr();

    let comparison = engine
        .ask_in_conversation("cmp", "Compara el Importe por Zona entre Norte y Sur.")
        .unwrap();
    assert!(
        !comparison.verified,
        "una comparación con un operando de OCR débil no puede verificarse: {}",
        comparison.text
    );
    assert!(comparison.warning.is_some(), "{}", comparison.text);
    assert!(
        weak_ocr_is_outside_the_visible_citations(&comparison),
        "el OCR débil debe quedar fuera de la muestra para que la prueba valga"
    );

    // La continuación deriva su cifra de la comparación anterior: hereda su
    // procedencia aunque sus propias citas sean fiables.
    let difference = engine
        .ask_in_conversation("cmp", "¿Cuál es la diferencia?")
        .unwrap();
    assert!(
        !difference.verified,
        "la diferencia derivada hereda el OCR débil de la comparación: {}",
        difference.text
    );
    assert!(difference.warning.is_some(), "{}", difference.text);
}

/// Un ranking agrupa y ordena sobre los mismos operandos. La señal debe
/// sobrevivir igual que en una suma.
#[test]
fn ocr_after_the_citation_limit_blocks_a_ranking() {
    let (_fixture, engine) = sixty_document_fixture_with_late_weak_ocr();

    let ranking = engine
        .ask_in_conversation("rank", "¿Cuál Zona tiene el mayor Importe?")
        .unwrap();

    assert!(
        !ranking.verified,
        "un ranking con un operando de OCR débil no puede verificarse: {}",
        ranking.text
    );
    assert!(ranking.warning.is_some(), "{}", ranking.text);
}

/// «¿Qué documentos respaldan ese total?» reutiliza la evidencia recordada,
/// que está recortada. La señal se guarda con el cálculo, no con las citas,
/// justamente para que esta continuación no pueda perderla.
#[test]
fn ocr_after_the_citation_limit_blocks_the_supporting_documents_continuation() {
    let (_fixture, engine) = sixty_document_fixture_with_late_weak_ocr();

    let sum = engine
        .ask_in_conversation("sup", "Suma el campo Importe.")
        .unwrap();
    assert!(!sum.verified, "{}", sum.text);
    assert!(
        weak_ocr_is_outside_the_visible_citations(&sum),
        "el OCR débil debe quedar fuera de la muestra para que la prueba valga"
    );

    let supporting = engine
        .ask_in_conversation("sup", "¿Qué documentos respaldan ese total?")
        .unwrap();
    assert!(
        !supporting.verified,
        "los documentos que respaldan un total heredan su procedencia: {}",
        supporting.text
    );
    assert!(supporting.warning.is_some(), "{}", supporting.text);
}

/// El agregado clásico publica la señal explícitamente, además de negarse a
/// declararse verificado.
#[test]
fn the_classic_aggregate_publishes_the_unreliable_evidence_signal() {
    let (_fixture, engine) = sixty_document_fixture_with_late_weak_ocr();
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    let result = tools
        .execute(
            "aggregate_values",
            &json!({
                "concept": "Importe",
                "operation": "sum",
                "filters": [],
                "origin": null,
                "currency": null,
                "date_from": null,
                "date_to": null,
                "group_by": null
            }),
        )
        .unwrap();

    assert_eq!(
        result.data["has_unreliable_evidence"].as_bool(),
        Some(true),
        "el agregado debe publicar la señal: {}",
        result.data
    );
    assert_eq!(result.data["verified"].as_bool(), Some(false), "{}", result.data);
    assert!(result.data["warning"].as_str().is_some(), "{}", result.data);
    // La suma sí es correcta y exacta: 60 documentos a $1.00.
    assert_eq!(result.data["rows"][0]["value"].as_str(), Some("$60.00 MXN"));
    assert_eq!(
        result.data["rows"][0]["has_unreliable_evidence"].as_bool(),
        Some(true),
        "la fila también conserva la señal: {}",
        result.data
    );
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
