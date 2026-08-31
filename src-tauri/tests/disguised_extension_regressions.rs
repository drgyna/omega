//! Ronda 4 · punto 3 — extensión declarada que no corresponde al contenido.
//!
//! Un `.pdf` cuyo contenido real es texto llano se despachaba al parser de
//! PDF, fallaba, y acababa como «documento sin contenido extraíble» sin que
//! nadie supiera por qué. Ahora la extensión se contrasta contra la firma real
//! de los primeros bytes **antes** de despachar: si no coinciden, se lee el
//! contenido real y toda respuesta que cite el documento declara la
//! discrepancia.
//!
//! El detector es deliberadamente conservador —sólo actúa ante una prueba
//! positiva— porque toca el despacho de TODOS los documentos. Sobre los 10.000
//! archivos del corpus de auditoría marca 19 y ninguno de los 6.064 PDF/DOCX/
//! XLSX reales: 0 falsos positivos. Los tests de abajo cubren las dos
//! direcciones del disfraz y, sobre todo, los formatos reales que no deben
//! marcarse nunca.

use std::{fs, io::Write, path::Path};

use omega_core::{Clock, DocumentParser, LocalDocumentParser, OmegaEngine};

#[path = "support/mod.rs"]
mod support;

const TODAY: &str = "2026-08-29";

/// Un `.pdf` que por dentro es texto llano: se lee como texto y se declara.
#[test]
fn a_pdf_that_is_really_plain_text_is_read_and_declared() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("08529_cotizacion.pdf"),
        "Cotización Comercial PED-2024-00164\n\nFolio: PED-2024-00164\nEstado: Abierto\n",
    )
    .unwrap();

    let parsed = LocalDocumentParser::default()
        .parse(&root.join("08529_cotizacion.pdf"))
        .unwrap();
    assert_eq!(
        parsed.declared_format_mismatch.as_deref(),
        Some("texto plano")
    );
    assert!(
        parsed.records.iter().any(|record| record.label == "Folio"),
        "el contenido real sí se extrae: no se castiga al usuario por el nombre del archivo"
    );

    let engine = index(root, "disfraz-1");
    let answer = engine.ask("¿Cuál es el Estado del folio PED-2024-00164?").unwrap();
    assert!(
        answer
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("no tienen el formato que declara su extensión")),
        "toda respuesta que lo cite declara la discrepancia: {:?}",
        answer.warning
    );
}

/// La otra dirección: un `.txt` que por dentro es un PDF.
#[test]
fn a_text_extension_hiding_a_binary_format_is_detected() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let mut file = fs::File::create(root.join("disfrazado.txt")).unwrap();
    file.write_all(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n").unwrap();
    drop(file);

    let parsed = LocalDocumentParser::default()
        .parse(&root.join("disfrazado.txt"))
        .unwrap_or_else(|_| panic!("un PDF vacío es ilegible, pero el disfraz debe detectarse"));
    assert_eq!(parsed.declared_format_mismatch.as_deref(), Some("un PDF"));
}

/// Los formatos reales nunca se marcan. Es la mitad más importante del test:
/// un `.docx` de verdad marcado como «disfrazado» sería mucho peor que no
/// detectar ninguno.
#[test]
fn real_files_of_every_supported_format_are_never_flagged() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let parser = LocalDocumentParser::default();

    support::write_docx(&root.join("real.docx"), &["Folio: A-1", "Estado: Abierto"]);
    support::write_xlsx(&root.join("real.xlsx"), &[vec!["Folio", "Estado"], vec!["A-1", "Abierto"]]);
    fs::write(root.join("real.txt"), "Folio: A-1\nEstado: Abierto\n").unwrap();
    fs::write(root.join("real.md"), "# Nota\n\nFolio: A-1\n").unwrap();
    fs::write(root.join("real.csv"), "Folio,Estado\nA-1,Abierto\n").unwrap();

    for name in ["real.docx", "real.xlsx", "real.txt", "real.md", "real.csv"] {
        let parsed = parser.parse(&root.join(name)).unwrap();
        assert_eq!(
            parsed.declared_format_mismatch, None,
            "{name} es un archivo real de su formato y no puede marcarse como disfrazado"
        );
    }
}

/// Un archivo vacío no es un disfraz: es un archivo vacío, y esa ruta ya
/// existía. Marcarlo confundiría dos hechos distintos.
#[test]
fn an_empty_file_is_not_a_disguise() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("vacio.pdf"), b"").unwrap();

    let parsed = LocalDocumentParser::default().parse(&root.join("vacio.pdf"));
    match parsed {
        Ok(document) => assert_eq!(document.declared_format_mismatch, None),
        // Un PDF de cero bytes puede fallar al abrirse; lo que no puede es
        // reportarse como un formato disfrazado.
        Err(_) => {}
    }
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

// ─────────────────────────────────────────────────────────────────────────
// Ronda 7 — la discrepancia deja de ser una nota al pie cuando la pregunta
// es justamente qué se puede sacar del archivo.

/// El indexador ya detectaba el disfraz (ronda 4), pero «¿qué información se
/// puede extraer de este archivo?» se contestaba con el contenido extraído y
/// la discrepancia iba en un aviso al final. Para esta pregunta, la
/// discrepancia ES la respuesta.
#[test]
fn asking_what_a_disguised_file_contains_leads_with_the_mismatch() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir_all(root.join("ventas")).unwrap();
    // Texto plano con extensión .pdf: el archivo no es un PDF.
    std::fs::write(
        root.join("ventas/08529_cotizacion.pdf"),
        "Cotización Comercial PED-2025-00049\n\nEmpresa: Grupo Nexo\nFolio: PED-1\nMoneda: USD\n",
    )
    .unwrap();

    let engine = omega_core::OmegaEngine::open_with_clock(
        root.join("omega-disfraz-r7.db"),
        omega_core::Clock::fixed("2026-08-30").unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();

    let answer = engine
        .ask("¿Qué información se puede extraer del archivo ventas/08529_cotizacion.pdf (D08529)?")
        .unwrap();

    let clarification = answer
        .clarification
        .as_ref()
        .unwrap_or_else(|| panic!("la discrepancia es la respuesta, no un aviso: {}", answer.text));
    assert_eq!(clarification.reason, "extension_enganosa");
    assert!(
        clarification.question.contains("(pdf) es engañosa"),
        "{}",
        clarification.question
    );
}

/// Un archivo cuyo contenido sí corresponde a su extensión no se desvía aquí.
#[test]
fn a_file_that_is_what_it_claims_is_answered_normally() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir_all(root.join("ventas")).unwrap();
    std::fs::write(
        root.join("ventas/08530_cotizacion.txt"),
        "Cotización Comercial PED-2025-00050\n\nEmpresa: Grupo Nexo\nFolio: PED-2\n",
    )
    .unwrap();

    let engine = omega_core::OmegaEngine::open_with_clock(
        root.join("omega-disfraz-r7b.db"),
        omega_core::Clock::fixed("2026-08-30").unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();

    let answer = engine
        .ask("¿Qué información se puede extraer del archivo ventas/08530_cotizacion.txt (D08530)?")
        .unwrap();

    assert!(
        answer
            .clarification
            .as_ref()
            .is_none_or(|clarification| clarification.reason != "extension_enganosa"),
        "un .txt con texto plano no tiene ninguna extensión que desmentir: {}",
        answer.text
    );
}
