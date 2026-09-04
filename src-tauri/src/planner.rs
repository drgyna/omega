//! Planificador local de consultas. Decide una ruta pequeña y auditable a
//! partir de señales lingüísticas genéricas y del esquema descubierto en el
//! índice. No contiene nombres de carpetas, campos, ciudades ni estados de un
//! corpus concreto.

use crate::{
    census,
    error::Result,
    model::{AggregateRequest, ConceptSummary, ToolFilter},
    normalize::{
        canonical_key, normalize_exact, normalize_spanish, search_terms, stems_match,
    },
    tools::{FormatRequest, ToolEngine, ValueQuery},
};

#[derive(Clone, Debug)]
pub enum QueryIntent {
    Inventory,
    Exact,
    Aggregate(AggregateRequest),
    CountDocuments,
    /// Conteo real de documentos por formato de archivo, con la declaración de
    /// cuántos archivos del alcance no se pudieron indexar.
    CountByFormat(FormatRequest),
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
    // «Total» no es un conteo. Cuenta documentos sólo cuando la pregunta no
    // dice de qué es el total («¿cuántos documentos hay en total?»); en cuanto
    // nombra una categoría de valor —«el total de los importes»— es una suma, y
    // contestarla con un número de documentos era responder otra pregunta.
    let totals_a_value = has("total") && generic_value_category(question).is_some();
    let asks_count = (has("cuant") || has("numer") || has("conte") || has("how") || has("total"))
        && !totals_a_value;
    let asks_sum = has("sum") || has("totaliz") || has("add") || totals_a_value;
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
    crate::trace!(
        "c) planner::plan senales lexicas: asks_count={asks_count} (cuant/numer/conte/how/total), asks_sum={asks_sum}, asks_group={asks_group}, asks_list={asks_list}, mentions_documents={mentions_documents}, mentions_index={mentions_index}, totals_a_value={totals_a_value}"
    );
    crate::trace!("c) planner::plan terminos = {terms:?}");

