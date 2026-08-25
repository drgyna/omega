//! Fase 2: la respuesta lee la evidencia que la búsqueda ya encontró.
//!
//! Los nombres de campo y los valores de estas fixtures son datos neutros
//! creados aquí mismo; no describen ningún rubro de negocio ni proceden de
//! ningún corpus real.

use std::fs;

use omega_core::{Answer, OmegaEngine, value_is_supported};

/// Un solo resultado se redacta como una frase directa que nombra el registro
/// y da el valor pedido, en lugar de contar resultados.
#[test]
fn a_single_result_becomes_a_direct_sentence() {
    let fixture = tempfile::tempdir().unwrap();
    write_amount_records(fixture.path(), 24, "MXN");
    let engine = index(fixture.path());

    let answer = engine
        .ask("¿Qué registro tiene upkeep amount de $3,300 MXN?")
        .unwrap();
    assert_eq!(
        answer.text,
        "El campo «Upkeep amount» de RC-023 es $3,300 MXN (record-023.txt, línea 2)."
    );
    assert!(answer.verified);
    assert_supported(&answer, &["Upkeep amount", "$3,300 MXN", "RC-023"]);
}

/// Muchos resultados numéricos del mismo campo se resumen con su conteo, su
/// rango y su total. El total no está escrito en ningún documento, así que se
/// cita como un cálculo local explícito.
#[test]
fn many_numeric_results_report_count_range_and_total() {
    let fixture = tempfile::tempdir().unwrap();
    write_amount_records(fixture.path(), 24, "MXN");
    let engine = index(fixture.path());

    let answer = engine.ask("¿Cuál es el upkeep amount?").unwrap();
    assert_eq!(
        answer.text,
        "Upkeep amount — 24 valores\n\n\
         | Mínimo | Máximo | Total |\n\
         |---|---|---|\n\
         | $1,000 MXN | $3,300 MXN | $51,600 MXN |"
    );
    assert!(answer.verified);
    // 24 evidencias recuperadas más la nota de cálculo que respalda el total.
    assert_eq!(answer.citations.len(), 25);
    assert_eq!(
        answer.citations.last().unwrap().value.as_deref(),
        Some("$51,600 MXN")
    );
    assert_supported(
        &answer,
        &["Upkeep amount", "$1,000 MXN", "$3,300 MXN", "$51,600 MXN"],
    );
}

/// Montos de monedas distintas nunca se combinan en un solo total.
#[test]
fn amounts_in_different_currencies_are_never_added_together() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, code, amount) in [
        ("record-000.txt", "RC-000", "$100 MXN"),
        ("record-001.txt", "RC-001", "$200 MXN"),
        ("record-002.txt", "RC-002", "$300 MXN"),
        ("record-003.txt", "RC-003", "$40 USD"),
        ("record-004.txt", "RC-004", "$60 USD"),
    ] {
        fs::write(
            root.join(name),
            format!("Record: {code}\nUpkeep amount: {amount}\n"),
        )
        .unwrap();
    }
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el upkeep amount?").unwrap();
    assert_eq!(
        answer.text,
        "Upkeep amount — 5 valores\n\n\
         | Moneda | Valores | Mínimo | Máximo | Total |\n\
         |---|---|---|---|---|\n\
         | MXN | 3 | $100 MXN | $300 MXN | $600 MXN |\n\
         | USD | 2 | $40 USD | $60 USD | $100 USD |\n\n\
         Los totales no se combinan entre monedas distintas."
    );
    assert!(answer.verified);
    assert_supported(&answer, &["$600 MXN", "$100 USD"]);
}

/// Resultados no numéricos del mismo campo se resumen con su conteo y la lista
/// de valores distintos que realmente aparecen.
#[test]
fn many_textual_results_report_count_and_distinct_values() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, state) in [
        ("state-1.txt", "Ready"),
        ("state-2.txt", "Ready"),
        ("state-3.txt", "Blocked"),
        ("state-4.txt", "Blocked"),
        ("state-5.txt", "Done"),
        ("state-6.txt", "Ready"),
    ] {
        fs::write(root.join(name), format!("Lifecycle: {state}\n")).unwrap();
    }
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el lifecycle?").unwrap();
    assert_eq!(
        answer.text,
        "Lifecycle — 6 valores, 3 valores distintos\n\nReady · Blocked · Done"
    );
    assert!(answer.verified);
    assert_supported(&answer, &["Ready", "Blocked", "Done"]);
}

