//! Un error de escritura nunca puede producir una respuesta segura de más.
//!
//! Cuando el error de dedo cae en una palabra de contenido —no en un acento,
//! que `normalize_spanish` ya pliega— el motor tiene tres salidas posibles y
//! sólo una es aceptable:
//!
//! * abstenerse diciendo qué criterio no supo aplicar (correcta);
//! * **descartar el criterio en silencio** y contestar un conteo más amplio,
//!   sellado como verificado;
//! * **resolver a un campo distinto** del que la pregunta nombra, también
//!   sellado.
//!
//! Las dos últimas son peores que no contestar: afirman con confianza algo
//! que nadie preguntó. Este archivo fija que las tres formas caen en la
//! primera, sin tolerar todavía ningún error de escritura: el objetivo aquí es
//! sólo que la salida insegura desaparezca.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-09-05";

/// El caso que YA funcionaba y que no puede cambiar: con el verbo bien
/// escrito, el criterio mal escrito se denuncia y no se cuenta nada.
#[test]
fn a_misspelled_criterion_next_to_a_readable_verb_still_abstains() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-1");

    let answer = engine.ask("¿Cuántos expedientes están en apelasión?").unwrap();

    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("No apliqué"), "{}", answer.text);
}

/// El defecto: sin un verbo copulativo reconocible —basta una letra de menos,
/// «stan» por «están»— no había dónde cortar sujeto y predicado, la
/// salvaguarda no llegaba a mirar nada y el conteo salía con un criterio
/// menos, sellado como verificado.
#[test]
fn a_misspelled_verb_no_longer_drops_the_criterion_in_silence() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-2");

    let answer = engine.ask("Cuantos expedientes stan en apelasion?").unwrap();

    assert!(
        !answer.verified,
        "un conteo al que le falta un criterio no puede ir sellado: {}",
        answer.text
    );
    assert!(answer.text.starts_with("No apliqué"), "{}", answer.text);
    assert!(
        !answer.text.contains(" documentos cumplen"),
        "no puede dar el conteo de los criterios restantes: {}",
        answer.text
    );
}

/// La pregunta bien escrita sigue contando, con su criterio aplicado: la
/// salvaguarda nueva no puede apagar una pregunta que sí se entendió.
#[test]
fn the_same_question_written_correctly_still_counts() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-3");

    let answer = engine.ask("¿Cuántos expedientes están en apelación?").unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("2 documentos"), "{}", answer.text);
}

/// El sujeto puede ir DETRÁS de una palabra que no aterriza en nada
/// («carpeta») sin que eso sea un criterio perdido: lo que se mira es lo que
/// viene después del sujeto, no la pregunta entera.
#[test]
fn a_word_before_the_subject_is_not_a_lost_criterion() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-4");

    let answer = engine
        .ask("¿Cuántos documentos hay en la carpeta expedientes?")
        .unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("3 documentos"), "{}", answer.text);
}

/// El otro defecto: la pregunta pide un campo cuyo nombre trae una letra
/// cambiada, no se reconoce ninguno, y la única evidencia encontrada es el
/// propio folio que el usuario tecleó. Devolvérselo sellado es contestar otra
/// pregunta.
#[test]
fn a_misspelled_field_never_answers_with_the_typed_identifier() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-5");

    let answer = engine
        .ask("Del expediente EXP-26-0001, cual fue la conclucion?")
        .unwrap();

    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("Sin concluir"), "{}", answer.text);
    assert!(
        answer.text.contains("conclucion"),
        "la respuesta debe decir qué palabra no reconoció: {}",
        answer.text
    );
}

/// Y la misma pregunta bien escrita sigue contestando el campo pedido.
#[test]
fn the_same_field_question_written_correctly_still_answers() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-6");

    let answer = engine
        .ask("Del expediente EXP-26-0001, ¿cuál fue la conclusión?")
        .unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.contains("Desfavorable"), "{}", answer.text);
}

fn index(root: &Path, name: &str) -> OmegaEngine {
    let expedientes = root.join("expedientes");
    fs::create_dir_all(&expedientes).unwrap();
    let filas = [
        ("EXP-26-0001", "En apelación", "Desfavorable"),
        ("EXP-26-0002", "En apelación", "Favorable"),
        ("EXP-26-0003", "En trámite", "Favorable"),
    ];
    for (folio, estado, conclusion) in filas {
        fs::write(
            expedientes.join(format!("{folio}.md")),
            format!("Folio: {folio}\nEstado: {estado}\nConclusión: {conclusion}\n"),
        )
        .unwrap();
    }
    let engine = OmegaEngine::open_with_clock(
        root.join(format!("omega-{name}.db")),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
