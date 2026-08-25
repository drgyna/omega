//! Parser de pares «Campo: valor» escritos literalmente en la pregunta:
//! valores largos, con «y», con acentos, con guiones, con paréntesis, y con
//! dos puntos internos («10:30»), además de varios pares en una misma
//! pregunta.
//!
//! Las fixtures son genéricas y se escriben en un directorio temporal
//! (`tempfile`). No proceden de ningún corpus del repositorio.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Un valor que contiene la palabra «y» no puede cortarse ahí: el par debe
/// conservarse completo hasta el final de la pregunta.
#[test]
fn a_value_containing_the_word_y_is_never_cut_there() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[
            ("Folio", "RG-01"),
            (
                "Campo textual",
                "Evaluación de seguridad y notificación al responsable",
            ),
        ],
    );
    write_record(
        root,
        "doc-02.md",
        &[("Folio", "RG-02"), ("Campo textual", "Evaluación de seguridad")],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Cuántos documentos tienen Campo textual: Evaluación de seguridad y notificación al responsable?",
        )
        .unwrap();

    // Sólo el primer documento tiene el valor completo; si el valor se
    // hubiera cortado en "y", el filtro habría emparejado también al
    // segundo (o habría quedado como "Evaluación de seguridad y", que no
    // existe en ningún documento).
    assert!(answer.text.contains('1'), "{}", answer.text);
    assert!(
        !answer.text.contains("2 documentos"),
        "el valor no debe truncarse: {}",
        answer.text
    );
}

/// Un valor con dos puntos internos, como una hora, no puede truncarse en el
/// primer segmento («10:30» no puede convertirse en «10»).
#[test]
fn a_time_like_value_with_an_internal_colon_is_not_truncated() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(root, "doc-01.md", &[("Folio", "RG-01"), ("Hora", "10:30")]);
    write_record(root, "doc-02.md", &[("Folio", "RG-02"), ("Hora", "10:45")]);
    let engine = index(root);

    let answer = engine
        .ask_in_conversation("c1", "¿Cuántos documentos tienen Hora: 10:30?")
        .unwrap();

    assert!(answer.text.contains('1'), "{}", answer.text);
    assert!(
        !answer.text.contains("2 documentos"),
        "«10:30» truncado a «10» empataría con ambos documentos: {}",
        answer.text
    );
}

/// Acentos, guiones y paréntesis en un valor escrito deben conservarse
/// exactamente: no son separadores ni puntuación desechable.
#[test]
fn accents_hyphens_and_parentheses_survive_in_a_written_value() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[
            ("Folio", "RG-01"),
            ("Descripción", "Revisión técnica (fase inicial) - sitio Norte-Sur"),
        ],
    );
    write_record(
        root,
        "doc-02.md",
        &[("Folio", "RG-02"), ("Descripción", "Revisión técnica")],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Cuántos documentos tienen Descripción: Revisión técnica (fase inicial) - sitio Norte-Sur?",
        )
        .unwrap();

    assert!(answer.text.contains('1'), "{}", answer.text);
    assert!(
        !answer.text.contains("2 documentos"),
        "{}",
        answer.text
    );
}

/// Varios pares «Campo: valor» en la misma pregunta se resuelven todos, cada
/// uno con su propio valor completo.
#[test]
fn several_written_pairs_in_one_question_all_resolve() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_record(
        root,
        "doc-01.md",
        &[("Folio", "RG-01"), ("Zona", "Norte"), ("Estado", "Pendiente")],
    );
    write_record(
        root,
        "doc-02.md",
        &[("Folio", "RG-02"), ("Zona", "Norte"), ("Estado", "Pagada")],
    );
    write_record(
        root,
        "doc-03.md",
        &[("Folio", "RG-03"), ("Zona", "Sur"), ("Estado", "Pendiente")],
    );
    let engine = index(root);

    let answer = engine
        .ask_in_conversation(
            "c1",
            "¿Cuántos documentos tienen Zona: Norte y Estado: Pendiente?",
        )
        .unwrap();

    // Sólo doc-01 cumple ambos pares a la vez.
    assert!(answer.text.contains('1'), "{}", answer.text);
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
