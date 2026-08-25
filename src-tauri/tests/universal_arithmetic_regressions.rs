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
        answer.text.contains("dividían entre cero") || answer.text.contains("dividia entre cero"),
        "debe explicar por qué el segundo documento no participó: {}",
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
