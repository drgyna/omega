//! Planificador local de consultas. Decide una ruta pequeña y auditable a
//! partir de señales lingüísticas genéricas y del esquema descubierto en el
//! índice. No contiene nombres de carpetas, campos, ciudades ni estados de un
//! corpus concreto.

use crate::{
    error::Result,
    model::{AggregateRequest, ConceptSummary, ToolFilter},
    normalize::{canonical_key, normalize_exact, normalize_spanish, search_terms, stems_match},
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
    let filters = tools.resolved_filters(
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

// ---------------------------------------------------------------------------
// Plan estructurado
//
// El plan clásico de arriba decide una ruta de recuperación. Lo que sigue
// decide una ruta de *razonamiento*: qué operación aplicar, sobre qué conjunto,
// con qué campo y con qué periodo. Es un dato tipado y auditable, no una
// cadena: la respuesta puede mostrar exactamente el mismo alcance que se
// consultó.
//
// Regla de convivencia: una consulta sólo entra aquí si trae una señal de alta
// precisión —una palabra de operación, una referencia al turno anterior, una
// expresión de calendario o una petición de evidencia, relación o
// contradicción—. Todo lo demás cae intacto en el planificador clásico, de modo
// que el comportamiento de recuperación ya verificado no cambia.
// ---------------------------------------------------------------------------

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    calc::{Operation, RowOperation},
    conversation::{
        ConversationState, DocumentSet, PendingChoice, PendingKind, Reference, reference_in,
    },
    dates::{self, Clock, DateRange},
    model::{Clarification, DateConstraint},
    relations,
};

#[derive(Clone, Debug)]
pub enum Command {
    /// Sin señales nuevas: lo resuelve el planificador clásico.
    Retrieval,
    /// Operación aritmética sobre un campo del alcance.
    Compute(Operation),
    /// Agrupa por un campo y ordena, para responder «cuál … más/menos».
    Rank {
        operation: Operation,
        descending: bool,
    },
    /// Dos valores del mismo campo, calculados por separado y comparados.
    CompareGroups {
        operation: Operation,
        concept: String,
        left: String,
        right: String,
    },
    /// El mismo alcance en dos periodos.
    ComparePeriods {
        operation: Operation,
        previous: DateConstraint,
    },
    /// Documentos que respaldan el último cálculo de la conversación.
    EvidenceForLast,
    /// Petición de relación sobre un texto que no produce clave estable.
    RelationWithoutKey,
    /// Contradicciones entre documentos vinculados. Cuando la pregunta nombra
    /// los campos («¿hay folios con estados diferentes?»), la búsqueda se
    /// restringe a esa clave y a ese campo.
    Contradictions {
        key: Option<String>,
        compared: Option<String>,
    },
    /// Ficha extractiva de un identificador y sus documentos vinculados.
    Dossier { canonical: String },
    /// El usuario pidió «todos» en vez de elegir un campo de una aclaración:
    /// se calcula la misma operación para cada campo ofrecido, sobre el mismo
    /// conjunto de documentos.
    ComputeMany {
        operation: Operation,
        concepts: Vec<String>,
    },
    /// Operación entre dos campos numéricos del mismo documento («Cantidad ×
    /// Precio unitario»). El resultado, si se agrega, es la suma de los
    /// resultados por documento — nunca una operación entre los totales
    /// globales de cada campo.
    ComputeRow {
        operation: RowOperation,
        left: String,
        right: String,
    },
    /// Diferencia o porcentaje de la comparación ya calculada en el turno
    /// anterior, sin volver a consultar el acervo.
    ComparisonFollowUp { percentage: bool },
    /// El motor no puede saber a qué se refiere la pregunta.
    Clarify(Clarification),
    /// La pregunta es resoluble, pero el acervo no tiene lo que pide. Se
    /// responde explicando qué falta, nunca con algo parecido.
    NoEvidence { message: String },
}

/// Alcance ya resuelto contra el índice.
#[derive(Clone, Debug, Default)]
pub struct PlannedScope {
    pub filters: Vec<ToolFilter>,
    pub origin: Option<String>,
    pub identifier: Option<String>,
    pub date: Option<DateConstraint>,
    pub range: Option<DateRange>,
    pub concept: Option<String>,
    pub group_by: Option<String>,
    pub currency: Option<String>,
    /// Documentos que cumplen el alcance, ya resueltos.
    pub documents: Vec<i64>,
    pub paths: Vec<String>,
    /// El alcance viene del turno anterior.
    pub inherited: bool,
    /// Procedencia de `filters`: claves (concepto, valor) que la pregunta
    /// ACTUAL escribió explícitamente con sintaxis «Campo: valor».
    ///
    /// Una heurística puede descartar un filtro inferido o heredado (por
    /// ejemplo, el que una comparación arma a partir de sus propios grupos),
    /// pero nunca uno que el usuario escribió a propósito, aunque su valor
    /// coincida por casualidad con uno de esos grupos.
    pub explicit_filters: BTreeSet<(String, String)>,
}

impl PlannedScope {
    pub fn as_document_set(&self) -> DocumentSet {
        DocumentSet {
            filters: self.filters.clone(),
            origin: self.origin.clone(),
            identifier: self.identifier.clone(),
            date: self.date.clone(),
            range: self.range.clone(),
            document_count: self.documents.len() as i64,
            paths: self.paths.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StructuredPlan {
    pub command: Command,
    pub scope: PlannedScope,
    /// Cuando el comando es una aclaración, lo que hay que recordar para poder
    /// ejecutar la operación pendiente en cuanto el usuario elija.
    pub pending: Option<PendingChoice>,
}

impl StructuredPlan {
    fn retrieval() -> Self {
        Self {
            command: Command::Retrieval,
            scope: PlannedScope::default(),
            pending: None,
        }
    }

    fn with_pending(mut self, pending: PendingChoice) -> Self {
        self.pending = Some(pending);
        self
    }

    fn without_evidence(message: impl Into<String>) -> Self {
        Self {
            command: Command::NoEvidence {
                message: message.into(),
            },
            scope: PlannedScope::default(),
            pending: None,
        }
    }

    fn clarify(reason: &str, question: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            command: Command::Clarify(Clarification {
                question: question.into(),
                options,
                reason: reason.into(),
            }),
            scope: PlannedScope::default(),
            pending: None,
        }
    }
}

/// Señales léxicas genéricas. Ninguna nombra un giro de negocio: son verbos y
/// sustantivos de operación del español, más sus equivalentes en inglés que ya
/// usaba el planificador clásico.
struct Signals {
    count: bool,
    sum: bool,
    average: bool,
    maximum: bool,
    minimum: bool,
    difference: bool,
    percent: bool,
    compare: bool,
    /// «compara», «comparar», «versus»: la pregunta pide explícitamente una
    /// comparación.
    compare_verb: bool,
    /// «contra», «frente a»: sólo es comparación si además hay dos grupos
    /// nombrados; en español son preposiciones corrientes.
    compare_preposition: bool,
    group: bool,
    evidence: bool,
    related: bool,
    contradictions: bool,
    /// «diferentes», «distintos»: pregunta por valores que no coinciden.
    differing: bool,
    summary: bool,
    superlative: bool,
    /// La pregunta nombra un extremo de forma explícita («máximo», «mínimo»),
    /// no sólo un comparativo suelto como «más».
    extreme_named: bool,
    descending: bool,
    container: bool,
    calendar: bool,
    /// «multiplicar», «multiplicada»: operación entre dos campos, no entre
    /// grupos de un mismo campo.
    multiply: bool,
    /// «dividir», «dividido», «división»: idem, para el cociente.
    divide: bool,
}

const CONTAINER_WORDS: &[&str] = &[
    "document", "archiv", "expedient", "registr", "caso", "carpet", "file",
];

const CALENDAR_WORDS: &[&str] = &[
    "fecha", "fechas", "mes", "meses", "ano", "anos", "periodo", "periodos", "trimestre",
    "durante", "entre", "desde", "hasta", "enero", "febrero", "marzo", "abril", "mayo", "junio",
    "julio", "agosto", "septiembre", "setiembre", "octubre", "noviembre", "diciembre",
];

/// Palabras de comparación. Se comparan como palabras completas y no como
/// raíces: «contra» es una comparación, pero «contratada» no lo es, y
/// «comparar» lo es mientras que «compareciente» no. Una raíz compartida basta
/// para robarle una consulta legítima al motor de recuperación.
const COMPARE_WORDS: &[&str] = &[
    "compara",
    "comparar",
    "comparalo",
    "comparala",
    "comparalos",
    "comparalas",
    "comparacion",
    "comparativa",
    "versus",
    "vs",
    // A partir de aquí, preposiciones: no bastan por sí solas.
    "contra",
    "frente",
];

/// Cuántas de las palabras anteriores son verbos o sustantivos de comparación.
const COMPARE_VERBS: usize = 10;

/// Palabras que preguntan por contradicciones. Como en las comparaciones, se
/// exigen palabras completas: un documento puede *mencionar* una inconsistencia
/// sin que la pregunta trate de buscar contradicciones en el acervo.
const CONTRADICTION_WORDS: &[&str] = &[
    "contradictorio",
    "contradictorios",
    "contradictoria",
    "contradictorias",
    "contradiccion",
    "contradicciones",
    "inconsistencias",
    "incompatibles",
    "discrepancias",
    "discrepancia",
];

/// Palabras que describen valores que no coinciden. Por sí solas no son una
/// consulta de contradicciones: hace falta además que la pregunta nombre los
/// campos implicados.
const DIFFERING_WORDS: &[&str] = &[
    "diferentes",
    "distintos",
    "distintas",
    "diferente",
    "distinto",
    "distinta",
    "incompatibles",
    "incompatible",
];

const MAXIMUM_WORDS: &[&str] = &["mas", "mayor", "mayores", "alto", "alta", "altos", "altas"];
const MINIMUM_WORDS: &[&str] = &["menos", "menor", "menores", "bajo", "baja", "bajos", "bajas"];
const DIFFERENCE_WORDS: &[&str] = &["diferencia", "diferencias", "resta", "restar", "difference"];

fn signals(question: &str) -> Signals {
    let terms = search_terms(question);
    let words = normalize_exact(question)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let has = |root: &str| terms.iter().any(|term| term.starts_with(root));
    let word = |value: &str| words.iter().any(|item| item == value);
    let any_word = |list: &[&str]| list.iter().any(|value| word(value));
    let maximum = has("maxim") || any_word(MAXIMUM_WORDS);
    let minimum = has("minim") || any_word(MINIMUM_WORDS);
    Signals {
        count: has("cuant") || has("numer") || has("conte") || has("how"),
        sum: has("sum") || has("totaliz"),
        average: has("promedi") || word("media") || has("average"),
        maximum,
        minimum,
        difference: any_word(DIFFERENCE_WORDS),
        percent: has("porcentaj") || has("porcentual") || has("variacion") || has("crecimient"),
        compare: any_word(COMPARE_WORDS),
        compare_verb: any_word(&COMPARE_WORDS[..COMPARE_VERBS]),
        compare_preposition: any_word(&COMPARE_WORDS[COMPARE_VERBS..]),
        group: has("agrup") || has("desglos") || has("group"),
        evidence: has("respald") || has("sustent") || has("evidenci") || has("justific"),
        related: has("relacionad") || has("vinculad") || has("ligad") || has("asociad"),
        contradictions: any_word(CONTRADICTION_WORDS),
        differing: any_word(DIFFERING_WORDS),
        summary: has("resum"),
        superlative: maximum || minimum,
        extreme_named: has("maxim") || has("minim"),
        descending: has("maxim") || any_word(MAXIMUM_WORDS),
        container: CONTAINER_WORDS
            .iter()
            .any(|root| terms.iter().any(|term| term.starts_with(root))),
        calendar: CALENDAR_WORDS.iter().any(|value| word(value)),
        multiply: has("multiplic"),
        divide: has("dividi") || has("division"),
    }
}

/// Campos que una aclaración ya resolvió y que el plan no debe volver a
/// deducir.
#[derive(Clone, Debug, Default)]
struct Forced {
    concept: Option<String>,
    date_concept: Option<String>,
}

/// Decide la ruta de razonamiento de una pregunta dentro de su conversación.
///
/// Si la conversación tenía una aclaración pendiente y el usuario responde con
/// una de las opciones ofrecidas, se vuelve a planificar la **pregunta
/// original** con ese campo ya fijado y sobre el mismo conjunto. Tratar la
/// respuesta como una consulta nueva perdería el alcance que motivó la
/// pregunta.
pub fn plan_structured(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    clock: &Clock,
) -> Result<StructuredPlan> {
    if let Some(pending) = &state.pending {
        if let Some(choice) = pending.chosen(question) {
            let field = choice
                .split_once(": ")
                .map(|(_, value)| value.to_owned())
                .unwrap_or(choice);
            let mut resumed = state.clone();
            resumed.pending = None;
            resumed.set = Some(pending.set.clone());
            let forced = match pending.kind {
                PendingKind::Concept => Forced {
                    concept: Some(field),
                    date_concept: None,
                },
                PendingKind::DateField => Forced {
                    concept: None,
                    date_concept: Some(field),
                },
            };
            return plan_inner(tools, &pending.question, &resumed, clock, &forced);
        }
        // La respuesta no fue ninguna de las opciones ofrecidas. Nunca se
        // trata como una consulta nueva sobre todo el acervo: o el usuario
        // pidió explícitamente «todos» —y se calcula cada opción sobre el
        // mismo conjunto—, o la aclaración sigue pendiente y se repite.
        if matches!(pending.kind, PendingKind::Concept) && pending.wants_all(question) {
            return compute_all_options(tools, pending);
        }
        return Ok(reask(pending, question));
    }
    plan_inner(tools, question, state, clock, &Forced::default())
}

/// Vuelve a resolver el conjunto de una aclaración pendiente y calcula la
/// misma operación de la pregunta original para cada campo ofrecido, en vez
/// de obligar a elegir uno solo.
fn compute_all_options(tools: &ToolEngine, pending: &PendingChoice) -> Result<StructuredPlan> {
    let marks = signals(&pending.question);
    let operation = requested_operation(&marks).unwrap_or(Operation::Sum);
    let mut scope = PlannedScope {
        filters: pending.set.filters.clone(),
        origin: pending.set.origin.clone(),
        identifier: pending.set.identifier.clone(),
        date: pending.set.date.clone(),
        range: pending.set.range.clone(),
        inherited: true,
        ..PlannedScope::default()
    };
    resolve_documents(tools, &mut scope)?;
    Ok(StructuredPlan {
        command: Command::ComputeMany {
            operation,
            concepts: pending.options.clone(),
        },
        scope,
        // «Todos» no resuelve la aclaración, la responde sin obligar a
        // elegir: el usuario puede seguir nombrando un campo concreto después
        // («Monto principal») y debe replanificarse igual que si lo hubiera
        // dicho desde el principio, sobre el mismo conjunto.
        pending: Some(pending.clone()),
    })
}

/// Repite la misma aclaración, conservando la elección pendiente intacta: la
/// respuesta del usuario no fue una de las opciones ni pidió «todos», así que
/// no hay nada nuevo que planificar todavía.
fn reask(pending: &PendingChoice, question: &str) -> StructuredPlan {
    let (reason, base) = match pending.kind {
        PendingKind::Concept => (
            "campo_ambiguo",
            "Ese alcance tiene más de un campo numérico y la pregunta no dice cuál usar. ¿Cuál de estos?",
        ),
        PendingKind::DateField => (
            "campo_fecha_ambiguo",
            "El acervo tiene más de un campo de fecha y la pregunta no dice cuál usar para el periodo. ¿Cuál de estos?",
        ),
    };
    let trimmed = question.trim();
    let message = if trimmed.is_empty() {
        base.to_owned()
    } else {
        format!("No reconocí «{trimmed}» como una de las opciones. {base}")
    };
    StructuredPlan::clarify(reason, message, pending.options.clone()).with_pending(pending.clone())
}

fn plan_inner(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    clock: &Clock,
    forced: &Forced,
) -> Result<StructuredPlan> {
    // Una cita entrecomillada es una búsqueda literal del acervo. Interceptarla
    // convertiría en cálculo o en informe de contradicciones una pregunta que
    // sólo quería encontrar un texto.
    if ToolEngine::query_has_quoted_literal(question) {
        return Ok(StructuredPlan::retrieval());
    }

    // Un par «Campo: valor» escrito por el usuario manda sobre cualquier
    // inferencia. Si el valor no existe completo en el acervo, el motor
    // pregunta o dice que no lo encontró; jamás responde con un valor más
    // corto que se le parezca.
    let written = tools.written_filters(question)?;
    if let Some(unresolved) = written.unresolved.first() {
        return Ok(if unresolved.near.is_empty() {
            StructuredPlan::without_evidence(format!(
                "No encontré evidencia local suficiente: el campo «{}» no tiene el valor «{}» en el acervo indexado.",
                unresolved.concept, unresolved.written
            ))
        } else {
            StructuredPlan::clarify(
                "valor_inexistente",
                format!(
                    "El campo «{}» no tiene el valor «{}». Existen estos valores emparentados, pero no voy a suponer que te referías a uno de ellos:",
                    unresolved.concept, unresolved.written
                ),
                unresolved
                    .near
                    .iter()
                    .map(|value| format!("{}: {value}", unresolved.concept))
                    .collect(),
            )
        });
    }

    let marks = signals(question);
    let reference = reference_in(question);

    // «¿Hay documentos contradictorios?» es una consulta global; «¿hay folios
    // con estados diferentes?» nombra la clave y el campo comparado, y se
    // resuelve sobre esos dos campos concretos. Listar los folios existentes no
    // responde ninguna de las dos.
    let named = named_concepts_in_order(tools, question)?;
    if marks.contradictions || (marks.differing && !named.is_empty()) {
        let mut named = named.into_iter();
        return Ok(StructuredPlan {
            command: Command::Contradictions {
                key: named.next(),
                compared: named.next(),
            },
            scope: PlannedScope::default(),
            pending: None,
        });
    }

    // Operación entre dos campos numéricos del mismo documento («Cantidad
    // multiplicada por Precio unitario», «Monto A entre Monto B»). Se exige
    // un verbo explícito de la operación: nunca se dispara por sí sola una
    // preposición como "por" o "entre" —agrupación y rangos de fecha ya las
    // usan— así que no compite con ninguna ruta existente.
    if let Some((row_operation, left, right)) = numeric_field_pair(tools, &named, &marks)? {
        if wants_totals_directly(question) {
            return Ok(StructuredPlan::clarify(
                "operacion_entre_totales_ambigua",
                format!(
                    "No queda claro si quieres «{left}» {} «{right}» documento por documento (sumando después los resultados) o directamente entre los totales ya calculados de cada campo: son cálculos distintos y no elijo uno por ti. Pregunta de nuevo sin mencionar «totales» para calcularlo documento por documento, que es la forma verificable.",
                    row_operation.verb()
                ),
                vec![],
            ));
        }
        let mut scope = resolve_scope(tools, question, state, clock, reference, &marks, true, forced)?;
        resolve_documents(tools, &mut scope)?;
        scope.concept = None;
        return Ok(StructuredPlan {
            command: Command::ComputeRow {
                operation: row_operation,
                left,
                right,
            },
            scope,
            pending: None,
        });
    }

    // Ficha de expediente: «resumen» más una palabra que nombra al continente.
    // Sin esa palabra, «resume X» sigue siendo la ficha del documento principal
    // que ya resuelve la síntesis clásica.
    if marks.summary && marks.container {
        let candidates = relations::identifier_candidates(tools, question)?;
        match candidates.len() {
            1 => {
                return Ok(StructuredPlan {
                    command: Command::Dossier {
                        canonical: candidates[0].clone(),
                    },
                    scope: PlannedScope::default(),
                    pending: None,
                });
            }
            0 => {}
            _ => {
                return Ok(StructuredPlan::clarify(
                    "identificador_ambiguo",
                    "Ese identificador coincide con más de un registro del acervo. ¿Cuál de estos quieres?",
                    candidates,
                ));
            }
        }
    }

    // Petición de relación sobre algo que no produce una clave estable. La
    // relación por identificador la resuelve la síntesis clásica; aquí sólo se
    // atiende el caso en que no hay clave, para decirlo en vez de unir
    // documentos por parecido de nombres.
    // Pedir el expediente o las relaciones de algo que no produce clave estable
    // no puede resolverse con una búsqueda de texto presentada como verificada:
    // la pregunta era por un vínculo, y ese vínculo no existe.
    if (marks.related || (marks.summary && marks.container))
        && marks.container
        && relations::identifier_candidates(tools, question)?.is_empty()
    {
        return Ok(StructuredPlan {
            command: Command::RelationWithoutKey,
            scope: PlannedScope::default(),
            pending: None,
        });
    }

    if marks.evidence && (reference == Reference::Explicit || state.last_result.is_some()) {
        if state.last_result.is_none() {
            return Ok(StructuredPlan::clarify(
                "sin_contexto",
                "No hay un cálculo previo en esta conversación al que pueda atribuir esa evidencia. ¿Sobre qué quieres ver los documentos?",
                vec![],
            ));
        }
        return Ok(StructuredPlan {
            command: Command::EvidenceForLast,
            scope: PlannedScope::default(),
            pending: None,
        });
    }

    // Una continuación que pide explícitamente comparar, ordenar o agrupar pero
    // no nombra la operación repite la del turno anterior. Un filtro sobre el
    // conjunto anterior («de esos, ¿cuáles son de X?») no la hereda: pregunta
    // por documentos, no por una cifra.
    let inherits_operation = reference == Reference::Explicit
        && (marks.compare
            || marks.superlative
            || marks.group
            || marks.difference
            || marks.percent
            || dates::asks_for_previous_period(question));
    let operation = requested_operation(&marks).or_else(|| {
        state
            .last_result
            .as_ref()
            .filter(|_| inherits_operation)
            .and_then(|previous| Operation::from_label(&previous.operation))
    });
    let wants_period_comparison = dates::asks_for_previous_period(question)
        && (marks.compare || marks.difference || marks.percent || reference == Reference::Explicit);

    // Puerta de entrada. El razonamiento estructurado sólo se hace cargo de una
    // pregunta cuando aporta algo que la recuperación clásica no sabe hacer:
    // una operación que no existe allí (promedio, máximo, mínimo), una
    // comparación, un periodo, una agrupación con superlativo o una referencia
    // al turno anterior. Un conteo o una suma autónomos siguen exactamente el
    // camino ya verificado.
    let novel_operation = matches!(
        operation,
        Some(Operation::Average | Operation::Maximum | Operation::Minimum)
    );
    let candidate = novel_operation
        || marks.compare
        || marks.difference
        || marks.percent
        || wants_period_comparison
        || marks.superlative
        || reference == Reference::Explicit
        || (operation.is_some() && marks.calendar)
        || (operation.is_some() && state.set.is_some());
    if !candidate {
        // Una operación que no nombra su campo, no acota nada y no tiene un
        // resultado anterior al que referirse no es una consulta resoluble:
        // preguntar es más honesto que elegir un campo cualquiera.
        if operation.is_some_and(|value| value.needs_numbers())
            && state.set.is_none()
            && !question_names_a_concept(tools, question)?
            && tools.resolved_filters(question, None, true)?.is_empty()
            && tools.match_origin(question)?.is_none()
        {
            return Ok(StructuredPlan::clarify(
                "sin_contexto",
                "No sé sobre qué documentos ni sobre qué campo quieres ese cálculo: esta conversación aún no tiene un resultado anterior.",
                vec![],
            ));
        }
        return Ok(StructuredPlan::retrieval());
    }

    if reference == Reference::Explicit && !state.has_context() {
        return Ok(StructuredPlan::clarify(
            "referencia_sin_contexto",
            "No sé a qué te refieres: esta conversación aún no tiene un resultado anterior. ¿Puedes decirme qué documentos o qué campo quieres?",
            vec![],
        ));
    }

    let mut scope = resolve_scope(
        tools,
        question,
        state,
        clock,
        reference,
        &marks,
        operation.is_some() || marks.compare || marks.percent || marks.difference,
        forced,
    )?;

    // Hay periodo pero no un campo de fecha al que anclarlo: preguntar cuál,
    // nunca ignorar el periodo y calcular sobre todo.
    if scope.range.is_some() && scope.date.is_none() {
        return date_field_clarification(tools, question, &scope);
    }

    // Comparación entre dos grupos del mismo campo.
    //
    // Los dos valores se reconocen porque la pregunta los escribe literalmente,
    // no porque coincidan por raíces: así «Ciudad de México contra Guadalajara»
    // produce siempre dos grupos y nunca un resumen global.
    if marks.compare_verb || marks.difference || marks.percent || marks.compare_preposition {
        let targets = comparison_targets(tools, question, &scope)?;
        if let Some((dimension, left, right)) = targets {
            let operation = operation.unwrap_or(Operation::Sum);
            // Los dos grupos comparados salen del alcance como filtros: son la
            // dimensión de la comparación, no un recorte previo. Se descarta
            // también cualquier filtro inferido con esos mismos valores en otro
            // campo —«Veracruz» puede ser a la vez una ciudad y un estado—,
            // porque recortaría la comparación con una condición que el usuario
            // no pidió.
            // Salvo que ese mismo filtro lo haya escrito el usuario a
            // propósito con «Campo: valor»: un filtro explícito nunca se
            // descarta por una heurística, aunque su valor coincida por
            // casualidad con uno de los grupos comparados.
            let compared = [normalize_exact(&left), normalize_exact(&right)];
            let explicit = scope.explicit_filters.clone();
            scope.filters.retain(|filter| {
                let key = (canonical_key(&filter.concept), normalize_exact(&filter.equals));
                explicit.contains(&key)
                    || (canonical_key(&filter.concept) != canonical_key(&dimension)
                        && !compared.contains(&normalize_exact(&filter.equals)))
            });
            let documents = resolve_documents(tools, &mut scope)?;
            let concept =
                match resolve_computation_concept(
                    tools,
                    question,
                    state,
                    &documents,
                    operation,
                    false,
                    forced,
                    Some(&dimension),
                )? {
                    Resolved::Concept(name) => name,
                    other => return Ok(unresolved_plan(other, question, &scope)),
                };
            scope.concept = Some(concept);
            scope.group_by = Some(dimension.clone());
            return Ok(StructuredPlan {
                command: Command::CompareGroups {
                    operation,
                    concept: dimension,
                    left,
                    right,
                },
                scope,
                pending: None,
            });
        }
        // Una comparación reconocida no puede degradarse a búsqueda: si falta
        // un lado, se pregunta.
        if marks.compare_verb && !wants_period_comparison && state.comparison.is_none() {
            let mentioned = tools.values_mentioned(question, scope.origin.as_deref())?;
            if mentioned.len() == 1 {
                return Ok(StructuredPlan::clarify(
                    "comparacion_incompleta",
                    format!(
                        "Sólo reconozco un grupo en esa comparación: «{}». ¿Con qué valor quieres compararlo?",
                        mentioned[0].1
                    ),
                    vec![],
                ));
            }
            if mentioned.is_empty() && !marks.calendar {
                return Ok(StructuredPlan::clarify(
                    "comparacion_incompleta",
                    "No reconocí los dos grupos que quieres comparar. Nómbralos con valores que existan en el acervo.",
                    vec![],
                ));
            }
        }
    }

    // Seguimiento de una comparación ya calculada: «¿cuál es la diferencia?» o
    // «¿qué porcentaje representa?» no vuelven a consultar el acervo.
    if (marks.difference || marks.percent) && state.comparison.is_some() && !marks.compare_verb {
        return Ok(StructuredPlan {
            command: Command::ComparisonFollowUp {
                percentage: marks.percent,
            },
            scope,
            pending: None,
        });
    }

    // Segunda puerta, ya con el alcance resuelto: una operación simple sólo se
    // queda aquí si trae contexto heredado, un periodo anclado, una comparación,
    // una agrupación o una operación que la ruta clásica no implementa. Si no,
    // vuelve a esa ruta intacta, con su comportamiento ya verificado.
    let ranking_intent = (marks.superlative || marks.group)
        && grouping_concept(tools, question, &[], scope.concept.as_deref())?.is_named();
    if !novel_operation
        && !scope.inherited
        && scope.date.is_none()
        && !wants_period_comparison
        && !marks.compare
        && !ranking_intent
    {
        return Ok(StructuredPlan::retrieval());
    }

    let documents = resolve_documents(tools, &mut scope)?;

    // Comparación entre periodos.
    if wants_period_comparison {
        let Some(current) = scope.range.clone() else {
            return Ok(StructuredPlan::clarify(
                "periodo_sin_referencia",
                "Para comparar periodos necesito saber cuál es el periodo actual: ni la pregunta ni el resultado anterior fijan un rango de fechas.",
                vec![],
            ));
        };
        let Some(previous) = current.previous_period() else {
            return Ok(StructuredPlan::clarify(
                "periodo_sin_referencia",
                "No pude calcular el periodo anterior a partir del rango aplicado.",
                vec![],
            ));
        };
        let Some(anchor) = scope.date.clone() else {
            return Ok(date_field_clarification(tools, question, &scope)?);
        };
        let operation = operation.unwrap_or(Operation::Sum);
        let concept =
            match resolve_computation_concept(
                tools,
                question,
                state,
                &documents,
                operation,
                false,
                forced,
                None,
            )? {
                Resolved::Concept(name) => name,
                other => return Ok(unresolved_plan(other, question, &scope)),
            };
        scope.concept = Some(concept);
        return Ok(StructuredPlan {
            command: Command::ComparePeriods {
                operation,
                previous: DateConstraint {
                    concept: anchor.concept,
                    from: previous.from.to_iso(),
                    to: previous.to.to_iso(),
                },
            },
            scope,
            pending: None,
        });
    }

    // Agrupación: «qué ciudad tiene el mayor …» pregunta por el grupo, no por
    // el valor individual más alto. El campo agrupador no puede ser numérico.
    if marks.superlative || marks.group {
        match grouping_concept(tools, question, &documents, scope.concept.as_deref())? {
            Grouping::One(group) => {
                // En un ranking, «más» y «menos» indican la dirección del orden,
                // no la operación: «cuál cliente debe más» pregunta por el total
                // de cada cliente. Sólo un extremo nombrado explícitamente («el
                // importe máximo por cliente») cambia la operación.
                let operation = if marks.extreme_named || marks.average || marks.sum {
                    operation.unwrap_or(Operation::Sum)
                } else {
                    state
                        .last_result
                        .as_ref()
                        .and_then(|previous| Operation::from_label(&previous.operation))
                        .filter(|operation| operation.needs_numbers())
                        .unwrap_or(Operation::Sum)
                };
                let concept = match resolve_computation_concept(
                    tools,
                    question,
                    state,
                    &documents,
                    operation,
                    false,
                    forced,
                    Some(&group),
                )? {
                    Resolved::Concept(name) => name,
                    other => return Ok(unresolved_plan(other, question, &scope)),
                };
                scope.concept = Some(concept);
                scope.group_by = Some(group);
                return Ok(StructuredPlan {
                    command: Command::Rank {
                        operation,
                        descending: marks.descending || !marks.minimum,
                    },
                    scope,
                    pending: None,
                });
            }
            Grouping::Ambiguous(options) => {
                return Ok(StructuredPlan::clarify(
                    "agrupacion_ambigua",
                    "Hay más de un campo por el que podría agrupar y la pregunta no dice cuál. ¿Cuál de estos?",
                    options,
                ));
            }
            Grouping::None => {}
        }
    }

    let Some(operation) = operation else {
        // Referencia explícita sin operación reconocible: puede ser un filtro
        // sobre el conjunto anterior («de esos, ¿cuáles son de X?»).
        if reference == Reference::Explicit && !scope.filters.is_empty() {
            // Filtrar el conjunto anterior devuelve documentos, no una cifra
            // sobre el campo que se estuviera calculando antes.
            scope.concept = None;
            return Ok(StructuredPlan {
                command: Command::Compute(Operation::Count),
                scope,
                pending: None,
            });
        }
        return Ok(StructuredPlan::retrieval());
    };

    if operation == Operation::Count && scope.concept.is_none() && !marks.group {
        // Conteo de documentos: no necesita campo.
        return Ok(StructuredPlan {
            command: Command::Compute(Operation::Count),
            scope,
            pending: None,
        });
    }

    let concept = match resolve_computation_concept(
        tools,
        question,
        state,
        &documents,
        operation,
        !scope.inherited,
        forced,
        None,
    )? {
        Resolved::Concept(name) => name,
        other => return Ok(unresolved_plan(other, question, &scope)),
    };
    scope.concept = Some(concept);
    Ok(StructuredPlan {
        command: Command::Compute(operation),
        scope,
        pending: None,
    })
}

/// Traduce a un plan lo que no se pudo resolver. Ninguna de estas ramas
/// responde con un campo distinto del que pidió el usuario.
fn unresolved_plan(resolved: Resolved, question: &str, scope: &PlannedScope) -> StructuredPlan {
    match resolved {
        Resolved::Concept(_) => StructuredPlan::retrieval(),
        Resolved::Ambiguous(options) => ambiguous_field(options, question, scope),
        Resolved::Absent(name) => StructuredPlan::without_evidence(format!(
            "No encontré valores de «{name}» con evidencia en ese alcance, así que no puedo calcular nada sobre ellos."
        )),
        Resolved::Unknown(name) => StructuredPlan::without_evidence(format!(
            "No encontré evidencia local suficiente: el acervo indexado no tiene un campo «{name}»."
        )),
        Resolved::NotNumeric(name) => StructuredPlan::without_evidence(format!(
            "«{name}» no es un campo numérico en el acervo, así que no puedo calcular con él."
        )),
        Resolved::Missing => StructuredPlan::retrieval(),
    }
}

/// Aclaración sobre qué campo de fecha usar, recordando el conjunto.
fn date_field_clarification(
    tools: &ToolEngine,
    question: &str,
    scope: &PlannedScope,
) -> Result<StructuredPlan> {
    let options = tools
        .list_concepts(None)?
        .into_iter()
        .filter(|concept| concept.value_type == "date")
        .map(|concept| concept.display_name)
        .take(8)
        .collect::<Vec<_>>();
    if options.is_empty() {
        return Ok(StructuredPlan::without_evidence(
            "No encontré evidencia local suficiente: el acervo no tiene ningún campo de fecha indexado.",
        ));
    }
    Ok(StructuredPlan::clarify(
        "campo_fecha_ambiguo",
        "El acervo tiene más de un campo de fecha y la pregunta no dice cuál usar para el periodo. ¿Cuál de estos?",
        options.clone(),
    )
    .with_pending(PendingChoice {
        question: question.to_owned(),
        set: scope.as_document_set(),
        options,
        kind: PendingKind::DateField,
    }))
}

fn ambiguous_field(options: Vec<String>, question: &str, scope: &PlannedScope) -> StructuredPlan {
    StructuredPlan::clarify(
        "campo_ambiguo",
        "Ese alcance tiene más de un campo numérico y la pregunta no dice cuál usar. ¿Cuál de estos?",
        options.clone(),
    )
    .with_pending(PendingChoice {
        question: question.to_owned(),
        set: scope.as_document_set(),
        options,
        kind: PendingKind::Concept,
    })
}

fn requested_operation(marks: &Signals) -> Option<Operation> {
    if marks.average {
        return Some(Operation::Average);
    }
    if marks.sum {
        return Some(Operation::Sum);
    }
    // Un superlativo sólo es máximo o mínimo cuando no acompaña a una
    // agrupación; el ranking lo decide después quien conoce los campos.
    if marks.maximum && !marks.minimum {
        return Some(Operation::Maximum);
    }
    if marks.minimum && !marks.maximum {
        return Some(Operation::Minimum);
    }
    if marks.count {
        return Some(Operation::Count);
    }
    None
}

/// Conceptos que la pregunta nombra por completo, en el orden en que
/// aparecen. El primero actúa como clave y el segundo como campo comparado.
/// ¿La pregunta nombra dos campos numéricos distintos y trae una señal
/// léxica de qué operación aplicar entre ellos?
///
/// Reutiliza `named_concepts_in_order`, que ya sabe encontrar hasta dos
/// conceptos nombrados en el orden en que aparecen; aquí sólo se exige que
/// ambos sean numéricos (money/number/percentage) y que exista un verbo de
/// operación. Sin ese verbo no hay operación entre campos: dos campos
/// numéricos mencionados en la misma pregunta sin más («¿cuáles son
/// Cantidad y Precio unitario?») siguen resolviéndose como antes.
fn numeric_field_pair(
    tools: &ToolEngine,
    named: &[String],
    marks: &Signals,
) -> Result<Option<(RowOperation, String, String)>> {
    let operation = if marks.multiply {
        RowOperation::Multiply
    } else if marks.divide {
        RowOperation::Divide
    } else if marks.difference {
        RowOperation::Subtract
    } else {
        return Ok(None);
    };
    if named.len() != 2 {
        return Ok(None);
    }
    let is_numeric = |value_type: &str| matches!(value_type, "money" | "number" | "percentage");
    let left = tools.concept_by_name(&named[0])?;
    let right = tools.concept_by_name(&named[1])?;
    Ok(match (left, right) {
        (Some(left), Some(right))
            if is_numeric(&left.value_type)
                && is_numeric(&right.value_type)
                && left.display_name != right.display_name =>
        {
            Some((operation, left.display_name, right.display_name))
        }
        _ => None,
    })
}

/// ¿La pregunta pide explícitamente operar entre los totales ya agregados de
/// cada campo, en vez de documento por documento?
///
/// Deliberadamente estrecho: la ausencia de esta señal es justo lo que hace
/// seguro el comportamiento por defecto (calcular por documento y sumar
/// después), así que sólo dispara con una mención repetida a «total(es)» —
/// nunca con la mera falta de más contexto, que pediría aclaraciones de más.
fn wants_totals_directly(question: &str) -> bool {
    normalize_exact(question)
        .split_whitespace()
        .filter(|word| matches!(*word, "total" | "totales"))
        .count()
        >= 2
}

fn named_concepts_in_order(tools: &ToolEngine, question: &str) -> Result<Vec<String>> {
    let asked = normalize_exact(question);
    let terms = search_terms(question);
    let mut named = tools
        .list_concepts(None)?
        .into_iter()
        .filter(|concept| {
            let field_terms = search_terms(&concept.display_name);
            !field_terms.is_empty()
                && field_terms.iter().all(|field_term| {
                    terms.iter().any(|term| stems_match(term, field_term))
                })
        })
        .map(|concept| {
            let head = search_terms(&concept.display_name)
                .first()
                .cloned()
                .unwrap_or_default();
            let position = asked
                .split_whitespace()
                .position(|word| stems_match(&normalize_spanish(word), &head))
                .unwrap_or(usize::MAX);
            (position, concept.display_name)
        })
        .collect::<Vec<_>>();
    named.sort();
    let mut seen = BTreeMap::new();
    Ok(named
        .into_iter()
        .filter(|(_, name)| seen.insert(canonical_key(name), ()).is_none())
        .map(|(_, name)| name)
        .take(2)
        .collect())
}

/// ¿La pregunta nombra algún concepto real del índice, o al menos intenta
/// nombrar uno explícitamente con «campo X» / «columna X»?
///
/// La segunda parte importa aunque X no exista: sin ella, «suma el campo
/// Kilometraje» sobre un acervo que no tiene ese campo caía en el mensaje
/// genérico de «esta conversación no tiene contexto» en vez de decir, con
/// precisión, que el campo no existe.
fn question_names_a_concept(tools: &ToolEngine, question: &str) -> Result<bool> {
    let concepts = tools.list_concepts(None)?;
    Ok(resolve_named_concept(&concepts, &search_terms(question), None).is_some()
        || field_after_keyword(question).is_some())
}

fn resolve_scope(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    clock: &Clock,
    reference: Reference,
    marks: &Signals,
    wants_operation: bool,
    forced: &Forced,
) -> Result<PlannedScope> {
    let mut scope = PlannedScope::default();
    let origin = tools.match_origin(question)?;
    let own_filters = tools.resolved_filters(question, origin.as_deref(), true)?;
    // Procedencia: sólo lo que la pregunta ACTUAL escribió con sintaxis
    // «Campo: valor» cuenta como explícito. Un filtro inferido de texto
    // libre o heredado del turno anterior puede ceder ante una heurística;
    // uno que el usuario tecleó a propósito, nunca.
    let written_now = tools.written_filters(question)?;
    for filter in &written_now.filters {
        scope
            .explicit_filters
            .insert((canonical_key(&filter.concept), normalize_exact(&filter.equals)));
    }
    let own_period = (marks.calendar || dates::asks_for_previous_period(question))
        && dates::range_in_question(question, clock, None).is_some();

    // Continuación elíptica: la pregunta pide una operación y no acota nada por
    // su cuenta —ni filtro, ni carpeta, ni periodo—, así que sólo puede
    // referirse al conjunto que la conversación ya tiene delante. Si la
    // pregunta trae cualquier alcance propio, manda el suyo y no se hereda
    // nada: el contexto nunca añade un filtro que el usuario no pidió.
    let elliptical = reference == Reference::None
        && wants_operation
        && own_filters.is_empty()
        && origin.is_none()
        && !own_period
        && state.set.is_some();
    let inherit = reference == Reference::Explicit || elliptical;
    if inherit {
        if let Some(previous) = &state.set {
            scope.filters = previous.filters.clone();
            scope.origin = previous.origin.clone();
            scope.identifier = previous.identifier.clone();
            scope.date = previous.date.clone();
            scope.range = previous.range.clone();
            scope.inherited = true;
        }
        scope.concept = state.concept.clone();
        scope.currency = state.currency.clone();
    }

    // Filtros propios de la pregunta. Se suman al alcance heredado: «de esos,
    // ¿cuáles son de X?» es una intersección, no un reemplazo.
    for filter in own_filters {
        if !scope.filters.iter().any(|existing| {
            canonical_key(&existing.concept) == canonical_key(&filter.concept)
                && normalize_exact(&existing.equals) == normalize_exact(&filter.equals)
        }) {
            scope.filters.push(filter);
        }
    }
    if scope.origin.is_none() {
        scope.origin = origin;
    }

    // Cuando la pregunta compara con el periodo anterior y el contexto ya trae
    // un periodo, ese periodo heredado es el «actual» de la comparación: volver
    // a desplazarlo aquí retrocedería dos veces.
    let comparing_periods = dates::asks_for_previous_period(question)
        && (marks.compare || marks.difference || marks.percent || reference == Reference::Explicit);
    let keep_inherited_period = comparing_periods && scope.range.is_some();

    // Periodo: sólo si la pregunta trae una señal de calendario explícita, para
    // que un número suelto nunca se interprete como un año.
    if !keep_inherited_period && (marks.calendar || dates::asks_for_previous_period(question)) {
        if let Some(range) = dates::range_in_question(question, clock, scope.range.as_ref()) {
            let anchor = match &forced.date_concept {
                Some(concept) => Some(concept.clone()),
                None => date_concept(tools, question, scope.date.as_ref())?,
            };
            // El rango se conserva aunque falte el campo al que anclarlo: el
            // plan lo detecta y pregunta, en lugar de calcular sobre todo el
            // acervo como si no hubiera periodo.
            if let Some(concept) = anchor {
                scope.date = Some(DateConstraint {
                    concept,
                    from: range.from.to_iso(),
                    to: range.to.to_iso(),
                });
            }
            scope.range = Some(range);
        }
    }
    Ok(scope)
}

/// Campo de fecha al que anclar un rango: el nombrado en la pregunta, el que ya
/// usaba el contexto, o el único que exista. Con varios candidatos no se elige
/// por frecuencia: el motor pregunta más adelante.
fn date_concept(
    tools: &ToolEngine,
    question: &str,
    inherited: Option<&DateConstraint>,
) -> Result<Option<String>> {
    let concepts = tools.list_concepts(None)?;
    let dated = concepts
        .into_iter()
        .filter(|concept| concept.value_type == "date")
        .collect::<Vec<_>>();
    if dated.is_empty() {
        return Ok(None);
    }
    let terms = search_terms(question);
    if let Some(named) = resolve_named_concept(&dated, &terms, None) {
        return Ok(Some(named.display_name));
    }
    if let Some(previous) = inherited {
        return Ok(Some(previous.concept.clone()));
    }
    Ok((dated.len() == 1).then(|| dated[0].display_name.clone()))
}

/// Resuelve el conjunto del alcance contra el índice actual.
///
/// Se reevalúa en cada turno a propósito: reindexar reasigna los identificadores
/// internos de fila, así que un conjunto guardado como lista de ids apuntaría
/// después a documentos distintos. El predicado, en cambio, sigue al índice.
fn resolve_documents(tools: &ToolEngine, scope: &mut PlannedScope) -> Result<Vec<i64>> {
    let matched =
        tools.documents_matching(&scope.filters, scope.origin.as_deref(), scope.date.as_ref())?;
    let mut documents = matched
        .iter()
        .map(|document| (document.id, document.path.clone()))
        .collect::<Vec<_>>();
    if let Some(identifier) = &scope.identifier {
        let linked = relations::documents_for(tools, identifier)?
            .map(|group| group.document_ids())
            .unwrap_or_default();
        documents.retain(|(id, _)| linked.contains(id));
    }
    scope.documents = documents.iter().map(|(id, _)| *id).collect();
    scope.paths = documents
        .iter()
        .take(50)
        .map(|(_, path)| path.clone())
        .collect();
    Ok(scope.documents.clone())
}

enum Resolved {
    Concept(String),
    Ambiguous(Vec<String>),
    /// El campo existe en el acervo, pero no en el alcance consultado.
    Absent(String),
    /// El usuario nombró un campo que el acervo no tiene.
    Unknown(String),
    /// El campo existe pero no es numérico.
    NotNumeric(String),
    Missing,
}

/// Campo sobre el que opera el cálculo.
///
/// Orden deliberado y no negociable: lo que una aclaración ya fijó, lo que la
/// pregunta **nombra**, y sólo si el usuario no nombró nada, lo que la
/// conversación estaba calculando o el único campo del tipo adecuado que exista
/// en el alcance.
///
/// El campo nombrado se busca en todo el acervo, no sólo en el alcance: si el
/// usuario escribe un campo que existe pero no está en estos documentos, la
/// respuesta es que ahí no hay valores suyos — nunca el campo del turno
/// anterior. El contexto rellena lo que el usuario omitió; jamás sustituye lo
/// que escribió.
fn resolve_computation_concept(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    documents: &[i64],
    operation: Operation,
    require_named: bool,
    forced: &Forced,
    // `exclude` es el campo que la pregunta nombra como agrupador o dimensión:
    // no puede ser a la vez el campo que se calcula.
    exclude: Option<&str>,
) -> Result<Resolved> {
    let in_scope = tools.concepts_in_documents(documents)?;
    let usable = |value_type: &str| {
        if operation.needs_numbers() {
            matches!(value_type, "money" | "number" | "percentage")
        } else {
            true
        }
    };
    let present = |concept: &ConceptSummary| in_scope.iter().any(|item| item.key == concept.key);

    if let Some(name) = &forced.concept {
        return Ok(match tools.concept_by_name(name)? {
            Some(concept) if present(&concept) => Resolved::Concept(concept.display_name),
            Some(concept) => Resolved::Absent(concept.display_name),
            None => Resolved::Unknown(name.clone()),
        });
    }

    let terms = search_terms(question);
    let catalogue = tools.list_concepts(None)?;
    let usable_catalogue = catalogue
        .iter()
        .filter(|concept| usable(&concept.value_type))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(concept) = resolve_named_concept(&usable_catalogue, &terms, exclude) {
        return Ok(if present(&concept) {
            Resolved::Concept(concept.display_name)
        } else {
            Resolved::Absent(concept.display_name)
        });
    }
    if operation.needs_numbers() {
        if let Some(concept) = resolve_named_concept(&catalogue, &terms, exclude) {
            return Ok(Resolved::NotNumeric(concept.display_name));
        }
    }
    // Un campo nombrado con todas las letras («suma el campo X») que no existe
    // en el acervo no puede resolverse con otro campo.
    if let Some(written) = field_after_keyword(question) {
        if resolve_named_concept(&catalogue, &search_terms(&written), None).is_none() {
            return Ok(Resolved::Unknown(written));
        }
    }

    if let Some(previous) = &state.concept {
        if let Some(concept) = tools.concept_by_name(previous)? {
            if usable(&concept.value_type) && present(&concept) {
                return Ok(Resolved::Concept(concept.display_name));
            }
        }
    }
    let candidates = in_scope
        .iter()
        .filter(|concept| usable(&concept.value_type))
        .cloned()
        .collect::<Vec<_>>();
    // Sin contexto que herede, una operación que no nombra su campo no puede
    // elegirlo por descarte: la pregunta vuelve al motor de recuperación en vez
    // de convertirse en un cálculo que nadie pidió.
    if require_named {
        return Ok(Resolved::Missing);
    }
    match candidates.len() {
        0 => Ok(Resolved::Missing),
        1 => Ok(Resolved::Concept(candidates[0].display_name.clone())),
        _ => Ok(Resolved::Ambiguous(
            candidates
                .iter()
                .take(6)
                .map(|concept| concept.display_name.clone())
                .collect(),
        )),
    }
}

/// Texto que sigue a «campo» o «columna»: lo que el usuario declaró como
/// nombre de campo, aunque no exista.
fn field_after_keyword(question: &str) -> Option<String> {
    // Se recorre el texto original para conservar mayúsculas y acentos: el
    // nombre que el usuario escribió es lo que la respuesta debe citar.
    let words = question.split_whitespace().collect::<Vec<_>>();
    let position = words
        .iter()
        .position(|word| matches!(normalize_exact(word).as_str(), "campo" | "columna"))?;
    let tail = words
        .get(position + 1..)
        .map(|rest| rest.join(" "))
        .unwrap_or_default();
    let tail = tail
        .trim()
        .trim_end_matches(['.', '?', '!', ',', ';'])
        .trim()
        .to_owned();
    (!tail.is_empty()).then_some(tail)
}

/// Dos grupos del mismo campo nombrados literalmente en la pregunta.
fn comparison_targets(
    tools: &ToolEngine,
    question: &str,
    scope: &PlannedScope,
) -> Result<Option<(String, String, String)>> {
    let mentioned = tools.values_mentioned(question, scope.origin.as_deref())?;
    let mut by_concept: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (concept, value) in mentioned {
        let values = by_concept.entry(concept).or_default();
        if !values.iter().any(|existing| existing == &value) {
            values.push(value);
        }
    }
    // Un valor contenido en otro más largo del mismo campo no es un grupo
    // distinto: «Guadalajara» dentro de «Guadalajara Centro» sería el mismo
    // documento contado dos veces.
    for values in by_concept.values_mut() {
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut kept: Vec<String> = Vec::new();
        for value in values.iter() {
            if !kept
                .iter()
                .any(|longer| normalize_exact(longer).contains(&normalize_exact(value)))
            {
                kept.push(value.clone());
            }
        }
        *values = kept;
    }
    let mut pairs = by_concept
        .into_iter()
        .filter(|(_, values)| values.len() >= 2)
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        return Ok(None);
    }
    // Un mismo valor puede vivir en varios campos («Veracruz» como ciudad base
    // y como centro de trabajo). Gana el campo que aporta exactamente dos
    // grupos y, entre ellos, el que cubre más documentos del acervo: agrupar
    // por el campo más marginal dejaría fuera la mayor parte de la evidencia.
    let coverage = tools
        .list_concepts(None)?
        .into_iter()
        .map(|concept| (concept.display_name, concept.occurrences))
        .collect::<BTreeMap<_, _>>();
    pairs.sort_by_key(|(concept, values)| {
        (
            values.len() != 2,
            std::cmp::Reverse(coverage.get(concept).copied().unwrap_or(0)),
            concept.clone(),
        )
    });
    let (concept, mut values) = pairs.remove(0);
    values.sort_by_key(|value| ToolEngine::mention_position(question, value));
    Ok(Some((concept, values[0].clone(), values[1].clone())))
}

/// Campo por el que agrupar un ranking. Nunca es numérico: agrupar por el mismo
/// campo que se suma no responde «cuál … más».
enum Grouping {
    One(String),
    Ambiguous(Vec<String>),
    None,
}

impl Grouping {
    fn is_named(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Resuelve el campo agrupador nombrado en la pregunta.
///
/// Acepta el nombre completo («Cliente») y también un nombre parcial cuando es
/// inequívoco: «qué ciudad» debe poder agrupar por «Ciudad base» sin que el
/// usuario tenga que escribir el nombre exacto del campo. Si más de un campo
/// comparte esa palabra, se pregunta en vez de elegir.
fn grouping_concept(
    tools: &ToolEngine,
    question: &str,
    documents: &[i64],
    exclude: Option<&str>,
) -> Result<Grouping> {
    let pool = if documents.is_empty() {
        tools.list_concepts(None)?
    } else {
        tools.concepts_in_documents(documents)?
    };
    let candidates = pool
        .into_iter()
        .filter(|concept| matches!(concept.value_type.as_str(), "text" | "state"))
        .filter(|concept| exclude != Some(concept.display_name.as_str()))
        .collect::<Vec<_>>();
    let terms = search_terms(question);
    if let Some(concept) = resolve_named_concept(&candidates, &terms, None) {
        return Ok(Grouping::One(concept.display_name));
    }
    // Coincidencia parcial: una palabra distintiva de la pregunta que aparece
    // en el nombre del campo.
    let partial = candidates
        .iter()
        .filter(|concept| {
            search_terms(&concept.display_name).iter().any(|field_term| {
                field_term.len() >= 4
                    && terms
                        .iter()
                        .any(|term| term.len() >= 4 && stems_match(term, field_term))
            })
        })
        .collect::<Vec<_>>();
    Ok(match partial.len() {
        0 => Grouping::None,
        1 => Grouping::One(partial[0].display_name.clone()),
        _ => Grouping::Ambiguous(
            partial
                .iter()
                .take(6)
                .map(|concept| concept.display_name.clone())
                .collect(),
        ),
    })
}
