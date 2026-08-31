//! Ronda 4 · punto 2 — una cita de solo-nombre-de-archivo no es evidencia.
//!
//! Cuando la pregunta escribe la ruta del archivo («¿Qué información se puede
//! extraer del archivo ventas/02476_cotizacion.csv?») y la búsqueda no
//! encuentra nada dentro del documento, quedaba una única coincidencia: el
//! metadato «nombre de archivo», sin valor. Con ella, la respuesta salía como
//! «1 resultados con evidencia específica» y **verificada** — una afirmación
//! vacía que sólo repetía el nombre que el usuario acababa de escribir. En la
//! ronda 3 se midieron 23 respuestas así en las 4.000 preguntas.
//!
//! La regla nueva es de dos piezas y las dos se comprueban aquí:
//!
//! - `Evidence::is_substantive()` distingue un metadato **con** valor
//!   («carpeta = calidad», «formato = DOCX»: hechos del acervo que sostienen
//!   un conteo) de uno **sin** valor (el nombre de archivo).
//! - `Answer::verified()` —único constructor que puede marcar `verified`—
//!   nunca lo marca cuando ninguna cita es sustantiva.

use std::{fs, path::Path};

use omega_core::{Answer, Clock, Evidence, OmegaEngine};

const TODAY: &str = "2026-08-29";

/// El caso que se arregla: documento sin contenido legible cuyo nombre sí
/// aparece en la pregunta.
#[test]
fn a_document_whose_only_match_is_its_filename_answers_without_evidence() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // Con contenido, para que el índice tenga algo y la fixture sea realista.
    fs::write(root.join("00001_pedido.md"), "Folio: P-1\nEstado: Abierto\n").unwrap();
    // Un archivo cuyo contenido no dice nada que se parezca a la pregunta.
    fs::write(root.join("02476_cotizacion.csv"), "a,b\n1,2\n").unwrap();

    let engine = index(root, "sustantiva-1");
    let answer = engine
        .ask("¿Qué información se puede extraer del archivo 02476_cotizacion.csv?")
        .unwrap();

    assert!(
        answer.citations.is_empty(),
        "una cita que sólo repite el nombre del archivo no puede sostener la respuesta: {:?}",
        answer.citations
    );
    assert!(!answer.verified);
    assert!(
        !answer.text.contains("evidencia específica"),
        "no puede presentarse como un hallazgo: {}",
        answer.text
    );
}

/// El caso normal que NO debe cambiar: un metadato **con** valor sigue siendo
/// evidencia buena y sostiene un conteo verificado.
#[test]
fn a_metadata_citation_that_carries_a_value_is_still_evidence() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("calidad")).unwrap();
    for index in 0..3 {
        fs::write(
            root.join(format!("calidad/{index:05}_hallazgo.md")),
            format!("Folio: C-{index}\nEstado: Abierto\n"),
        )
        .unwrap();
    }

    let engine = index(root, "sustantiva-2");
    let answer = engine.ask("¿Cuántos documentos hay en la carpeta calidad?").unwrap();

    assert!(answer.text.starts_with("3 documentos"), "{}", answer.text);
    assert!(!answer.citations.is_empty());
    assert!(
        answer.citations.iter().any(Evidence::is_substantive),
        "«carpeta de origen = calidad» lleva su valor: es evidencia real"
    );
    assert!(answer.verified);
}

/// El otro caso normal: una respuesta apoyada en un valor extraído del
/// documento sigue siendo verificada.
#[test]
fn an_answer_backed_by_an_extracted_value_is_still_verified() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(
        root.join("00001_pedido.md"),
        "Folio: PED-2024-0001\nEstado: Abierto\n",
    )
    .unwrap();

    let engine = index(root, "sustantiva-3");
    let answer = engine.ask("¿Cuál es el Estado del folio PED-2024-0001?").unwrap();

    assert!(answer.citations.iter().any(Evidence::is_substantive));
    assert!(answer.verified, "{} / {:?}", answer.text, answer.warning);
}

/// El candado vive en el constructor: aunque una ruta futura construyera la
/// respuesta con citas de sólo-existencia, no podría marcarla verificada.
#[test]
fn the_verified_constructor_refuses_existence_only_citations() {
    let existence = Evidence {
        id: "m-1-nombre de archivo".into(),
        document_id: 1,
        path: "/tmp/x/00001_pedido.md".into(),
        origin: "x".into(),
        location: "metadato: nombre de archivo".into(),
        excerpt: "00001_pedido.md".into(),
        normalized_value: None,
        value: None,
        matched: Some("00001_pedido.md".into()),
        field: Some("nombre de archivo".into()),
        match_kind: "exacta".into(),
        reliable: true,
        ocr_status: None,
        ocr_confidence: None,
        confidence: None,
    };
    assert!(!existence.is_substantive());
    let answer = Answer::verified("cualquier texto", vec![existence.clone()]);
    assert!(!answer.verified);
    assert!(answer.warning.is_some());

    let with_value = Evidence {
        value: Some("calidad".into()),
        location: "metadato: carpeta de origen".into(),
        field: Some("carpeta de origen".into()),
        ..existence
    };
    assert!(with_value.is_substantive());
    assert!(Answer::verified("cualquier texto", vec![with_value]).verified);
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