/// Un campo compuesto cuyo nombre CONTIENE a otro campo más corto como
/// prefijo ("Estado del registro" contiene "estado") no puede perder su
/// palabra distintiva sólo porque esa palabra también forma parte del
/// vocabulario de relleno de preguntas ("registro"/"registros" describen
/// genéricamente qué se está pidiendo). Sin este resguardo, "Estado del
/// registro" quedaba reducido a "estado" y empataba con el campo "estado" de
/// otro documento, cerrando la búsqueda por ambigüedad.
#[test]
fn a_compound_field_keeps_its_distinguishing_word_even_if_it_is_filler_elsewhere() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("registro-1.txt"),
        "Estado del registro: Pendiente
",
    )
    .unwrap();
    fs::write(
        root.join("registro-2.txt"),
        "Estado del registro: Pagado
",
    )
    .unwrap();
    fs::write(
        root.join("bitacora.csv"),
        "estado
Abierto
",
    )
    .unwrap();
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el estado del registro?").unwrap();
    assert_eq!(
        answer.text,
        "Estado del registro — 2 valores, 2 valores distintos\n\nPendiente · Pagado"
    );
    assert!(answer.verified);
}

/// Un campo compuesto ("Estado de la propiedad") también debe seguir
/// resolviendo aunque un campo más corto ("estado", de otro documento) sea
/// literalmente un prefijo suyo dentro del texto normalizado de la pregunta.
#[test]
fn a_longer_field_match_is_not_defeated_by_a_shorter_contained_one() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("ficha-1.txt"),
        "Estado de la propiedad: Disponible
",
    )
    .unwrap();
    fs::write(
        root.join("ficha-2.txt"),
        "Estado de la propiedad: Vendida
",
    )
    .unwrap();
    fs::write(
        root.join("otro.csv"),
        "estado
Abierto
",
    )
    .unwrap();
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el estado de la propiedad?").unwrap();
    assert_eq!(
        answer.text,
        "Estado de la propiedad — 2 valores, 2 valores distintos\n\nDisponible · Vendida"
    );
    assert!(answer.verified);
}

/// Una frase que nombra la carpeta de origen ("de mantenimiento") describe el
/// campo, no un intento de valor -incluso para un campo de texto, donde no hay
/// ninguna forma numérica que lo distinga de un valor real-. Se reconoce
/// contra las carpetas que el propio acervo ya autorizó, no contra un
/// diccionario de negocio.
#[test]
fn a_phrase_naming_a_real_origin_folder_does_not_close_a_text_field_search() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // El prefijo numérico sigue la convención real del corpus validado en esta
    // fase (p.ej. "05_mantenimiento"): la pregunta natural nunca lo escribe,
    // así que el metadato de carpeta no compite por la misma evidencia y esta
    // prueba aísla específicamente el mecanismo que se corrigió aquí.
    let category = root.join("05_mantenimiento");
    fs::create_dir(&category).unwrap();
    fs::write(
        category.join("orden-1.txt"),
        "Proveedor: Servicios Nova
",
    )
    .unwrap();
    fs::write(
        category.join("orden-2.txt"),
        "Proveedor: Servicios Nova
",
    )
    .unwrap();
    let engine = index(root);

    let described = engine
        .ask("¿Cuál es el proveedor de mantenimiento?")
        .unwrap();
    assert_eq!(
        described.text,
        "Encontré 2 valores de «Proveedor», todos «Servicios Nova»."
    );
    assert!(described.verified);

    // Un valor real que no existe sigue cerrando la búsqueda: la carpeta de
    // origen no es un comodín que perdone cualquier palabra.
    let missing_value = engine
        .ask("Busca documentos cuyo proveedor sea Fabricante Inexistente")
        .unwrap();
    assert!(missing_value.citations.is_empty());
}