    if asks_count && mentions_documents && mentions_index {
        crate::trace!("c) planner::plan SALE por rama: Inventory (asks_count+documents+index)");
        return Ok(QueryPlan {
            intent: QueryIntent::Inventory,
            filters: vec![],
            origin: None,
        });
    }
    // Conteo por formato de archivo. Va **antes** del corte por señal exacta
    // porque estas preguntas entrecomillan el área («¿Cuántos documentos del
    // área "…" están en formato DOCX?») y ese corte las mandaba a la ruta de
    // búsqueda literal con tope de muestra: la respuesta decía «20 valores»,
    // que era el tope, no un conteo. El desvío es deliberadamente estrecho —
    // sólo una pregunta que pide un conteo Y nombra un formato existente en el
    // índice— así que ninguna otra pregunta entrecomillada cambia de ruta.
    if asks_count {
        if let Some(request) = format_request(question, &tools.available_extensions()?) {
            let origin = tools.match_origin(question)?;
            let filters = tools.resolved_filters(question, origin.as_deref(), true)?;
            return Ok(QueryPlan {
                intent: QueryIntent::CountByFormat(request),
                filters,
                origin,
            });
        }
    }
    if ToolEngine::query_has_exact_signal(question) {
        crate::trace!("c) planner::plan SALE por rama: Exact (senal de literal entrecomillado)");
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
        crate::trace!("c) planner::plan SALE por rama: CountDocuments (asks_count Y hay filtros/carpeta)");
        return Ok(QueryPlan {
            intent: QueryIntent::CountDocuments,
            filters,
            origin,
        });
    }
    if asks_list && !filters.is_empty() {
        crate::trace!("c) planner::plan SALE por rama: ListDocuments");
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
    crate::trace!("c) planner::plan target(concepto nombrado)={:?} question_word={question_word} filtros={}", target.as_ref().map(|c| c.display_name.clone()), filters.len());
    if target.is_some() && (!filters.is_empty() || !question_word) {
        crate::trace!("c) planner::plan SALE por rama: LegacySearch (concepto nombrado)");
        return Ok(QueryPlan {
            intent: QueryIntent::LegacySearch,
            filters: vec![],
            origin,
        });
    }
    if asks_count || (asks_list && origin.is_some()) || question_word {
        crate::trace!("c) planner::plan SALE por rama: FreeText");
        return Ok(QueryPlan {
            intent: QueryIntent::FreeText,
            filters: vec![],
            origin,
        });
    }
    if asks_list {
        crate::trace!("c) planner::plan SALE por rama: BoundedSearch");
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

/// La pregunta sin sus identificadores.
///
/// Un identificador de negocio mezcla letras y dígitos en la misma palabra
/// (`OC-2024-00001`, `EMP-2019-0506`); un nombre de campo no. Quitarlos deja
/// sólo las palabras con las que el usuario pudo nombrar un campo.
fn question_without_identifiers(question: &str) -> String {
    question
        .split_whitespace()
        .filter(|word| {
            !(word.chars().any(char::is_alphabetic) && word.chars().any(char::is_numeric))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// ¿Pregunta si dos valores coinciden?
///
/// Palabras completas, como el resto de las listas de este módulo: «coincide»
/// es la pregunta, «coincidencia» dentro de otra frase no tiene por qué serlo.
fn asks_whether_values_coincide(question: &str) -> bool {
    const WORDS: &[&str] = &[
        "coinciden",
        "coincide",
        "iguales",
        "igual",
        "difieren",
        "coinciden",
        "concuerdan",
        "concuerda",
    ];
    let words = normalize_exact(question)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    WORDS.iter().any(|word| words.iter().any(|item| item == word))
}

/// Comparación de un campo nombrado entre dos documentos señalados por su
/// clave de localización.
///
/// Las tres condiciones son verificables y ninguna se infiere: la pregunta
/// pregunta si algo coincide, señala exactamente **dos** documentos, y
/// entrecomilla un nombre de campo que el acervo tiene. Con un documento o con
/// tres no actúa; sin el campo entrecomillado tampoco, porque entonces no hay
/// nada concreto que comparar y elegir un campo sería adivinar.
fn field_comparison_between_documents(
    tools: &ToolEngine,
    question: &str,
) -> Result<Option<(String, Vec<i64>, Vec<String>)>> {
    if !asks_whether_values_coincide(question) {
        return Ok(None);
    }
    let located = tools.locate_documents_by_key(question)?;
    if located.len() != 2 {
        return Ok(None);
    }
    let concepts = tools.list_concepts(None)?;
    let quoted = ToolEngine::quoted_literals(question);
    let Some(field) = quoted.iter().find_map(|literal| {
        concepts
            .iter()
            .find(|concept| canonical_key(&concept.display_name) == canonical_key(literal))
            .map(|concept| concept.display_name.clone())
    }) else {
        return Ok(None);
    };
    Ok(Some((
        field,
        located.iter().map(|document| document.id).collect(),
        located
            .iter()
            .map(|document| document.path.clone())
            .collect(),
    )))
}

/// Cómo nombra la pregunta uno de los dos operandos de una operación entre
/// campos.
///
/// `Field` es el caso normal: el usuario escribió el nombre del campo.
/// `Category` es el caso que faltaba: «el importe» no es el nombre de ningún
/// campo del acervo —los campos se llaman «Importe del pedido», «Costo
/// estimado de no conformidad», «Importe facturado»— sino la palabra genérica
/// con la que se habla de una cantidad de dinero. Sobre un conjunto eso sería
/// ambiguo; sobre un documento que registra **un solo** valor monetario, cuál
/// es «el importe» lo decide el documento y no el motor. Es el mismo criterio
/// que ya usa `ComputeCategory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowOperandSpec {
    Field(String),
    Category(&'static str),
}

/// Palabras con las que el español nombra una cantidad de dinero sin nombrar
/// ningún campo concreto. Como el resto de las listas de este módulo
/// (`CONTAINER_WORDS`, `CALENDAR_WORDS`), son vocabulario del idioma, no de un
/// acervo: ninguna es el nombre de un campo de un corpus concreto.
const GENERIC_MONEY_WORDS: &[&str] = &["importe", "importes", "monto", "montos"];

/// Ídem para una cuenta de unidades.
const GENERIC_QUANTITY_WORDS: &[&str] = &["cantidad", "cantidades", "unidades"];

/// La categoría de valor que una pregunta nombra de forma genérica, sin
/// nombrar ningún campo concreto: «el total de los importes», «la suma de las
/// cantidades».
///
/// Existe porque un acervo real no guarda «Importe» a secas: guarda «Importe
/// facturado», «Importe pagado», «Importe de la orden»… Exigir que la pregunta
/// nombre uno de ellos por su nombre completo dejaba sin respuesta la pregunta
/// más común de todas —cuánto suma el dinero de este conjunto— y la resolución
/// caía en un conteo de documentos, que no es lo que se preguntó.
///
/// El vocabulario es del idioma, no de ningún acervo: las mismas palabras
/// valen para cualquier colección de documentos.
/// ¿Pide la pregunta un total o un acumulado?
///
/// Sólo la señal léxica: quién la usa decide si eso basta para llamarlo suma.
/// Por sí sola no lo es —«¿cuántos documentos hay en total?» cuenta— y por eso
/// nunca se consulta sin acompañarla de qué se totaliza.
fn question_mentions_a_total(question: &str) -> bool {
    let terms = search_terms(question);
    terms
        .iter()
        .any(|term| term.starts_with("total") || term.starts_with("acumulad"))
}

/// La categoría de valor que la pregunta pide, incluido el caso en que la
/// nombra con una moneda («el total en MXN») en vez de con la palabra
/// «importe». La lista de monedas sale del índice, no del código, así que
/// funciona con cualquier acervo y no conoce ninguna en particular.
fn requested_value_category(
    tools: &ToolEngine,
    question: &str,
) -> Result<Option<&'static str>> {
    if let Some(category) = generic_value_category(question) {
        return Ok(Some(category));
    }
    if question_mentions_a_total(question)
        && explicit_currency(question, &tools.available_currencies()?).is_some()
    {
        return Ok(Some("money"));
    }
    Ok(None)
}

fn generic_value_category(question: &str) -> Option<&'static str> {
    let terms = search_terms(question);
    let mentions = |list: &[&str]| {
        list.iter().any(|word| {
            search_terms(word)
                .first()
                .is_some_and(|root| terms.iter().any(|term| stems_match(term, root)))
        })
    };
    if mentions(GENERIC_MONEY_WORDS) {
        return Some("money");
    }
    if mentions(GENERIC_QUANTITY_WORDS) {
        return Some("number");
    }
    None
}

/// Los dos operandos tal y como la pregunta los escribe, a los lados del
/// conector de la operación («dividir X entre Y»).
fn row_operand_phrases(question: &str, operation: RowOperation) -> Option<(String, String)> {
    let words = question.split_whitespace().collect::<Vec<_>>();
    let normalized = words
        .iter()
        .map(|word| normalize_exact(word))
        .collect::<Vec<_>>();
    let verb_at = normalized.iter().position(|word| match operation {
        RowOperation::Divide => word.starts_with("dividi") || word.starts_with("division"),
        RowOperation::Multiply => word.starts_with("multiplic"),
        RowOperation::Subtract => word.starts_with("rest") || word.starts_with("diferenci"),
    })?;
    let connectors: &[&str] = match operation {
        RowOperation::Divide => &["entre"],
        RowOperation::Multiply => &["por"],
        RowOperation::Subtract => &["menos", "y"],
    };
    let split_at = normalized
        .iter()
        .enumerate()
        .skip(verb_at + 1)
        .find(|(_, word)| connectors.contains(&word.as_str()))
        .map(|(index, _)| index)?;
    let clean = |slice: &[&str]| {
        slice
            .join(" ")
            .trim_matches(['?', '¿', '.', ',', ';', ':', ' '])
            .to_owned()
    };
    let left = clean(&words[verb_at + 1..split_at]);
    let right = clean(&words[split_at + 1..]);
    (!left.is_empty() && !right.is_empty()).then_some((left, right))
}

/// Qué designa un operando: un campo del acervo, o una categoría de valor
/// nombrada de forma genérica. El campo manda: si el usuario escribió el
/// nombre de un campo real, es ése y no una lectura genérica de la palabra.
fn row_operand_spec(
    concepts: &[ConceptSummary],
    phrase: &str,
) -> Option<RowOperandSpec> {
    let terms = search_terms(phrase);
    if terms.is_empty() {
        return None;
    }
    if let Some(concept) = resolve_named_concept(concepts, &terms, None) {
        return Some(RowOperandSpec::Field(concept.display_name));
    }
    let mentions = |list: &[&str]| {
        list.iter().any(|word| {
            let root = search_terms(word);
            root.first()
                .is_some_and(|root| terms.iter().any(|term| stems_match(term, root)))
        })
    };
    if mentions(GENERIC_MONEY_WORDS) {
        return Some(RowOperandSpec::Category("money"));
    }
    if mentions(GENERIC_QUANTITY_WORDS) {
        return Some(RowOperandSpec::Category("number"));
    }
    None
}

/// Operación entre dos campos de un documento concreto.
///
/// Tres condiciones, todas comprobables y ninguna heurística: la pregunta trae
/// el verbo de la operación, señala **un** documento por su clave de
/// localización, y nombra los dos operandos a los lados del conector. Con
/// varios documentos localizados no actúa: el resultado por documento no se
/// puede presentar como uno solo.
fn document_row_operation(
    tools: &ToolEngine,
    question: &str,
    marks: &Signals,
) -> Result<Option<(RowOperation, RowOperandSpec, RowOperandSpec, i64, String)>> {
    let operation = if marks.divide {
        RowOperation::Divide
    } else if marks.multiply {
        RowOperation::Multiply
    } else {
        return Ok(None);
    };
    let Some((left, right)) = row_operand_phrases(question, operation) else {
        return Ok(None);
    };
    let located = tools.locate_documents_by_key(question)?;
    let [document] = located.as_slice() else {
        return Ok(None);
    };
    let concepts = tools.list_concepts(None)?;
    let (Some(left), Some(right)) = (
        row_operand_spec(&concepts, &left),
        row_operand_spec(&concepts, &right),
    ) else {
        return Ok(None);
    };
    if left == right {
        return Ok(None);
    }
    Ok(Some((
        operation,
        left,
        right,
        document.id,
        document.path.clone(),
    )))
}

/// Petición de censo del acervo: cuántos ARCHIVOS hay, no cuántos valores se
/// extrajeron. Ver `census` para por qué la distinción decide si la cifra
/// puede ser completa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CensusRequest {
    pub filter: census::CensusFilter,
    /// Valor de campo con el que la pregunta nombró la carpeta, junto con
    /// cuántos documentos indexados lo registran. La respuesta necesita los
    /// dos números por separado: los archivos que hay en la carpeta y los
    /// documentos que escriben ese valor no son la misma cifra, y presentarlos
    /// como si lo fueran sería afirmar algo que el índice no dice.
    pub origin_from_value: Option<(String, i64)>,
    /// Reparte el conteo por tipo de documento en vez de dar un solo total.
    pub group_by_kind: bool,
    /// La pregunta escribió un filtro que nadie en el motor sabe aplicar: ni el
    /// censo, ni un campo del acervo, ni ninguna otra ruta. Cuando está, no se
    /// cuenta nada; se dice.
    pub unknown_filter: Option<String>,
}

/// Claves de filtro escritas «clave=valor» que el censo sabe leer.
///
/// Devuelve `None` en cuanto la pregunta usa una clave que no está en esta
/// lista. Es deliberado: la alternativa —quedarse con las que se entienden y
/// seguir— es exactamente el defecto que esta ronda encontró, una respuesta
/// que decía «cumplen simultáneamente los criterios» después de haber tirado
/// en silencio el filtro que no supo leer.
fn census_filter_pairs(question: &str) -> Option<Vec<(String, String)>> {
    let pattern = regex::Regex::new(r"(?u)([A-Za-z_áéíóúñ]+)\s*=\s*([^,;?.]+)")
        .expect("valid filter regex");
    let mut pairs = Vec::new();
    for capture in pattern.captures_iter(question) {
        let key = normalize_exact(&capture[1]);
        let value = capture[2].trim().trim_matches(['"', '«', '»', '\'']).to_owned();
        if key.is_empty() || value.is_empty() {
            return None;
        }
        pairs.push((key, value));
    }
    (!pairs.is_empty()).then_some(pairs)
}

/// Lo que la pregunta escribe después de «área» o «carpeta»: el texto con el
/// que nombró el ámbito.
///
/// Se recorre la pregunta palabra por palabra sobre la cadena original —no
/// sobre una versión normalizada— porque normalizar cambia la longitud en
/// bytes (una vocal acentuada ocupa dos y su equivalente sin acento uno), así
/// que un desplazamiento calculado sobre el texto normalizado no señala el
/// mismo punto del original.
///
/// El nombre termina donde empieza algo que ya no forma parte de él: el signo
/// de interrogación o los dos puntos que cierran la frase, o una de las colas
/// («en todo el corpus», «existen») con las que estas preguntas rematan.
fn area_phrase(question: &str) -> Option<String> {
    const MARKERS: [&str; 4] = ["area", "areas", "carpeta", "categoria"];
    const TAILS: [&str; 5] = ["en todo", "en el corpus", "del corpus", "existen", "hay"];
    let words = question
        .split_whitespace()
        .map(|word| (word, normalize_exact(word)))
        .collect::<Vec<_>>();
    let start = words
        .iter()
        .position(|(_, normalized)| MARKERS.contains(&normalized.as_str()))?
        + 1;
    let mut collected: Vec<&str> = Vec::new();
    for (index, (word, normalized)) in words.iter().enumerate().skip(start) {
        // Artículos de enlace, sólo si van pegados al marcador: «del área DE
        // Recursos humanos». Más adentro ya son parte del nombre («Recursos
        // humanos Y capacitación»).
        if collected.is_empty() && matches!(normalized.as_str(), "de" | "del" | "la" | "el") {
            continue;
        }
        // Una cola arranca aquí: se corta antes de tomar la palabra.
        let rest = words[index..]
            .iter()
            .map(|(_, normalized)| normalized.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if TAILS.iter().any(|tail| rest.starts_with(tail)) {
            break;
        }
        let trimmed = word.trim_end_matches(['?', ':', '.', '»', '"']);
        let cut = trimmed.len() < word.len();
        let trimmed = trimmed.trim_matches(['"', '«', '»', '\'']);
        if !trimmed.is_empty() {
            collected.push(trimmed);
        }
        if cut {
            break;
        }
    }
    let phrase = collected.join(" ");
    let phrase = phrase.trim_end_matches(',').trim();
    (!phrase.is_empty()).then(|| phrase.to_owned())
}

/// Tipo de documento que la pregunta nombra («documentos de tipo "factura"»).
fn asked_kind(question: &str) -> Option<String> {
    let pattern = regex::Regex::new(r#"(?ui)\btipo\s+"([^"]+)"|\btipo\s+«([^»]+)»"#)
        .expect("valid kind regex");
    let capture = pattern.captures(question)?;
    let value = capture
        .get(1)
        .or_else(|| capture.get(2))?
        .as_str()
        .trim()
        .to_owned();
    (!value.is_empty()).then_some(value)
}

/// Carpeta a la que se refiere la pregunta cuando cuenta archivos.
///
/// Primero el nombre de la carpeta tal cual (`match_origin`, que ya sabe
/// reconocerlo escrito de varias formas). Si eso no da nada, se prueba con el
/// valor de campo que la pregunta escribe después de «área» — pero sólo si el
/// índice demuestra que ese valor identifica una sola carpeta. La
/// correspondencia no se supone: se comprueba.
fn census_origin(tools: &ToolEngine, question: &str) -> Result<Option<(String, Option<(String, i64)>)>> {
    if let Some(origin) = tools.match_origin(question)? {
        return Ok(Some((origin, None)));
    }
    let Some(phrase) = area_phrase(question) else {
        return Ok(None);
    };
    if normalize_exact(&phrase).split_whitespace().count() < 2 {
        return Ok(None);
    }
    let Some((origin, documents)) = tools.origin_identified_by_value(&phrase)? else {
        return Ok(None);
    };
    Ok(Some((origin.clone(), Some((phrase, documents)))))
}

/// ¿Es esta pregunta un censo del acervo?
///
/// Tres formas, todas exigiendo que la pregunta cuente DOCUMENTOS (no valores
/// de un campo):
///
///  1. Sintaxis explícita de filtro (`area=rh, kind=vacaciones`), y sólo si
///     TODAS las claves se entienden.
///  2. Un tipo de documento nombrado entre comillas, con o sin carpeta.
///  3. Un total: la carpeta o el acervo entero, sin ningún campo de por medio.
///
/// La forma 3 exige que la pregunta no nombre ningún campo del acervo: en
/// cuanto lo hace («¿cuántos documentos registran Moneda = EUR?») deja de ser
/// un conteo de archivos y vuelve a la ruta de contenido, donde la cifra sólo
/// puede hablar de lo que se logró leer.
fn census_request(
    tools: &ToolEngine,
    question: &str,
    marks: &Signals,
) -> Result<Option<CensusRequest>> {
    if !marks.count || !marks.container {
        return Ok(None);
    }
    // Cualquier otra operación en la misma pregunta la saca de aquí: el censo
    // sólo sabe contar archivos, y una pregunta que además suma, compara,
    // ordena o busca contradicciones no es un censo aunque diga «cuántos».
    if marks.sum
        || marks.average
        || marks.superlative
        || marks.difference
        || marks.percent
        || marks.compare
        || marks.contradictions
        || marks.differing
        || marks.multiply
        || marks.divide
        || marks.related
        || marks.evidence
    {
        return Ok(None);
    }
    // Un conteo por formato ya tiene su propia ruta, que además declara los no
    // indexados. El censo no se la quita.
    if format_request(question, &tools.available_extensions()?).is_some() {
        return Ok(None);
    }
    // Las señales de intención se leen FUERA de las comillas. Un texto
    // entrecomillado es lo que la pregunta busca, no cómo lo pide: sin esta
    // separación, «¿Cuántos documentos mencionan "AUSENCIA TOTAL …"?» pasaba
    // por un censo del acervo entero porque la palabra «TOTAL» iba dentro de
    // la cita, y una pregunta cuya respuesta correcta era «ninguno» se
    // contestaba con el tamaño del acervo.
    let intent = ToolEngine::query_without_quoted_literals(question);
    // El inventario del acervo («¿cuántos documentos hay indexados y qué
    // categorías contiene el acervo?») contesta más que un número: el total y
    // el reparto por carpeta, y ya dice «indexados» sin fingir que son todos.
    // El censo, que sólo sabe dar la cifra, le cedería una respuesta mejor por
    // una peor.
    let terms = search_terms(&intent);
    let has = |root: &str| terms.iter().any(|term| term.starts_with(root));
    let mentions_the_index = has("indic") || has("index") || has("acerv") || has("coleccion");
    let mentions_documents =
        has("document") || has("archiv") || has("expedient") || has("registro");
    if mentions_the_index && mentions_documents {
        return Ok(None);
    }
    let normalized = normalize_exact(&intent);
    let group_by_kind = normalized.contains("de cada tipo") || normalized.contains("por tipo");

    if let Some(pairs) = census_filter_pairs(question) {
        let mut filter = census::CensusFilter::default();
        let mut origin_from_value = None;
        let concepts = tools.list_concepts(None)?;
        for (key, value) in pairs {
            match key.as_str() {
                "area" | "carpeta" | "folder" | "origen" | "fuente" => {
                    let named = tools.match_origin(&value)?;
                    match named {
                        Some(origin) => filter.origin = Some(origin),
                        None => match tools.origin_identified_by_value(&value)? {
                            Some((origin, documents)) => {
                                filter.origin = Some(origin);
                                origin_from_value = Some((value, documents));
                            }
                            None => return Ok(None),
                        },
                    }
                }
                "tipo" | "kind" | "clase" => filter.kind = Some(value),
                // Claves que otra ruta del motor sí sabe resolver: el censo se
                // aparta y las deja pasar, tal cual se comportaba antes.
                "doc_id" | "id" | "documento" | "archivo" | "ruta" | "path" => return Ok(None),
                other => {
                    // ¿La clave nombra un campo del acervo? Entonces es un
                    // filtro de contenido y lo resuelve la ruta de siempre.
                    if resolve_named_concept(&concepts, &search_terms(other), None).is_some() {
                        return Ok(None);
                    }
                    // Nadie puede aplicarla. Antes se caía en silencio y la
                    // respuesta seguía diciendo «cumplen simultáneamente los
                    // criterios», en plural, sobre los filtros que sí se
                    // entendieron: una cifra correcta presentada como respuesta
                    // a una pregunta que no era la que se hizo.
                    return Ok(Some(CensusRequest {
                        filter: census::CensusFilter::default(),
                        origin_from_value: None,
                        group_by_kind: false,
                        unknown_filter: Some(other.to_owned()),
                    }));
                }
            }
        }
        return Ok(Some(CensusRequest {
            filter,
            origin_from_value,
            group_by_kind,
            unknown_filter: None,
        }));
    }

    let origin = census_origin(tools, question)?;
    // Si la pregunta nombra un ámbito y el motor no consigue resolverlo a una
    // carpeta, el censo se retira. Contar sin ese recorte daría una cifra del
    // acervo entero presentada como si fuera la del área que se preguntó: el
    // mismo defecto que esta ronda corrigió para «kind=», cometido aquí.
    let names_a_scope = ["area", "areas", "carpeta", "categoria"]
        .iter()
        .any(|marker| normalized.split_whitespace().any(|word| word == *marker));
    if names_a_scope && origin.is_none() {
        return Ok(None);
    }
    // Toda cita entrecomillada tiene que quedar explicada por el propio censo:
    // el tipo que se cuenta, o el nombre del ámbito. Una cita que el censo no
    // consume es un texto que buscar dentro de los documentos, y esa pregunta
    // no es un recuento de archivos.
    let kind = asked_kind(question);
    for literal in ToolEngine::quoted_literals(question) {
        let is_the_kind = kind
            .as_deref()
            .is_some_and(|kind| normalize_exact(kind) == normalize_exact(&literal));
        let names_the_scope = match &origin {
            Some((origin, from_value)) => {
                tools.match_origin(&literal)?.as_deref() == Some(origin.as_str())
                    || from_value.as_ref().is_some_and(|(value, _)| {
                        normalize_exact(value) == normalize_exact(&literal)
                    })
            }
            None => false,
        };
        if !is_the_kind && !names_the_scope {
            return Ok(None);
        }
    }
    if let Some(kind) = kind {
        let (origin, origin_from_value) = match origin {
            Some((origin, from_value)) => (Some(origin), from_value),
            None => (None, None),
        };
        return Ok(Some(CensusRequest {
            filter: census::CensusFilter {
                origin,
                kind: Some(kind),
            },
            origin_from_value,
            group_by_kind,
            unknown_filter: None,
        }));
    }

    // Forma 3. Sin ningún filtro de contenido en pie, «cuántos documentos hay
    // en total» es una pregunta por el acervo como conjunto de archivos.
    //
    // «En pie» hace todo el trabajo: la pregunta «¿cuántos documentos totales
    // pertenecen al área X?» SÍ produce un filtro —el campo «Área» con ese
    // valor— y aun así es un censo, porque ese valor es la forma en que la
    // pregunta nombró la carpeta, no un recorte adicional dentro de ella.
    // Cualquier filtro que no se explique así deja la pregunta donde estaba:
    // un conteo de contenido no puede hablar de lo que no se logró leer.
    let named_origin = origin.as_ref().map(|(origin, _)| origin.clone());
    for filter in tools.resolved_filters(question, named_origin.as_deref(), false)? {
        let names_the_same_folder = match named_origin.as_deref() {
            Some(origin) => tools.value_lives_only_in_origin(origin, &filter.equals)?,
            None => false,
        };
        if !names_the_same_folder {
            return Ok(None);
        }
    }
    // La palabra que convierte «cuántos documentos hay aquí» en «cuántos
    // documentos hay en total» es lo que distingue esta ruta del conteo que ya
    // existía. Sin ella se deja pasar: el conteo clásico sigue respondiendo lo
    // que respondía, y esta ruta no le roba ninguna pregunta ya verificada.
    let whole_archive = ["corpus", "acervo", "total", "totales", "todo", "todos"]
        .iter()
        .any(|word| normalized.split_whitespace().any(|term| term == *word));
    if !whole_archive && !group_by_kind {
        return Ok(None);
    }
    Ok(Some(CensusRequest {
        filter: census::CensusFilter {
            origin: origin.as_ref().map(|(origin, _)| origin.clone()),
            kind: None,
        },
        origin_from_value: origin.and_then(|(_, from_value)| from_value),
        group_by_kind,
        unknown_filter: None,
    }))
}

/// Formato de archivo nombrado en la pregunta.
///
/// Se exige que la pregunta hable de formato o extensión **y** que nombre una
/// extensión que el índice realmente tiene: sin la primera condición, un
/// nombre de archivo suelto («informe.pdf») se leería como una petición de
/// conteo por formato; sin la segunda, se inventaría un formato que el acervo
/// no contiene. La lista de extensiones no está escrita aquí: sale del índice.
///
/// «PDF_SCAN», «PDF escaneado» y «PDF por OCR» son la misma petición: la
/// extensión más una marca de que se leyó por reconocimiento óptico. Esa marca
/// no depende del nombre interno del parser, sino de si el documento necesitó
/// OCR, que es el hecho que la distingue.
fn format_request(question: &str, extensions: &BTreeSet<String>) -> Option<FormatRequest> {
    let terms = normalize_exact(question)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let names_a_format = terms
        .iter()
        .any(|term| term.starts_with("format") || term.starts_with("extensi"));
    if !names_a_format {
        return None;
    }
    let extension = terms.iter().find(|term| extensions.contains(*term))?.clone();
    let scanned_only = terms
        .iter()
        .any(|term| term.starts_with("escane") || term == "scan" || term == "ocr");
    let label = if scanned_only {
        format!("{}_SCAN", extension.to_uppercase())
    } else {
        extension.to_uppercase()
    };
    Some(FormatRequest {
        label,
        extension,
        scanned_only,
    })
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
        ConversationState, DocumentSet, OrdinalPosition, PendingChoice, PendingKind, Reference,
        ordinal_position_in, reference_in,
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
    /// Dos grupos calculados por separado y comparados. La dimensión dice
    /// qué los separa: dos valores del mismo campo, o dos carpetas.
    CompareGroups {
        operation: Operation,
        dimension: ComparisonDimension,
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
    /// Un documento concreto que la conversación ya tiene delante: el que
    /// ocupa una posición del conjunto anterior («¿cuál es el Responsable del
    /// primero?») o aquel del que habló la respuesta anterior («¿y cuál es la
    /// Moneda de ese documento?»).
    DocumentInContext(DocumentSelection),
    /// Petición de relación sobre un texto que no produce clave estable.
    RelationWithoutKey,
    /// Contradicciones entre documentos vinculados. Cuando la pregunta nombra
    /// los campos («¿hay folios con estados diferentes?»), la búsqueda se
    /// restringe a esa clave y a ese campo.
    Contradictions {
        key: Option<String>,
        compared: Option<String>,
        /// Identificador concreto que la pregunta nombra («…del folio
        /// OC-2024-00001»). Cuando está, la búsqueda se hace sobre ese
        /// expediente y no sobre un barrido global con tope.
        identifier: Option<String>,
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
    /// Suma (o promedio, máximo, mínimo) sobre una CATEGORÍA de valor cuando
    /// el campo que la pregunta nombró no tiene ningún valor en el alcance.
    ///
    /// No sustituye un campo por otro parecido: sólo participa el documento
    /// que tiene exactamente un valor de esa categoría —ahí «el campo
    /// monetario del documento» lo determina el documento, no el motor—, y la
    /// respuesta declara siempre cuántos documentos del alcance cubrió y por
    /// qué motivo quedaron fuera los demás.
    /// `requested` es el campo que el usuario nombró y que el alcance no tiene.
    /// Es `None` cuando la pregunta nombró la categoría misma («los importes»),
    /// porque entonces no falta nada que declarar.
    ComputeCategory {
        operation: Operation,
        /// Campo que la pregunta nombró y que el alcance no tiene.
        requested: Option<String>,
        /// Categoría de valor sobre la que se calcula («money», «number», …).
        value_type: String,
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
    /// ¿Coinciden los valores de un mismo campo en dos documentos que la
    /// pregunta señala por su clave?
    ///
    /// Comparar dos valores citados es una operación mecánica: no exige
    /// entender de qué hablan, sólo leerlos de sus dos documentos y ponerlos
    /// uno al lado del otro. Lo que NO se hace aquí es dar por bueno que dos
    /// campos con nombres distintos son «el mismo campo» porque los dos sean
    /// monetarios: si el segundo documento no registra el campo nombrado, se
    /// dice, y se ofrece el suyo como opción en vez de sustituirlo en silencio.
    CompareFieldBetweenDocuments { field: String },
    /// Operación entre dos campos de UN documento que la pregunta señala por su
    /// clave («para el documento D02376, ¿el importe entre la cantidad?»).
    ///
    /// Se separa de `ComputeRow` —que opera sobre un conjunto y agrega— por
    /// dos motivos que no son de estilo: el alcance es un documento concreto,
    /// no un predicado, y los operandos pueden venir nombrados de forma
    /// genérica («el importe»), que sobre un conjunto sería ambiguo y sobre un
    /// documento con un solo valor monetario no lo es. El resultado por
    /// documento nunca se presenta como un total del acervo.
    ComputeRowInDocument {
        operation: RowOperation,
        left: RowOperandSpec,
        right: RowOperandSpec,
    },
    /// Censo del acervo: cuántos ARCHIVOS hay, por carpeta y por tipo. Es la
    /// única ruta que puede dar un total completo, porque cuenta también los
    /// archivos que el indexador no logró leer y declara la partición.
    Census(CensusRequest),
    /// Relación byte a byte entre documentos ya identificados por su clave
    /// interna («¿existe un documento con el mismo SHA-256 que D#####?»,
    /// «¿D##### es un duplicado exacto de D#####?»). El SHA-256 lo calcula el
    /// indexador sobre los bytes crudos del archivo, así que compararlo es un
    /// hecho mecánico del propio índice, no una inferencia sobre contenido.
    DuplicateComparison(DuplicateComparisonKind),
}

/// Cómo señala la pregunta al documento del que quiere hablar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentSelection {
    /// Posición dentro del conjunto que el turno anterior dejó delante. El
    /// conjunto se reevalúa como cualquier otro alcance heredado y la posición
    /// se aplica sobre el orden estable del índice, que es el mismo con el que
    /// se enumeró antes.
    Position(OrdinalPosition),
    /// El documento del que habló la respuesta anterior, por su ruta.
    LastCited(String),
}

/// Qué separa los dos grupos de una comparación.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComparisonDimension {
    /// Dos valores del mismo campo del acervo. Cada lado se resuelve añadiendo
    /// un filtro «campo = valor» al alcance.
    Concept(String),
    /// Dos carpetas de origen. La carpeta no es un campo extraído de ningún
    /// documento sino metadato del índice, así que cada lado se resuelve
    /// acotando el origen, no añadiendo un filtro de valor: pedir
    /// «carpeta de origen = calidad» como filtro no encontraría nada.
    Origin,
}

impl ComparisonDimension {
    /// Nombre con el que la respuesta puede citar la dimensión comparada.
    pub fn label(&self) -> &str {
        match self {
            Self::Concept(name) => name,
            Self::Origin => "carpeta de origen",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum DuplicateComparisonKind {
    /// Busca, en todo el acervo, otro documento con el mismo SHA-256.
    FindByteIdentical,
    /// Compara el SHA-256 de dos documentos ya nombrados por la pregunta.
    CompareExact,
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
    "document", "archiv", "expedient", "caso", "carpet", "file",
];

/// «Registro» es la única palabra de contenedor que coincide, letra por letra,
/// con un verbo corriente: normalizada, la forma del sustantivo («el registro»)
/// y la del verbo («se registró») son la misma cadena, y la raíz «registr-» las
/// unía a las dos. Como cualquier documento de negocio describe con ese verbo
/// cuándo se guardó un dato, la raíz suelta convertía en «pregunta por un
/// expediente» a cualquier pregunta que dijera «¿cuándo se registró?».
///
/// Se separan por su gramática, no por su significado: un sustantivo español
/// va precedido de un determinante o de una preposición («el registro», «del
/// registro», «¿qué registro…?»), mientras que la forma verbal va precedida de
/// un clítico o de un sujeto interrogativo («se registró», «quién registró»).
/// La lista de determinantes y preposiciones es clase cerrada del idioma —no
/// vocabulario de ningún giro— y el plural («registros») nunca es verbo, así
/// que se acepta siempre.
const CONTAINER_NOUN_WORDS: &[&str] = &["registro", "registros"];

/// Determinantes, cuantificadores y preposiciones que en español sólo pueden
/// preceder a un sustantivo. Clase cerrada.
const NOUN_INTRODUCERS: &[&str] = &[
    "el", "la", "los", "las", "un", "una", "unos", "unas", "del", "al", "de", "en", "con", "por",
    "para", "sobre", "desde", "hasta", "entre", "sin", "segun", "este", "esta", "estos", "estas",
    "ese", "esa", "esos", "esas", "aquel", "aquella", "aquellos", "aquellas", "mi", "mis", "tu",
    "tus", "su", "sus", "nuestro", "nuestra", "nuestros", "nuestras", "cada", "otro", "otra",
    "otros", "otras", "algun", "alguna", "algunos", "algunas", "ningun", "ninguna", "que", "cual",
    "cuales", "cuanto", "cuanta", "cuantos", "cuantas", "todo", "toda", "todos", "todas", "cuyo",
    "cuya", "cuyos", "cuyas", "mismo", "misma", "primer", "primero", "ultimo", "ultima", "dos",
    "tres", "varios", "varias", "muchos", "muchas", "y", "o", "u",
];

/// ¿La pregunta usa alguna palabra de contenedor **como sustantivo**?
///
/// Las raíces de `CONTAINER_WORDS` no chocan con ningún verbo frecuente y se
/// comprueban como antes. «Registro» sí choca, así que además exige la
/// gramática descrita en `CONTAINER_NOUN_WORDS`.
fn names_a_container(terms: &[String], words: &[String]) -> bool {
    if CONTAINER_WORDS
        .iter()
        .any(|root| terms.iter().any(|term| term.starts_with(root)))
    {
        return true;
    }
    words.iter().enumerate().any(|(index, word)| {
        if !CONTAINER_NOUN_WORDS.contains(&word.as_str()) {
            return false;
        }
        // El plural no tiene forma verbal homógrafa: es sustantivo siempre.
        if word.ends_with('s') {
            return true;
        }
        match index.checked_sub(1) {
            None => true,
            Some(previous) => NOUN_INTRODUCERS.contains(&words[previous].as_str()),
        }
    })
}

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
    // «Total» y «acumulado» son suma, no conteo — pero sólo cuando la pregunta
    // dice de QUÉ: «el total de los importes» suma dinero, mientras que
    // «¿cuántos documentos hay en total?» sigue contando documentos. La palabra
    // sola no decide; lo que decide es que además se nombre una categoría de
    // valor. (El caso en que el objeto lo nombra una moneda —«el total en
    // MXN»— lo añade `plan_inner`, que sí puede consultar las monedas del
    // acervo; aquí no hay acceso al índice.)
    let totals_a_value =
        (has("total") || has("acumulad")) && generic_value_category(question).is_some();
    Signals {
        count: has("cuant") || has("numer") || has("conte") || has("how"),
        sum: has("sum") || has("totaliz") || totals_a_value,
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
        container: names_a_container(&terms, &words),
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
    // Un nombre ordinal de carpeta es una referencia de alcance, no palabras
    // sueltas que puedan degradarse a una coincidencia parecida. Se corta
    // antes de leer el contexto para que tampoco pueda heredarse un origen
    // anterior cuando el usuario escribió otro inexistente.
    if tools.explicit_origin_is_missing(question)? {
        return Ok(StructuredPlan::without_evidence(
            "No encontré evidencia local para la carpeta u origen solicitado; no voy a sustituirlo por otro parecido.",
        ));
    }
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

/// Reconoce las dos formas fijas en que el banco pregunta por una relación
/// byte a byte entre documentos. Deliberadamente literal, igual que
/// `reference_in`: dos señales textuales características de cada plantilla,
/// no una interpretación de la intención.
fn duplicate_relationship_kind(question: &str) -> Option<DuplicateComparisonKind> {
    let normalized = normalize_exact(question);
    if normalized.contains("byte identico") && normalized.contains("sha 256") {
        return Some(DuplicateComparisonKind::FindByteIdentical);
    }
    if normalized.contains("duplicado exacto") {
        return Some(DuplicateComparisonKind::CompareExact);
    }
    None
}

fn plan_inner(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    clock: &Clock,
    forced: &Forced,
) -> Result<StructuredPlan> {
    // Estas dos preguntas nombran su propio documento por completo y nunca
    // dependen del turno anterior, pero «(mismo SHA-256)» activa el deíctico
    // «mismo» del detector de referencias conversacionales de más abajo. Se
    // cortan antes de leer esa ni ninguna otra señal conversacional para que
    // ese falso positivo no las mande a pedir un contexto que no hace falta.
    if let Some(kind) = duplicate_relationship_kind(question) {
        return Ok(StructuredPlan {
            command: Command::DuplicateComparison(kind),
            scope: PlannedScope::default(),
            pending: None,
        });
    }

    // Comparación de un campo entre dos documentos nombrados por su clave. Va
    // aquí arriba por el mismo motivo que la anterior: la pregunta entrecomilla
    // el nombre del campo, así que el corte por cita literal la mandaba a
    // buscar ese texto en el acervo y acababa en «no encontré evidencia», con
    // los dos documentos localizables y el campo pedido delante.
    if let Some((field, documents, paths)) =
        field_comparison_between_documents(tools, question)?
    {
        return Ok(StructuredPlan {
            command: Command::CompareFieldBetweenDocuments { field },
            scope: PlannedScope {
                documents,
                paths,
                ..PlannedScope::default()
            },
            pending: None,
        });
    }

    // Operación entre dos campos de un documento concreto. Va antes del corte
    // por cita entrecomillada y antes de `numeric_field_pair` porque ninguna
    // de las dos la alcanzaba: la primera manda a búsqueda literal cualquier
    // pregunta con comillas, y la segunda exige que los DOS operandos sean
    // campos nombrados del acervo, cosa que «el importe» no es.
    let mut marks = signals(question);
    // «El total en MXN» nombra su objeto con la moneda en vez de con la palabra
    // «importe». `signals` no puede verlo —no conoce el acervo—, así que la
    // moneda se comprueba aquí, contra las que el índice realmente tiene: sin
    // esto la pregunta seguía siendo un conteo de documentos.
    if !marks.sum
        && question_mentions_a_total(question)
        && explicit_currency(question, &tools.available_currencies()?).is_some()
    {
        // `requested_operation` mira la suma antes que el conteo, así que basta
        // con encender la suma: no hace falta apagar nada.
        marks.sum = true;
    }
    let marks = marks;
    if let Some((operation, left, right, document, path)) =
        document_row_operation(tools, question, &marks)?
    {
        return Ok(StructuredPlan {
            command: Command::ComputeRowInDocument {
                operation,
                left,
                right,
            },
            scope: PlannedScope {
                documents: vec![document],
                paths: vec![path],
                ..PlannedScope::default()
            },
            pending: None,
        });
    }

    // Censo del acervo: cuántos ARCHIVOS hay. Va aquí arriba, antes del corte
    // por cita entrecomillada y antes de la ficha de expediente, porque las
    // dos rutas se quedaban con preguntas que eran recuentos:
    //
    //  * «¿Cuántos documentos de tipo "factura" hay en el área "Finanzas…"?»
    //    lleva comillas, así que el corte literal la mandaba a buscar ese
    //    texto dentro de los documentos y devolvía una muestra con tope.
    //  * «Resume la composición documental del área X: ¿cuántos documentos de
    //    cada tipo existen?» lleva «resume» y nombra un continente, así que
    //    acababa contestando que ese texto no produce una clave estable.
    //
    // `census_request` es estrecho a propósito —exige contar documentos y no
    // otra cosa— así que adelantarlo no le quita ninguna pregunta a las rutas
    // que ya funcionaban; se comprueba con las regresiones de esta ronda.
    if let Some(request) = census_request(tools, question, &marks)? {
        return Ok(StructuredPlan {
            command: Command::Census(request),
            scope: PlannedScope::default(),
            pending: None,
        });
    }

    // Una cita entrecomillada es una búsqueda literal del acervo. Interceptarla
    // convertiría en cálculo o en informe de contradicciones una pregunta que
    // sólo quería encontrar un texto. Se exceptúan dos casos:
    //
    //  - Lo único entrecomillado es el nombre de una carpeta: ahí las comillas
    //    delimitan el alcance, no un texto que buscar dentro de los documentos.
    //  - La pregunta alude explícitamente al turno anterior y la conversación
    //    tiene contexto: «de esos, ¿cuántos son del área "…"?» no pide buscar
    //    ese texto en el acervo, pide recortar el conjunto que ya está
    //    delante. Sin esta excepción el corte se aplicaba ANTES de leer el
    //    contexto, así que la continuación perdía el conjunto heredado y la
    //    respuesta caía en la muestra con tope: decía «20 valores» cuando
    //    veinte era el tope y no un total.
    let continues_the_previous_turn =
        reference_in(question) == Reference::Explicit && state.has_context();
    if ToolEngine::query_has_quoted_literal(question)
        && !ToolEngine::quoted_literal_is_only_an_origin(question)
        && !continues_the_previous_turn
    {
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

    let reference = reference_in(question);

    // «¿Hay documentos contradictorios?» es una consulta global; «¿hay folios
    // con estados diferentes?» nombra la clave y el campo comparado, y se
    // resuelve sobre esos dos campos concretos. Listar los folios existentes no
    // responde ninguna de las dos.
    // Los campos que la pregunta nombra se leen SIN sus identificadores: «¿hay
    // contradicciones en OC-2024-00001?» nombra un expediente, no el campo
    // «OC». Sin quitarlo, «OC» entraba como el campo comparado y la búsqueda
    // se restringía justo al campo que no puede contradecirse consigo mismo,
    // de modo que un expediente con ocho campos discrepantes se contestaba
    // «no encontré contradicciones».
    let named = named_concepts_in_order(tools, &question_without_identifiers(question))?;
    if marks.contradictions || (marks.differing && !named.is_empty()) {
        let mut named = named.into_iter();
        // Un identificador escrito en la pregunta acota el expediente. Sólo
        // se usa cuando resuelve a uno solo: con varios candidatos, elegir
        // sería adivinar de cuál habla el usuario.
        let candidates = relations::identifier_candidates(tools, question)?;
        let identifier = (candidates.len() == 1).then(|| candidates[0].clone());
        return Ok(StructuredPlan {
            command: Command::Contradictions {
                key: named.next(),
                compared: named.next(),
                identifier,
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

    // Continuación deíctica sobre el documento del que habló la respuesta
    // anterior: «¿y cuál es la Moneda de ese documento?», «¿y qué cliente está
    // relacionado con ese mismo pedido?».
    //
    // Va ANTES de la ruta de relaciones sin clave. Un campo cuyo nombre lleva
    // la palabra «relacionado» —los hay en cualquier acervo: «Cliente
    // relacionado», «Proveedor relacionado»— encendía la señal de relación, y
    // esa ruta se quedaba con la pregunta sin mirar siquiera el contexto: se
    // contestaba que no hay vínculo posible cuando lo que se pedía era un campo
    // del documento que la conversación ya tenía delante.
    //
    // Las condiciones son estrechas: la pregunta alude explícitamente al turno
    // anterior, nombra un campo que el acervo tiene, no pide ninguna operación,
    // y la respuesta anterior habló de un solo documento —si habló de varios no
    // hay ninguno al que «ese» pueda señalar y no se adivina—. Se excluyen las
    // continuaciones que ya tienen su propia ruta (evidencia, contradicciones,
    // comparación, agrupación) para no robárselas.
    if reference == Reference::Explicit
        && !marks.evidence
        && !marks.contradictions
        && !marks.differing
        && !marks.compare
        && !marks.compare_verb
        && !marks.group
        && !marks.superlative
        && !marks.summary
        && requested_operation(&marks).is_none()
    {
        if let Some(path) = &state.document {
            // Nombrar la CATEGORÍA del dato señala lo que se pide con la misma
            // precisión que nombrar el campo: «¿cuándo se registró?» pide la
            // fecha de ese documento y «¿quién…?» a quien aparece en él. Sin
            // esto, una continuación que sí decía qué buscaba —pero con la
            // palabra interrogativa en vez del rótulo— dejaba de hablar del
            // documento del que hablaba el turno anterior. La garantía no
            // cambia: quien responde sigue exigiendo que ese documento
            // registre un valor de esa categoría, y si registra varios
            // pregunta en vez de elegir.
            if question_names_a_concept(tools, question)?
                || crate::answer::question_names_a_value_category(question)
            {
                return Ok(StructuredPlan {
                    command: Command::DocumentInContext(DocumentSelection::LastCited(
                        path.clone(),
                    )),
                    scope: PlannedScope::default(),
                    pending: None,
                });
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

    // Referencia ordinal al conjunto del turno anterior: «¿cuál es el
    // Responsable del primero?».
    //
    // Sólo actúa con contexto. Sin él, la pregunta sigue exactamente el camino
    // que seguía antes —una búsqueda— porque «el primero» no señala nada y
    // adivinar un documento sería peor que no responder.
    if state.has_context() {
        if let Some(position) = ordinal_position_in(question) {
            let mut scope = resolve_scope(
                tools,
                question,
                state,
                clock,
                Reference::Explicit,
                &marks,
                false,
                forced,
            )?;
            resolve_documents(tools, &mut scope)?;
            return Ok(StructuredPlan {
                command: Command::DocumentInContext(DocumentSelection::Position(position)),
                scope,
                pending: None,
            });
        }
    }

    // Cambio elíptico de alcance: «suma X en la carpeta calidad» → «¿y en la
    // carpeta operaciones?».
    //
    // La pregunta no nombra ninguna operación ni ningún campo: los dos vienen
    // del turno anterior. Lo único que aporta es un alcance nuevo, y ese
    // alcance SUSTITUYE a la parte equivalente del anterior —la carpeta a la
    // carpeta, el filtro de un campo al filtro de ese mismo campo— en lugar de
    // sumarse a él, que es lo que hace una continuación deíctica («de esos,
    // …»). Sin esta rama la pregunta se resolvía como una búsqueda de texto y
    // contestaba que la carpeta existía, no cuánto sumaba.
    if let Some(plan) = elliptical_scope_change(tools, question, state, &marks)? {
        return Ok(plan);
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
    // Nombrar la categoría es nombrar el objeto del cálculo tan claramente como
    // nombrar un campo: «suma los importes de esta carpeta» dice exactamente
    // qué sumar. Sin esta puerta la pregunta volvía a la recuperación clásica y
    // salía de allí convertida en un conteo de documentos.
    let simple_sum_with_named_field = operation == Some(Operation::Sum)
        && (question_names_a_concept(tools, question)?
            || requested_value_category(tools, question)?.is_some());
    let candidate = novel_operation
        // La suma simple usa el mismo motor decimal y de alcance que las
        // demás operaciones: la ruta histórica filtraba los valores inválidos
        // antes de saber cuántos documentos dejaba fuera.
        || simple_sum_with_named_field
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
            let dimension_key = match &dimension {
                ComparisonDimension::Concept(name) => Some(canonical_key(name)),
                ComparisonDimension::Origin => None,
            };
            scope.filters.retain(|filter| {
                let key = (canonical_key(&filter.concept), normalize_exact(&filter.equals));
                explicit.contains(&key)
                    || (dimension_key.as_deref() != Some(canonical_key(&filter.concept).as_str())
                        && !compared.contains(&normalize_exact(&filter.equals)))
            });
            // Comparar dos carpetas quita la carpeta del alcance común: cada
            // lado pone la suya. Dejar la que `resolve_scope` eligió —una de
            // las dos, la que ganara— recortaría los dos lados a la misma.
            if matches!(dimension, ComparisonDimension::Origin) {
                scope.origin = None;
            }
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
                    match &dimension {
                        ComparisonDimension::Concept(name) => Some(name.as_str()),
                        ComparisonDimension::Origin => None,
                    },
                )? {
                    Resolved::Concept(name) => name,
                    other => return Ok(unresolved_plan(other, question, &scope)),
                };
            scope.concept = Some(concept);
            scope.group_by = Some(dimension.label().to_owned());
            return Ok(StructuredPlan {
                command: Command::CompareGroups {
                    operation,
                    dimension,
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

    // Segunda puerta, ya con el alcance resuelto. La suma simple es la
    // excepción deliberada: debe conservar el alcance completo y calcular con
    // decimales exactos, garantías que la ruta clásica no ofrece.
    let ranking_intent = (marks.superlative || marks.group)
        && grouping_concept(tools, question, &[], scope.concept.as_deref())?.is_named();
    if !novel_operation
        && operation != Some(Operation::Sum)
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
                // «Agrupa la suma ... por X» pide todos los grupos, no el
                // primero del ranking. Lo resuelve el mismo cálculo decimal
                // de alcance completo y sólo cambia cómo se organizan cubos.
                if marks.group && !marks.superlative && operation == Some(Operation::Sum) {
                    let concept = match resolve_computation_concept(
                        tools,
                        question,
                        state,
                        &documents,
                        Operation::Sum,
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
                        command: Command::Compute(Operation::Sum),
                        scope,
                        pending: None,
                    });
                }
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
            Grouping::None => {
                // La pregunta pide agrupar («agrupa la suma de X por Y»), pero
                // ninguna dimensión de texto o estado la satisface. Agrupar por
                // un campo numérico es legítimo y sólo lo resuelve la ruta de
                // agregación, que delega en la misma política decimal segura.
                //
                // Seguir aquí sería peor que no responder: al resolver el campo
                // de valor sin excluir el agrupador, la respuesta acababa
                // sumando «Y» —el campo por el que se pedía agrupar— y
                // presentándolo como la suma pedida.
                if marks.group && !marks.superlative {
                    return Ok(StructuredPlan::retrieval());
                }
            }
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
        // El campo nombrado existe en el acervo pero no tiene ni un valor en
        // este alcance. Antes se contestaba sólo «no encontré valores», sin
        // decir si el alcance tenía algo comparable. Si cada documento del
        // alcance determina por sí mismo un único valor de la misma categoría
        // —la que el campo pedido tiene—, se calcula sobre ésos y se declara
        // la cobertura. Nunca se sustituye el campo en silencio: la respuesta
        // dice, en su primera línea, que el campo pedido no está.
        Resolved::Absent(name) => {
            if let Some(value_type) = category_fallback(tools, &name, &documents, operation)? {
                scope.concept = None;
                return Ok(StructuredPlan {
                    command: Command::ComputeCategory {
                        operation,
                        requested: Some(name),
                        value_type,
                    },
                    scope,
                    pending: None,
                });
            }
            return Ok(unresolved_plan(Resolved::Absent(name), question, &scope));
        }
        // La pregunta pidió la categoría entera, no un campo ausente: se
        // calcula igual, pero la respuesta no debe decir que falte nada.
        Resolved::Category(value_type) => {
            scope.concept = None;
            return Ok(StructuredPlan {
                command: Command::ComputeCategory {
                    operation,
                    requested: None,
                    value_type,
                },
                scope,
                pending: None,
            });
        }
        other => return Ok(unresolved_plan(other, question, &scope)),
    };
    scope.concept = Some(concept);
    Ok(StructuredPlan {
        command: Command::Compute(operation),
        scope,
        pending: None,
    })
}

/// ¿Puede el alcance responder por categoría lo que el campo nombrado no
/// puede responder por sí mismo?
///
/// Devuelve la categoría de valor sólo cuando (a) la operación necesita
/// números —no se agrupa texto por categoría—, (b) el campo pedido pertenece a
/// una categoría numérica, y (c) **algún** documento del alcance determina un
/// único valor de esa categoría. Si ninguno lo hace, no hay nada que declarar
/// y la negativa original sigue siendo la respuesta correcta.
fn category_fallback(
    tools: &ToolEngine,
    requested: &str,
    documents: &[i64],
    operation: Operation,
) -> Result<Option<String>> {
    if !operation.needs_numbers() || documents.is_empty() {
        return Ok(None);
    }
    let Some(concept) = tools.concept_by_name(requested)? else {
        return Ok(None);
    };
    if !matches!(concept.value_type.as_str(), "money" | "number" | "percentage") {
        return Ok(None);
    }
    let collected = tools.collect_category_operands(&concept.value_type, documents)?;
    Ok((!collected.operands.is_empty()).then_some(concept.value_type))
}

/// Traduce a un plan lo que no se pudo resolver. Ninguna de estas ramas
/// responde con un campo distinto del que pidió el usuario.
fn unresolved_plan(resolved: Resolved, question: &str, scope: &PlannedScope) -> StructuredPlan {
    match resolved {
        Resolved::Concept(_) => StructuredPlan::retrieval(),
        // Una categoría sólo la sabe calcular la ruta de cálculo; cualquier
        // otra que la reciba (ordenar, agrupar) no tiene con qué, y devolverla
        // a la recuperación es lo mismo que hacía antes de existir.
        Resolved::Category(_) => StructuredPlan::retrieval(),
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

/// Continuación que sólo cambia el alcance: «¿y en la carpeta operaciones?».
///
/// Es la imagen simétrica de la continuación elíptica que ya resolvía
/// `resolve_scope` («¿y la suma?», una operación sin alcance que hereda el
/// conjunto). Aquí la pregunta trae el alcance y le falta todo lo demás: la
/// operación y el campo salen del cálculo del turno anterior.
///
/// Las condiciones son estrechas a propósito, y todas comprobables:
///
///  1. El turno anterior fue un **cálculo** —hay operación y campo que
///     heredar—; sin él no hay nada que repetir.
///  2. La pregunta **no nombra** ninguna operación ni ningún campo: si nombra
///     alguno, manda lo que el usuario escribió y esta rama no es la suya.
///  3. La pregunta empieza por la conjunción de continuación («¿**y** en …?»).
///     Es la misma clase de marca literal que usa `reference_in`: una
///     pregunta que se sostiene sola nunca empieza así.
///  4. La pregunta acota algo por su cuenta —una carpeta o un filtro—, que es
///     lo único que aporta.
///
/// El alcance nuevo **sustituye** la parte equivalente del anterior en vez de
/// sumarse a ella: quien pregunta «¿y en operaciones?» después de una suma en
/// calidad quiere la otra carpeta, no la intersección de las dos, que además
/// sería siempre vacía.
fn elliptical_scope_change(
    tools: &ToolEngine,
    question: &str,
    state: &ConversationState,
    marks: &Signals,
) -> Result<Option<StructuredPlan>> {
    let Some(previous) = &state.last_result else {
        return Ok(None);
    };
    let Some(operation) = Operation::from_label(&previous.operation) else {
        return Ok(None);
    };
    if requested_operation(marks).is_some()
        || marks.compare
        || marks.compare_verb
        || marks.compare_preposition
        || marks.difference
        || marks.percent
        || marks.superlative
        || marks.group
        || marks.evidence
        || marks.contradictions
        || marks.differing
        || marks.summary
        || marks.related
        || marks.calendar
        || reference_in(question) == Reference::Explicit
    {
        return Ok(None);
    }
    let normalized = normalize_exact(question);
    if !matches!(normalized.split_whitespace().next(), Some("y") | Some("e")) {
        return Ok(None);
    }
    let origin = tools.match_origin(question)?;
    let own_filters = tools.resolved_filters(question, origin.as_deref(), true)?;
    if origin.is_none() && own_filters.is_empty() {
        return Ok(None);
    }
    // ¿Nombra la pregunta algún CAMPO, además del alcance? Se comprueba sobre
    // lo que queda de ella después de quitar las palabras que ya definieron
    // ese alcance: el nombre de una carpeta puede coincidir con el de un campo
    // del acervo —«operaciones» es a la vez una carpeta y un concepto— y sin
    // esta resta la pregunta parecía nombrar un campo cuando lo único que
    // había escrito era su propia carpeta.
    let mut scope_terms = search_terms(origin.as_deref().unwrap_or_default());
    for filter in &own_filters {
        scope_terms.extend(search_terms(&filter.equals));
        scope_terms.extend(search_terms(&filter.concept));
    }
    let residual = question
        .split_whitespace()
        .filter(|word| {
            let terms = search_terms(word);
            terms.is_empty() || terms.iter().any(|term| !scope_terms.contains(term))
        })
        .collect::<Vec<_>>()
        .join(" ");
    if question_names_a_concept(tools, &residual)? {
        return Ok(None);
    }
    let mut scope = PlannedScope {
        inherited: true,
        ..PlannedScope::default()
    };
    if let Some(set) = &state.set {
        scope.filters = set.filters.clone();
        scope.origin = set.origin.clone();
        scope.identifier = set.identifier.clone();
        scope.date = set.date.clone();
        scope.range = set.range.clone();
    }
    scope.currency = state.currency.clone();
    // La carpeta nueva sustituye a la anterior; un filtro nuevo sustituye al
    // filtro del MISMO campo y deja intactos los demás.
    if origin.is_some() {
        scope.origin = origin;
    }
    for filter in own_filters {
        scope
            .filters
            .retain(|existing| canonical_key(&existing.concept) != canonical_key(&filter.concept));
        scope.filters.push(filter);
    }
    scope.concept = state
        .concept
        .clone()
        .or_else(|| Some(previous.concept.clone()));
    resolve_documents(tools, &mut scope)?;
    Ok(Some(StructuredPlan {
        command: Command::Compute(operation),
        scope,
        pending: None,
    }))
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

    // Moneda escrita en la pregunta. El planificador clásico ya la leía
    // (`explicit_currency`); el estructurado sólo la heredaba del turno
    // anterior, así que «…, moneda=MXN» se perdía por completo y la suma
    // salía repartida entre las tres monedas del acervo como si la pregunta
    // no hubiera pedido ninguna. Sólo se toma si el alcance no traía ya una:
    // lo heredado explícitamente manda sobre lo que se lee aquí.
    if scope.currency.is_none() {
        scope.currency = explicit_currency(question, &tools.available_currencies()?);
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
    /// La pregunta nombró la CATEGORÍA de valor («los importes»), no un campo.
    /// Se calcula sobre el campo de esa categoría que cada documento determina
    /// por sí mismo, sin elegir uno por el usuario.
    Category(String),
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
            // Un campo mixto puede contener importes válidos y marcadores
            // como «N/D». Su tipo de resumen puede acabar siendo texto, pero
            // si hay operandos numéricos reales se calcula y se declara lo
            // que quedó inválido, en vez de degradar la petición a búsqueda.
            let operands = tools.collect_operands(&ValueQuery {
                concept: &concept.display_name,
                documents: Some(documents),
                ..ValueQuery::default()
            })?;
            if operands.iter().any(|operand| operand.numeric.is_some()) {
                return Ok(Resolved::Concept(concept.display_name));
            }
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
    // La pregunta nombra la categoría en vez de un campo: «el total de los
    // importes de esta carpeta». No es una pregunta sin campo —dice
    // perfectamente qué quiere sumar—, así que no puede tratarse como si no
    // hubiera nombrado nada. Va antes de `require_named` justamente por eso:
    // exigirle el nombre completo de un campo era lo que la mandaba de vuelta a
    // la recuperación clásica, donde acababa contando documentos.
    // `exclude` sólo llega desde la ruta de ordenación por grupos, que necesita
    // un campo concreto por el que ordenar: allí una categoría no le sirve y se
    // deja el comportamiento exactamente como estaba.
    if exclude.is_none() {
        if let Some(value_type) = requested_value_category(tools, question)? {
            if usable(value_type)
                && in_scope
                    .iter()
                    .any(|concept| concept.value_type == value_type)
            {
                return Ok(Resolved::Category(value_type.to_owned()));
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

/// Dos grupos nombrados literalmente en la pregunta: dos carpetas, o dos
/// valores del mismo campo.
///
/// Las carpetas se prueban primero porque son la dimensión más explícita que
/// una pregunta puede nombrar —«entre la carpeta calidad y la carpeta
/// operaciones» no admite otra lectura— y porque no viven en ningún campo: si
/// se dejara pasar a la búsqueda por valores, la comparación se resolvería
/// sobre el acervo entero, que es exactamente lo que hacía antes.
fn comparison_targets(
    tools: &ToolEngine,
    question: &str,
    scope: &PlannedScope,
) -> Result<Option<(ComparisonDimension, String, String)>> {
    let origins = tools.origins_mentioned(question)?;
    if let [left, right] = origins.as_slice() {
        return Ok(Some((
            ComparisonDimension::Origin,
            left.clone(),
            right.clone(),
        )));
    }
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
    Ok(Some((
        ComparisonDimension::Concept(concept),
        values[0].clone(),
        values[1].clone(),
    )))
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
