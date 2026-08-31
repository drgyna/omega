//! P1-C — Política de duplicados.
//!
//! La política, explícita y probada aquí, es ésta:
//!
//! 1. Un archivo duplicado byte a byte se conserva y se indexa como un
//!    documento más. Omega **no** deduplica por su cuenta: dos copias de una
//!    factura pueden ser un error de archivo o dos entregas reales, y el motor
//!    no tiene forma de saber cuál es.
//! 2. Los conteos y las sumas **no cambian** por la presencia de duplicados.
//!    Cambiarlos en silencio sería alterar el hecho que el acervo contiene.
//! 3. El duplicado se detecta, se cuenta y se nombra en el reporte de
//!    indexación, y toda respuesta que se apoye en documentos de contenido
//!    idéntico lo advierte.
//!
//! Fixtures genéricas: facturas con folio e importe.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-25";

/// Dos copias idénticas en la misma carpeta: se conservan las dos, la suma
/// las cuenta a las dos, y tanto el reporte como la respuesta lo dicen.
#[test]
fn an_exact_duplicate_in_the_same_folder_is_kept_counted_and_reported() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let contenido = "# Registro\n\nFolio: FAC-001\nImporte: $100.00 MXN\n";
    fs::write(root.join("001-factura.md"), contenido).unwrap();
    fs::write(root.join("002-factura-copia.md"), contenido).unwrap();

    let (engine, report) = index(root);
    assert_eq!(report.indexed, 2, "las dos copias se conservan");
    assert_eq!(report.duplicate_groups, 1);
    assert_eq!(report.duplicate_documents, 2);
    assert!(
        report.warnings.iter().any(|warning| {
            warning.contains("001-factura.md") && warning.contains("002-factura-copia.md")
        }),
        "el reporte debe nombrar el grupo duplicado: {:?}",
        report.warnings
    );

    let answer = engine
        .ask_in_conversation("dup", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(
        answer.text.contains("$200.00 MXN"),
        "la política no cambia el conteo: dos documentos, 100 + 100 = 200: {}",
        answer.text
    );
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("idéntico")),
        "la respuesta depende de duplicados exactos y tiene que advertirlo: {:?}",
        answer.warning
    );
}

/// El duplicado entre carpetas distintas se detecta igual: lo que define un
/// duplicado es el contenido, no dónde está guardado.
#[test]
fn an_exact_duplicate_across_folders_is_detected_too() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("2025")).unwrap();
    fs::create_dir(root.join("2026")).unwrap();
    let contenido = "# Registro\n\nFolio: FAC-001\nImporte: $100.00 MXN\n";
    fs::write(root.join("2025/factura.md"), contenido).unwrap();
    fs::write(root.join("2026/factura.md"), contenido).unwrap();

    let (engine, report) = index(root);
    assert_eq!(report.indexed, 2);
    assert_eq!(report.duplicate_groups, 1);
    assert_eq!(report.duplicate_documents, 2);

    let answer = engine
        .ask_in_conversation("dup", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(answer.text.contains("$200.00 MXN"), "{}", answer.text);
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("idéntico")),
        "{:?}",
        answer.warning
    );
}

/// Mismo nombre en dos carpetas, contenido distinto: no es un duplicado.
#[test]
fn the_same_name_in_two_folders_with_different_content_is_not_a_duplicate() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("2025")).unwrap();
    fs::create_dir(root.join("2026")).unwrap();
    fs::write(
        root.join("2025/factura.md"),
        "# Registro\n\nFolio: FAC-001\nImporte: $100.00 MXN\n",
    )
    .unwrap();
    fs::write(
        root.join("2026/factura.md"),
        "# Registro\n\nFolio: FAC-002\nImporte: $250.00 MXN\n",
    )
    .unwrap();

    let (engine, report) = index(root);
    assert_eq!(report.indexed, 2);
    assert_eq!(
        report.duplicate_groups, 0,
        "compartir nombre no es compartir contenido"
    );
    assert_eq!(report.duplicate_documents, 0);

    let answer = engine
        .ask_in_conversation("dup", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(answer.text.contains("$350.00 MXN"), "{}", answer.text);
    assert!(
        !answer
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("idéntico"),
        "{:?}",
        answer.warning
    );
    assert!(answer.verified, "{}", answer.text);
}