/// Una frase descriptiva después del nombre de un campo numérico no puede
/// apagar la búsqueda: "el upkeep amount de mantenimiento" no es un intento de
/// valor fallido, es cómo la pregunta describe el campo en lenguaje natural.
/// Antes cualquier palabra no listada como relleno cerraba la búsqueda a cero;
/// ahora sólo lo hace si lo que sigue tiene forma de un valor real de ese tipo
/// (aquí, un dígito).
#[test]
fn a_descriptive_phrase_after_a_numeric_field_does_not_close_the_search() {
    let fixture = tempfile::tempdir().unwrap();
    write_amount_records(fixture.path(), 24, "MXN");
    let engine = index(fixture.path());

    let described = engine
        .ask("¿Cuál es el upkeep amount de mantenimiento?")
        .unwrap();
    assert_eq!(
        described.text,
        "Upkeep amount — 24 valores\n\n\
         | Mínimo | Máximo | Total |\n\
         |---|---|---|\n\
         | $1,000 MXN | $3,300 MXN | $51,600 MXN |"
    );
    assert!(described.verified);

    // Un valor real que no existe sí debe seguir cerrando la búsqueda: no es
    // el mismo caso, es justo lo que este mecanismo protege.
    let missing_value = engine
        .ask("Busca documentos cuyo upkeep amount sea $9,999,999 MXN")
        .unwrap();
    assert!(missing_value.citations.is_empty());
}

/// Pregunta por identificador más campo: el campo pedido no está entre la
/// evidencia que la búsqueda devolvió (toda ella habla del identificador), así
/// que se resuelve el documento principal y se consulta ese campo en él.
#[test]
fn an_identifier_plus_a_field_answers_the_field_and_never_the_identifier() {
    let fixture = tempfile::tempdir().unwrap();
    write_cross_referenced_record(fixture.path());
    let engine = index(fixture.path());

    // La búsqueda por sí sola sólo encuentra dónde se menciona el
    // identificador: ninguna de esas evidencias contiene el campo preguntado.
    let retrieved = engine.search("¿Cuál es el condition de ZX-9001?").unwrap();
    assert_eq!(retrieved.len(), 3);
    assert!(
        retrieved
            .iter()
            .all(|hit| hit.evidence.value.as_deref() == Some("ZX-9001"))
    );

    let answer = engine.ask("¿Cuál es el condition de ZX-9001?").unwrap();
    assert_eq!(
        answer.text,
        "El campo «Condition» de ZX-9001 es Sealed (record-alpha.txt, línea 3)."
    );
    assert!(answer.verified);
    // El documento principal es la ficha, no el contrato que sólo lo menciona
    // ni el listado que menciona muchos identificadores.
    assert!(answer.citations[0].path.ends_with("record-alpha.txt"));
    assert_supported(&answer, &["Condition", "Sealed", "ZX-9001"]);
}

/// Sin un documento principal claro no se fuerza una respuesta.
#[test]
fn a_real_ambiguity_is_left_unresolved_instead_of_guessed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("twin-a.txt"),
        "Registry code: QQ-5000\nCondition: Sealed\n",
    )
    .unwrap();
    fs::write(
        root.join("twin-b.txt"),
        "Registry code: QQ-5000\nCondition: Open\n",
    )
    .unwrap();
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el condition de QQ-5000?").unwrap();
    assert!(
        answer.text.starts_with("Sin concluir:"),
        "texto inesperado: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "una duda no puede marcarse como verificada"
    );
    assert!(!answer.text.contains("Sealed") && !answer.text.contains("Open"));
    // La evidencia encontrada se sigue mostrando aunque no se pueda concluir.
    assert_eq!(answer.citations.len(), 2);
    assert_supported(&answer, &["QQ-5000"]);
}

