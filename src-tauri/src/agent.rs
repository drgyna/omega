use std::collections::HashSet;

use crate::{
    answer,
    error::Result,
    model::{AggregateRequest, AggregateRow, Answer, ToolFilter},
    normalize::normalize_exact,
    planner::{self, QueryIntent},
    tools::{DocumentQueryResult, OriginSummary, TextQueryResult, ToolEngine},
};

const MAX_DOCUMENT_SAMPLE: usize = 12;
const MAX_DOCUMENT_CITATIONS: usize = 24;
const MAX_TEXT_CITATIONS: usize = 6;
const MAX_AGGREGATE_CITATIONS: usize = 18;

#[derive(Clone)]
pub struct Agent {
    tools: ToolEngine,
}

impl Agent {
    pub fn new(tools: ToolEngine) -> Self {
        Self { tools }
    }

    pub fn answer_local(&self, question: &str) -> Result<Answer> {
        let plan = planner::plan(&self.tools, question)?;
        match plan.intent {
            QueryIntent::Inventory => Ok(inventory_answer(self.tools.origin_summaries()?)),
            QueryIntent::Exact => self.legacy_answer(question, 20),
            QueryIntent::Aggregate(request) => {
                let rows = self.tools.aggregate(&request)?;
                Ok(aggregate_answer(&self.tools, &request, rows))
            }
            QueryIntent::CountDocuments | QueryIntent::ListDocuments => {
                let evidence_per_document = plan.filters.len() + usize::from(plan.origin.is_some());
                let sample_limit = if evidence_per_document == 0 {
                    MAX_DOCUMENT_SAMPLE
                } else {
                    (MAX_DOCUMENT_CITATIONS / evidence_per_document)
                        .max(1)
                        .min(MAX_DOCUMENT_SAMPLE)
                };
                let result = self.tools.query_documents(
                    &plan.filters,
                    plan.origin.as_deref(),
                    sample_limit,
                )?;
                Ok(document_answer(
                    result,
                    &plan.filters,
                    plan.origin.as_deref(),
                ))
            }
            QueryIntent::FreeText => {
                let result =
                    self.tools
                        .search_text(question, plan.origin.as_deref(), MAX_TEXT_CITATIONS)?;
                Ok(text_answer(question, result))
            }
            QueryIntent::BoundedSearch => self.legacy_answer(question, 20),
            QueryIntent::LegacySearch => self.legacy_answer(question, usize::MAX),
        }
    }

    fn legacy_answer(&self, question: &str, limit: usize) -> Result<Answer> {
        let hits = self.tools.search(question, &[], limit)?;
        if hits.is_empty() {
            return Ok(no_evidence_answer());
        }
        if let Some(synthesis) = answer::synthesize(&self.tools, question, &hits)? {
            return Ok(Answer {
                text: synthesis.text,
                mode: "local".into(),
                verified: synthesis.verified,
                citations: synthesis.citations,
                warning: None,
            });
        }
        Ok(Answer {
            text: format!("{} resultados con evidencia específica.", hits.len()),
            mode: "local".into(),
            verified: true,
            citations: hits.into_iter().map(|hit| hit.evidence).collect(),
            warning: None,
        })
    }

    pub fn answer(&self, question: &str) -> Result<Answer> {
        self.answer_local(question)
    }
}

fn inventory_answer(summaries: Vec<OriginSummary>) -> Answer {
    let total = summaries
        .iter()
        .map(|item| item.document_count)
        .sum::<i64>();
    let categories = summaries
        .iter()
        .map(|item| format!("- {}: {} documentos", item.origin, item.document_count))
        .collect::<Vec<_>>()
        .join("\n");
    Answer {
        text: format!(
            "Hay {total} documentos indexados en {} categorías:\n\n{categories}",
            summaries.len()
        ),
        mode: "local".into(),
        verified: true,
        citations: summaries.into_iter().map(|item| item.evidence).collect(),
        warning: None,
    }
}