/// Modificar una de las dos copias deshace el duplicado en la reindexación.
#[test]
fn a_modified_file_stops_being_a_duplicate_after_reindexing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let contenido = "# Registro\n\nFolio: FAC-001\nImporte: $100.00 MXN\n";
    fs::write(root.join("001-factura.md"), contenido).unwrap();
    fs::write(root.join("002-factura-copia.md"), contenido).unwrap();
    let (engine, first) = index(root);
    assert_eq!(first.duplicate_groups, 1);

    fs::write(
        root.join("002-factura-copia.md"),
        "# Registro\n\nFolio: FAC-002\nImporte: $250.00 MXN\n",
    )
    .unwrap();
    let second = reindex(&engine);
    assert_eq!(second.indexed, 2);
    assert_eq!(second.modified, 1);
    assert_eq!(second.duplicate_groups, 0, "ya no son idénticos");
    assert_eq!(second.duplicate_documents, 0);

    let answer = engine
        .ask_in_conversation("dup", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(answer.text.contains("$350.00 MXN"), "{}", answer.text);
    assert!(
        !answer
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("idéntico"),
        "{:?}",
        answer.warning
    );
}

/// Borrar una de las dos copias deshace el duplicado y borra su evidencia.
#[test]
fn a_deleted_duplicate_disappears_with_its_warning() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let contenido = "# Registro\n\nFolio: FAC-001\nImporte: $100.00 MXN\n";
    fs::write(root.join("001-factura.md"), contenido).unwrap();
    fs::write(root.join("002-factura-copia.md"), contenido).unwrap();
    let (engine, first) = index(root);
    assert_eq!(first.duplicate_groups, 1);

    fs::remove_file(root.join("002-factura-copia.md")).unwrap();
    let second = reindex(&engine);
    assert_eq!(second.indexed, 1);
    assert_eq!(second.duplicate_groups, 0);

    let answer = engine
        .ask_in_conversation("dup", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(answer.text.contains("$100.00 MXN"), "{}", answer.text);
    assert!(answer.verified, "{}", answer.text);
    assert!(
        !answer
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("idéntico"),
        "{:?}",
        answer.warning
    );
    assert!(
        engine
            .search("FAC-001")
            .unwrap()
            .iter()
            .all(|hit| !hit.evidence.path.contains("002-factura-copia")),
        "la evidencia del archivo borrado desaparece"
    );
}

/// Un duplicado en el acervo no contamina una respuesta que no lo toca.
#[test]
fn a_duplicate_elsewhere_never_taints_an_unrelated_answer() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let repetido = "# Registro\n\nFolio: FAC-001\nCuota: $10.00 MXN\n";
    fs::write(root.join("001-factura.md"), repetido).unwrap();
    fs::write(root.join("002-factura-copia.md"), repetido).unwrap();
    fs::write(
        root.join("003-factura.md"),
        "# Registro\n\nFolio: FAC-003\nImporte: $250.00 MXN\n",
    )
    .unwrap();

    let (engine, report) = index(root);
    assert_eq!(report.duplicate_groups, 1);

    let answer = engine
        .ask_in_conversation("otro", "¿Cuánto suma el Importe?")
        .unwrap();
    assert!(answer.text.contains("$250.00 MXN"), "{}", answer.text);
    assert!(
        !answer
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("idéntico"),
        "el duplicado no participó en este cálculo: {:?}",
        answer.warning
    );
    // La respuesta sí es parcial, pero por el motivo real: dos documentos del
    // alcance no tienen el campo. Ése es un hecho del acervo, no del duplicado.
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Resultado parcial")),
        "{:?}",
        answer.warning
    );
    assert!(answer.text.contains("2 documentos sin ese campo"), "{}", answer.text);
}

// ─────────────────────────────────────────────────────────────────────────

fn index(root: &Path) -> (OmegaEngine, omega_core::IndexReport) {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-dup.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    (engine, report)
}

fn reindex(engine: &OmegaEngine) -> omega_core::IndexReport {
    let source = engine.sources().unwrap()[0].id;
    engine.index_source(source).unwrap()
}
