use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use omega_core::{OmegaEngine, ToolEngine};
use walkdir::WalkDir;

/// Esta prueba no conoce ningún corpus concreto. El acervo se inyecta desde el
/// entorno para que las rutas, campos y valores sigan siendo datos externos.
#[test]
fn retrieves_only_documents_with_specific_evidence_from_the_configured_corpus() {
    let Some(corpus) = std::env::var_os("OMEGA_SCALE_CORPUS").map(PathBuf::from) else {
        eprintln!("OMEGA_SCALE_CORPUS no está definida: prueba externa omitida.");
        return;
    };
    assert!(
        corpus.is_dir(),
        "la fuente configurada debe ser una carpeta"
    );

    let facts = inspect_corpus(&corpus);
    assert!(
        !facts.files.is_empty(),
        "el corpus debe contener archivos compatibles"
    );
    let temporary = tempfile::tempdir().unwrap();
    let database_path = temporary.path().join("omega-retrieval.db");
    let engine = OmegaEngine::open(&database_path).unwrap();
    let source_id = engine.authorize_source(&corpus).unwrap();
    let report = engine.index_source(source_id).unwrap();
    assert_eq!(report.indexed, facts.files.len());

    let tools = ToolEngine::new(omega_core::Database::open(&database_path).unwrap());

    // Folio/identificador: es una igualdad de valor, no una coincidencia FTS.
    let identifier = facts
        .identifier
        .as_ref()
        .expect("el corpus debe incluir un identificador");
    let exact = tools.exact_lookup(&identifier.value, 20).unwrap();
    assert!(!exact.is_empty());
    assert_evidence(
        &exact[0].evidence,
        &identifier.value,
        Some(&identifier.label),
        &corpus,
    );

    // La consulta completa que llega desde la interfaz no puede degradarse a
    // una búsqueda por el nombre del campo ni a FTS parcial. El identificador
    // se descubre del fixture configurado y el conjunto esperado se calcula
    // a partir del contenido real, sin datos incrustados en producción.
    let expected_identifier_paths = facts
        .files
        .iter()
        .filter(|path| {
            fs::read_to_string(path)
                .map(|text| text.contains(&identifier.value))
                .unwrap_or(false)
        })
        .map(|path| path.canonicalize().unwrap())
        .collect::<HashSet<_>>();
    assert!(
        !expected_identifier_paths.is_empty(),
        "el identificador descubierto debe aparecer completo en el fixture"
    );
    let natural_exact = engine
        .ask(&format!(
            "Encuentra el documento que contiene exactamente el identificador {}",
            identifier.value
        ))
        .unwrap();
    let actual_identifier_paths = natural_exact
        .citations
        .iter()
        .map(|evidence| PathBuf::from(&evidence.path).canonicalize().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(actual_identifier_paths, expected_identifier_paths);
    assert_eq!(
        natural_exact.citations.len(),
        expected_identifier_paths.len()
    );
    for evidence in &natural_exact.citations {
        assert_eq!(evidence.match_kind, "canónica");
        assert!(evidence.excerpt.contains(&identifier.value));
        assert_eq!(evidence.matched.as_deref(), Some(identifier.value.as_str()));
        assert!(
            expected_identifier_paths
                .contains(&PathBuf::from(&evidence.path).canonicalize().unwrap()),
            "no debe aparecer un documento que sólo comparta el campo o parte del valor"
        );
    }
    let absent_identifier = format!("omega-no-existe-{}", identifier.value);
    let absent_exact = engine
        .ask(&format!(
            "Encuentra el documento que contiene exactamente el identificador {absent_identifier}"
        ))
        .unwrap();
    assert!(absent_exact.citations.is_empty());

    // Casos end-to-end del corpus externo: cada pregunta exige la igualdad
    // simultánea de concepto y valor. Estas etiquetas pertenecen sólo al
    // fixture configurado; el motor de producción no las conoce.
    for (label, value) in [
        ("Estado del expediente", "Revocado"),
        ("Tipo de asunto", "Juicio hipotecario"),
        ("Estado de la factura", "Vencida"),
    ] {
        let expected = files_with_exact_field_value(&facts.files, label, value);
        assert!(
            !expected.is_empty(),
            "el fixture externo debe aportar el caso {label}: {value}"
        );
        let answer = engine.ask(&format!("{label} {value}")).unwrap();
        let actual = answer
            .citations
            .iter()
            .map(|evidence| PathBuf::from(&evidence.path).canonicalize().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(actual, expected, "la pareja debe limitar el resultado");
        assert_eq!(answer.citations.len(), expected.len());
        for evidence in &answer.citations {
            assert_eq!(evidence.match_kind, "campo");
            assert_eq!(evidence.field.as_deref(), Some(label));
            assert_eq!(evidence.value.as_deref(), Some(value));
            assert_eq!(evidence.matched.as_deref(), Some(value));
            assert!(evidence.excerpt.contains(value));
        }
    }

    // Se toma un prefijo del identificador real, sin asumir su convención. La
    // palabra "exactamente" no autoriza a expandirlo a coincidencias parciales.
    let incomplete_prefix = identifier
        .value
        .chars()
        .take_while(|character| character.is_alphabetic())
        .collect::<String>();
    assert!(!incomplete_prefix.is_empty());
    let incomplete = engine
        .ask(&format!("Encuentra exactamente {incomplete_prefix}"))
        .unwrap();
    assert!(incomplete.citations.is_empty());

    // El nombre real del archivo se resuelve contra metadatos autorizados.
    let filename = facts.files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let filename_hits = tools.exact_lookup(&filename, 20).unwrap();
    assert!(filename_hits.iter().any(|hit| {
        hit.evidence.path.ends_with(&filename)
            && hit.evidence.field.as_deref() == Some("nombre de archivo")
    }));

    // Estado: la evidencia debe ser la línea del valor estructurado solicitado.
    let state = facts
        .state
        .as_ref()
        .expect("el corpus debe incluir un campo de estado");
    let state_hits = tools
        .search(&format!("{} {}", state.label, state.value), &[], 20)
        .unwrap();
    assert!(!state_hits.is_empty());
    assert_evidence(
        &state_hits[0].evidence,
        &state.value,
        Some(&state.label),
        &corpus,
    );
    let state_value_hits = tools.search(&state.value, &[], 20).unwrap();
    assert_evidence(
        &state_value_hits[0].evidence,
        &state.value,
        Some(&state.label),
        &corpus,
    );

    // Un nombre propio escrito dentro de una pregunta se compara contra el
    // valor extraído, sin depender de que el FTS encuentre un encabezado.
    let person = facts
        .person
        .as_ref()
        .expect("el corpus debe incluir un valor de nombre propio");
    let person_hits = tools
        .search(&format!("busca {}", person.value), &[], 20)
        .unwrap();
    assert_evidence(
        &person_hits[0].evidence,
        &person.value,
        Some(&person.label),
        &corpus,
    );

    // Tipo: no se fija ningún vocabulario de negocio; se usa el campo hallado
    // en el fixture configurado.
    let kind = facts
        .kind
        .as_ref()
        .expect("el corpus debe incluir un campo de tipo");
    let kind_hits = tools
        .search(&format!("{} {}", kind.label, kind.value), &[], 20)
        .unwrap();
    assert!(!kind_hits.is_empty());
    assert_evidence(
        &kind_hits[0].evidence,
        &kind.value,
        Some(&kind.label),
        &corpus,
    );

    // Categoría/carpeta: se consulta el metadato de procedencia del índice,
    // que es la evidencia pertinente para una relación de carpeta.
    let origin = facts
        .origins
        .first()
        .expect("se esperaba una carpeta de origen");
    let category_hits = tools.search(origin, &[], 20).unwrap();
    assert!(!category_hits.is_empty());
    assert!(
        category_hits
            .iter()
            .all(|hit| hit.evidence.origin == *origin)
    );
    assert!(
        category_hits
            .iter()
            .all(|hit| !hit.evidence.location.is_empty())
    );
    assert!(
        category_hits
            .iter()
            .all(|hit| hit.evidence.field.as_deref() == Some("carpeta de origen"))
    );

    // Un encabezado repetido no desplaza a la coincidencia de campo exacta.
    if let Some(repeated) = facts.repeated_line.as_deref() {
        assert_ne!(state_hits[0].evidence.excerpt.trim(), repeated.trim());
    }

    let answer = engine.ask(&identifier.value).unwrap();
    assert!(
        answer
            .text
            .ends_with("resultados con evidencia específica.")
    );
    assert!(!answer.citations.is_empty());
    assert_evidence(
        &answer.citations[0],
        &identifier.value,
        Some(&identifier.label),
        &corpus,
    );
    assert!(!answer.text.contains("Encontré"));
    assert!(!answer.text.contains("resumen"));

    let global = engine
        .ask("¿Cuántos documentos hay indexados y qué categorías contiene el acervo?")
        .unwrap();
    assert!(global.text.contains("índice completo"));
    assert!(
        global.citations.is_empty(),
        "una consulta global no usa cinco archivos como muestra"
    );

    let absent = engine
        .ask("zzzxqv dato completamente ausente qqqwww")
        .unwrap();
    assert_eq!(
        absent.text,
        "No encontré documentos que coincidan exactamente con ese criterio."
    );
    assert!(absent.citations.is_empty());
}

#[derive(Clone)]
struct FieldFact {
    label: String,
    value: String,
}

struct CorpusFacts {
    files: Vec<PathBuf>,
    identifier: Option<FieldFact>,
    state: Option<FieldFact>,
    kind: Option<FieldFact>,
    person: Option<FieldFact>,
    origins: Vec<String>,
    repeated_line: Option<String>,
}

fn inspect_corpus(root: &Path) -> CorpusFacts {
    let files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|part| part.to_str()),
                Some("txt") | Some("md")
            )
        })
        .collect::<Vec<_>>();
    let mut identifier = None;
    let mut state = None;
    let mut kind = None;
    let mut person = None;
    let mut origins = Vec::new();
    let mut line_counts = HashMap::<String, usize>::new();

    for path in &files {
        let parent = path.parent().unwrap_or(root);
        let relative = parent
            .strip_prefix(root)
            .unwrap_or(parent)
            .to_string_lossy()
            .trim_matches('/')
            .to_owned();
        let origin = if relative.is_empty() || relative == "." {
            root.file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("fuente")
                .to_owned()
        } else {
            relative
        };
        if !origins.contains(&origin) {
            origins.push(origin);
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            *line_counts.entry(line.to_owned()).or_default() += 1;
            let Some((label, value)) = line.split_once(':') else {
                continue;
            };
            let label = label.trim();
            let value = value.trim();
            if label.is_empty() || value.is_empty() {
                continue;
            }
            let normalized = label.to_lowercase();
            let field = FieldFact {
                label: label.to_owned(),
                value: value.to_owned(),
            };
            if identifier.is_none() && looks_like_identifier(value) {
                identifier = Some(field.clone());
            }
            if state.is_none() && normalized.contains("estado") {
                state = Some(field.clone());
            }
            if kind.is_none() && normalized.contains("tipo") {
                kind = Some(field);
            } else if person.is_none() && looks_like_proper_name(value) {
                person = Some(field);
            }
        }
    }
    origins.sort();
    CorpusFacts {
        files,
        identifier,
        state,
        kind,
        person,
        origins,
        repeated_line: line_counts
            .into_iter()
            .find_map(|(line, count)| (count > 1).then_some(line)),
    }
}