/// Una búsqueda simple por identificador no pide ningún campo: no debe
/// activarse la consulta adicional ni convertirse en "Sin concluir".
#[test]
fn a_plain_identifier_search_keeps_its_previous_behaviour() {
    let fixture = tempfile::tempdir().unwrap();
    write_cross_referenced_record(fixture.path());
    let engine = index(fixture.path());

    let retrieved = engine.search("Encuentra ZX-9001").unwrap();
    let answer = engine.ask("Encuentra ZX-9001").unwrap();

    assert!(!answer.text.contains("Sin concluir"));
    assert!(answer.verified);
    assert_eq!(
        answer.text,
        "Encontré 3 valores de «Registry code», todos «ZX-9001»."
    );
    // La evidencia es exactamente la que la búsqueda encuentra por su cuenta:
    // la síntesis no añadió, quitó ni reordenó documentos.
    assert_eq!(
        answer
            .citations
            .iter()
            .map(|evidence| evidence.id.clone())
            .collect::<Vec<_>>(),
        retrieved
            .iter()
            .map(|hit| hit.evidence.id.clone())
            .collect::<Vec<_>>()
    );
}

/// Cuando la pregunta no describe un patrón sintetizable, se conserva el
/// mensaje genérico con la evidencia intacta.
#[test]
fn heterogeneous_evidence_keeps_the_generic_message() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("free-a.txt"), "shared marker phrase in prose\n").unwrap();
    fs::write(root.join("free-b.txt"), "another shared marker phrase\n").unwrap();
    let engine = index(root);

    let answer = engine.ask("menciona shared marker phrase").unwrap();
    assert_eq!(answer.text, "2 resultados con evidencia específica.");
    assert_eq!(answer.citations.len(), 2);
}

/// La palabra que sólo nombra el TIPO de la entidad ("la propiedad X") no puede
/// resolver un campo. Antes, "¿cuál es el color de X?" respondía el «Estado del
/// activo» porque era el único campo que contenía la palabra "activo".
#[test]
fn a_type_word_next_to_the_identifier_never_resolves_a_field() {
    let fixture = tempfile::tempdir().unwrap();
    write_typed_entity_record(fixture.path());
    let engine = index(fixture.path());

    let invented = engine.ask("¿Cuál es el color del activo ZX-9001?").unwrap();
    assert_eq!(invented.text, "2 resultados con evidencia específica.");
    assert!(!invented.text.contains("Sellado"));

    // El mismo documento y la misma forma de pregunta sí responden cuando el
    // campo se nombra de verdad: el filtro no apaga la resolución legítima.
    let asked = engine
        .ask("¿Cuál es el estado del activo ZX-9001?")
        .unwrap();
    assert_eq!(
        asked.text,
        "El campo «Estado del activo» de ZX-9001 es Sellado (ficha.txt, línea 2)."
    );
    assert_supported(&asked, &["Estado del activo", "Sellado", "ZX-9001"]);
}

/// "Resume X" es una intención propia: lista los campos del registro principal
/// en vez de elegir uno solo.
#[test]
fn a_summary_request_lists_the_fields_of_the_principal_record() {
    let fixture = tempfile::tempdir().unwrap();
    write_typed_entity_record(fixture.path());
    let engine = index(fixture.path());

    let answer = engine.ask("Resume el activo ZX-9001").unwrap();
    assert_eq!(
        answer.text,
        "Resumen de ZX-9001 — ficha.txt\n\n\
         - Clave del activo: ZX-9001\n\
         - Estado del activo: Sellado\n\
         - Costo del activo: $500 MXN"
    );
    assert!(answer.verified);
    assert_supported(
        &answer,
        &[
            "Clave del activo",
            "Estado del activo",
            "Sellado",
            "Costo del activo",
            "$500 MXN",
        ],
    );
}

