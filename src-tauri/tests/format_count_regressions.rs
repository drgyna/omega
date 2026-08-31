//! Ronda 4 · punto 1(b) — conteo real por formato de archivo, con lo que no se
//! pudo indexar declarado al lado.
//!
//! Antes, «¿Cuántos documentos … están en formato DOCX?» caía en la ruta de
//! búsqueda literal (el área va entrecomillada, y una cita entrecomillada
//! cortaba el planificador antes de llegar a la rama de conteo) y respondía
//! con una muestra recortada: «20 valores» era el tope de la muestra, no un
//! conteo. Ahora el formato es un filtro real —sobre `documents.extension` y
//! sobre si el documento necesitó OCR— y la respuesta declara además cuántos
//! archivos del alcance no se pudieron indexar, en vez de excluirlos callando.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// Conteo exacto por extensión, no una muestra recortada.
#[test]
fn a_count_by_format_is_a_real_count_not_a_capped_sample() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    for index in 0..25 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.md")),
            format!("Área: Ventas y clientes\nFolio: P-{index}\n"),
        )
        .unwrap();
    }
    for index in 25..29 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.txt")),
            format!("Área: Ventas y clientes\nFolio: P-{index}\n"),
        )
        .unwrap();
    }

    let engine = index(root, "formato-1");
    let answer = engine
        .ask("¿Cuántos documentos del área \"Ventas y clientes\" están en formato MD?")
        .unwrap();

    assert!(
        answer.text.contains(": 25."),
        "el conteo es real y completo, no el tope de una muestra: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Muestra recortada"),
        "esta ruta ya no pasa por la muestra con tope: {}",
        answer.text
    );
    assert!(answer.verified, "sin nada sin indexar, el conteo es exacto");
    let scope = answer.scope.expect("el conteo declara su alcance");
    assert_eq!(scope.document_count, Some(25));
    assert_eq!(scope.excluded_count, Some(0));
}

/// Un archivo del alcance que no se pudo indexar no desaparece: se cuenta
/// aparte y la respuesta deja de presentarse como exacta.
#[test]
fn files_that_could_not_be_indexed_are_declared_next_to_the_count() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    for index in 0..3 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.md")),
            format!("Folio: P-{index}\nEstado: Abierto\n"),
        )
        .unwrap();
    }
    // Sin contenido extraíble: se descubre, no se indexa.
    fs::write(root.join("ventas/00009_vacio.md"), "   \n\n").unwrap();

    let engine = index(root, "formato-2");
    let answer = engine
        .ask("¿Cuántos documentos hay en la carpeta ventas en formato MD?")
        .unwrap();

    assert!(answer.text.contains(": 3."), "{}", answer.text);
    assert!(
        answer.text.contains("1 archivo .md de este alcance no se pudo indexar"),
        "lo no indexado se declara con su número: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "un conteo que excluye archivos ilegibles no puede presentarse como el total del acervo"
    );
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Conteo parcial")),
        "{:?}",
        answer.warning
    );
    let scope = answer.scope.expect("el conteo declara su alcance");
    assert_eq!(scope.excluded_count, Some(1));
}

/// Una pregunta que menciona un nombre de archivo pero no habla de formato no
/// se desvía a esta ruta.
#[test]
fn a_question_that_merely_names_a_file_is_not_a_format_count() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("00001_pedido.md"), "Folio: P-1\nEstado: Abierto\n").unwrap();

    let engine = index(root, "formato-3");
    let answer = engine
        .ask("¿Cuántos documentos mencionan 00001_pedido.md?")
        .unwrap();

    assert!(
        !answer.text.starts_with("Documentos indexados en formato"),
        "sin la palabra «formato» o «extensión» no hay conteo por formato: {}",
        answer.text
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn index(root: &Path, name: &str) -> OmegaEngine {
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