fn document_answer(
    result: DocumentQueryResult,
    filters: &[ToolFilter],
    origin: Option<&str>,
) -> Answer {
    if result.document_count == 0 {
        return no_evidence_answer();
    }
    let mut criteria = filters
        .iter()
        .map(|filter| format!("{} = {}", filter.concept, filter.equals))
        .collect::<Vec<_>>();
    if let Some(origin) = origin {
        criteria.insert(0, format!("carpeta = {origin}"));
    }
    let scope = if criteria.is_empty() {
        String::new()
    } else {
        format!(" ({})", criteria.join("; "))
    };
    Answer {
        text: format!(
            "{} documentos cumplen simultáneamente los criterios{}.",
            result.document_count, scope
        ),
        mode: "local".into(),
        verified: true,
        citations: result.evidence,
        warning: None,
    }
}

fn aggregate_answer(
    tools: &ToolEngine,
    request: &AggregateRequest,
    rows: Vec<AggregateRow>,
) -> Answer {
    if rows.is_empty() {
        return no_evidence_answer();
    }
    let total_values = rows.iter().map(|row| row.matched_values).sum::<i64>();
    let text = if request.operation == "count" {
        format!(
            "El campo «{}» tiene {total_values} valores con evidencia.",
            request.concept
        )
    } else if rows.len() == 1 && rows[0].group.is_none() {
        format!(
            "Suma de «{}»: {}, calculada a partir de {total_values} valores.",
            request.concept,
            render_aggregate_value(&rows[0])
        )
    } else {
        let table = rows
            .iter()
            .map(|row| {
                format!(
                    "| {} | {} | {} |",
                    row.group.as_deref().unwrap_or("Sin valor"),
                    render_aggregate_value(row),
                    row.matched_values
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Suma de «{}» agrupada por «{}» ({total_values} valores):\n\n| Grupo | Suma | Valores |\n|---|---:|---:|\n{table}",
            request.concept,
            request.group_by.as_deref().unwrap_or("grupo")
        )
    };

    let mut citations = tools
        .aggregate_calculation_evidence(request, &rows)
        .into_iter()
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    for evidence in rows.iter().flat_map(|row| row.evidence.iter()) {
        if citations.len() >= MAX_AGGREGATE_CITATIONS {
            break;
        }
        if seen.insert(evidence.id.clone()) {
            citations.push(evidence.clone());
        }
    }
    Answer {
        text,
        mode: "local".into(),
        verified: true,
        citations,
        warning: None,
    }
}

fn text_answer(question: &str, result: TextQueryResult) -> Answer {
    if result.hits.is_empty() {
        return no_evidence_answer();
    }
    let excerpts = result
        .hits
        .iter()
        .take(3)
        .map(|hit| format!("- {}", hit.evidence.excerpt.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let legal = asks_for_legal_guidance(question);
    Answer {
        text: format!(
            "Encontré evidencia pertinente en {} documentos. Extractos del acervo:\n\n{}{}",
            result.document_count,
            excerpts,
            if legal {
                "\n\nEsta respuesta se limita al material indexado y no sustituye asesoría legal ni una fuente oficial."
            } else {
                ""
            }
        ),
        mode: "local".into(),
        verified: true,
        citations: result.hits.into_iter().map(|hit| hit.evidence).collect(),
        warning: legal
            .then(|| "Contenido extractivo del acervo local; no constituye asesoría legal.".into()),
    }
}

fn no_evidence_answer() -> Answer {
    Answer {
        text: "No encontré evidencia local suficiente para responder esa consulta.".into(),
        mode: "local".into(),
        verified: false,
        citations: vec![],
        warning: None,
    }
}

fn render_aggregate_value(row: &AggregateRow) -> String {
    let number = format_grouped(row.value, 2);
    match row.currency.as_deref() {
        Some(currency) => format!("${number} {currency}"),
        None => number,
    }
}

fn format_grouped(value: f64, decimals: usize) -> String {
    let raw = format!("{value:.decimals$}");
    let (integer, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = integer.starts_with('-').then_some("-").unwrap_or("");
    let digits = integer.trim_start_matches('-');
    let mut grouped = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(character);
    }
    if decimals == 0 {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

fn asks_for_legal_guidance(question: &str) -> bool {
    let normalized = normalize_exact(question);
    [
        "legal",
        "ley",
        "leyes",
        "norma",
        "normativa",
        "regulacion",
        "obligacion",
        "obligaciones",
        "asesoria",
    ]
    .iter()
    .any(|term| normalized.split_whitespace().any(|word| word == *term))
}
