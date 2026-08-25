use std::collections::HashSet;

use crate::{
    answer,
    calc::{self, Bucket, Decimal, Operation, RowComputation, RowOperation},
    conversation::{ComparisonMemory, ComparisonSide, ComputationMemory, ConversationState},
    dates::Clock,
    error::Result,
    model::{
        AggregateRequest, AggregateRow, Answer, AnswerScope, Clarification, DateConstraint,
        Evidence, ToolFilter,
    },
    normalize::normalize_exact,
    planner::{self, Command, PlannedScope, QueryIntent, QueryPlan},
    relations,
    report,
    tools::{DocumentQueryResult, OriginSummary, TextQueryResult, ToolEngine, ValueQuery},
};

const MAX_DOCUMENT_SAMPLE: usize = 12;
const MAX_DOCUMENT_CITATIONS: usize = 24;
const MAX_TEXT_CITATIONS: usize = 6;
const MAX_AGGREGATE_CITATIONS: usize = 18;

#[derive(Clone)]
pub struct Agent {
    tools: ToolEngine,
    /// Fuente de «hoy». Nunca se lee del sistema dentro del razonamiento: llega
    /// desde fuera para que una prueba pueda fijarla.
    clock: Clock,
}

impl Agent {
    pub fn new(tools: ToolEngine, clock: Clock) -> Self {
        Self { tools, clock }
    }

    /// Responde dentro de una conversación. El estado entra y sale por
    /// referencia: el agente no guarda memoria propia ni la comparte entre
    /// conversaciones.
    pub fn answer_in(&self, question: &str, state: &mut ConversationState) -> Result<Answer> {
        let plan = planner::plan_structured(&self.tools, question, state, &self.clock)?;
        let answer = match plan.command.clone() {
            Command::Retrieval => self.retrieval(question, state)?,
            Command::Clarify(clarification) => clarify(clarification),
            Command::NoEvidence { message } => Answer::unverified(message),
            Command::Compute(operation) => self.compute(operation, &plan.scope, state)?,
            Command::Rank {
                operation,
                descending,
            } => self.rank(operation, descending, &plan.scope, state)?,
            Command::CompareGroups {
                operation,
                concept,
                left,
                right,
            } => self.compare_groups(operation, &concept, &left, &right, &plan.scope, state)?,
            Command::ComparePeriods {
                operation,
                previous,
            } => self.compare_periods(operation, &previous, &plan.scope, state)?,
            Command::ComparisonFollowUp { percentage } => {
                self.comparison_follow_up(percentage, state)
            }
            Command::EvidenceForLast => self.evidence_for_last(state),
            Command::RelationWithoutKey => self.relation_without_key(question)?,
            Command::Contradictions { key, compared } => {
                self.contradictions(key.as_deref(), compared.as_deref())?
            }
            Command::Dossier { canonical } => self.dossier(&canonical, state)?,
            Command::ComputeMany {
                operation,
                concepts,
            } => self.compute_many(operation, &concepts, &plan.scope, state)?,
            Command::ComputeRow {
                operation,
                left,
                right,
            } => self.compute_row(operation, &left, &right, &plan.scope, state)?,
        };
        // La aclaración pendiente vive exactamente un turno: o el usuario elige
        // una opción, o la siguiente pregunta la sustituye.
        state.pending = plan.pending.clone();
        state.last_question = Some(question.to_owned());
        Ok(answer)
    }

    /// Ruta de recuperación clásica. Además de responder, deja en el contexto
    /// el predicado del conjunto que acaba de producir, para que el siguiente
    /// turno pueda referirse a él.
    fn retrieval(&self, question: &str, state: &mut ConversationState) -> Result<Answer> {
        let plan = planner::plan(&self.tools, question)?;
        let answer = self.execute(question, &plan)?;
        self.remember_retrieval(question, &plan, state)?;
        Ok(answer)
    }

