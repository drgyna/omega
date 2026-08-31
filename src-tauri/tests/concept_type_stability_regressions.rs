//! P1-B.1 — El tipo de un concepto no puede depender de qué archivo se indexó
//! primero.
//!
//! Un campo cuyo primer registro es un marcador de ausencia (`N/D`) quedaba
//! clasificado como texto para siempre, y con él todo el campo dejaba de poder
//! sumarse aunque el resto del acervo trajera importes reales. El tipo tiene
//! que salir de lo que el acervo contiene, no del orden alfabético de los
//! nombres de archivo.
//!
//! Fixtures genéricas: facturas con folio e importe.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// El marcador de ausencia llega primero en el orden de indexación. El campo
/// sigue siendo dinero: se calcula con los importes reales y el valor no
/// numérico queda excluido y contado, nunca convierte el campo en texto.
#[test]
fn a_placeholder_in_the_first_file_never_freezes_the_field_as_text() {
    let answer = sum_of_importe(&[
        ("001-factura.md", "N/D"),
        ("002-factura.md", "$25.00 MXN"),
        ("003-factura.md", "$35.00 MXN"),
    ]);
    assert_computed_or_clarified(&answer);
}

/// Misma fixture, orden inverso: el marcador de ausencia llega al final. La
/// respuesta tiene que ser exactamente la misma.
#[test]
fn the_same_archive_indexed_in_reverse_order_gives_the_same_answer() {
    let answer = sum_of_importe(&[
        ("001-factura.md", "$25.00 MXN"),
        ("002-factura.md", "$35.00 MXN"),
        ("003-factura.md", "N/D"),
    ]);
    assert_computed_or_clarified(&answer);
}

/// Las seis permutaciones del mismo acervo producen el mismo tipo de concepto
/// y el mismo texto de respuesta. El orden de indexación no es un dato.
#[test]
fn every_permutation_of_the_same_archive_types_the_field_the_same_way() {
    let values = ["N/D", "$25.00 MXN", "$35.00 MXN"];
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut answers = Vec::new();
    let mut types = Vec::new();
    for order in permutations {
        let files = order
            .iter()
            .enumerate()
            .map(|(position, value)| {
                (format!("{:03}-factura.md", position + 1), values[*value])
            })
            .collect::<Vec<_>>();
        let borrowed = files
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect::<Vec<_>>();
        let (engine, answer) = sum_with_engine(&borrowed);
        let importe = engine
            .concepts(Some("Importe"))
            .unwrap()
            .into_iter()
            .find(|concept| concept.display_name == "Importe")
            .expect("el concepto Importe debe existir");
        types.push(importe.value_type);
        answers.push(answer.text);
    }
    types.dedup();
    answers.dedup();
    assert_eq!(
        types.len(),
        1,
        "el tipo del concepto cambió con el orden de los archivos: {types:?}"
    );
    assert_eq!(types[0], "money");
    assert_eq!(
        answers.len(),
        1,
        "la respuesta cambió con el orden de los archivos: {answers:?}"
    );
}

/// El contrapeso: un campo que de verdad es texto no se convierte en numérico
/// porque uno solo de sus valores parezca un número. La mayoría manda.
#[test]
fn a_single_numeric_looking_value_never_retypes_a_text_field() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, referencia) in [
        ("001-contrato.md", "ABC-100"),
        ("002-contrato.md", "ABD-200"),
        ("003-contrato.md", "ABE-300"),
        ("004-contrato.md", "500"),
    ] {
        write_record(root, name, &[("Folio", name), ("Referencia", referencia)]);
    }
    let engine = index(root);
    let referencia = engine
        .concepts(Some("Referencia"))
        .unwrap()
        .into_iter()
        .find(|concept| concept.display_name == "Referencia")
        .expect("el concepto Referencia debe existir");
    assert_eq!(
        referencia.value_type, "text",
        "tres de cuatro valores son texto: el campo es texto"
    );
}

/// Reindexar después de quitar los importes devuelve el campo a texto: el
/// tipo describe lo que el acervo contiene ahora, no lo que contuvo antes.
#[test]
fn removing_every_numeric_value_retypes_the_field_back() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "001-factura.md", &[("Importe", "$25.00 MXN")]);
    write_record(root, "002-factura.md", &[("Importe", "N/D")]);
    let engine = index(root);
    assert_eq!(concept_type(&engine, "Importe"), "money");

    write_record(root, "001-factura.md", &[("Importe", "N/D")]);
    let source = engine.sources().unwrap()[0].id;
    engine.index_source(source).unwrap();
    assert_eq!(
        concept_type(&engine, "Importe"),
        "text",
        "ya no queda ningún importe real en el acervo"
    );
}

// ─────────────────────────────────────────────────────────────────────────

/// El campo tiene que quedar utilizable: o el motor calcula con los valores
/// reales y declara la exclusión del marcador de ausencia, o pregunta. Lo que
/// no puede hacer es negar que el campo sea numérico.
fn assert_computed_or_clarified(answer: &omega_core::Answer) {
    if answer.clarification.is_some() {
        return;
    }
    assert!(
        !answer.text.contains("no es un campo numérico"),
        "un campo con importes reales no puede declararse no numérico: {}",
        answer.text
    );
    assert!(
        answer.text.contains("$60.00 MXN"),
        "25 + 35 = 60, con el marcador de ausencia excluido: {}",
        answer.text
    );
    let scope = answer.scope.clone().expect("la respuesta declara su alcance");
    assert_eq!(scope.value_count, Some(2));
    assert_eq!(
        scope.excluded_count,
        Some(1),
        "el valor no numérico se excluye y se cuenta, nunca se ignora en silencio"
    );
}

fn sum_of_importe(files: &[(&str, &str)]) -> omega_core::Answer {
    sum_with_engine(files).1
}

fn sum_with_engine(files: &[(&str, &str)]) -> (OmegaEngine, omega_core::Answer) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, importe) in files {
        write_record(root, name, &[("Folio", name), ("Importe", importe)]);
    }
    let engine = index(root);
    let answer = engine
        .ask_in_conversation("tipos", "¿Cuánto suma el Importe?")
        .unwrap();
    // `fixture` se conserva vivo mientras dure el motor.
    std::mem::forget(fixture);
    (engine, answer)
}

fn concept_type(engine: &OmegaEngine, name: &str) -> String {
    engine
        .concepts(Some(name))
        .unwrap()
        .into_iter()
        .find(|concept| concept.display_name == name)
        .unwrap_or_else(|| panic!("el concepto {name} debe existir"))
        .value_type
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-tipos.db"), Clock::fixed(TODAY).unwrap())
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
