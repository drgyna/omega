//! Planificador local de consultas. Decide una ruta pequeña y auditable a
//! partir de señales lingüísticas genéricas y del esquema descubierto en el
//! índice. No contiene nombres de carpetas, campos, ciudades ni estados de un
//! corpus concreto.

use crate::{
    error::Result,
    model::{AggregateRequest, ConceptSummary, ToolFilter},
    normalize::{normalize_exact, search_terms, stems_match},
    tools::ToolEngine,
};

#[derive(Clone, Debug)]
pub enum QueryIntent {
    Inventory,
    Exact,
    Aggregate(AggregateRequest),
    CountDocuments,
    ListDocuments,
    FreeText,
    BoundedSearch,
    LegacySearch,
}

#[derive(Clone, Debug)]
pub struct QueryPlan {
    pub intent: QueryIntent,
    pub filters: Vec<ToolFilter>,
    pub origin: Option<String>,
}

pub fn plan(tools: &ToolEngine, question: &str) -> Result<QueryPlan> {
    let terms = search_terms(question);
    let has = |root: &str| terms.iter().any(|term| term.starts_with(root));
    let asks_count = has("cuant") || has("numer") || has("conte") || has("how") || has("total");
    let asks_sum = has("sum") || has("totaliz") || has("add");
    let asks_group = has("agrup") || has("desglos") || has("group");
    let asks_list = has("muestr")
        || has("list")
        || has("busc")
        || has("encuentr")
        || has("find")
        || has("show");
    let mentions_documents =
        has("document") || has("archiv") || has("expedient") || has("registro");
    let mentions_index = has("indic") || has("index") || has("acerv") || has("coleccion");

    if asks_count && mentions_documents && mentions_index {
        return Ok(QueryPlan {
            intent: QueryIntent::Inventory,
            filters: vec![],
            origin: None,
        });
    }
    if ToolEngine::query_has_exact_signal(question) {
        return Ok(QueryPlan {
            intent: QueryIntent::Exact,
            filters: vec![],
            origin: None,
        });
    }

    let mut origin = tools.match_origin(question)?;
    let concepts = tools.list_concepts(None)?;
    if let Some(selected_origin) = origin.as_deref() {
        let origin_terms = search_terms(selected_origin);
        let lower_question = question.to_lowercase();
        let origin_came_from_named_field = concepts.iter().any(|concept| {
            lower_question.contains(&format!("{}:", concept.display_name.to_lowercase()))
                && search_terms(&concept.display_name)
                    .iter()
                    .any(|field_term| {
                        origin_terms
                            .iter()
                            .any(|origin_term| stems_match(field_term, origin_term))
                    })
        });
        if origin_came_from_named_field {
            origin = None;
        }
    }
    let possible_group = if asks_group {
        resolve_group_concept(&concepts, question, None)
    } else {
        None
    };
    let target = resolve_named_concept(
        &concepts,
        &terms,
        possible_group
            .as_ref()
            .map(|concept| concept.display_name.as_str()),
    );
    let group_by = possible_group;
    let explicitly_scopes_origin = has("carpet")
        || has("categori")
        || has("origen")
        || has("fuente")
        || has("folder")
        || has("source");
    let filters = tools.filters_from_query(
        question,
        origin.as_deref(),
        (asks_count || asks_list) && !explicitly_scopes_origin,
    )?;

    let asks_value_count = asks_count && has("valor");
    if asks_sum || asks_group || asks_value_count {
        if let Some(target) = target.as_ref() {
            let numeric_target = matches!(
                target.value_type.as_str(),
                "money" | "number" | "percentage"
            );
            if !asks_sum || numeric_target {
                return Ok(QueryPlan {
                    intent: QueryIntent::Aggregate(AggregateRequest {
                        concept: target.display_name.clone(),
                        operation: if asks_value_count { "count" } else { "sum" }.into(),
                        filters,
                        origin: origin.clone(),
                        currency: explicit_currency(question, &tools.available_currencies()?),
                        date_from: None,
                        date_to: None,
                        group_by: group_by
                            .as_ref()
                            .map(|concept| concept.display_name.clone()),
                    }),
                    filters: vec![],
                    origin,
                });
            }
        }
    }

    if asks_count && (!filters.is_empty() || origin.is_some()) {
        return Ok(QueryPlan {
            intent: QueryIntent::CountDocuments,
            filters,
            origin,
        });
    }
    if asks_list && !filters.is_empty() {
        return Ok(QueryPlan {
            intent: QueryIntent::ListDocuments,
            filters,
            origin,
        });
    }
    let natural = normalize_exact(question);
    let question_word = ["que", "quien", "quienes", "existe", "resume"]
        .iter()
        .any(|word| natural.split_whitespace().any(|term| term == *word));
    if target.is_some() && (!filters.is_empty() || !question_word) {
        return Ok(QueryPlan {
            intent: QueryIntent::LegacySearch,
            filters: vec![],
            origin,
        });
    }
    if asks_count || (asks_list && origin.is_some()) || question_word {
        return Ok(QueryPlan {
            intent: QueryIntent::FreeText,
            filters: vec![],
            origin,
        });
    }
    if asks_list {
        return Ok(QueryPlan {
            intent: QueryIntent::BoundedSearch,
            filters: vec![],
            origin,
        });
    }
    Ok(QueryPlan {
        intent: QueryIntent::LegacySearch,
        filters: vec![],
        origin,
    })
}

fn resolve_named_concept(
    concepts: &[ConceptSummary],
    query_terms: &[String],
    excluded: Option<&str>,
) -> Option<ConceptSummary> {
    let mut candidates = concepts
        .iter()
        .filter(|concept| excluded != Some(concept.display_name.as_str()))
        .filter_map(|concept| {
            let field_terms = search_terms(&concept.display_name);
            (!field_terms.is_empty()
                && field_terms.iter().all(|field_term| {
                    query_terms
                        .iter()
                        .any(|query_term| stems_match(query_term, field_term))
                }))
            .then_some((field_terms.len(), concept.clone()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.occurrences.cmp(&left.1.occurrences))
    });
    candidates.into_iter().next().map(|(_, concept)| concept)
}

fn resolve_group_concept(
    concepts: &[ConceptSummary],
    question: &str,
    target: Option<&ConceptSummary>,
) -> Option<ConceptSummary> {
    let normalized = normalize_exact(question);
    let after_group = normalized
        .rsplit_once(" por ")
        .map(|(_, value)| value)
        .or_else(|| normalized.rsplit_once(" by ").map(|(_, value)| value))?;
    resolve_named_concept(
        concepts,
        &search_terms(after_group),
        target.map(|concept| concept.display_name.as_str()),
    )
}

fn explicit_currency(
    question: &str,
    available: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let currency = regex::Regex::new(r"(?u)\b[A-Z]{3}\b").expect("valid currency regex");
    currency
        .find_iter(question)
        .map(|value| value.as_str().to_owned())
        .find(|value| {
            value
                .chars()
                .all(|character| character.is_ascii_uppercase())
                && available.contains(value)
        })
}