    fn remember_retrieval(
        &self,
        question: &str,
        plan: &QueryPlan,
        state: &mut ConversationState,
    ) -> Result<()> {
        // Cada turno de recuperación reemplaza el conjunto recordado. Conservar
        // el anterior haría que «esos» apuntara a algo que el usuario ya no
        // tiene delante.
        *state = ConversationState {
            last_question: state.last_question.clone(),
            ..ConversationState::default()
        };
        let (filters, origin, concept, group_by) = match &plan.intent {
            QueryIntent::CountDocuments | QueryIntent::ListDocuments => (
                plan.filters.clone(),
                plan.origin.clone(),
                None,
                None,
            ),
            QueryIntent::Aggregate(request) => (
                request.filters.clone(),
                request.origin.clone(),
                Some(request.concept.clone()),
                request.group_by.clone(),
            ),
            _ => (vec![], plan.origin.clone(), None, None),
        };
        state.concept = concept;
        state.group_by = group_by;
        if filters.is_empty() && origin.is_none() {
            // Sin predicado no hay conjunto que recordar. Un identificador sí
            // se recuerda: define un conjunto reevaluable por clave estable.
            let candidates = relations::identifier_candidates(&self.tools, question)?;
            if candidates.len() == 1 {
                state.identifier = Some(candidates[0].clone());
                state.set = Some(crate::conversation::DocumentSet {
                    identifier: Some(candidates[0].clone()),
                    ..Default::default()
                });
            }
            return Ok(());
        }
        let documents = self
            .tools
            .documents_matching(&filters, origin.as_deref(), None)?;
        state.set = Some(
            crate::conversation::DocumentSet {
                filters,
                origin,
                document_count: documents.len() as i64,
                ..Default::default()
            }
            .with_paths(documents.iter().map(|document| document.path.clone())),
        );
        Ok(())
    }


    fn execute(&self, question: &str, plan: &QueryPlan) -> Result<Answer> {
        match plan.intent.clone() {
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
                ..Answer::default()
            });
        }
        Ok(Answer {
            text: format!("{} resultados con evidencia específica.", hits.len()),
            mode: "local".into(),
            verified: true,
            citations: hits.into_iter().map(|hit| hit.evidence).collect(),
            warning: None,
            ..Answer::default()
        })
    }

    pub fn answer(&self, question: &str) -> Result<Answer> {
        let mut state = ConversationState::default();
        self.answer_in(question, &mut state)
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
        ..Answer::default()
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
        ..Answer::default()
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
        ..Answer::default()
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
        ..Answer::default()
    }
}