/// Una palabra que aparece por casualidad en varios nombres de campo de
/// documentos distintos no es una pregunta por un campo: no puede terminar en
/// "Sin concluir".
#[test]
fn an_ambiguous_global_vocabulary_never_produces_an_unresolved_answer() {
    let fixture = tempfile::tempdir().unwrap();
    write_related_documents(fixture.path());
    let engine = index(fixture.path());

    // Intención dedicada: la pregunta habla del acervo, no de un campo.
    let related = engine
        .ask("¿Cuáles son todos los documentos relacionados con QQ-7000?")
        .unwrap();
    assert_eq!(
        related.text,
        "QQ-7000 aparece en 3 documentos:\n\n\
         1. ficha.txt — Clave del activo\n\
         2. finanza-a.txt — Activo relacionado\n\
         3. finanza-b.txt — Cliente relacionado"
    );
    assert!(related.verified);
    assert_eq!(related.citations.len(), 3);
    assert_supported(&related, &["QQ-7000", "Activo relacionado"]);

    // Sin la palabra que nombra al continente sigue siendo un empate del
    // vocabulario global: cae al mensaje genérico, nunca a "Sin concluir".
    let ambiguous = engine.ask("¿Cuál es el relacionado de QQ-7000?").unwrap();
    assert_eq!(ambiguous.text, "3 resultados con evidencia específica.");
    assert!(!ambiguous.text.contains("Sin concluir"));
}

/// Un solo documento que escribe el campo de otra forma no puede descartar el
/// lote entero: el encabezado "estado" de un CSV frente a "Estado de la
/// propiedad" de las fichas.
#[test]
fn one_outlier_field_name_does_not_discard_the_majority() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (number, state) in [
        "Disponible",
        "Disponible",
        "Vendida",
        "Apartada",
        "Vendida",
        "Disponible",
        "Apartada",
        "Vendida",
    ]
    .iter()
    .enumerate()
    {
        fs::write(
            root.join(format!("estado-{}.txt", number + 1)),
            format!(
                "Estado de la propiedad: {state}
"
            ),
        )
        .unwrap();
    }
    fs::write(
        root.join("inventario.csv"),
        "estado,codigo
Rentada,INV-1
",
    )
    .unwrap();
    let engine = index(root);

    let answer = engine
        .ask("¿Cuáles son los estados de las propiedades?")
        .unwrap();
    assert_eq!(
        answer.text,
        "Estado de la propiedad — 8 valores, 3 valores distintos\n\n\
         Disponible · Vendida · Apartada"
    );
    assert!(answer.verified);
    // El texto habla del grupo mayoritario, pero la evidencia mostrada sigue
    // siendo toda la que la búsqueda encontró, el CSV incluido.
    assert_eq!(answer.citations.len(), 9);
    assert!(
        answer
            .citations
            .iter()
            .any(|evidence| evidence.path.ends_with("inventario.csv"))
    );
    assert_supported(&answer, &["Disponible", "Vendida", "Apartada"]);
}

/// El campo que la pregunta nombra explícitamente se elige aunque sea
/// minoritario frente a otro campo que sólo lo referencia con más frecuencia.
/// Antes se elegía el grupo más grande sin comprobar si era el pedido; aquí el
/// campo minoritario («Tipo de activo», 4) es justo el que la pregunta nombra,
/// frente a la referencia cruzada mayoritaria («Activo», 12).
#[test]
fn the_named_field_wins_even_when_it_is_the_minority_group() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (number, kind) in ["Bodega", "Casa", "Bodega", "Local"].iter().enumerate() {
        fs::write(
            root.join(format!("ficha-{number}.txt")),
            format!(
                "Tipo de activo: {kind}
"
            ),
        )
        .unwrap();
    }
    for number in 0..12 {
        fs::write(
            root.join(format!("referencia-{number}.txt")),
            format!(
                "Activo: REF-{number:03}
"
            ),
        )
        .unwrap();
    }
    let engine = index(root);

    let answer = engine.ask("¿Cuáles son los tipos de activo?").unwrap();
    assert_eq!(
        answer.text,
        "Tipo de activo — 4 valores, 3 valores distintos\n\nBodega · Casa · Local"
    );
    assert!(answer.verified);
    // La evidencia mostrada sigue siendo todo lo que la búsqueda encontró: 4
    // fichas más 12 referencias.
    assert_eq!(answer.citations.len(), 16);
    assert_supported(&answer, &["Bodega", "Casa", "Local"]);
}

