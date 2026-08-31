//! P1-D.1 — La recuperación no puede cortar candidatos antes de rankearlos.
//!
//! El texto se recuperaba con `LIMIT 4000` sobre los fragmentos que coinciden
//! en el índice de texto completo, ordenados por `bm25`. El ranking real —el
//! que pesa cobertura de términos, especificidad y longitud del fragmento—
//! corría *después*, sólo sobre lo que había sobrevivido al corte. En un
//! acervo con más de cuatro mil fragmentos coincidentes, el documento
//! relevante podía quedar fuera antes de que nadie lo evaluara, y el conteo de
//! documentos que la respuesta declara salía igual de truncado.
//!
//! Fixtures genéricas: un inventario con muchas filas de estado y un
//! expediente de proyecto con el párrafo relevante.

use std::{fs, path::Path};

use omega_core::{Clock, Database, OmegaEngine, ToolEngine};

const TODAY: &str = "2026-08-25";

/// Fragmentos de ruido por encima del antiguo límite. El documento relevante
/// queda deliberadamente al final del orden `bm25` —su párrafo es largo y
/// menciona el término una sola vez—, que es justo la posición que el corte
/// borraba.
const NOISE_FILES: usize = 30;
const NOISE_ROWS: usize = 160;

#[test]
fn a_relevant_document_past_the_old_limit_is_still_ranked() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_noise(root);
    write_target(root);

    let engine = index(root);
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    // Punto de partida del caso: hay más fragmentos coincidentes que el
    // antiguo límite, así que el corte era alcanzable de verdad.
    assert!(
        matching_chunks(&engine, "activo") > 4_000,
        "la fixture debe superar el antiguo límite de 4000 fragmentos"
    );

    let result = tools.search_text("activo", None, 6).unwrap();
    assert!(
        result
            .hits
            .iter()
            .any(|hit| hit.evidence.path.ends_with("expediente-proyecto.md")),
        "el documento relevante quedaba fuera antes de que el ranking lo viera: {:#?}",
        result
            .hits
            .iter()
            .map(|hit| hit.evidence.path.clone())
            .collect::<Vec<_>>()
    );
}

/// El alcance declarado tiene que contar todos los documentos que coinciden,
/// no sólo los que cupieron en el corte.
#[test]
fn the_declared_document_count_is_not_truncated_by_the_retrieval_cut() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_noise(root);
    write_target(root);

    let engine = index(root);
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    let result = tools.search_text("activo", None, 6).unwrap();
    assert_eq!(
        result.document_count,
        NOISE_FILES + 1,
        "los {} documentos de ruido y el relevante coinciden todos",
        NOISE_FILES
    );
}

/// El corte tampoco puede esconder un documento en la ruta que usa la
/// interfaz para buscar.
#[test]
fn the_interface_search_also_reaches_past_the_old_limit() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    write_noise(root);
    write_target(root);
    let engine = index(root);

    let hits = engine.search("expediente hallazgo relevante").unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.evidence.path.ends_with("expediente-proyecto.md")),
        "{:#?}",
        hits.iter()
            .map(|hit| hit.evidence.path.clone())
            .collect::<Vec<_>>()
    );
}

/// La ruta `search` aplicaba su propio corte —hasta 400 fragmentos— sobre las
/// coincidencias de texto completo. Ahí el orden del corte (`bm25`) sí
/// coincide con el del ranking, así que ningún documento que fuera a ganar
/// quedaba fuera. Lo que sí quedaba fuera era todo documento tapado por otro
/// con muchísimos fragmentos: un solo archivo con cientos de filas llenaba la
/// ventana entero y los demás desaparecían del resultado aunque hubiera sitio
/// de sobra en él.
#[test]
fn one_chunk_heavy_document_never_crowds_the_others_out_of_the_results() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // Un único inventario con quinientas filas cortas: más fragmentos que la
    // ventana entera.
    let mut contents = String::from("Estado\n");
    for _ in 0..500 {
        contents.push_str("expediente activo\n");
    }
    fs::write(root.join("inventario.csv"), contents).unwrap();

    // Un expediente con un solo fragmento, largo, que también coincide. Su
    // `bm25` es peor, así que cae detrás de las quinientas filas.
    let relleno =
        "El expediente documenta el seguimiento del periodo y sus anexos. ".repeat(18);
    fs::write(
        root.join("expediente-proyecto.md"),
        format!("Nota: {relleno} El expediente permanece activo hasta el cierre.\n"),
    )
    .unwrap();

    let engine = OmegaEngine::open_with_clock(
        root.join("omega-search.db"),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    assert_eq!(engine.index_source(source).unwrap().indexed, 2);

    let hits = engine.search("expediente activo").unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.evidence.path.ends_with("expediente-proyecto.md")),
        "el segundo documento cabía de sobra en el resultado: {:#?}",
        hits.iter()
            .map(|hit| hit.evidence.path.clone())
            .collect::<Vec<_>>()
    );
}

// ─────────────────────────────────────────────────────────────────────────

/// Inventario de ruido: filas cortas que repiten el término y por eso ganan
/// en `bm25` a cualquier párrafo largo.
fn write_noise(root: &Path) {
    for file in 0..NOISE_FILES {
        let mut contents = String::from("Estado\n");
        for _ in 0..NOISE_ROWS {
            contents.push_str("activo\n");
        }
        fs::write(root.join(format!("inventario-{file:03}.csv")), contents).unwrap();
    }
}

/// Expediente relevante: un párrafo largo que menciona el término una sola
/// vez. Es el peor lugar posible en el orden `bm25` y el mejor en el ranking
/// real, que premia cobertura y longitud del fragmento.
fn write_target(root: &Path) {
    let relleno = "El expediente documenta el hallazgo relevante del periodo y su seguimiento. "
        .repeat(16);
    fs::write(
        root.join("expediente-proyecto.md"),
        format!("Clave: PRY-7001\n{relleno} El proyecto permanece activo hasta el cierre.\n"),
    )
    .unwrap();
}

fn matching_chunks(engine: &OmegaEngine, term: &str) -> i64 {
    let database = Database::open(engine.database_path()).unwrap();
    let connection = database.connect().unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH ?1",
            [term],
            |row| row.get(0),
        )
        .unwrap()
}

fn index(root: &Path) -> OmegaEngine {
    let engine =
        OmegaEngine::open_with_clock(root.join("omega-full.db"), Clock::fixed(TODAY).unwrap())
            .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert_eq!(report.indexed, NOISE_FILES + 1);
    engine
}