fn no_evidence_answer() -> Answer {
    Answer {
        text: "No encontré evidencia local suficiente para responder esa consulta.".into(),
        mode: "local".into(),
        verified: false,
        citations: vec![],
        warning: None,
        ..Answer::default()
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

// ---------------------------------------------------------------------------
// Ejecución del plan estructurado
//
// Cada rama consulta el índice, calcula con el motor aritmético y redacta con
// `report`. Ninguna inventa una cifra: si no hay operandos con evidencia, lo
// dice. Al terminar, deja en el contexto los hechos estructurados del turno.
// ---------------------------------------------------------------------------

/// Muestra de operandos que acompaña a un cálculo. No es el conjunto entero:
/// la respuesta declara cuántos valores usó y la interfaz ofrece ver más.
const MAX_OPERAND_CITATIONS: usize = 12;

/// Notas de cálculo por respuesta. Una comparación necesita las dos; un
/// ranking, las de los primeros grupos, no una por cada fila.
const MAX_CALCULATION_CITATIONS: usize = 4;

impl Agent {
    fn compute(
        &self,
        operation: Operation,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let Some(concept) = scope.concept.clone() else {
            // Conteo de documentos: la operación no necesita un campo.
            let citations = self
                .tools
                .evidence_for_documents(&scope.documents, MAX_OPERAND_CITATIONS)?;
            let text = with_scope(report::document_count(scope.documents.len()), scope);
            self.remember_scope(scope, state);
            return Ok(Answer::verified(text, citations).with_scope(answer_scope(scope, None)));
        };
        let operands = self.tools.collect_operands(&ValueQuery {
            concept: &concept,
            documents: Some(&scope.documents),
            currency: scope.currency.as_deref(),
            ..ValueQuery::default()
        })?;
        let buckets = calc::compute(operation, &operands);
        if buckets.is_empty() {
            return Ok(missing_values_answer(&concept, scope));
        }
        let documents = operand_documents(&buckets);
        let text = with_scope_of(
            report::computation(operation, &concept, &buckets),
            scope,
            documents,
        );
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        self.remember_scope(scope, state);
        self.remember_computation(operation.label(), &concept, &buckets, state);
        Ok(Answer::verified(text, citations).with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    /// Calcula la misma operación para varios campos sobre un único conjunto
    /// de documentos, en vez de exigir que el usuario elija uno solo.
    ///
    /// Un campo sin valores en el alcance se dice explícitamente en la tabla
    /// («Sin datos»); nunca se inventa un cero ni se omite la fila.
    fn compute_many(
        &self,
        operation: Operation,
        concepts: &[String],
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let mut results = Vec::with_capacity(concepts.len());
        let mut documents = HashSet::new();
        let mut total = 0i64;
        for concept in concepts {
            let operands = self.tools.collect_operands(&ValueQuery {
                concept,
                documents: Some(&scope.documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?;
            let buckets = calc::compute(operation, &operands);
            documents.extend(buckets.iter().flat_map(|bucket| bucket.document_ids.iter().copied()));
            total += total_values(&buckets);
            results.push((concept.clone(), buckets));
        }
        if results.iter().all(|(_, buckets)| buckets.is_empty()) {
            let names = concepts.join("», «");
            return Ok(missing_values_answer(&names, scope));
        }
        let text = with_scope_of(
            report::computation_many(operation, &results),
            scope,
            documents.len(),
        );
        let mut citations = Vec::new();
        let mut seen = HashSet::new();
        for (concept, buckets) in &results {
            if buckets.is_empty() {
                continue;
            }
            for evidence in calculation_citations(operation.label(), concept, buckets) {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence);
                }
            }
        }
        self.remember_scope(scope, state);
        // Falta uno de los campos pedidos, o alguno de ellos mezcla monedas
        // distintas dentro de sí mismo: la tabla es real y no inventa nada,
        // pero no puede declararse enteramente verificada porque no cubre
        // todo lo que se pidió con una sola cifra por campo.
        let fully_verified = calc::multi_field_is_fully_verified(&results);
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: fully_verified,
            citations,
            warning: (!fully_verified).then(|| {
                "Resultado parcial: no todos los campos pedidos tienen una cifra única y verificable en este alcance.".to_owned()
            }),
            ..Answer::default()
        };
        Ok(answer.with_scope(answer_scope_of(scope, Some(total), documents.len())))
    }

    /// Combina dos campos numéricos del mismo documento («Cantidad ×
    /// Precio unitario»), documento por documento, y suma los resultados si
    /// hay más de uno. Nunca opera entre los totales globales de cada
    /// campo: esa sería una cifra distinta de la que se pidió.
    fn compute_row(
        &self,
        operation: RowOperation,
        left: &str,
        right: &str,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let left_operands = self.tools.collect_operands(&ValueQuery {
            concept: left,
            documents: Some(&scope.documents),
            ..ValueQuery::default()
        })?;
        let right_operands = self.tools.collect_operands(&ValueQuery {
            concept: right,
            documents: Some(&scope.documents),
            ..ValueQuery::default()
        })?;
        if left_operands.is_empty() {
            return Ok(missing_values_answer(left, scope));
        }
        if right_operands.is_empty() {
            return Ok(missing_values_answer(right, scope));
        }
        let computation = calc::compute_row(operation, &left_operands, &right_operands);
        let text = with_scope_of(
            report::row_computation(operation, left, right, &computation),
            scope,
            row_documents(&computation),
        );
        if computation.outcomes.is_empty() {
            let mut answer = Answer::unverified(text);
            answer.used_context = scope.inherited;
            return Ok(answer);
        }
        let mut citations = Vec::new();
        let mut seen = HashSet::new();
        for outcome in computation.outcomes.iter().take(MAX_OPERAND_CITATIONS) {
            for evidence in [&outcome.left_evidence, &outcome.right_evidence] {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence.clone());
                }
            }
        }
        // Un documento excluido también se rastrea a sus dos operandos: la
        // explicación de por qué no entró debe poder verificarse igual que
        // la cifra que sí se publicó.
        for skip in computation.skipped.iter().take(MAX_OPERAND_CITATIONS) {
            for evidence in [&skip.left_evidence, &skip.right_evidence] {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence.clone());
                }
            }
        }
        self.remember_scope(scope, state);
        // Un documento con unidades incompatibles, dividido entre cero, o
        // presente sólo en uno de los dos campos, no se descarta en
        // silencio: se cuenta y se explica, pero le quita a la respuesta el
        // derecho a declararse totalmente verificada.
        let has_issues = !computation.skipped.is_empty() || computation.unmatched_documents > 0;
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: !has_issues,
            citations,
            warning: has_issues.then(|| {
                "Resultado parcial: algunos documentos del alcance no participaron en el cálculo.".to_owned()
            }),
            ..Answer::default()
        };
        Ok(answer.with_scope(answer_scope_of(
            scope,
            Some(computation.outcomes.len() as i64),
            row_documents(&computation),
        )))
    }

    fn rank(
        &self,
        operation: Operation,
        descending: bool,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let (Some(concept), Some(group)) = (scope.concept.clone(), scope.group_by.clone()) else {
            return Ok(empty_scope_answer(scope));
        };
        let operands = self.tools.collect_operands(&ValueQuery {
            concept: &concept,
            documents: Some(&scope.documents),
            group_by: Some(&group),
            currency: scope.currency.as_deref(),
            ..ValueQuery::default()
        })?;
        let mut buckets = calc::compute(operation, &operands);
        if buckets.is_empty() {
            return Ok(missing_values_answer(&concept, scope));
        }
        buckets.sort_by(|left, right| {
            if descending {
                right.value.cmp(&left.value)
            } else {
                left.value.cmp(&right.value)
            }
            .then_with(|| left.group.cmp(&right.group))
        });
        let documents = operand_documents(&buckets);
        let text = with_scope_of(
            report::ranking(operation, &concept, &group, &buckets, descending),
            scope,
            documents,
        );
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        self.remember_scope(scope, state);
        self.remember_computation(operation.label(), &concept, &buckets, state);
        state.group_by = Some(group);
        Ok(Answer::verified(text, citations).with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    fn compare_groups(
        &self,
        operation: Operation,
        dimension: &str,
        left: &str,
        right: &str,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let Some(concept) = scope.concept.clone() else {
            return Ok(missing_values_answer("el campo pedido", scope));
        };
        let mut sides = Vec::new();
        for value in [left, right] {
            let mut filters = scope.filters.clone();
            filters.push(ToolFilter {
                concept: dimension.to_owned(),
                equals: value.to_owned(),
            });
            let documents = self
                .tools
                .documents_matching(&filters, scope.origin.as_deref(), scope.date.as_ref())?
                .into_iter()
                .map(|document| document.id)
                .collect::<Vec<_>>();
            let operands = self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?;
            sides.push(calc::compute(operation, &operands));
        }
        let (left_buckets, right_buckets) = (sides[0].clone(), sides[1].clone());
        let mut buckets = left_buckets.clone();
        buckets.extend(right_buckets.clone());
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        let documents = operand_documents(&buckets);

        if left_buckets.len() > 1 || right_buckets.len() > 1 {
            self.remember_scope(scope, state);
            return Ok(multi_currency_comparison(
                operation,
                &concept,
                dimension,
                (left, &left_buckets),
                (right, &right_buckets),
                scope,
                documents,
            ));
        }
        let text = with_scope_of(
            report::comparison(
                operation,
                &concept,
                dimension,
                (left, left_buckets.first()),
                (right, right_buckets.first()),
            ),
            scope,
            documents,
        );
        // Una comparación a la que le falta un lado o que mezcla monedas no
        // resolvió la pregunta: se responde explicándolo, sin marcarla como
        // verificada.
        let complete = left_buckets.first().is_some()
            && right_buckets.first().is_some()
            && left_buckets.first().map(|bucket| bucket.currency.clone())
                == right_buckets.first().map(|bucket| bucket.currency.clone());
        self.remember_scope(scope, state);
        state.concept = Some(concept.clone());
        state.comparison = Some(ComparisonMemory {
            concept: concept.clone(),
            dimension: dimension.to_owned(),
            left_label: left.to_owned(),
            right_label: right.to_owned(),
            left: side_memory(left_buckets.first()),
            right: side_memory(right_buckets.first()),
            evidence: citations.clone(),
        });
        Ok(Answer {
            text,
            mode: "local".into(),
            verified: complete,
            citations,
            ..Answer::default()
        }
        .with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    fn compare_periods(
        &self,
        operation: Operation,
        previous: &DateConstraint,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let Some(concept) = scope.concept.clone() else {
            return Ok(missing_values_answer("el campo pedido", scope));
        };
        let current = calc::compute(
            operation,
            &self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&scope.documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?,
        );
        let previous_documents = self
            .tools
            .documents_matching(&scope.filters, scope.origin.as_deref(), Some(previous))?
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        let earlier = calc::compute(
            operation,
            &self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&previous_documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?,
        );
        let current_label = scope
            .range
            .as_ref()
            .map(|range| range.label())
            .unwrap_or_else(|| "periodo actual".into());
        let previous_label = format!("{} a {}", previous.from, previous.to);
        let text = report::periods(
            operation,
            &concept,
            &previous.concept,
            (&previous_label, earlier.first()),
            (&current_label, current.first()),
        );
        let mut buckets = earlier.clone();
        buckets.extend(current.clone());
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        let documents = operand_documents(&buckets);
        let complete = earlier.first().is_some()
            && current.first().is_some()
            && earlier.first().map(|bucket| bucket.currency.clone())
                == current.first().map(|bucket| bucket.currency.clone());
        self.remember_scope(scope, state);
        state.concept = Some(concept.clone());
        state.comparison = Some(ComparisonMemory {
            concept: concept.clone(),
            dimension: previous.concept.clone(),
            left_label: previous_label,
            right_label: current_label,
            left: side_memory(earlier.first()),
            right: side_memory(current.first()),
            evidence: citations.clone(),
        });
        Ok(Answer {
            text: with_scope_of(text, scope, documents),
            mode: "local".into(),
            verified: complete,
            citations,
            ..Answer::default()
        }
        .with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    /// Diferencia o porcentaje de la comparación anterior, sin volver a
    /// consultar el acervo: los dos lados ya están en el contexto con su
    /// evidencia.
    fn comparison_follow_up(&self, percentage: bool, state: &ConversationState) -> Answer {
        let Some(comparison) = &state.comparison else {
            return no_evidence_answer();
        };
        let (Some(left), Some(right)) = (&comparison.left, &comparison.right) else {
            let missing = if comparison.left.is_none() {
                &comparison.left_label
            } else {
                &comparison.right_label
            };
            let mut answer = Answer::unverified(format!(
                "La comparación anterior no tiene valores para «{missing}», así que su diferencia no está definida."
            ));
            answer.used_context = true;
            return answer;
        };
        if left.currency != right.currency {
            let mut answer = Answer::unverified(format!(
                "No puedo restar los dos lados de la comparación anterior: «{}» está en {} y «{}» en {}.",
                comparison.left_label,
                left.currency.clone().unwrap_or_else(|| "sin moneda".into()),
                comparison.right_label,
                right.currency.clone().unwrap_or_else(|| "sin moneda".into())
            ));
            answer.used_context = true;
            return answer;
        }
        let first = Decimal::from_raw(left.units);
        let second = Decimal::from_raw(right.units);
        let difference = second.sub(first);
        let text = if percentage {
            match Decimal::percent_change(first, second) {
                Some(change) => format!(
                    "Respecto a «{}» ({}), «{}» ({}) representa una variación de {} %. La diferencia absoluta es {}. Cálculo local de Omega sobre {} y {} valores con evidencia.",
                    comparison.left_label,
                    left.rendered,
                    comparison.right_label,
                    right.rendered,
                    change.render_signed(),
                    calc::render_amount(difference.abs(), left.currency.as_deref()),
                    left.value_count,
                    right.value_count
                ),
                None => format!(
                    "La variación porcentual no está definida: «{}» vale {} y no se puede dividir entre cero. La diferencia absoluta es {}.",
                    comparison.left_label,
                    left.rendered,
                    calc::render_amount(difference.abs(), left.currency.as_deref())
                ),
            }
        } else {
            format!(
                "Diferencia entre «{}» ({}) y «{}» ({}) en «{}», comparados por «{}»: {}; en valor absoluto, {}. Cálculo local de Omega sobre {} y {} valores con evidencia.",
                comparison.right_label,
                right.rendered,
                comparison.left_label,
                left.rendered,
                comparison.concept,
                comparison.dimension,
                calc::render_amount(difference, left.currency.as_deref()),
                calc::render_amount(difference.abs(), left.currency.as_deref()),
                left.value_count,
                right.value_count
            )
        };
        let mut answer = Answer::verified(text, comparison.evidence.clone());
        answer.used_context = true;
        answer
    }

    fn evidence_for_last(&self, state: &ConversationState) -> Answer {
        let Some(previous) = &state.last_result else {
            return no_evidence_answer();
        };
        let text = report::supporting_documents(
            &previous.operation,
            &previous.concept,
            &previous.rendered,
            previous.value_count,
            &previous.evidence,
        );
        let mut answer = Answer::verified(text, previous.evidence.clone());
        answer.used_context = true;
        answer
    }

    fn relation_without_key(&self, question: &str) -> Result<Answer> {
        let subject = self
            .tools
            .filters_from_query(question, None, true)?
            .into_iter()
            .map(|filter| filter.equals)
            .next()
            .or_else(|| subject_after_container(question));
        let Some(subject) = subject else {
            return Ok(Answer::unverified(
                "No puedo afirmar una relación: no encontré en la pregunta un valor con clave estable, y unir documentos por parecido de nombres no es evidencia.",
            ));
        };
        let mentions = relations::mentions_without_key(&self.tools, &subject)?;
        let citations = mentions
            .iter()
            .map(|mention| mention.evidence.clone())
            .collect::<Vec<_>>();
        let text = report::relation_without_key(&subject, &mentions);
        // Hay evidencia de menciones, pero no de un vínculo: la respuesta se
        // marca como no verificada porque lo que se pidió —una relación— no
        // está respaldado.
        Ok(Answer {
            text,
            mode: "local".into(),
            verified: false,
            citations,
            ..Answer::default()
        })
    }

    fn contradictions(&self, key: Option<&str>, compared: Option<&str>) -> Result<Answer> {
        let found = relations::contradictions(&self.tools, key, compared)?;
        if found.is_empty() {
            return Ok(Answer::unverified(match (key, compared) {
                (Some(key), Some(compared)) => format!(
                    "No encontré evidencia de contradicción: ningún valor de «{key}» se repite en dos documentos con «{compared}» distintos."
                ),
                (Some(key), None) => format!(
                    "No encontré evidencia de contradicción: ningún valor de «{key}» se repite en dos documentos con campos incompatibles."
                ),
                _ => "No encontré documentos que compartan una clave estable y declaren valores incompatibles.".to_owned(),
            }));
        }
        let citations = found
            .iter()
            .flat_map(|item| item.entries.iter().map(|entry| entry.evidence.clone()))
            .take(MAX_OPERAND_CITATIONS)
            .collect::<Vec<_>>();
        Ok(Answer::verified(report::contradictions(&found), citations))
    }

    fn dossier(&self, canonical: &str, state: &mut ConversationState) -> Result<Answer> {
        let Some(dossier) = relations::dossier(&self.tools, canonical)? else {
            return Ok(no_evidence_answer());
        };
        let mut citations = dossier
            .group
            .documents
            .iter()
            .map(|document| document.evidence.clone())
            .collect::<Vec<_>>();
        let mut seen = citations
            .iter()
            .map(|evidence| evidence.id.clone())
            .collect::<HashSet<_>>();
        for field in &dossier.fields {
            for value in &field.values {
                if citations.len() >= 60 {
                    break;
                }
                if seen.insert(value.evidence.id.clone()) {
                    citations.push(value.evidence.clone());
                }
            }
        }
        let text = report::dossier(&dossier);
        state.identifier = Some(canonical.to_owned());
        state.set = Some(crate::conversation::DocumentSet {
            identifier: Some(canonical.to_owned()),
            document_count: dossier.group.documents.len() as i64,
            ..Default::default()
        });
        Ok(Answer::verified(text, citations))
    }

    fn remember_scope(&self, scope: &PlannedScope, state: &mut ConversationState) {
        state.set = Some(scope.as_document_set());
        state.concept = scope.concept.clone();
        state.currency = scope.currency.clone();
        state.identifier = scope.identifier.clone();
    }

    fn remember_computation(
        &self,
        operation: &str,
        concept: &str,
        buckets: &[Bucket],
        state: &mut ConversationState,
    ) {
        let Some(first) = buckets.first() else {
            return;
        };
        let rendered = if buckets.len() == 1 {
            report::bucket_amount(first)
        } else {
            buckets
                .iter()
                .map(report::bucket_amount)
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let evidence = buckets
            .iter()
            .flat_map(|bucket| bucket.evidence.iter().cloned())
            .collect::<Vec<_>>();
        state.last_result = Some(ComputationMemory::new(
            operation,
            concept,
            rendered,
            total_values(buckets) as usize,
            evidence,
        ));
        state.currency = first.currency.clone();
    }
}

/// Sujeto escrito después de la palabra que nombra al contenedor: «resume el
/// expediente X» habla de X, aunque X no exista como valor de ningún campo.
fn subject_after_container(question: &str) -> Option<String> {
    const CONTAINERS: &[&str] = &[
        "expediente",
        "expedientes",
        "documento",
        "documentos",
        "registro",
        "registros",
        "archivo",
        "archivos",
        "caso",
        "casos",
    ];
    let words = question.split_whitespace().collect::<Vec<_>>();
    let position = words
        .iter()
        .position(|word| CONTAINERS.contains(&normalize_exact(word).as_str()))?;
    let tail = words
        .get(position + 1..)
        .map(|rest| rest.join(" "))
        .unwrap_or_default();
    let tail = tail
        .trim()
        .trim_end_matches(['.', '?', '!', ',', ';'])
        .trim()
        .to_owned();
    (!tail.is_empty() && tail.split_whitespace().count() <= 6).then_some(tail)
}

fn total_values(buckets: &[Bucket]) -> i64 {
    buckets.iter().map(|bucket| bucket.value_count as i64).sum()
}

/// Documentos que realmente aportaron un operando. El alcance publicado debe
/// ser este, no el tamaño del conjunto consultado: decir «600 documentos»
/// cuando sólo 140 tenían el campo describe mal el cálculo.
fn operand_documents(buckets: &[Bucket]) -> usize {
    buckets
        .iter()
        .flat_map(|bucket| bucket.document_ids.iter().copied())
        .collect::<HashSet<_>>()
        .len()
}

/// Documentos que de verdad se examinaron para una operación entre dos
/// campos: los que produjeron un resultado y los que se descartaron por una
/// razón concreta (unidades incompatibles, división entre cero). Un
/// documento al que sólo le faltaba uno de los dos campos nunca contó como
/// «examinado».
fn row_documents(computation: &RowComputation) -> usize {
    computation
        .outcomes
        .iter()
        .map(|outcome| outcome.document_id)
        .chain(computation.skipped.iter().map(|skip| skip.document_id))
        .collect::<HashSet<_>>()
        .len()
}

/// Añade al texto la línea de alcance con los mismos datos que consultó el
/// motor. Un cálculo sin su alcance no es reproducible.
fn with_scope(text: String, scope: &PlannedScope) -> String {
    with_scope_of(text, scope, scope.documents.len())
}

fn with_scope_of(text: String, scope: &PlannedScope, documents: usize) -> String {
    let mut parts = Vec::new();
    if scope.inherited {
        parts.push("resultado anterior".to_owned());
    }
    if let Some(origin) = &scope.origin {
        parts.push(format!("carpeta = {origin}"));
    }
    for filter in &scope.filters {
        parts.push(format!("{} = {}", filter.concept, filter.equals));
    }
    if let Some(identifier) = &scope.identifier {
        parts.push(format!("identificador = {identifier}"));
    }
    if let Some(date) = &scope.date {
        parts.push(date.label());
    }
    parts.push(format!(
        "{documents} {}",
        report::plural(documents, "documento", "documentos")
    ));
    match report::scope_line(&parts) {
        Some(line) => format!("{text}\n\n{line}"),
        None => text,
    }
}

fn answer_scope_of(
    scope: &PlannedScope,
    value_count: Option<i64>,
    documents: usize,
) -> AnswerScope {
    AnswerScope {
        document_count: Some(documents as i64),
        ..answer_scope(scope, value_count)
    }
}

fn answer_scope(scope: &PlannedScope, value_count: Option<i64>) -> AnswerScope {
    AnswerScope {
        filters: scope.filters.clone(),
        origin: scope.origin.clone(),
        concept: scope.concept.clone(),
        group_by: scope.group_by.clone(),
        date: scope.date.clone(),
        currency: scope.currency.clone(),
        document_count: Some(scope.documents.len() as i64),
        value_count,
        inherited: scope.inherited,
    }
}

/// Citas de un cálculo: primero la nota que declara la operación local, después
/// los operandos que la respaldan.
fn calculation_citations(operation: &str, concept: &str, buckets: &[Bucket]) -> Vec<Evidence> {
    let mut citations = Vec::new();
    for bucket in buckets.iter().take(MAX_CALCULATION_CITATIONS) {
        citations.push(report::calculation_evidence(
            operation,
            concept,
            &report::bucket_amount(bucket),
            bucket.value_count,
            bucket.evidence.first(),
        ));
    }
    let mut seen = citations
        .iter()
        .map(|evidence| evidence.id.clone())
        .collect::<HashSet<_>>();
    // Muestra por turnos entre grupos: una comparación o un ranking deben
    // enseñar evidencia de todos sus lados, no llenar el cupo con el primero.
    let mut round = 0;
    while citations.len() < MAX_OPERAND_CITATIONS + citations.len().min(MAX_CALCULATION_CITATIONS) {
        let mut added = false;
        for bucket in buckets {
            let Some(evidence) = bucket.evidence.get(round) else {
                continue;
            };
            added = true;
            if seen.insert(evidence.id.clone()) {
                citations.push(evidence.clone());
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    citations
}

fn side_memory(bucket: Option<&Bucket>) -> Option<ComparisonSide> {
    bucket.map(|bucket| ComparisonSide {
        rendered: report::bucket_amount(bucket),
        value_count: bucket.value_count,
        currency: bucket.currency.clone(),
        units: bucket.value.raw(),
    })
}

fn multi_currency_comparison(
    operation: Operation,
    concept: &str,
    dimension: &str,
    left: (&str, &[Bucket]),
    right: (&str, &[Bucket]),
    scope: &PlannedScope,
    documents: usize,
) -> Answer {
    let mut rows = Vec::new();
    for (label, buckets) in [left, right] {
        for bucket in buckets {
            rows.push(vec![
                label.to_owned(),
                bucket
                    .currency
                    .clone()
                    .unwrap_or_else(|| "Sin moneda".into()),
                report::bucket_amount(bucket),
                bucket.value_count.to_string(),
            ]);
        }
    }
    let text = with_scope_of(
        format!(
            "No puedo comparar «{}» y «{}» con una sola cifra: hay más de una moneda en «{concept}» y las cantidades no se combinan. Cada moneda por separado:\n\n{}",
            left.0,
            right.0,
            report::table(
                &[dimension, "Moneda", report::operation_title(operation), "Valores"],
                &rows
            )
        ),
        scope,
        documents,
    );
    let mut buckets = left.1.to_vec();
    buckets.extend(right.1.to_vec());
    // La pregunta pedía una comparación y no se pudo dar: la respuesta informa,
    // pero no se presenta como verificada.
    Answer {
        text,
        mode: "local".into(),
        verified: false,
        citations: calculation_citations(operation.label(), concept, &buckets),
        ..Answer::default()
    }
    .with_scope(answer_scope_of(scope, Some(total_values(&buckets)), documents))
}

fn empty_scope_answer(scope: &PlannedScope) -> Answer {
    let mut answer = Answer::unverified(with_scope(
        "No encontré documentos con evidencia local que cumplan ese alcance.".to_owned(),
        scope,
    ));
    answer.used_context = scope.inherited;
    answer
}

fn missing_values_answer(concept: &str, scope: &PlannedScope) -> Answer {
    let mut answer = Answer::unverified(with_scope(
        format!("No encontré valores de «{concept}» con evidencia en ese alcance, así que no puedo calcular nada sobre ellos."),
        scope,
    ));
    answer.used_context = scope.inherited;
    answer
}

fn clarify(clarification: Clarification) -> Answer {
    let text = if clarification.options.is_empty() {
        clarification.question.clone()
    } else {
        format!(
            "{}\n\n{}",
            clarification.question,
            clarification
                .options
                .iter()
                .map(|option| format!("- {option}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Answer {
        text,
        mode: "local".into(),
        verified: false,
        clarification: Some(clarification),
        ..Answer::default()
    }
}
