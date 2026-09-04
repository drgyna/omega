//! Infraestructura de QA local. El oráculo de este módulo lee los Markdown
//! directamente y nunca llama al planificador, ToolEngine, SQL ni a los
//! normalizadores de producción para calcular resultados esperados.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::{Answer, OmegaEngine};

const MAX_COUNT_CITATIONS: usize = 24;
const MAX_AGGREGATE_CITATIONS: usize = 18;
const MAX_TEXT_CITATIONS: usize = 20;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvaluationManifest {
    pub version: u32,
    pub seed: u64,
    pub corpora: Vec<CorpusConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CorpusConfig {
    pub id: String,
    pub path: String,
    /// Un fixture explícito evita derivar el oráculo con los parsers de
    /// producción cuando el corpus contiene formatos binarios u OCR.
    #[serde(default)]
    pub fixture: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug)]
pub struct EvaluationOptions {
    pub manifest_path: PathBuf,
    pub corpus_id: Option<String>,
    pub report_root: PathBuf,
    pub stress: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StageMetric {
    pub corpus: String,
    pub stage: String,
    pub duration_ms: u128,
    pub documents: usize,
    pub peak_memory_bytes_approx: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunSummary {
    pub manifest_version: u32,
    pub seed: u64,
    pub corpora: usize,
    pub documents: usize,
    pub cases: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: u128,
    pub stages: Vec<StageMetric>,
    pub peak_memory_bytes_approx: Option<u64>,
    pub report_directory: String,
}

#[derive(Clone, Debug)]
pub struct EvaluationOutput {
    pub report_directory: PathBuf,
    pub summary: RunSummary,
    pub results: Vec<CaseResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvalCase {
    pub id: String,
    pub corpus: String,
    pub category: String,
    pub format: Option<String>,
    pub file: Option<String>,
    pub question: String,
    pub facts: Value,
    #[serde(skip)]
    expectation: Expectation,
}

#[derive(Clone, Debug)]
enum Expectation {
    Inventory {
        total: usize,
        origins: BTreeMap<String, usize>,
    },
    Documents {
        count: usize,
        paths: BTreeSet<String>,
        origin: Option<String>,
    },
    Exact {
        value: String,
        paths: BTreeSet<String>,
    },
    Absent,
    Sum {
        field: String,
        total: f64,
        values: usize,
        currency: Option<String>,
    },
    GroupSum {
        value_field: String,
        group_field: String,
        groups: BTreeMap<String, f64>,
        values: usize,
    },
    Text {
        phrase: String,
        paths: BTreeSet<String>,
    },
    Keywords {
        terms: Vec<String>,
        path_contains: String,
    },
    Location {
        value: String,
        path: String,
        location_contains: String,
    },
    Skipped {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Serialize)]
pub struct CitationRecord {
    pub path: String,
    pub origin: String,
    pub location: String,
    pub excerpt: String,
    pub field: Option<String>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub corpus: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub question: String,
    pub expected: Value,
    pub obtained: Value,
    pub status: CaseStatus,
    pub duration_ms: u128,
    pub citations: Vec<CitationRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexing_errors: Vec<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct FormatFixture {
    version: u32,
    expected_index: ExpectedIndex,
    cases: Vec<FormatFixtureCase>,
}

#[derive(Clone, Debug, Deserialize)]
struct ExpectedIndex {
    discovered: usize,
    indexed: usize,
    skipped: usize,
    warnings_for: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct FormatFixtureCase {
    id: String,
    category: String,
    format: String,
    file: String,
    question: String,
    expectation: FormatExpectation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FormatExpectation {
    Inventory {
        total: usize,
        origins: BTreeMap<String, usize>,
    },
    Documents {
        count: usize,
        #[serde(default)]
        origin: Option<String>,
    },
    Location {
        value: String,
        path: String,
        location_contains: String,
    },
    Sum {
        field: String,
        total: f64,
        values: usize,
        #[serde(default)]
        currency: Option<String>,
    },
    GroupSum {
        value_field: String,
        group_field: String,
        groups: BTreeMap<String, f64>,
        values: usize,
    },
    Keywords {
        terms: Vec<String>,
        path_contains: String,
    },
    Absent,
}

#[derive(Clone, Debug)]
struct OracleCorpus {
    id: String,
    root: PathBuf,
    documents: Vec<OracleDocument>,
    origins: BTreeMap<String, usize>,
}

#[derive(Clone, Debug)]
struct OracleDocument {
    path: String,
    origin: String,
    fields: Vec<(String, String)>,
    paragraphs: Vec<String>,
    raw: String,
}

#[derive(Clone, Debug)]
struct MoneySeries {
    field: String,
    currency: Option<String>,
    values: Vec<(usize, f64)>,
}

pub fn load_manifest(path: &Path) -> Result<EvaluationManifest, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("no se pudo leer {}: {error}", path.display()))?;
    let manifest: EvaluationManifest = serde_json::from_str(&text)
        .map_err(|error| format!("manifiesto inválido {}: {error}", path.display()))?;
    if manifest.version != 1 {
        return Err(format!(
            "versión de manifiesto no soportada: {}",
            manifest.version
        ));
    }
    let mut ids = BTreeSet::new();
    for corpus in &manifest.corpora {
        if corpus.id.trim().is_empty() || corpus.path.trim().is_empty() {
            return Err("cada corpus necesita id y path".into());
        }
        if !ids.insert(corpus.id.clone()) {
            return Err(format!("id de corpus duplicado: {}", corpus.id));
        }
    }
    Ok(manifest)
}

pub fn generate_cases_from_directory(
    corpus_id: &str,
    root: &Path,
) -> Result<Vec<EvalCase>, String> {
    let corpus = read_oracle_corpus(corpus_id, root)?;
    Ok(generate_cases(&corpus))
}

pub fn run_evaluations(options: &EvaluationOptions) -> Result<EvaluationOutput, String> {
    let started = Instant::now();
    let manifest = load_manifest(&options.manifest_path)?;
    let manifest_directory = options
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let selected = manifest
        .corpora
        .iter()
        .filter(|corpus| corpus.enabled)
        .filter(|corpus| {
            options
                .corpus_id
                .as_ref()
                .is_none_or(|wanted| wanted == &corpus.id)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() && !options.stress {
        return Err(match &options.corpus_id {
            Some(id) => format!("no existe un corpus activo con id '{id}'"),
            None => "el manifiesto no contiene corpus activos".into(),
        });
    }

    let report_directory = create_report_directory(&options.report_root)?;
    if options.stress {
        let (documents, results, stages) = run_stress_evaluation()?;
        return finish_run(
            &manifest,
            &report_directory,
            started,
            documents,
            results,
            stages,
        );
    }
    let mut results = Vec::new();
    let mut document_count = 0usize;
    let mut stages = Vec::new();
    for config in selected {
        let root = manifest_directory.join(&config.path);
        if let Some(fixture_path) = &config.fixture {
            match run_format_corpus(config, &root, &manifest_directory.join(fixture_path)) {
                Ok((documents, corpus_results, corpus_stages)) => {
                    document_count += documents;
                    results.extend(corpus_results);
                    stages.extend(corpus_stages);
                }
                Err(error) => results.push(system_failure(&config.id, "fixture", error)),
            }
            continue;
        }
        let corpus_started = Instant::now();
        match read_oracle_corpus(&config.id, &root) {
            Ok(corpus) => {
                document_count += corpus.documents.len();
                let cases = generate_cases(&corpus);
                results.extend(run_corpus(&corpus, cases));
                stages.push(StageMetric {
                    corpus: config.id.clone(),
                    stage: "lectura, indexación y preguntas".into(),
                    duration_ms: corpus_started.elapsed().as_millis(),
                    documents: corpus.documents.len(),
                    peak_memory_bytes_approx: resident_memory_bytes(),
                });
            }
            Err(error) => results.push(system_failure(&config.id, "lectura", error)),
        }
    }

    finish_run(
        &manifest,
        &report_directory,
        started,
        document_count,
        results,
        stages,
    )
}

fn finish_run(
    manifest: &EvaluationManifest,
    report_directory: &Path,
    started: Instant,
    document_count: usize,
    results: Vec<CaseResult>,
    stages: Vec<StageMetric>,
) -> Result<EvaluationOutput, String> {
    let summary = RunSummary {
        manifest_version: manifest.version,
        seed: manifest.seed,
        corpora: results
            .iter()
            .map(|result| result.corpus.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        documents: document_count,
        cases: results.len(),
        passed: results
            .iter()
            .filter(|result| result.status == CaseStatus::Passed)
            .count(),
        failed: results
            .iter()
            .filter(|result| result.status == CaseStatus::Failed)
            .count(),
        skipped: results
            .iter()
            .filter(|result| result.status == CaseStatus::Skipped)
            .count(),
        duration_ms: started.elapsed().as_millis(),
        peak_memory_bytes_approx: stages
            .iter()
            .filter_map(|stage| stage.peak_memory_bytes_approx)
            .max(),
        stages,
        report_directory: report_directory.display().to_string(),
    };
    write_reports(report_directory, &summary, &results)?;
    Ok(EvaluationOutput {
        report_directory: report_directory.to_path_buf(),
        summary,
        results,
    })
}

fn read_oracle_corpus(id: &str, root: &Path) -> Result<OracleCorpus, String> {
    if !root.is_dir() {
        return Err(format!("la carpeta no existe: {}", root.display()));
    }
    let mut paths = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("md"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err("el corpus no contiene Markdown".into());
    }

    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut documents = Vec::with_capacity(paths.len());
    let mut origins = BTreeMap::new();
    for path in paths {
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("no se pudo leer {}: {error}", path.display()))?;
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let origin = relative
            .components()
            .next()
            .map(|component| component.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| ".".into());
        *origins.entry(origin.clone()).or_insert(0) += 1;
        let mut fields = Vec::new();
        let mut paragraphs = Vec::new();
        for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if !line.starts_with('#') {
                if let Some((label, value)) = parse_field_line(line) {
                    fields.push((label, value));
                    continue;
                }
                if line.chars().count() >= 100 {
                    paragraphs.push(line.to_owned());
                }
            }
        }
        let absolute = path
            .canonicalize()
            .unwrap_or_else(|_| canonical_root.join(relative));
        documents.push(OracleDocument {
            path: absolute.display().to_string(),
            origin,
            fields,
            paragraphs,
            raw,
        });
    }
    Ok(OracleCorpus {
        id: id.into(),
        root: canonical_root,
        documents,
        origins,
    })
}

fn parse_field_line(line: &str) -> Option<(String, String)> {
    let (label, value) = line.split_once(':')?;
    let label = label.trim().trim_start_matches(['-', '*']).trim();
    let value = value.trim();
    let valid_label = !label.is_empty()
        && label.chars().count() <= 80
        && label.chars().any(char::is_alphabetic)
        && !label.contains("http");
    (valid_label && !value.is_empty()).then(|| (label.to_owned(), value.to_owned()))
}

fn generate_cases(corpus: &OracleCorpus) -> Vec<EvalCase> {
    let mut cases = Vec::new();
    let mut serials: HashMap<&str, usize> = HashMap::new();
    let mut push =
        |category: &'static str, question: String, facts: Value, expectation: Expectation| {
            let serial = serials.entry(category).or_insert(0);
            *serial += 1;
            cases.push(EvalCase {
                id: format!("{}-{}-{:03}", corpus.id, category.to_lowercase(), *serial),
                corpus: corpus.id.clone(),
                category: category.into(),
                format: Some("Markdown".into()),
                file: None,
                question,
                facts,
                expectation,
            });
        };

    push(
        "A",
        "¿Cuántos documentos hay indexados y qué categorías contiene el acervo?".into(),
        json!({"documents": corpus.documents.len(), "origins": corpus.origins}),
        Expectation::Inventory {
            total: corpus.documents.len(),
            origins: corpus.origins.clone(),
        },
    );
    for (origin, count) in &corpus.origins {
        let paths = corpus
            .documents
            .iter()
            .filter(|document| &document.origin == origin)
            .map(|document| document.path.clone())
            .collect::<BTreeSet<_>>();
        push(
            "A",
            format!("¿Cuántos documentos pertenecen a la carpeta {origin}?"),
            json!({"origin": origin, "documents": count}),
            Expectation::Documents {
                count: *count,
                paths,
                origin: Some(origin.clone()),
            },
        );
    }

    for origin in corpus.origins.keys() {
        let identifier = corpus
            .documents
            .iter()
            .filter(|document| &document.origin == origin)
            .flat_map(|document| document.fields.iter())
            .find(|(label, value)| is_identifier_field(label, value));
        if let Some((_, value)) = identifier {
            let paths = corpus
                .documents
                .iter()
                .filter(|document| document.raw.contains(value))
                .map(|document| document.path.clone())
                .collect::<BTreeSet<_>>();
            push(
                "B",
                format!("Encuentra exactamente \"{value}\"."),
                json!({"identifier": value, "paths": paths}),
                Expectation::Exact {
                    value: value.clone(),
                    paths,
                },
            );
        } else {
            push(
                "B",
                String::new(),
                json!({"origin": origin}),
                Expectation::Skipped {
                    reason: format!("{origin} no contiene identificadores completos"),
                },
            );
        }
    }
    let absent = format!("OMEGA-EVAL-{}-999999", corpus.id.to_uppercase());
    push(
        "B",
        format!("Encuentra exactamente \"{absent}\"."),
        json!({"absent_identifier": absent}),
        Expectation::Absent,
    );

    let field_sets = field_value_paths(corpus);
    let repeated = field_sets
        .iter()
        .filter(|((label, value), paths)| {
            paths.len() >= 2
                && paths.len() < corpus.documents.len()
                && !is_identifier_field(label, value)
        })
        .collect::<Vec<_>>();
    if let Some(((label, value), paths)) = repeated.first() {
        push(
            "C",
            format!("¿Cuántos documentos tienen {label}: {value}?"),
            json!({"filters": [{"field": label, "value": value}], "documents": paths.len()}),
            Expectation::Documents {
                count: paths.len(),
                paths: (*paths).clone(),
                origin: None,
            },
        );
    } else {
        push(
            "C",
            String::new(),
            json!({}),
            Expectation::Skipped {
                reason: "no hay valores estructurados repetidos adecuados".into(),
            },
        );
    }
    if let Some((left, right, intersection)) = find_intersection(&repeated) {
        push(
            "C",
            format!(
                "Muestra documentos con {}: {} y {}: {}.",
                left.0, left.1, right.0, right.1
            ),
            json!({"filters": [
                {"field": left.0, "value": left.1},
                {"field": right.0, "value": right.1}
            ], "documents": intersection.len()}),
            Expectation::Documents {
                count: intersection.len(),
                paths: intersection,
                origin: None,
            },
        );
    } else {
        push(
            "C",
            String::new(),
            json!({}),
            Expectation::Skipped {
                reason: "no hay dos filtros repetidos con intersección no vacía".into(),
            },
        );
    }

    let money = discover_money_series(corpus);
    if let Some(series) = money.first() {
        let total = series.values.iter().map(|(_, value)| value).sum::<f64>();
        let currency_phrase = series
            .currency
            .as_deref()
            .map(|currency| format!(" en {currency}"))
            .unwrap_or_default();
        push(
            "D",
            format!("Suma el campo {}{}.", series.field, currency_phrase),
            json!({"field": series.field, "total": total, "values": series.values.len(), "currency": series.currency}),
            Expectation::Sum {
                field: series.field.clone(),
                total,
                values: series.values.len(),
                currency: series.currency.clone(),
            },
        );
        push(
            "D",
            format!("¿Cuántos valores tiene el campo {}?", series.field),
            json!({"field": series.field, "values": series.values.len()}),
            Expectation::Sum {
                field: series.field.clone(),
                total,
                values: series.values.len(),
                currency: series.currency.clone(),
            },
        );
        if let Some((group_field, groups)) = discover_grouping(corpus, series) {
            push(
                "D",
                format!("Agrupa la suma de {} por {}.", series.field, group_field),
                json!({"value_field": series.field, "group_field": group_field, "groups": groups, "values": series.values.len()}),
                Expectation::GroupSum {
                    value_field: series.field.clone(),
                    group_field,
                    groups,
                    values: series.values.len(),
                },
            );
        } else {
            push(
                "D",
                String::new(),
                json!({"field": series.field}),
                Expectation::Skipped {
                    reason: "no hay un campo repetido adecuado para agrupar la serie".into(),
                },
            );
        }
    } else {
        for reason in [
            "no hay una serie monetaria consistente con cinco valores",
            "sin serie monetaria no se genera conteo de valores",
            "sin serie monetaria no se genera agrupación",
        ] {
            push(
                "D",
                String::new(),
                json!({}),
                Expectation::Skipped {
                    reason: reason.into(),
                },
            );
        }
    }

    let text_cases = discover_text_phrases(corpus);
    if text_cases.is_empty() {
        push(
            "E",
            String::new(),
            json!({}),
            Expectation::Skipped {
                reason: "no hay párrafos repetidos con una frase distintiva".into(),
            },
        );
    } else {
        for (phrase, paths) in text_cases.into_iter().take(2) {
            push(
                "E",
                format!("¿Qué información contienen los documentos sobre \"{phrase}\"?"),
                json!({"phrase": phrase, "paths": paths}),
                Expectation::Text { phrase, paths },
            );
        }
    }

    push(
        "F",
        format!(
            "¿Cuántos documentos mencionan \"AUSENCIA TOTAL {} 774411\"?",
            corpus.id
        ),
        json!({"expected": "no evidence", "max_citations": 0}),
        Expectation::Absent,
    );
    cases
}

fn field_value_paths(corpus: &OracleCorpus) -> BTreeMap<(String, String), BTreeSet<String>> {
    let mut values = BTreeMap::new();
    for document in &corpus.documents {
        for (label, value) in &document.fields {
            values
                .entry((label.clone(), value.clone()))
                .or_insert_with(BTreeSet::new)
                .insert(document.path.clone());
        }
    }
    values
}

type RepeatedField<'a> = (&'a (String, String), &'a BTreeSet<String>);

fn find_intersection(
    repeated: &[RepeatedField<'_>],
) -> Option<((String, String), (String, String), BTreeSet<String>)> {
    let mut fallback = None;
    for (index, (left, left_paths)) in repeated.iter().enumerate() {
        for (right, right_paths) in repeated.iter().skip(index + 1) {
            if left.0 == right.0 {
                continue;
            }
            let intersection = left_paths
                .intersection(right_paths)
                .cloned()
                .collect::<BTreeSet<_>>();
            if intersection.len() < 2 {
                continue;
            }
            let candidate = ((*left).clone(), (*right).clone(), intersection);
            if candidate.2.len() < left_paths.len().min(right_paths.len()) {
                return Some(candidate);
            }
            fallback.get_or_insert(candidate);
        }
    }
    fallback
}

fn is_identifier_field(label: &str, value: &str) -> bool {
    let label = label.to_lowercase();
    let relevant = [
        "folio",
        "identificador",
        "número de control",
        "numero de control",
        "número de póliza",
        "numero de poliza",
        "instrumento",
        "expediente",
    ]
    .iter()
    .any(|candidate| label.contains(candidate));
    relevant
        && value.chars().any(char::is_alphabetic)
        && value.chars().any(char::is_numeric)
        && value.chars().count() >= 6
}

fn discover_money_series(corpus: &OracleCorpus) -> Vec<MoneySeries> {
    let money = Regex::new(r"^\$\s*([0-9][0-9,]*(?:\.[0-9]{1,2})?)(?:\s+([A-Z]{3}))?$")
        .expect("valid independent money regex");
    let mut series: BTreeMap<(String, Option<String>), Vec<(usize, f64)>> = BTreeMap::new();
    for (index, document) in corpus.documents.iter().enumerate() {
        for (field, raw) in &document.fields {
            let Some(capture) = money.captures(raw) else {
                continue;
            };
            let numeric = capture[1].replace(',', "").parse::<f64>().ok();
            if let Some(numeric) = numeric {
                series
                    .entry((
                        field.clone(),
                        capture.get(2).map(|value| value.as_str().to_owned()),
                    ))
                    .or_default()
                    .push((index, numeric));
            }
        }
    }
    let mut result = series
        .into_iter()
        .filter(|(_, values)| values.len() >= 5)
        .map(|((field, currency), values)| MoneySeries {
            field,
            currency,
            values,
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| {
        right
            .values
            .len()
            .cmp(&left.values.len())
            .then_with(|| left.field.cmp(&right.field))
    });
    result
}

fn discover_grouping(
    corpus: &OracleCorpus,
    series: &MoneySeries,
) -> Option<(String, BTreeMap<String, f64>)> {
    let value_by_document = series
        .values
        .iter()
        .copied()
        .collect::<BTreeMap<usize, f64>>();
    let mut fields = BTreeSet::new();
    for (index, _) in &series.values {
        for (label, _) in &corpus.documents[*index].fields {
            if label != &series.field {
                fields.insert(label.clone());
            }
        }
    }
    let mut candidates = Vec::new();
    for field in fields {
        let mut groups = BTreeMap::new();
        let mut covered = 0usize;
        for (index, value) in &value_by_document {
            let values = corpus.documents[*index]
                .fields
                .iter()
                .filter(|(label, _)| label == &field)
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            if values.len() == 1 {
                covered += 1;
                *groups.entry(values[0].clone()).or_insert(0.0) += value;
            }
        }
        if covered == series.values.len() && (2..=12).contains(&groups.len()) {
            candidates.push((groups.len(), field, groups));
        }
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .next()
        .map(|(_, field, groups)| (field, groups))
}

fn discover_text_phrases(corpus: &OracleCorpus) -> Vec<(String, BTreeSet<String>)> {
    const WORDS_PER_PHRASE: usize = 6;
    let mut phrases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for document in &corpus.documents {
        for paragraph in &document.paragraphs {
            let words = paragraph
                .split_whitespace()
                .map(|word| {
                    word.trim_matches(|character: char| !character.is_alphanumeric())
                        .to_owned()
                })
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>();
            for window in words.windows(WORDS_PER_PHRASE) {
                if window
                    .iter()
                    .filter(|word| word.chars().count() >= 5)
                    .count()
                    < 2
                {
                    continue;
                }
                phrases
                    .entry(window.join(" "))
                    .or_default()
                    .insert(document.path.clone());
            }
        }
    }
    let mut found = Vec::new();
    for (phrase, paths) in phrases {
        if paths.len() < 2 {
            continue;
        }
        let expected = corpus
            .documents
            .iter()
            .filter(|document| document.raw.contains(&phrase))
            .map(|document| document.path.clone())
            .collect::<BTreeSet<_>>();
        if expected.len() >= 2 {
            found.push((phrase, expected));
        }
    }
    found.sort_by(|left, right| {
        left.1
            .len()
            .cmp(&right.1.len())
            .then_with(|| left.0.cmp(&right.0))
    });
    found
}

fn run_format_corpus(
    config: &CorpusConfig,
    root: &Path,
    fixture_path: &Path,
) -> Result<(usize, Vec<CaseResult>, Vec<StageMetric>), String> {
    let fixture_text = fs::read_to_string(fixture_path)
        .map_err(|error| format!("no se pudo leer {}: {error}", fixture_path.display()))?;
    let fixture: FormatFixture = serde_json::from_str(&fixture_text)
        .map_err(|error| format!("fixture inválido {}: {error}", fixture_path.display()))?;
    if fixture.version != 1 {
        return Err(format!(
            "versión de fixture no soportada: {}",
            fixture.version
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("no se pudo abrir {}: {error}", root.display()))?;
    let temp = temporary_directory(&config.id);
    fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    let database = temp.join("evaluation.db");
    let engine = OmegaEngine::open(&database).map_err(|error| error.to_string())?;
    let source_id = engine
        .authorize_source(&canonical_root)
        .map_err(|error| error.to_string())?;

    let index_started = Instant::now();
    let index_report = engine
        .index_source(source_id)
        .map_err(|error| error.to_string())?;
    let index_duration = index_started.elapsed();
    let warnings_ok = fixture.expected_index.warnings_for.iter().all(|file| {
        index_report
            .warnings
            .iter()
            .any(|warning| warning.contains(file))
    });
    let index_ok = index_report.discovered == fixture.expected_index.discovered
        && index_report.indexed == fixture.expected_index.indexed
        && index_report.skipped == fixture.expected_index.skipped
        && warnings_ok;
    let mut results = vec![direct_result(
        format!("{}-index-contract", config.id),
        &config.id,
        "indexación",
        Some("multiformato".into()),
        Some("*".into()),
        "Indexar todos los formatos sin aceptar archivos sin evidencia.",
        json!({
            "discovered": fixture.expected_index.discovered,
            "indexed": fixture.expected_index.indexed,
            "skipped": fixture.expected_index.skipped,
            "warnings_for": fixture.expected_index.warnings_for,
        }),
        serde_json::to_value(&index_report).unwrap_or_default(),
        index_ok,
        index_duration,
        index_report.warnings.clone(),
        "el contrato de indexación o sus avisos por archivo no coincide",
    )];

    let allowed_paths = WalkDir::new(&canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| canonical_path_string(entry.path()))
        .collect::<BTreeSet<_>>();
    let question_started = Instant::now();
    for fixture_case in fixture.cases {
        let eval_case =
            format_fixture_case(&config.id, &canonical_root, &allowed_paths, fixture_case);
        results.push(evaluate_case(&engine, eval_case));
    }
    let question_duration = question_started.elapsed();
    drop(engine);

    let resilience_started = Instant::now();
    results.extend(run_resilience_cases(&config.id, &canonical_root)?);
    let resilience_duration = resilience_started.elapsed();
    let _ = fs::remove_dir_all(&temp);

    let stages = vec![
        StageMetric {
            corpus: config.id.clone(),
            stage: "indexación multiformato".into(),
            duration_ms: index_duration.as_millis(),
            documents: index_report.indexed,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
        StageMetric {
            corpus: config.id.clone(),
            stage: "preguntas con oráculo explícito".into(),
            duration_ms: question_duration.as_millis(),
            documents: index_report.indexed,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
        StageMetric {
            corpus: config.id.clone(),
            stage: "reindexación, reapertura y seguridad".into(),
            duration_ms: resilience_duration.as_millis(),
            documents: index_report.indexed,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
    ];
    Ok((index_report.indexed, results, stages))
}

fn format_fixture_case(
    corpus_id: &str,
    root: &Path,
    allowed_paths: &BTreeSet<String>,
    fixture: FormatFixtureCase,
) -> EvalCase {
    let (facts, expectation) = match fixture.expectation {
        FormatExpectation::Inventory { total, origins } => (
            json!({"documents": total, "origins": origins}),
            Expectation::Inventory { total, origins },
        ),
        FormatExpectation::Documents { count, origin } => (
            json!({"documents": count, "origin": origin}),
            Expectation::Documents {
                count,
                paths: allowed_paths.clone(),
                origin,
            },
        ),
        FormatExpectation::Location {
            value,
            path,
            location_contains,
        } => {
            let absolute = canonical_path_string(&root.join(&path));
            (
                json!({"value": value, "path": path, "location_contains": location_contains}),
                Expectation::Location {
                    value,
                    path: absolute,
                    location_contains,
                },
            )
        }
        FormatExpectation::Sum {
            field,
            total,
            values,
            currency,
        } => (
            json!({"field": field, "total": total, "values": values, "currency": currency}),
            Expectation::Sum {
                field,
                total,
                values,
                currency,
            },
        ),
        FormatExpectation::GroupSum {
            value_field,
            group_field,
            groups,
            values,
        } => (
            json!({"value_field": value_field, "group_field": group_field, "groups": groups, "values": values}),
            Expectation::GroupSum {
                value_field,
                group_field,
                groups,
                values,
            },
        ),
        FormatExpectation::Keywords {
            terms,
            path_contains,
        } => (
            json!({"terms": terms, "path_contains": path_contains}),
            Expectation::Keywords {
                terms,
                path_contains,
            },
        ),
        FormatExpectation::Absent => (json!({"expected": "no evidence"}), Expectation::Absent),
    };
    EvalCase {
        id: fixture.id,
        corpus: corpus_id.into(),
        category: fixture.category,
        format: Some(fixture.format),
        file: Some(fixture.file),
        question: fixture.question,
        facts,
        expectation,
    }
}

#[allow(clippy::too_many_arguments)]
fn direct_result(
    id: String,
    corpus: &str,
    category: &str,
    format: Option<String>,
    file: Option<String>,
    question: &str,
    expected: Value,
    obtained: Value,
    passed: bool,
    duration: Duration,
    indexing_errors: Vec<String>,
    failure: &str,
) -> CaseResult {
    CaseResult {
        id,
        corpus: corpus.into(),
        category: category.into(),
        format,
        file,
        question: question.into(),
        expected,
        obtained,
        status: if passed {
            CaseStatus::Passed
        } else {
            CaseStatus::Failed
        },
        duration_ms: duration.as_millis(),
        citations: vec![],
        indexing_errors,
        error: (!passed).then(|| failure.into()),
    }
}

fn run_resilience_cases(corpus_id: &str, source: &Path) -> Result<Vec<CaseResult>, String> {
    let temp = temporary_directory(&format!("{corpus_id}-resilience"));
    let working = temp.join("corpus");
    fs::create_dir_all(&working).map_err(|error| error.to_string())?;
    copy_directory(source, &working)?;
    let outside = temp.join("fuera-de-fuente.txt");
    fs::write(
        &outside,
        "Folio: OUTSIDE-26-0001\nSecreto fuera de la fuente autorizada.",
    )
    .map_err(|error| error.to_string())?;
    create_test_symlink(&outside, &working.join("enlace-fuera.txt"))?;

    let database = temp.join("resilience.db");
    let engine = OmegaEngine::open(&database).map_err(|error| error.to_string())?;
    let source_id = engine
        .authorize_source(&working)
        .map_err(|error| error.to_string())?;
    let initial = engine
        .index_source(source_id)
        .map_err(|error| error.to_string())?;

    fs::remove_file(working.join("01_pdf_texto/001-contrato-de-suministro.pdf"))
        .map_err(|error| error.to_string())?;
    fs::remove_file(working.join("03_word_docx/001-minuta-de-comité.docx"))
        .map_err(|error| error.to_string())?;
    let modified_path = working.join("06_markdown_largo/001-política-de-conservación.md");
    let modified = fs::read_to_string(&modified_path)
        .map_err(|error| error.to_string())?
        .replace("FMT-26-0071", "FMT-26-9071");
    fs::write(&modified_path, modified).map_err(|error| error.to_string())?;
    fs::write(
        working.join("06_markdown_largo/011-agregado.md"),
        "# Documento agregado\n\nFolio: ADD-26-0001\nEstado: Activo\nImporte total: $1,111.00 MXN\n\nEvidencia nueva para validar la reindexación local.",
    )
    .map_err(|error| error.to_string())?;

    let reindex_started = Instant::now();
    let reindexed = engine
        .index_source(source_id)
        .map_err(|error| error.to_string())?;
    let removed_absent = ["FMT-26-0001", "FMT-26-0051", "FMT-26-0071"]
        .iter()
        .all(|value| {
            engine
                .ask(&format!("Encuentra exactamente {value}."))
                .is_ok_and(|answer| answer.citations.is_empty())
        });
    let added_present = ["FMT-26-9071", "ADD-26-0001"].iter().all(|value| {
        engine
            .ask(&format!("Encuentra exactamente {value}."))
            .is_ok_and(|answer| !answer.citations.is_empty())
    });
    let reindex_ok = initial.discovered == 54
        && reindexed.discovered == 53
        && reindexed.indexed == 52
        && reindexed.skipped == 1
        && reindexed.modified == 1
        && removed_absent
        && added_present;
    let mut results = vec![direct_result(
        format!("{corpus_id}-resilience-reindex"),
        corpus_id,
        "resiliencia",
        Some("multiformato".into()),
        Some("copia temporal mutada".into()),
        "Eliminar dos archivos, modificar uno, agregar otro y reindexar sin fantasmas.",
        json!({"initial_discovered": 54, "reindexed_discovered": 53, "indexed": 52, "skipped": 1, "modified": 1, "removed_absent": true, "added_present": true}),
        json!({"initial": initial, "reindexed": reindexed, "removed_absent": removed_absent, "added_present": added_present}),
        reindex_ok,
        reindex_started.elapsed(),
        vec![],
        "la reindexación dejó fantasmas, perdió altas o reportó conteos incorrectos",
    )];

    let unauthorized = engine.open_document(&outside).is_err();
    let outside_absent = engine
        .ask("Encuentra exactamente OUTSIDE-26-0001.")
        .is_ok_and(|answer| answer.citations.is_empty());
    results.push(direct_result(
        format!("{corpus_id}-security-paths"),
        corpus_id,
        "seguridad",
        Some("ruta/symlink".into()),
        Some("enlace-fuera.txt".into()),
        "Impedir acceso y evidencia fuera de la fuente autorizada, incluso mediante symlink.",
        json!({"unauthorized": true, "outside_absent": true}),
        json!({"unauthorized": unauthorized, "outside_absent": outside_absent}),
        unauthorized && outside_absent,
        Duration::ZERO,
        vec![],
        "una ruta externa fue autorizada o apareció como evidencia",
    ));

    drop(engine);
    let reopen_started = Instant::now();
    let reopened = OmegaEngine::open(&database).map_err(|error| error.to_string())?;
    let status = reopened.status().map_err(|error| error.to_string())?;
    let reopen_answer = reopened
        .ask("Encuentra exactamente ADD-26-0001.")
        .map_err(|error| error.to_string())?;
    let reopen_ok = status.documents == 52 && !reopen_answer.citations.is_empty();
    results.push(direct_result(
        format!("{corpus_id}-resilience-reopen"),
        corpus_id,
        "resiliencia",
        Some("SQLite".into()),
        Some("resilience.db".into()),
        "Cerrar y reabrir la base conservando índice y evidencia.",
        json!({"documents": 52, "identifier": "ADD-26-0001"}),
        json!({"documents": status.documents, "citations": reopen_answer.citations.len()}),
        reopen_ok,
        reopen_started.elapsed(),
        vec![],
        "la reapertura no conservó el índice utilizable",
    ));
    drop(reopened);

    let corrupt = temp.join("corrupt.db");
    fs::write(&corrupt, b"no es una base SQLite").map_err(|error| error.to_string())?;
    let corrupt_started = Instant::now();
    let corrupt_rejected = OmegaEngine::open(&corrupt).is_err();
    results.push(direct_result(
        format!("{corpus_id}-resilience-corrupt"),
        corpus_id,
        "resiliencia",
        Some("SQLite corrupto".into()),
        Some("corrupt.db".into()),
        "Abrir una base corrupta debe devolver un error controlado.",
        json!({"controlled_error": true}),
        json!({"controlled_error": corrupt_rejected}),
        corrupt_rejected,
        corrupt_started.elapsed(),
        vec![],
        "la base corrupta no produjo un error controlado",
    ));
    let _ = fs::remove_dir_all(&temp);
    Ok(results)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = entry.map_err(|error| error.to_string())?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|error| error.to_string())?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_test_symlink(target: &Path, link: &Path) -> Result<(), String> {
    std::os::unix::fs::symlink(target, link).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn create_test_symlink(_target: &Path, _link: &Path) -> Result<(), String> {
    Ok(())
}

fn run_stress_evaluation() -> Result<(usize, Vec<CaseResult>, Vec<StageMetric>), String> {
    const DOCUMENTS: usize = 5_000;
    let temp = temporary_directory("stress-5000");
    let corpus = temp.join("corpus");
    fs::create_dir_all(&corpus).map_err(|error| error.to_string())?;
    let generation_started = Instant::now();
    for index in 0..DOCUMENTS {
        let state = if index % 3 == 0 { "Activo" } else { "Cerrado" };
        fs::write(
            corpus.join(format!("stress-{index:05}.md")),
            format!(
                "# Registro sintético de estrés\n\nFolio: STRESS-26-{index:05}\nEstado: {state}\nImporte total: ${}.00 MXN\n\nDocumento local sintético para medir indexación y recuperación sin red.\n",
                1000 + index
            ),
        )
        .map_err(|error| error.to_string())?;
    }
    let generation_duration = generation_started.elapsed();
    let database = temp.join("stress.db");
    let engine = OmegaEngine::open(&database).map_err(|error| error.to_string())?;
    let source_id = engine
        .authorize_source(&corpus)
        .map_err(|error| error.to_string())?;
    let index_started = Instant::now();
    let report = engine
        .index_source(source_id)
        .map_err(|error| error.to_string())?;
    let index_duration = index_started.elapsed();
    let index_ok =
        report.discovered == DOCUMENTS && report.indexed == DOCUMENTS && report.skipped == 0;
    let mut results = vec![direct_result(
        "stress-index-5000".into(),
        "stress-5000",
        "estrés",
        Some("Markdown generado".into()),
        Some("5,000 archivos temporales".into()),
        "Indexar al menos 5,000 documentos sintéticos.",
        json!({"discovered": DOCUMENTS, "indexed": DOCUMENTS, "skipped": 0}),
        serde_json::to_value(&report).unwrap_or_default(),
        index_ok,
        index_duration,
        report.warnings.clone(),
        "la carga de estrés no indexó todos los documentos",
    )];
    let allowed = WalkDir::new(&corpus)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| canonical_path_string(entry.path()))
        .collect::<BTreeSet<_>>();
    let query_started = Instant::now();
    for case in [
        EvalCase {
            id: "stress-inventory".into(),
            corpus: "stress-5000".into(),
            category: "estrés".into(),
            format: Some("Markdown generado".into()),
            file: Some("*".into()),
            question: "¿Cuántos documentos hay indexados y qué categorías contiene el acervo?"
                .into(),
            facts: json!({"documents": DOCUMENTS, "origins": {"corpus": DOCUMENTS}}),
            expectation: Expectation::Inventory {
                total: DOCUMENTS,
                origins: BTreeMap::from([("corpus".into(), DOCUMENTS)]),
            },
        },
        EvalCase {
            id: "stress-exact-last".into(),
            corpus: "stress-5000".into(),
            category: "estrés".into(),
            format: Some("Markdown generado".into()),
            file: Some("stress-04999.md".into()),
            question: "Encuentra exactamente STRESS-26-04999.".into(),
            facts: json!({"value": "STRESS-26-04999"}),
            expectation: Expectation::Exact {
                value: "STRESS-26-04999".into(),
                paths: BTreeSet::from([canonical_path_string(&corpus.join("stress-04999.md"))]),
            },
        },
        EvalCase {
            id: "stress-active-count".into(),
            corpus: "stress-5000".into(),
            category: "estrés".into(),
            format: Some("Markdown generado".into()),
            file: Some("*".into()),
            question: "¿Cuántos documentos tienen Estado: Activo?".into(),
            facts: json!({"documents": 1667}),
            expectation: Expectation::Documents {
                count: 1667,
                paths: allowed,
                origin: None,
            },
        },
    ] {
        results.push(evaluate_case(&engine, case));
    }
    let query_duration = query_started.elapsed();
    let stages = vec![
        StageMetric {
            corpus: "stress-5000".into(),
            stage: "generación temporal".into(),
            duration_ms: generation_duration.as_millis(),
            documents: DOCUMENTS,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
        StageMetric {
            corpus: "stress-5000".into(),
            stage: "indexación".into(),
            duration_ms: index_duration.as_millis(),
            documents: DOCUMENTS,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
        StageMetric {
            corpus: "stress-5000".into(),
            stage: "consultas".into(),
            duration_ms: query_duration.as_millis(),
            documents: DOCUMENTS,
            peak_memory_bytes_approx: resident_memory_bytes(),
        },
    ];
    drop(engine);
    let _ = fs::remove_dir_all(&temp);
    Ok((DOCUMENTS, results, stages))
}

fn resident_memory_bytes() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: getrusage inicializa completamente la estructura indicada
        // cuando devuelve cero; RUSAGE_SELF no requiere punteros adicionales.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        // macOS expresa ru_maxrss en bytes; Linux y otros Unix, en KiB.
        let maximum = unsafe { usage.assume_init() }.ru_maxrss as u64;
        #[cfg(target_os = "macos")]
        return Some(maximum);
        #[cfg(not(target_os = "macos"))]
        return Some(maximum * 1024);
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn run_corpus(corpus: &OracleCorpus, cases: Vec<EvalCase>) -> Vec<CaseResult> {
    let temp = temporary_directory(&corpus.id);
    if let Err(error) = fs::create_dir_all(&temp) {
        return vec![system_failure(
            &corpus.id,
            "base_temporal",
            error.to_string(),
        )];
    }
    let database = temp.join("evaluation.db");
    let engine = match OmegaEngine::open(&database) {
        Ok(engine) => engine,
        Err(error) => {
            let result = system_failure(&corpus.id, "apertura", error.to_string());
            let _ = fs::remove_dir_all(&temp);
            return vec![result];
        }
    };
    let indexed = engine
        .authorize_source(&corpus.root)
        .and_then(|source| engine.index_source(source));
    if let Err(error) = indexed {
        let result = system_failure(&corpus.id, "indexacion", error.to_string());
        drop(engine);
        let _ = fs::remove_dir_all(&temp);
        return vec![result];
    }
    let mut results = cases
        .into_iter()
        .map(|case| evaluate_case(&engine, case))
        .collect::<Vec<_>>();
    // Los escenarios conversacionales usan el mismo motor ya indexado, pero
    // cada uno con su propia clave de conversación.
    results.extend(run_conversations(
        &engine,
        &corpus.id,
        generate_conversations(corpus),
    ));
    drop(engine);
    let _ = fs::remove_dir_all(&temp);
    results
}

fn evaluate_case(engine: &OmegaEngine, case: EvalCase) -> CaseResult {
    if let Expectation::Skipped { reason } = &case.expectation {
        return CaseResult {
            id: case.id,
            corpus: case.corpus,
            category: case.category,
            format: case.format,
            file: case.file,
            question: case.question,
            expected: case.facts,
            obtained: json!({}),
            status: CaseStatus::Skipped,
            duration_ms: 0,
            citations: vec![],
            indexing_errors: vec![],
            error: Some(reason.clone()),
        };
    }
    let started = Instant::now();
    match engine.ask(&case.question) {
        Ok(answer) => result_from_answer(case, answer, started.elapsed()),
        Err(error) => CaseResult {
            id: case.id,
            corpus: case.corpus,
            category: case.category,
            format: case.format,
            file: case.file,
            question: case.question,
            expected: case.facts,
            obtained: json!({}),
            status: CaseStatus::Failed,
            duration_ms: started.elapsed().as_millis(),
            citations: vec![],
            indexing_errors: vec![],
            error: Some(error.to_string()),
        },
    }
}

fn result_from_answer(case: EvalCase, answer: Answer, duration: Duration) -> CaseResult {
    let (passed, message) = evaluate_answer(&case.expectation, &answer);
    let citations = answer
        .citations
        .iter()
        .map(|evidence| CitationRecord {
            path: evidence.path.clone(),
            origin: evidence.origin.clone(),
            location: evidence.location.clone(),
            excerpt: evidence.excerpt.clone(),
            field: evidence.field.clone(),
            value: evidence.value.clone(),
        })
        .collect::<Vec<_>>();
    CaseResult {
        id: case.id,
        corpus: case.corpus,
        category: case.category,
        format: case.format,
        file: case.file,
        question: case.question,
        expected: case.facts,
        obtained: json!({
            "text": answer.text,
            "verified": answer.verified,
            "warning": answer.warning,
            "citation_count": citations.len()
        }),
        status: if passed {
            CaseStatus::Passed
        } else {
            CaseStatus::Failed
        },
        duration_ms: duration.as_millis(),
        citations,
        indexing_errors: vec![],
        error: (!passed).then_some(message),
    }
}

fn evaluate_answer(expectation: &Expectation, answer: &Answer) -> (bool, String) {
    let actual_paths = answer
        .citations
        .iter()
        .map(|evidence| canonical_path_string(Path::new(&evidence.path)))
        .collect::<BTreeSet<_>>();
    match expectation {
        Expectation::Inventory { total, origins } => {
            let citations = answer
                .citations
                .iter()
                .map(|evidence| evidence.origin.as_str())
                .collect::<BTreeSet<_>>();
            let passed = text_has_usize(&answer.text, *total)
                && origins.keys().all(|origin| answer.text.contains(origin))
                && origins
                    .keys()
                    .all(|origin| citations.contains(origin.as_str()))
                && answer.citations.len() <= origins.len();
            (passed, "inventario, categorías o citas no coinciden".into())
        }
        Expectation::Documents {
            count,
            paths,
            origin,
        } => {
            let bounded = answer.citations.len() <= MAX_COUNT_CITATIONS;
            let sources_ok = actual_paths.iter().all(|path| paths.contains(path));
            let origin_ok = origin.as_ref().is_none_or(|expected| {
                !answer.citations.is_empty()
                    && answer
                        .citations
                        .iter()
                        .all(|citation| &citation.origin == expected)
            });
            (
                text_has_usize(&answer.text, *count)
                    && answer.text.to_lowercase().contains("document")
                    && (*count == 0 || !answer.citations.is_empty())
                    && bounded
                    && sources_ok
                    && origin_ok,
                "conteo, unidad, origen, rutas o límite de citas incorrecto".into(),
            )
        }
        Expectation::Exact { value, paths } => (
            !answer.citations.is_empty()
                && actual_paths == *paths
                && answer
                    .citations
                    .iter()
                    .any(|citation| citation.excerpt.contains(value)),
            "la búsqueda exacta no quedó limitada a los documentos correctos".into(),
        ),
        Expectation::Absent => (
            answer.citations.is_empty() && !answer.verified,
            "una ausencia produjo evidencia o se marcó como verificada".into(),
        ),
        Expectation::Sum {
            field,
            total,
            values,
            currency,
        } => {
            let asks_count = answer.text.contains("tiene") && answer.text.contains("valores");
            let fact_ok = if asks_count {
                text_has_usize(&answer.text, *values)
            } else {
                text_has_number(&answer.text, *total) && text_has_usize(&answer.text, *values)
            };
            let currency_ok = currency
                .as_ref()
                .is_none_or(|currency| answer.text.contains(currency));
            (
                answer.text.contains(field)
                    && fact_ok
                    && currency_ok
                    && answer.citations.len() <= MAX_AGGREGATE_CITATIONS
                    && answer
                        .citations
                        .iter()
                        .any(|citation| citation.location.contains("cálculo local")),
                "suma/conteo, moneda, número de valores o evidencia de cálculo incorrectos".into(),
            )
        }
        Expectation::GroupSum {
            value_field,
            group_field,
            groups,
            values,
        } => (
            answer.text.contains(value_field)
                && answer.text.contains(group_field)
                && text_has_usize(&answer.text, *values)
                && groups.iter().all(|(group, total)| {
                    answer.text.contains(group) && text_has_number(&answer.text, *total)
                })
                && answer.citations.len() <= MAX_AGGREGATE_CITATIONS,
            "la agrupación no conserva todos los grupos, sumas o límites de citas".into(),
        ),
        Expectation::Text { phrase, paths } => {
            let relevant = answer.citations.iter().any(|citation| {
                paths.contains(&canonical_path_string(Path::new(&citation.path)))
                    && citation.excerpt.contains(phrase)
            });
            (
                relevant
                    && !answer.text.trim().is_empty()
                    && answer.citations.len() <= MAX_TEXT_CITATIONS,
                "la respuesta extractiva no cita la frase y fuente descubiertas".into(),
            )
        }
        Expectation::Keywords {
            terms,
            path_contains,
        } => {
            let relevant = answer.citations.iter().any(|citation| {
                citation.path.contains(path_contains)
                    && terms.iter().all(|term| {
                        normalize_for_oracle(&citation.excerpt)
                            .contains(&normalize_for_oracle(term))
                    })
            });
            (
                relevant
                    && !answer.text.trim().is_empty()
                    && answer.citations.len() <= MAX_TEXT_CITATIONS,
                "la respuesta no aporta evidencia con los conceptos esperados".into(),
            )
        }
        Expectation::Location {
            value,
            path,
            location_contains,
        } => {
            let found = answer.citations.iter().any(|citation| {
                canonical_path_string(Path::new(&citation.path)) == *path
                    && citation.location.contains(location_contains)
                    && citation.excerpt.contains(value)
            });
            (
                found && answer.citations.len() <= MAX_TEXT_CITATIONS,
                "el valor no quedó citado en el archivo y ubicación esperados".into(),
            )
        }
        Expectation::Skipped { .. } => (true, String::new()),
    }
}

fn normalize_for_oracle(value: &str) -> String {
    value
        .to_lowercase()
        .replace(['á', 'à', 'ä'], "a")
        .replace(['é', 'è', 'ë'], "e")
        .replace(['í', 'ì', 'ï'], "i")
        .replace(['ó', 'ò', 'ö'], "o")
        .replace(['ú', 'ù', 'ü'], "u")
}

fn text_has_usize(text: &str, expected: usize) -> bool {
    let compact = text.replace(',', "");
    Regex::new(&format!(r"(?:^|\D){}(?:\D|$)", expected))
        .expect("valid integer assertion")
        .is_match(&compact)
}

fn text_has_number(text: &str, expected: f64) -> bool {
    let compact = text.replace([',', '$'], "");
    [format!("{expected:.2}"), format!("{expected:.0}")]
        .iter()
        .any(|number| compact.contains(number))
}

fn canonical_path_string(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn temporary_directory(corpus: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "omega-evaluation-{}-{}-{stamp}",
        std::process::id(),
        safe_component(corpus)
    ))
}

fn create_report_directory(root: &Path) -> Result<PathBuf, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let directory = root.join(millis.to_string());
    fs::create_dir_all(&directory)
        .map_err(|error| format!("no se pudo crear {}: {error}", directory.display()))?;
    Ok(directory)
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn system_failure(corpus: &str, phase: &str, error: String) -> CaseResult {
    CaseResult {
        id: format!("{corpus}-system-{phase}"),
        corpus: corpus.into(),
        category: "sistema".into(),
        format: None,
        file: None,
        question: String::new(),
        expected: json!({"phase": phase}),
        obtained: json!({}),
        status: CaseStatus::Failed,
        duration_ms: 0,
        citations: vec![],
        indexing_errors: vec![],
        error: Some(error),
    }
}

fn write_reports(
    directory: &Path,
    summary: &RunSummary,
    results: &[CaseResult],
) -> Result<(), String> {
    let jsonl_path = directory.join("resultados.jsonl");
    let mut jsonl = BufWriter::new(
        File::create(&jsonl_path)
            .map_err(|error| format!("no se pudo crear {}: {error}", jsonl_path.display()))?,
    );
    for result in results {
        serde_json::to_writer(&mut jsonl, result).map_err(|error| error.to_string())?;
        writeln!(jsonl).map_err(|error| error.to_string())?;
    }
    jsonl.flush().map_err(|error| error.to_string())?;

    let summary_path = directory.join("resumen.json");
    fs::write(
        &summary_path,
        serde_json::to_string_pretty(summary).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("no se pudo escribir {}: {error}", summary_path.display()))?;

    let report_path = directory.join("reporte.md");
    let mut report = String::from("# Evaluación local de Omega\n\n");
    report.push_str(&format!(
        "- Corpus: {}\n- Documentos: {}\n- Casos: {}\n- Aprobados: {}\n- Fallidos: {}\n- Omitidos: {}\n- Duración: {} ms\n- Memoria máxima aproximada: {}\n\n",
        summary.corpora,
        summary.documents,
        summary.cases,
        summary.passed,
        summary.failed,
        summary.skipped,
        summary.duration_ms,
        summary
            .peak_memory_bytes_approx
            .map(|bytes| format!("{} MiB", bytes / 1024 / 1024))
            .unwrap_or_else(|| "no disponible".into())
    ));
    report.push_str("## Métricas por etapa\n\n| Corpus | Etapa | Documentos | Tiempo | Memoria aproximada |\n|---|---|---:|---:|---:|\n");
    for stage in &summary.stages {
        report.push_str(&format!(
            "| {} | {} | {} | {} ms | {} |\n",
            escape_markdown(&stage.corpus),
            escape_markdown(&stage.stage),
            stage.documents,
            stage.duration_ms,
            stage
                .peak_memory_bytes_approx
                .map(|bytes| format!("{} MiB", bytes / 1024 / 1024))
                .unwrap_or_else(|| "n/d".into())
        ));
    }
    report.push_str(
        "\n## Alcance y limitaciones\n\n\
- Esta evidencia valida únicamente los corpus y casos descritos; no demuestra preparación general para producción.\n\
- El OCR probado usa Vision/PDFKit local en macOS. Debe revalidarse en cada sistema operativo y entorno de distribución; un sandbox que bloquee la aceleración de Vision no representa la ejecución nativa.\n\
- La memoria es una aproximación del máximo residente del proceso y la prueba de estrés usa documentos sintéticos; no fija un SLA de tiempo ni sustituye una prueba con datos y hardware finales.\n\
- Una SQLite corrupta se rechaza de forma controlada, pero esta suite no implementa recuperación automática, respaldos ni restauración.\n\
- Los archivos vacíos, truncados o engañosos se omiten con aviso; otros daños, cifrados o variantes de formato no presentes en el fixture requieren pruebas adicionales.\n\n",
    );
    report.push_str("\n## Resumen de casos\n\n");
    report.push_str(
        "| Estado | Corpus | Familia | Formato | Archivo | Caso | Tiempo | Pregunta |\n|---|---|---|---|---|---|---:|---|\n",
    );
    for result in results {
        report.push_str(&format!(
            "| {:?} | {} | {} | {} | {} | {} | {} ms | {} |\n",
            result.status,
            escape_markdown(&result.corpus),
            escape_markdown(&result.category),
            escape_markdown(result.format.as_deref().unwrap_or("n/d")),
            escape_markdown(result.file.as_deref().unwrap_or("n/d")),
            escape_markdown(&result.id),
            result.duration_ms,
            escape_markdown(&result.question)
        ));
    }
    report.push_str("\n## Detalle por caso\n\n");
    for result in results {
        report.push_str(&format!(
            "### {} — {:?}\n\n- Formato: {}\n- Archivo: {}\n- Pregunta: {}\n- Esperado: `{}`\n- Obtenido: `{}`\n- Errores de indexación: `{}`\n- Mensaje: {}\n- Duración: {} ms\n- Citas: {}\n\n",
            result.id,
            result.status,
            result.format.as_deref().unwrap_or("n/d"),
            result.file.as_deref().unwrap_or("n/d"),
            result.question,
            result.expected,
            result.obtained,
            serde_json::to_string(&result.indexing_errors).unwrap_or_default(),
            result.error.as_deref().unwrap_or("sin mensaje"),
            result.duration_ms,
            result.citations.len()
        ));
        for citation in &result.citations {
            report.push_str(&format!(
                "  - `{}` — {} — {}\n",
                citation.path,
                escape_markdown(&citation.location),
                escape_markdown(&citation.excerpt)
            ));
        }
        report.push('\n');
    }
    fs::write(&report_path, report)
        .map_err(|error| format!("no se pudo escribir {}: {error}", report_path.display()))?;
    Ok(())
}

fn escape_markdown(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_reader_and_generator_are_reproducible() {
        let fixture = tempfile::tempdir().unwrap();
        for (folder, number, state, amount) in [
            ("01_alpha", 1, "Abierto", "$100.00"),
            ("01_alpha", 2, "Abierto", "$200.00"),
            ("02_beta", 3, "Cerrado", "$300.00"),
            ("02_beta", 4, "Cerrado", "$400.00"),
            ("02_beta", 5, "Cerrado", "$500.00"),
        ] {
            let directory = fixture.path().join(folder);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(format!("{number}.md")),
                format!(
                    "# Registro\n\nFolio: QA-26-{number:04}\nEstado: {state}\nImporte: {amount}\nGrupo: {folder}\n\nEl personal autorizado conserva evidencia suficiente para realizar el seguimiento operativo correspondiente.\n"
                ),
            )
            .unwrap();
        }
        let first = generate_cases_from_directory("fixture", fixture.path()).unwrap();
        let second = generate_cases_from_directory("fixture", fixture.path()).unwrap();
        assert_eq!(
            first.iter().map(|case| &case.id).collect::<Vec<_>>(),
            second.iter().map(|case| &case.id).collect::<Vec<_>>()
        );
        assert!(first.iter().any(|case| case.category == "A"));
        assert!(first.iter().any(|case| case.category == "B"));
        assert!(first.iter().any(|case| case.category == "C"));
        assert!(first.iter().any(|case| case.category == "D"));
        assert!(first.iter().any(|case| case.category == "E"));
        assert!(first.iter().any(|case| case.category == "F"));
    }

    #[test]
    fn positive_document_counts_require_nonempty_evidence() {
        let expectation = Expectation::Documents {
            count: 3,
            paths: BTreeSet::new(),
            origin: None,
        };
        let answer = Answer {
            text: "El índice contiene 3 documentos para el criterio.".into(),
            mode: "local".into(),
            verified: true,
            citations: vec![],
            warning: None,
            ..Answer::default()
        };
        assert!(!evaluate_answer(&expectation, &answer).0);
    }
}

// ---------------------------------------------------------------------------
// Escenarios conversacionales
//
// Un escenario encadena varios turnos sobre una misma conversación. El oráculo
// vuelve a leer el corpus por su cuenta —los mismos ficheros, con su propio
// lector y su propio parser de importes— y calcula qué debe responder cada
// turno. La lógica de producción no participa en el cálculo esperado.
// ---------------------------------------------------------------------------

/// Turno de una conversación evaluada.
#[derive(Clone, Debug)]
struct ConversationTurn {
    question: String,
    facts: Value,
    expectation: TurnExpectation,
    /// Antes de preguntar, se borra el contexto: sirve para comprobar que una
    /// conversación nueva no hereda nada.
    reset_before: bool,
}

#[derive(Clone, Debug)]
enum TurnExpectation {
    /// La respuesta cuenta documentos del conjunto y sólo cita esos.
    DocumentCount {
        count: usize,
        paths: BTreeSet<String>,
    },
    /// La suma se calcula sobre el conjunto anterior, no sobre todo el acervo.
    ContextTotal {
        field: String,
        total: f64,
        values: usize,
        paths: BTreeSet<String>,
        /// Total de todo el acervo para ese campo: aparecer aquí significaría
        /// que el contexto se perdió.
        acervo_total: f64,
    },
    /// La petición de evidencia devuelve exactamente los documentos usados.
    SupportingDocuments { paths: BTreeSet<String> },
    /// El motor pide aclaración con un motivo concreto.
    Clarification { reason: String },
}

#[derive(Clone, Debug)]
struct ConversationScenario {
    id: String,
    category: &'static str,
    turns: Vec<ConversationTurn>,
}

/// Deriva escenarios conversacionales del corpus, sin vocabulario fijo: elige
/// un campo repetido con al menos dos documentos y un campo monetario presente
/// en todos ellos.
fn generate_conversations(corpus: &OracleCorpus) -> Vec<ConversationScenario> {
    let Some(series) = discover_money_series(corpus)
        .into_iter()
        .find(|series| series.values.len() >= 3)
    else {
        return vec![];
    };
    let documents_with_money = series
        .values
        .iter()
        .map(|(index, _)| *index)
        .collect::<BTreeSet<_>>();
    let amount_by_document = series
        .values
        .iter()
        .copied()
        .collect::<BTreeMap<usize, f64>>();
    let acervo_total = series.values.iter().map(|(_, value)| value).sum::<f64>();

    // Campo repetido que parte el acervo en un subconjunto propio: ni un solo
    // documento ni todos.
    let mut candidates: Vec<(String, String, BTreeSet<usize>)> = Vec::new();
    for ((label, value), _) in field_value_paths(corpus) {
        if label == series.field || is_identifier_field(&label, &value) {
            continue;
        }
        let members = corpus
            .documents
            .iter()
            .enumerate()
            .filter(|(_, document)| {
                document
                    .fields
                    .iter()
                    .any(|(field, item)| field == &label && item == &value)
            })
            .map(|(index, _)| index)
            .collect::<BTreeSet<_>>();
        let inside = members
            .intersection(&documents_with_money)
            .copied()
            .collect::<BTreeSet<_>>();
        if inside.len() >= 2 && inside.len() < documents_with_money.len() && inside == members {
            candidates.push((label, value, inside));
        }
    }
    candidates.sort_by(|left, right| {
        right
            .2
            .len()
            .cmp(&left.2.len())
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });
    let Some((field, value, members)) = candidates.into_iter().next() else {
        return vec![];
    };

    let paths = members
        .iter()
        .map(|index| corpus.documents[*index].path.clone())
        .collect::<BTreeSet<_>>();
    let total = members
        .iter()
        .filter_map(|index| amount_by_document.get(index))
        .sum::<f64>();
    let values = members.len();

    // ¿Cuántos campos numéricos distintos conviven en ese conjunto? De eso
    // depende qué debe responder el motor: con uno solo la continuación se
    // resuelve sola; con varios, «¿cuánto suman?» es genuinamente ambigua y lo
    // correcto es preguntar en vez de elegir un campo.
    //
    // El oráculo reimplementa la regla del índice —el tipo de un campo lo fija
    // su primera aparición, en el mismo orden alfabético en que se recorren los
    // archivos— con sus propias expresiones regulares.
    let mut first_value: BTreeMap<String, String> = BTreeMap::new();
    for document in &corpus.documents {
        for (label, raw) in &document.fields {
            first_value
                .entry(label.clone())
                .or_insert_with(|| raw.clone());
        }
    }
    let money_fields = corpus
        .documents
        .iter()
        .enumerate()
        .filter(|(index, _)| members.contains(index))
        .flat_map(|(_, document)| document.fields.iter().map(|(label, _)| label.clone()))
        .filter(|label| {
            first_value
                .get(label)
                .is_some_and(|value| oracle_value_is_numeric(value))
        })
        .collect::<BTreeSet<_>>();
    let ambiguous_field = money_fields.len() > 1;

    let count_question = format!("¿Cuántos documentos tienen {field}: {value}?");
    let mut continuity = vec![
        ConversationTurn {
            question: count_question.clone(),
            facts: json!({ "campo": field, "valor": value, "documentos": values }),
            expectation: TurnExpectation::DocumentCount {
                count: values,
                paths: paths.clone(),
            },
            reset_before: true,
        },
    ];
    if ambiguous_field {
        // Con varios campos numéricos en el mismo conjunto, la continuación sin
        // campo debe pedir aclaración en vez de elegir uno.
        continuity.push(ConversationTurn {
            question: "¿Cuánto suman?".into(),
            facts: json!({
                "espera": "aclaracion",
                "campos_numericos": money_fields.iter().cloned().collect::<Vec<_>>()
            }),
            expectation: TurnExpectation::Clarification {
                reason: "campo_ambiguo".into(),
            },
            reset_before: false,
        });
    }
    // Responder con una de las opciones ofrecidas debe ejecutar la operación
    // pendiente sobre el mismo conjunto, no lanzar una consulta nueva sobre
    // todo el acervo.
    let sum_question = if ambiguous_field {
        series.field.clone()
    } else {
        "¿Cuánto suman?".to_owned()
    };
    let mut scenarios = vec![ConversationScenario {
        id: "conversacion-continuidad".into(),
        category: "H",
        turns: {
            continuity.push(ConversationTurn {
                question: sum_question,
                facts: json!({
                    "campo_sumado": series.field,
                    "total_del_conjunto": total,
                    "valores": values,
                    "total_del_acervo": acervo_total
                }),
                expectation: TurnExpectation::ContextTotal {
                    field: series.field.clone(),
                    total,
                    values,
                    paths: paths.clone(),
                    acervo_total,
                },
                reset_before: false,
            });
            continuity.push(ConversationTurn {
                question: "¿Qué documentos respaldan ese total?".into(),
                facts: json!({ "documentos": paths.len() }),
                expectation: TurnExpectation::SupportingDocuments {
                    paths: paths.clone(),
                },
                reset_before: false,
            });
            continuity
        },
    }];

    scenarios.push(ConversationScenario {
        id: "valor-inexistente".into(),
        category: "H",
        turns: vec![ConversationTurn {
            // El valor real más una palabra que no está en el acervo: el motor
            // no puede recortarlo hasta encontrar algo parecido.
            question: format!("¿Cuántos documentos tienen {field}: {value} inexistente?"),
            facts: json!({
                "campo": field,
                "valor_pedido": format!("{value} inexistente"),
                "espera": "aclaracion sin degradar el valor"
            }),
            expectation: TurnExpectation::Clarification {
                reason: "valor_inexistente".into(),
            },
            reset_before: true,
        }],
    });

    scenarios.push(ConversationScenario {
        id: "conversacion-referencia-ambigua".into(),
        category: "H",
        turns: vec![ConversationTurn {
            question: "¿Cuánto suman esos?".into(),
            facts: json!({ "espera": "aclaracion" }),
            expectation: TurnExpectation::Clarification {
                reason: "referencia_sin_contexto".into(),
            },
            reset_before: true,
        }],
    });

    scenarios.push(ConversationScenario {
        id: "conversacion-contexto-borrado".into(),
        category: "H",
        turns: vec![
            ConversationTurn {
                question: count_question,
                facts: json!({ "documentos": values }),
                expectation: TurnExpectation::DocumentCount {
                    count: values,
                    paths,
                },
                reset_before: true,
            },
            ConversationTurn {
                question: "¿Cuánto suman?".into(),
                facts: json!({ "espera": "aclaracion tras borrar el contexto" }),
                expectation: TurnExpectation::Clarification {
                    reason: "sin_contexto".into(),
                },
                // El borrado ocurre entre los dos turnos: es la prueba de que
                // «Nueva conversación» olvida de verdad.
                reset_before: true,
            },
        ],
    });
    scenarios
}

/// ¿Este valor sería numérico para el índice? Reproduce, de forma
/// independiente, las tres formas que el clasificador reconoce como número:
/// importe, porcentaje y número simple.
fn oracle_value_is_numeric(raw: &str) -> bool {
    let money = Regex::new(r"^\$?\s*[0-9][0-9,]*(?:\.[0-9]{1,2})?\s*(?:[A-Z]{3})?$")
        .expect("valid independent money regex");
    let percentage = Regex::new(r"^[0-9][0-9,]*(?:\.[0-9]+)?\s*%$")
        .expect("valid independent percentage regex");
    let value = raw.trim();
    (money.is_match(value) && (value.contains('$') || value.ends_with(char::is_uppercase)))
        || percentage.is_match(value)
        || value
            .replace(',', "")
            .parse::<f64>()
            .is_ok()
}

fn run_conversations(
    engine: &OmegaEngine,
    corpus: &str,
    scenarios: Vec<ConversationScenario>,
) -> Vec<CaseResult> {
    let mut results = Vec::new();
    for scenario in scenarios {
        let conversation = format!("{corpus}-{}", scenario.id);
        for (index, turn) in scenario.turns.into_iter().enumerate() {
            if turn.reset_before {
                engine.reset_conversation(&conversation);
            }
            let started = Instant::now();
            let id = format!("{}-{}-t{}", corpus, scenario.id, index + 1);
            match engine.ask_in_conversation(&conversation, &turn.question) {
                Ok(answer) => {
                    let (passed, message) = evaluate_turn(&turn.expectation, &answer);
                    results.push(CaseResult {
                        id,
                        corpus: corpus.to_owned(),
                        category: scenario.category.to_owned(),
                        format: None,
                        file: None,
                        question: turn.question,
                        expected: turn.facts,
                        obtained: json!({
                            "text": answer.text,
                            "verified": answer.verified,
                            "used_context": answer.used_context,
                            "citation_count": answer.citations.len()
                        }),
                        status: if passed {
                            CaseStatus::Passed
                        } else {
                            CaseStatus::Failed
                        },
                        duration_ms: started.elapsed().as_millis(),
                        citations: answer
                            .citations
                            .iter()
                            .map(|evidence| CitationRecord {
                                path: evidence.path.clone(),
                                origin: evidence.origin.clone(),
                                location: evidence.location.clone(),
                                excerpt: evidence.excerpt.clone(),
                                field: evidence.field.clone(),
                                value: evidence.value.clone(),
                            })
                            .collect(),
                        indexing_errors: vec![],
                        error: (!passed).then_some(message),
                    });
                }
                Err(error) => results.push(system_failure(corpus, "conversacion", error.to_string())),
            }
        }
    }
    results
}

fn evaluate_turn(expectation: &TurnExpectation, answer: &Answer) -> (bool, String) {
    let cited = answer
        .citations
        .iter()
        .filter(|evidence| evidence.match_kind != "cálculo")
        .map(|evidence| canonical_path_string(Path::new(&evidence.path)))
        .collect::<BTreeSet<_>>();
    match expectation {
        TurnExpectation::DocumentCount { count, paths } => {
            let expected_paths = paths
                .iter()
                .map(|path| canonical_path_string(Path::new(path)))
                .collect::<BTreeSet<_>>();
            let outside = cited.difference(&expected_paths).count();
            (
                text_has_usize(&answer.text, *count) && answer.verified && outside == 0,
                "el conteo, la verificación o las citas del conjunto no coinciden".into(),
            )
        }
        TurnExpectation::ContextTotal {
            field,
            total,
            values,
            paths,
            acervo_total,
        } => {
            let expected_paths = paths
                .iter()
                .map(|path| canonical_path_string(Path::new(path)))
                .collect::<BTreeSet<_>>();
            let outside = cited.difference(&expected_paths).count();
            let leaked = (acervo_total - total).abs() > 0.005
                && text_has_number(&answer.text, *acervo_total);
            let scope_is_right = answer
                .scope
                .as_ref()
                .is_some_and(|scope| scope.inherited && scope.value_count == Some(*values as i64));
            (
                text_has_number(&answer.text, *total)
                    && answer.used_context
                    && answer.verified
                    && scope_is_right
                    && outside == 0
                    && !leaked
                    && answer.text.contains(field.as_str()),
                "la suma contextual, su alcance o sus citas no corresponden al conjunto anterior"
                    .into(),
            )
        }
        TurnExpectation::SupportingDocuments { paths } => {
            let expected_paths = paths
                .iter()
                .map(|path| canonical_path_string(Path::new(path)))
                .collect::<BTreeSet<_>>();
            (
                answer.used_context && !cited.is_empty() && cited.is_subset(&expected_paths),
                "la evidencia del total no corresponde a los documentos usados".into(),
            )
        }
        TurnExpectation::Clarification { reason } => (
            answer
                .clarification
                .as_ref()
                .is_some_and(|clarification| clarification.reason == *reason)
                && !answer.verified
                && answer.citations.is_empty(),
            format!("se esperaba una aclaración con motivo «{reason}»"),
        ),
    }
}
