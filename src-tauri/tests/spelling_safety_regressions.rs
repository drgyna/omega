//! Errores de escritura reales —letras de más, de menos, cambiadas o
//! intercambiadas— en las palabras que nombran un campo o un criterio.
//!
//! Los acentos ya no son problema: `normalize_spanish` los pliega. El problema
//! era el error de dedo, y tenía tres salidas posibles de las que sólo una era
//! aceptable:
//!
//! * abstenerse diciendo qué criterio no supo aplicar;
//! * **descartar el criterio en silencio** y contestar un conteo más amplio,
//!   sellado como verificado;
//! * **resolver a un campo distinto** del que la pregunta nombra, también
//!   sellado.
//!
//! Este archivo fija las dos cosas que se pidieron, en este orden: que las dos
//! salidas inseguras no existan (fase 1) y que la mayoría de las erratas
//! reales se resuelvan bien en vez de conformarse con la abstención (fase 2).

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-09-05";

/// Una errata en el valor del criterio no impide aplicarlo: la pregunta
/// cuenta lo mismo que si estuviera bien escrita.
#[test]
fn a_misspelled_criterion_is_applied_like_the_correct_one() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-1");

    let correcta = engine.ask("¿Cuántos expedientes están en apelación?").unwrap();
    let con_errata = engine.ask("¿Cuántos expedientes están en apelasión?").unwrap();

    assert!(correcta.text.starts_with("2 documentos"), "{}", correcta.text);
    assert_eq!(con_errata.text, correcta.text);
    assert!(con_errata.verified, "{}", con_errata.text);
}

/// La errata puede caer en el verbo. «stan» por «están» dejaba a la pregunta
/// sin corte entre sujeto y predicado: el criterio desaparecía en silencio y
/// el conteo salía más amplio y sellado.
#[test]
fn a_misspelled_verb_no_longer_drops_the_criterion() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-2");

    let answer = engine.ask("Cuantos expedientes stan en apelasion?").unwrap();

    assert!(answer.text.starts_with("2 documentos"), "{}", answer.text);
    assert!(answer.verified, "{}", answer.text);
    assert!(
        answer.text.contains("En apelación"),
        "el criterio tiene que aparecer aplicado: {}",
        answer.text
    );
}

/// Y puede caer en el nombre del campo pedido. Antes se contestaba con el
/// único campo encontrado —el propio folio tecleado— y sellado.
#[test]
fn a_misspelled_field_answers_the_field_it_names() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-3");

    let answer = engine
        .ask("Del expediente EXP-26-0001, cual fue la conclucion?")
        .unwrap();

    assert!(answer.text.contains("Conclusión"), "{}", answer.text);
    assert!(answer.text.contains("Desfavorable"), "{}", answer.text);
    assert!(answer.verified, "{}", answer.text);
}

/// Las cuatro formas del error de dedo sobre la misma palabra, para que la
/// tolerancia no quede fijada a la errata concreta del encargo.
#[test]
fn the_four_shapes_of_a_typo_resolve_the_same_field() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-4");

    for pregunta in [
        "Del expediente EXP-26-0001, ¿cuál fue la conclusiónn?", // letra de más
        "Del expediente EXP-26-0001, ¿cuál fue la conclsión?",   // letra de menos
        "Del expediente EXP-26-0001, ¿cuál fue la conclusiin?",  // letra cambiada
        "Del expediente EXP-26-0001, ¿cuál fue la conclusino?",  // transpuestas
    ] {
        let answer = engine.ask(pregunta).unwrap();
        assert!(
            answer.text.contains("Desfavorable") && answer.verified,
            "{pregunta} -> {}",
            answer.text
        );
    }
}

/// La vara de confianza no baja: una errata que queda igual de cerca de dos
/// campos distintos no elige ninguno.
#[test]
fn a_typo_that_fits_two_fields_equally_well_never_picks_one() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("polizas")).unwrap();
    fs::write(
        root.join("polizas/POL-1.md"),
        "Folio: POL-1\nVigencia: 2026-01-01\nUrgencia: Alta\n",
    )
    .unwrap();
    let engine = open(root, "ortografia-5");

    // «vrgencia» está a una letra de «vigencia» y a una letra de «urgencia».
    let answer = engine
        .ask("De la póliza POL-1, ¿cuál es la vrgencia?")
        .unwrap();

    assert!(
        !answer.text.contains("2026-01-01") && !answer.text.contains("Alta"),
        "no puede elegir uno de los dos campos: {}",
        answer.text
    );
    assert!(!answer.verified, "{}", answer.text);
}

/// Lo que la tolerancia no alcanza sigue cayendo en la abstención segura, no
/// en un conteo más amplio: la salvaguarda de la fase 1 no se desarma.
#[test]
fn a_typo_too_large_to_resolve_still_abstains_instead_of_counting() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-6");

    let answer = engine.ask("¿Cuántos expedientes están en apelasón?").unwrap();

    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("No apliqué"), "{}", answer.text);
    assert!(
        !answer.text.contains(" documentos cumplen"),
        "no puede dar el conteo de los criterios restantes: {}",
        answer.text
    );
}

/// Y un campo que no se parece a ninguno tampoco se contesta con el folio que
/// el usuario acaba de teclear.
#[test]
fn an_unrecognizable_field_never_answers_with_the_typed_identifier() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-7");

    let answer = engine
        .ask("Del expediente EXP-26-0001, cual fue la resolusion?")
        .unwrap();

    assert!(!answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("Sin concluir"), "{}", answer.text);
    assert!(
        answer.text.contains("resolusion"),
        "la respuesta debe decir qué palabra no reconoció: {}",
        answer.text
    );
}

/// El sujeto puede ir DETRÁS de una palabra que no aterriza en nada
/// («carpeta») sin que eso sea un criterio perdido: lo que se mira es lo que
/// viene después del sujeto, no la pregunta entera.
#[test]
fn a_word_before_the_subject_is_not_a_lost_criterion() {
    let fixture = tempfile::tempdir().unwrap();
    let engine = index(fixture.path(), "ortografia-8");

    let answer = engine
        .ask("¿Cuántos documentos hay en la carpeta expedientes?")
        .unwrap();

    assert!(answer.verified, "{}", answer.text);
    assert!(answer.text.starts_with("3 documentos"), "{}", answer.text);
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
    open(root, name)
}

fn open(root: &Path, name: &str) -> OmegaEngine {
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
