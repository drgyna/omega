//! Ronda 3 — Relación byte a byte entre dos documentos ya nombrados por su
//! clave interna («¿existe un documento con el mismo SHA-256 que D#####?»,
//! «¿D##### es un duplicado exacto de D#####?»).
//!
//! Dos defectos distintos bloqueaban esta pregunta:
//!
//! 1. Omega no tenía la capacidad: nunca comparaba `content_hash`, el SHA-256
//!    que el indexador ya calcula sobre los bytes crudos del archivo.
//! 2. «(mismo SHA-256)» activaba el deíctico «mismo» del detector de
//!    referencias conversacionales (`reference_in`), así que la pregunta se
//!    leía como una alusión al turno anterior — que una pregunta autónoma,
//!    hecha de una sola vez, nunca tiene — y Omega respondía «no sé a qué te
//!    refieres» sin llegar siquiera a intentar resolverla.
//!
//! Fixtures genéricas: registros con nombre de archivo `NNNNN_algo.md`, el
//! mismo prefijo de cinco dígitos que usa el corpus real para el
//! identificador interno `D#####`.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Dos documentos con contenido idéntico en carpetas distintas: la pregunta
/// se resuelve de una sola vez (sin conversación previa) y encuentra al otro
/// por su clave interna, citando el SHA-256 real.
#[test]
fn a_byte_identical_document_is_found_across_the_whole_archive() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let contenido = "# Acta\n\nFolio: ACT-001\nResponsable: Equipo A\n";
    fs::write(root.join("04230_acta.md"), contenido).unwrap();
    fs::write(root.join("03903_acta_copia.md"), contenido).unwrap();
    fs::write(root.join("04466_acta_distinta.md"), "# Acta\n\nFolio: ACT-002\n").unwrap();

    let engine = index(root);
    let answer = engine
        .ask("¿Existe algún documento byte-idéntico (mismo SHA-256) al documento D04230? ¿Cuál?")
        .unwrap();

    assert!(
        !answer.text.contains("No sé a qué te refieres"),
        "una pregunta autónoma nunca puede leerse como una referencia al turno anterior: {}",
        answer.text
    );
    assert!(answer.text.starts_with("Sí: D03903"), "{}", answer.text);
    assert!(answer.verified, "el SHA-256 es un hecho mecánico del índice, no una inferencia");
    let hash = sha256_hex(contenido.as_bytes());
    assert!(
        answer.text.contains(&hash),
        "debe citar el SHA-256 real, no un resumen: {}",
        answer.text
    );
    assert!(answer.citations.iter().all(|citation| citation.reliable));
}

/// Un documento sin ninguna copia byte a byte lo dice con la misma certeza:
/// la ausencia de coincidencia también es un hecho mecánico verificable.
#[test]
fn a_document_with_no_byte_identical_copy_says_so() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("04230_acta.md"), "# Acta\n\nFolio: ACT-001\n").unwrap();
    fs::write(root.join("04466_acta_distinta.md"), "# Acta\n\nFolio: ACT-002\n").unwrap();

    let engine = index(root);
    let answer = engine
        .ask("¿Existe algún documento byte-idéntico (mismo SHA-256) al documento D04230? ¿Cuál?")
        .unwrap();
    assert!(answer.text.starts_with("No:"), "{}", answer.text);
    assert!(answer.verified);
}

/// Dos documentos con contenido distinto: no son un duplicado exacto, y la
/// respuesta lo dice sin inventar que además son «del mismo tipo y área» —
/// eso Omega no lo verificó.
#[test]
fn two_documents_with_different_content_are_not_an_exact_duplicate() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("02413_acta.md"), "# Acta\n\nFolio: ACT-100\n").unwrap();
    fs::write(root.join("08353_acta.md"), "# Acta\n\nFolio: ACT-200\n").unwrap();

    let engine = index(root);
    let answer = engine
        .ask("¿El documento D02413 es un duplicado exacto del documento D08353, o solo un documento similar?")
        .unwrap();
    assert!(
        answer.text.contains("No es un duplicado exacto"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("SHA-256"));
    assert!(answer.verified);
}

/// Dos documentos con contenido idéntico: sí son un duplicado exacto, aunque
/// la pregunta los nombre bajo la plantilla que normalmente espera un «no».
#[test]
fn two_documents_with_identical_content_are_an_exact_duplicate() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let contenido = "# Acta\n\nFolio: ACT-300\n";
    fs::write(root.join("02413_acta.md"), contenido).unwrap();
    fs::write(root.join("08353_acta_copia.md"), contenido).unwrap();

    let engine = index(root);
    let answer = engine
        .ask("¿El documento D02413 es un duplicado exacto del documento D08353, o solo un documento similar?")
        .unwrap();
    assert!(
        answer.text.starts_with("Sí, es un duplicado exacto"),
        "{}",
        answer.text
    );
    assert!(answer.verified);
}

// ─────────────────────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-duplicados.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
