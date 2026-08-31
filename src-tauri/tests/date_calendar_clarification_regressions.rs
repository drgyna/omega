//! Ronda 3 — «En el documento D#####, la fecha aparece como "A/B/AAAA". ¿A
//! qué fecha calendario corresponde?»
//!
//! El texto de la pregunta ya trae el dato completo (la cadena entre
//! comillas), así que resolverla no necesita leer el acervo: sólo el
//! calendario gregoriano dice si esa cadena tiene una lectura válida, dos
//! lecturas válidas y distintas (DD/MM frente a MM/DD), o ninguna. Sin este
//! arreglo, Omega resolvía el campo «Fecha» del documento localizado y
//! devolvía la cadena tal cual, como si no hubiera ambigüedad ni fecha
//! imposible.
//!
//! Deliberadamente NO cubre la fecha implausible por ser anterior a una
//! fecha de fundación de referencia: ese dato no está escrito en ningún
//! documento del acervo, así que Omega no puede afirmarlo sin inventar
//! conocimiento externo. Ese caso sigue su curso normal — el último test de
//! este archivo lo deja constancia.
//!
//! Fixtures genéricas: registros con nombre de archivo `NNNNN_algo.md`, el
//! mismo prefijo de cinco dígitos que usa el corpus real para `D#####`.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// «31/02/2025»: ni como 31 de febrero (no existe ese día) ni como el mes 31
/// (no existe ese mes) tiene una lectura válida.
#[test]
fn a_calendar_impossible_date_asks_for_clarification() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("01874_registro.md"),
        "# Registro\n\nFolio: REG-001\nFecha: 31/02/2025\n",
    )
    .unwrap();

    let engine = index(root);
    let answer = engine
        .ask("En el documento D01874, la fecha aparece como \"31/02/2025\". ¿A qué fecha calendario corresponde?")
        .unwrap();
    assert!(
        answer.clarification.is_some(),
        "una fecha imposible en cualquier lectura debe pedir aclaración: {}",
        answer.text
    );
    assert!(answer.text.contains("31/02/2025"));
    assert!(!answer.verified);
}

/// «07/01/2024»: como DD/MM es el 7 de enero; como MM/DD es el 1 de julio.
/// Ambas lecturas son fechas reales y distintas: no hay una sola respuesta.
#[test]
fn an_ambiguous_date_asks_for_clarification() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("02719_registro.md"),
        "# Registro\n\nFolio: REG-002\nFecha: 07/01/2024\n",
    )
    .unwrap();

    let engine = index(root);
    let answer = engine
        .ask("En el documento D02719, la fecha aparece como \"07/01/2024\". ¿A qué fecha calendario corresponde?")
        .unwrap();
    assert!(
        answer.clarification.is_some(),
        "dos lecturas de calendario válidas y distintas deben pedir aclaración: {}",
        answer.text
    );
    assert!(!answer.verified);
}

/// «28/07/1991»: sólo la lectura DD/MM existe (el mes 28 no existe), así que
/// no hay ambigüedad de calendario que señalar y esta pregunta NO se
/// intercepta como aclaración. Lo implausible aquí es que la fecha sea
/// anterior a una referencia que no está escrita en ningún documento, y Omega
/// no puede inventarla.
#[test]
fn a_single_valid_reading_is_not_intercepted_even_when_implausible() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("02238_registro.md"),
        "# Registro\n\nFolio: REG-003\nFecha: 28/07/1991\n",
    )
    .unwrap();

    let engine = index(root);
    let answer = engine
        .ask("En el documento D02238, la fecha aparece como \"28/07/1991\". ¿A qué fecha calendario corresponde?")
        .unwrap();
    assert!(
        answer.clarification.is_none(),
        "una sola lectura de calendario válida no es un caso de aclaración: {}",
        answer.text
    );
}

// ── Ronda 8 ──────────────────────────────────────────────────────────────
//
// La ronda 3 dejó bien resuelto qué NO es una aclaración, pero nada
// contestaba esos casos: la ruta de localización devolvía el campo «Fecha»
// del documento —la cadena tal cual, «es 30/06/2023»— sellada como
// verificada. Es el mismo defecto que el «eco de folio» de la ronda 7:
// devolverle al usuario, como dato extraído y verificado, exactamente el
// texto que él acababa de escribir, sin contestar lo que preguntó.

/// «30/06/2023»: junio SÍ tiene 30 días, así que la lectura DD/MM es válida;
/// la lectura MM/DD no existe porque no hay un mes 30. Una sola lectura, y
/// por tanto una respuesta: la fecha se interpreta en vez de repetirse.
#[test]
fn a_single_valid_reading_is_interpreted_instead_of_echoed() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("03110_registro.md"),
        "# Registro\n\nFolio: REG-004\nFecha: 30/06/2023\n",
    )
    .unwrap();

    let engine = index(root);
    let answer = engine
        .ask("En el documento D03110, la fecha aparece como \"30/06/2023\". ¿A qué fecha calendario corresponde?")
        .unwrap();
    assert!(
        answer.text.contains("30 de junio de 2023"),
        "la fecha se interpreta, no se repite: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("El campo «Fecha»"),
        "no se contesta devolviendo el campo del documento: {}",
        answer.text
    );
    // La lectura sale del calendario aplicado a la cadena de la pregunta, no
    // de ningún documento: no hay evidencia que la sostenga, así que no puede
    // declararse verificada (P0-1).
    assert!(
        !answer.verified,
        "una lectura de calendario no es un dato extraído del acervo: {}",
        answer.text
    );
    assert!(answer.citations.is_empty(), "y no cita ningún documento");
}

/// «11/11/2025»: las dos lecturas son válidas pero caen en el MISMO día, así
/// que no hay ambigüedad que declarar. Antes de esta ronda la pregunta caía
/// al eco; declararla ambigua sería afirmar una duda que no existe.
#[test]
fn two_readings_that_fall_on_the_same_day_are_not_ambiguous() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("03220_registro.md"),
        "# Registro\n\nFolio: REG-005\nFecha: 11/11/2025\n",
    )
    .unwrap();

    let engine = index(root);
    let answer = engine
        .ask("En el documento D03220, la fecha aparece como \"11/11/2025\". ¿A qué fecha calendario corresponde?")
        .unwrap();
    assert!(
        answer.clarification.is_none(),
        "dos lecturas que dan el mismo día no son una ambigüedad: {}",
        answer.text
    );
    assert!(
        answer.text.contains("11 de noviembre de 2025"),
        "y se contesta con esa única fecha: {}",
        answer.text
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-fechas.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