/// Un grupo de un solo valor dentro de una fila por moneda sigue rindiendo una
/// tabla bien formada: la columna "Valores" es un conteo, no una frase, así
/// que no hay concordancia de número que romper.
#[test]
fn a_single_value_group_still_renders_a_well_formed_row() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("a.txt"),
        "Importe: $100 MXN
",
    )
    .unwrap();
    fs::write(
        root.join("b.txt"),
        "Importe: $200 MXN
",
    )
    .unwrap();
    fs::write(
        root.join("c.txt"),
        "Importe: $50 USD
",
    )
    .unwrap();
    let engine = index(root);

    let answer = engine.ask("¿Cuál es el importe?").unwrap();
    assert_eq!(
        answer.text,
        "Importe — 3 valores\n\n\
         | Moneda | Valores | Mínimo | Máximo | Total |\n\
         |---|---|---|---|---|\n\
         | MXN | 2 | $100 MXN | $200 MXN | $300 MXN |\n\
         | USD | 1 | $50 USD | $50 USD | $50 USD |\n\n\
         Los totales no se combinan entre monedas distintas."
    );
    assert!(answer.verified);
}

fn write_typed_entity_record(root: &std::path::Path) {
    fs::write(
        root.join("ficha.txt"),
        "Clave del activo: ZX-9001\nEstado del activo: Sellado\nCosto del activo: $500 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("contrato.txt"),
        "Tipo de acuerdo: Arrendamiento\nActivo referido: ZX-9001\n",
    )
    .unwrap();
}

fn write_related_documents(root: &std::path::Path) {
    // Dos campos distintos contienen la palabra "relacionado": ninguno es el
    // que la pregunta pide, sólo comparten una palabra con ella.
    fs::write(
        root.join("ficha.txt"),
        "Clave del activo: QQ-7000\nEstado del activo: Abierto\n",
    )
    .unwrap();
    fs::write(
        root.join("finanza-a.txt"),
        "Activo relacionado: QQ-7000\nImporte: $10 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("finanza-b.txt"),
        "Cliente relacionado: QQ-7000\nImporte: $20 MXN\n",
    )
    .unwrap();
}

fn index(root: &std::path::Path) -> OmegaEngine {
    // La base vive en el mismo directorio temporal que la fixture, de modo que
    // cada prueba parte de un índice vacío.
    let engine = OmegaEngine::open(root.join("omega-test.db")).unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();
    engine
}

fn write_amount_records(root: &std::path::Path, count: usize, currency: &str) {
    for number in 0..count {
        let amount = 1_000 + number * 100;
        fs::write(
            root.join(format!("record-{number:03}.txt")),
            format!(
                "Record: RC-{number:03}\nUpkeep amount: ${},{:03} {currency}\n",
                amount / 1_000,
                amount % 1_000
            ),
        )
        .unwrap();
    }
}

fn write_cross_referenced_record(root: &std::path::Path) {
    // El identificador encabeza su propia ficha, aparece a media altura en un
    // documento que sólo lo referencia y es una fila más en un listado.
    fs::write(
        root.join("record-alpha.txt"),
        "Registry code: ZX-9001\nListed amount: $2,500 MXN\nCondition: Sealed\n",
    )
    .unwrap();
    fs::write(
        root.join("contract.txt"),
        "Agreement type: Lease\nParty: North Group\nRegistry code: ZX-9001\nSettled amount: $900 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("inventory.csv"),
        "Registry code,Condition,Listed amount\n\
         AA-1000,Open,$100 MXN\n\
         BB-2000,Open,$200 MXN\n\
         CC-3000,Sealed,$300 MXN\n\
         ZX-9001,Open,$400 MXN\n",
    )
    .unwrap();
}

/// Ningún valor del texto sintetizado puede faltar en la evidencia citada. Es
/// el mismo candado que verifica una respuesta del modelo.
fn assert_supported(answer: &Answer, values: &[&str]) {
    let citations = answer.citations.iter().collect::<Vec<_>>();
    for value in values {
        assert!(
            answer.text.contains(value),
            "'{value}' no aparece en el texto de la respuesta"
        );
        assert!(
            value_is_supported(&citations, value),
            "'{value}' no está respaldado por la evidencia citada"
        );
    }
}
