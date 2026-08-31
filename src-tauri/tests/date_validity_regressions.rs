//! P1-B.2 — Una fecha imposible no es una fecha.
//!
//! `2024-02-31` tiene forma de fecha y no existe. Aceptarla la convertía en un
//! valor de tipo fecha con `date_value` propio, y a partir de ahí actuaba como
//! una fecha válida: entraba en los rangos de febrero por simple orden
//! lexicográfico y sumaba un documento a un periodo al que no pertenece.
//!
//! Fixtures genéricas: facturas con fecha de emisión e importe.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Días que no existen en su mes, en las dos formas que el motor admite.
#[test]
fn impossible_days_are_rejected_in_every_supported_format() {
    for impossible in [
        "2024-02-30",
        "2024-02-31",
        "2024-04-31",
        "2024-06-31",
        "2024-09-31",
        "2024-11-31",
        "2023-02-29",
        "1900-02-29",
        "2024-01-32",
        "2024-01-00",
        "2024-00-15",
        "2024-13-01",
    ] {
        assert!(
            !is_date(impossible),
            "{impossible} no existe y no puede clasificarse como fecha"
        );
    }
    for impossible in [
        "31 de febrero de 2024",
        "30 de febrero de 2024",
        "31 de abril de 2024",
        "29 de febrero de 2023",
    ] {
        assert!(
            !is_date(impossible),
            "«{impossible}» no existe y no puede clasificarse como fecha"
        );
    }
}

/// Los límites que sí existen siguen siendo fechas, incluidos los bisiestos
/// por la regla de los siglos.
#[test]
fn real_boundaries_and_leap_years_stay_valid() {
    for valid in [
        "2024-02-29",
        "2000-02-29",
        "2023-02-28",
        "2024-01-31",
        "2024-04-30",
        "2024-12-31",
        "2024-01-01",
        "1900-02-28",
        "2200-12-31",
    ] {
        assert!(is_date(valid), "{valid} existe y debe clasificarse como fecha");
    }
    for valid in [
        "29 de febrero de 2024",
        "30 de abril de 2024",
        "31 de diciembre de 2024",
        "1 de enero de 2024",
    ] {
        assert!(
            is_date(valid),
            "«{valid}» existe y debe clasificarse como fecha"
        );
    }
}

/// Un año fuera del rango que el índice admite tampoco es una fecha.
#[test]
fn years_outside_the_supported_range_are_not_dates() {
    assert!(!is_date("1899-12-31"));
    assert!(!is_date("2201-01-01"));
    assert!(is_date("1900-01-01"));
}

/// El extremo del caso: una fecha imposible no puede colarse en el rango de
/// un mes. Sólo los dos documentos con fechas reales de febrero entran.
#[test]
fn an_impossible_date_never_acts_as_a_valid_date_filter() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, fecha, importe) in [
        ("001-factura.md", "2024-02-10", "$10.00 MXN"),
        ("002-factura.md", "2024-02-20", "$20.00 MXN"),
        // Imposible: febrero de 2024 tiene 29 días.
        ("003-factura.md", "2024-02-31", "$400.00 MXN"),
        ("004-factura.md", "2024-03-05", "$50.00 MXN"),
    ] {
        write_record(
            root,
            name,
            &[
                ("Folio", name),
                ("Fecha de emisión", fecha),
                ("Importe", importe),
            ],
        );
    }
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("fechas", "¿Cuánto suma el Importe en febrero de 2024?")
        .unwrap();

    assert!(
        answer.text.contains("$30.00 MXN"),
        "sólo 10 + 20 son de febrero de 2024: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("430"),
        "una fecha inexistente no puede sumar su importe al periodo: {}",
        answer.text
    );
    assert!(
        !answer
            .citations
            .iter()
            .any(|evidence| evidence.path.contains("003-factura")),
        "el documento de fecha imposible no puede citarse dentro del periodo"
    );

    // Un rango que abarca dos meses es donde el fallo era visible de verdad:
    // «2024-02-31» cae dentro de febrero-marzo por puro orden de cadenas.
    let spanning = engine
        .ask_in_conversation(
            "rango",
            "¿Cuánto suma el Importe entre 2024-02-01 y 2024-03-31?",
        )
        .unwrap();
    assert!(
        spanning.text.contains("$80.00 MXN"),
        "10 + 20 + 50 = 80; la fecha inexistente no pertenece a ningún rango: {}",
        spanning.text
    );
    assert!(!spanning.text.contains("480"), "{}", spanning.text);

    // Y el valor imposible sigue siendo visible en el acervo: no se borra, se
    // deja de tratar como fecha.
    let hits = engine.search("2024-02-31").unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.evidence.path.contains("003-factura")),
        "el valor imposible sigue existiendo como evidencia literal"
    );
}

/// El campo se sigue tipando por lo que el acervo contiene: tres fechas
/// reales y una imposible dejan el campo como fecha, y el valor imposible
/// queda fuera del filtro en vez de arrastrar el tipo consigo.
#[test]
fn one_impossible_value_does_not_untype_a_real_date_field() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, fecha) in [
        ("001-factura.md", "2024-02-10"),
        ("002-factura.md", "2024-02-20"),
        ("003-factura.md", "2024-03-05"),
        ("004-factura.md", "2024-02-31"),
    ] {
        write_record(root, name, &[("Folio", name), ("Fecha de emisión", fecha)]);
    }
    let engine = index(root);
    let concept = engine
        .concepts(Some("Fecha de emisión"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.display_name == "Fecha de emisión")
        .expect("el concepto debe existir");
    assert_eq!(concept.value_type, "date");
}

// ─────────────────────────────────────────────────────────────────────────

/// Comprueba la clasificación a través del índice real: un valor sólo es
/// fecha si el motor le asigna el tipo `date`.
fn is_date(value: &str) -> bool {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "001-registro.md", &[("Fecha de emisión", value)]);
    let engine = index(root);
    engine
        .concepts(Some("Fecha de emisión"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.display_name == "Fecha de emisión")
        .expect("el concepto debe existir")
        .value_type
        == "date"
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-fechas.db"), Clock::fixed(TODAY).unwrap())
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