fn looks_like_identifier(value: &str) -> bool {
    value.chars().any(|character| character.is_ascii_digit())
        && value
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        && (value.contains('-') || value.contains('/') || value.contains('_'))
}

fn looks_like_proper_name(value: &str) -> bool {
    let words = value.split_whitespace().collect::<Vec<_>>();
    words.len() >= 2
        && words.len() <= 8
        && words
            .iter()
            .all(|word| word.chars().next().map(char::is_uppercase).unwrap_or(false))
}

fn files_with_exact_field_value(files: &[PathBuf], label: &str, value: &str) -> HashSet<PathBuf> {
    let expected_line = format!("{label}: {value}");
    files
        .iter()
        .filter(|path| {
            fs::read_to_string(path)
                .map(|text| text.lines().any(|line| line.trim() == expected_line))
                .unwrap_or(false)
        })
        .map(|path| path.canonicalize().unwrap())
        .collect()
}

fn assert_evidence(evidence: &omega_core::Evidence, value: &str, field: Option<&str>, root: &Path) {
    assert!(Path::new(&evidence.path).starts_with(root));
    assert!(!evidence.origin.is_empty());
    assert!(!evidence.location.is_empty());
    assert!(evidence.excerpt.contains(value));
    if let Some(field) = field {
        assert_eq!(evidence.field.as_deref(), Some(field));
    }
}
