//! Capa de síntesis: convierte la evidencia que la búsqueda ya encontró en una
//! frase que conteste la pregunta.
//!
//! Vive en su propio módulo, y no dentro de `tools`, porque no recupera nada:
//! recibe los `SearchHit` que la fase de recuperación ya decidió y sólo los
//! redacta. Así el motor de búsqueda —qué documentos encuentra y en qué orden—
//! queda intacto y esta capa puede fallar de forma segura: cuando no reconoce
//! un patrón claro devuelve `None` y el agente conserva su mensaje genérico.
//!
//! Ningún texto sale de aquí sin pasar por `value_is_supported`, el mismo
//! candado que verifica una respuesta del modelo.

use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::LazyLock,
};

use crate::{
    error::Result,
    extract::classify_value,
    model::{Evidence, SearchHit, TypedValue, ValueKind},
    normalize::{normalize_exact, normalize_spanish, search_terms, stems_match},
    tools::{DocumentValue, ToolEngine},
    verifier::value_is_supported,
};

/// Tope de valores distintos que se enumeran antes de resumir el resto.
const MAX_LISTED_VALUES: usize = 10;

/// Tope de campos que enumera un resumen antes de indicar cuántos quedan.
const MAX_SUMMARY_FIELDS: usize = 12;

/// Una pregunta por el campo de un identificador habla de un registro. Si el
/// identificador aparece en decenas de documentos no hay un registro principal
/// que resolver, así que no se paga la lectura por documento que haría falta
/// para intentarlo.
const MAX_PRINCIPAL_CANDIDATES: usize = 25;

/// Palabras con las que se formula una pregunta, no con las que el acervo
/// nombra un campo: verbos de consulta y contenedores genéricos. Ninguna
/// pertenece al vocabulario de un rubro de negocio concreto, y su único efecto
/// es impedir que "busca el documento X" se lea como una pregunta por un campo
/// llamado "Documento".
const QUESTION_FILLER: &[&str] = &[
    "busca",
    "buscar",
    "encuentra",
    "encontrar",
    "muestra",
    "mostrar",
    "lista",
    "listar",
    "dame",
    "dime",
    "cual",
    "cuales",
    "cuanto",
    "cuantos",
    "que",
    "quien",
    "quienes",
    "donde",
    "cuando",
    "documento",
    "documentos",
    "archivo",
    "archivos",
    "campo",
    "campos",
    "dato",
    "datos",
    "registro",
    "registros",
    "valor",
    "valores",
    "informacion",
    "detalle",
    "detalles",
    "tiene",
    "tienen",
    "hay",
    "es",
    "son",
    "sobre",
    "find",
    "show",
    "list",
    "document",
    "documents",
    "file",
    "files",
    "field",
    "fields",
    "value",
    "values",
    "what",
    "which",
    "who",
    "where",
];

static FILLER_ROOTS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    QUESTION_FILLER
        .iter()
        .map(|word| normalize_spanish(word))
        .collect()
});

/// Palabras que nombran al continente, no al contenido. Sirven para reconocer
/// que una pregunta habla de los documentos en sí ("qué documentos se
/// relacionan con X") y no de un campo suyo.
static CONTAINER_ROOTS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    [
        "documento",
        "documentos",
        "archivo",
        "archivos",
        "expediente",
    ]
    .iter()
    .map(|word| normalize_spanish(word))
    .collect()
});

pub struct Synthesis {
    pub text: String,
    /// Falso cuando la evidencia no alcanza para responder. El texto lo dice
    /// explícitamente y la interfaz no marca la respuesta como verificada.
    pub verified: bool,
    pub citations: Vec<Evidence>,
}

/// Redacta la evidencia ya encontrada. `None` significa que no hay un patrón
/// claro que sintetizar y que el agente debe conservar su mensaje genérico.
pub fn synthesize(
    tools: &ToolEngine,
    question: &str,
    hits: &[SearchHit],
) -> Result<Option<Synthesis>> {
    if hits.is_empty() {
        return Ok(None);
    }
    let identifier = queried_identifier(hits);
    let terms = search_terms(question);
    // La palabra que sólo nombra el tipo de la entidad no se descarta, se
    // degrada: puede desempatar entre dos campos, pero no habilitar a ninguno
    // por sí sola. Descartarla del todo apagaba preguntas legítimas donde esa
    // misma palabra era justo lo que distinguía un campo de otro.
    let type_words = match &identifier {
        Some((_, text)) => entity_type_words(question, text),
        None => Vec::new(),
    };

    if let Some((canonical, text)) = &identifier {
        // Una pregunta por los documentos relacionados habla del acervo, no de
        // un campo. Se responde con lo que la búsqueda ya encontró, sin leer
        // ningún documento.
        if asks_for_related_documents(&terms) {
            return Ok(related_documents_answer(text, hits));
        }
        if hits.len() <= MAX_PRINCIPAL_CANDIDATES {
            let documents = load_documents(tools, hits)?;
            if asks_for_summary(&terms) {
                return Ok(summary_answer(&documents, canonical, text, hits));
            }
            // El orden importa: cuando la pregunta pide un campo que la
            // evidencia encontrada no contiene, la síntesis directa
            // respondería con el identificador —el único campo que esos
            // documentos comparten— en lugar del dato pedido.
            if let Some(synthesis) =
                identified_field_answer(&documents, &terms, &type_words, canonical, text, hits)
            {
                return Ok(Some(synthesis));
            }
        }
    }
    shared_field_answer(tools, &terms, &type_words, hits)
}

// -------------------------------------------------------------------------
// Tipo A: la evidencia encontrada ya es la respuesta, sólo falta redactarla.
// -------------------------------------------------------------------------

fn shared_field_answer(
    tools: &ToolEngine,
    terms: &[String],
    type_words: &[String],
    hits: &[SearchHit],
) -> Result<Option<Synthesis>> {
    // Un solo documento con el campo escrito de otra forma —el encabezado de
    // un CSV frente al de una ficha— no puede descartar todo el lote. Se agrupa
    // por nombre de campo y se sintetiza sobre un grupo, no sobre el conjunto.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, hit) in hits.iter().enumerate() {
        if let Some((field, _)) = field_value(hit) {
            groups
                .entry(normalize_exact(field))
                .or_default()
                .push(index);
        }
    }
    // Un solo grupo es, por construcción, el campo del que habla toda la
    // evidencia: no hace falta que la pregunta lo nombre explícitamente (así
    // sigue funcionando "Encuentra ABC-123", que no menciona ningún campo).
    // Con más de un grupo sí hace falta resolverlo contra lo que la pregunta
    // pide — nunca por tamaño: el campo pedido puede ser minoritario frente a
    // otro que sólo aparece como referencia cruzada mucho más frecuente
    // (p.ej. "Tipo de inmueble", 30 apariciones, frente al campo "Inmueble"
    // que sólo referencia esa propiedad desde 90 documentos de otras
    // categorías). Elegir por tamaño habría respondido con el campo
    // equivocado.
    let members = if groups.len() == 1 {
        let Some(members) = groups.into_values().next() else {
            return Ok(None);
        };
        members
    } else {
        let vocabulary = groups
            .values()
            .filter_map(|group| field_value(&hits[group[0]]).map(|(field, _)| field.to_owned()))
            .collect::<Vec<_>>();
        let resolved = match resolve_field(&vocabulary, terms, type_words) {
            FieldMatch::Resolved(name) => name,
            FieldMatch::NotRequested | FieldMatch::Ambiguous => return Ok(None),
        };
        let Some(members) = groups.remove(&normalize_exact(&resolved)) else {
            return Ok(None);
        };
        members
    };
    let field = field_value(&hits[members[0]])
        .map(|(field, _)| field)
        .unwrap_or_default();

    // Las citas siguen siendo todas las que la búsqueda encontró: el texto
    // habla de un grupo, pero la evidencia mostrada no se recorta.
    let citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    let values = members
        .iter()
        .filter_map(|index| field_value(&hits[*index]).map(|(_, value)| value))
        .collect::<Vec<_>>();

    if let ([index], [value]) = (members.as_slice(), values.as_slice()) {
        return single_value_answer(tools, &hits[*index], field, value, citations);
    }

    let typed = values
        .iter()
        .map(|value| (classify_value(field, value), *value))
        .collect::<Vec<_>>();
    if typed.iter().all(|(typed, _)| typed.numeric_value.is_some()) {
        return Ok(numeric_summary(field, &typed, citations));
    }
    Ok(value_list_summary(field, &values, citations))
}

fn single_value_answer(
    tools: &ToolEngine,
    hit: &SearchHit,
    field: &str,
    value: &str,
    mut citations: Vec<Evidence>,
) -> Result<Option<Synthesis>> {
    let mut literals = vec![field.to_owned(), value.to_owned()];
    let identifier = document_identifier(tools, hit.evidence.document_id, value)?;
    if let Some(identifier) = &identifier {
        literals.push(identifier.value.clone());
        // La cita que nombra al registro acompaña a la que da el valor: sin
        // ella el identificador no estaría respaldado por la evidencia.
        if !citations
            .iter()
            .any(|item| item.id == identifier.evidence.id)
        {
            citations.push(identifier.evidence.clone());
        }
    }
    let text = direct_phrase(
        field,
        value,
        identifier.as_ref().map(|item| item.value.as_str()),
        &hit.evidence,
    );
    Ok(supported(text, &literals, citations, true))
}

fn numeric_summary(
    field: &str,
    values: &[(TypedValue, &str)],
    mut citations: Vec<Evidence>,
) -> Option<Synthesis> {
    let mut groups: BTreeMap<String, Vec<&(TypedValue, &str)>> = BTreeMap::new();
    for value in values {
        groups
            .entry(value.0.currency.clone().unwrap_or_default())
            .or_default()
            .push(value);
    }
    let mut literals = vec![field.to_owned()];
    let mut totals = Vec::new();
    struct Row {
        currency: String,
        count: usize,
        lowest: String,
        highest: String,
        total: Option<String>,
    }
    let mut rows = Vec::new();
    for (currency, group) in &groups {
        let mut ordered = group.clone();
        ordered.sort_by(|left, right| {
            left.0
                .numeric_value
                .unwrap_or_default()
                .total_cmp(&right.0.numeric_value.unwrap_or_default())
        });
        let lowest = ordered.first()?.1;
        let highest = ordered.last()?.1;
        literals.push(lowest.to_owned());
        literals.push(highest.to_owned());
        // Un porcentaje no se suma: el total sólo tiene sentido para importes y
        // cantidades. En ese caso se informa el rango sin inventar un agregado.
        let summable = group
            .iter()
            .all(|item| matches!(item.0.kind, ValueKind::Money | ValueKind::Number));
        let total = summable.then(|| {
            let samples = group.iter().map(|item| item.1).collect::<Vec<_>>();
            let sum = group
                .iter()
                .map(|item| item.0.numeric_value.unwrap_or_default())
                .sum::<f64>();
            let rendered = render_total(
                sum,
                &samples,
                (!currency.is_empty()).then_some(currency.as_str()),
            );
            totals.push(rendered.clone());
            rendered
        });
        rows.push(Row {
            currency: currency.clone(),
            count: group.len(),
            lowest: lowest.to_owned(),
            highest: highest.to_owned(),
            total,
        });
    }

    let count = counted(values.len(), "valor", "valores");
    let has_totals = !totals.is_empty();
    let multi_currency = rows.len() > 1;

    let mut header = vec!["Mínimo".to_owned(), "Máximo".to_owned()];
    if multi_currency {
        header.splice(0..0, ["Moneda".to_owned(), "Valores".to_owned()]);
    }
    if has_totals {
        header.push("Total".to_owned());
    }
    let table_rows = rows
        .iter()
        .map(|row| {
            let mut cells = vec![row.lowest.clone(), row.highest.clone()];
            if multi_currency {
                let label = if row.currency.is_empty() {
                    "Sin moneda declarada".to_owned()
                } else {
                    row.currency.clone()
                };
                cells.splice(0..0, [label, row.count.to_string()]);
            }
            if has_totals {
                // Un grupo no sumable (p.ej. porcentajes) no aporta total propio,
                // pero la tabla necesita la misma cantidad de columnas en cada fila.
                cells.push(row.total.clone().unwrap_or_else(|| "—".to_owned()));
            }
            cells
        })
        .collect::<Vec<_>>();

    let mut text = format!(
        "{field} — {count}\n\n{}",
        markdown_table(&header, &table_rows)
    );
    // Monedas distintas no se combinan en un solo total: la tabla ya las separa
    // por fila, pero se aclara explícitamente que no se suman entre sí.
    if multi_currency {
        text.push_str("\n\nLos totales no se combinan entre monedas distintas.");
    }

    if has_totals {
        let note = calculation_note(field, &totals, citations.first()?, values.len());
        literals.extend(totals);
        citations.push(note);
    }
    supported(text, &literals, citations, true)
}

fn value_list_summary(field: &str, values: &[&str], citations: Vec<Evidence>) -> Option<Synthesis> {
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();
    for value in values {
        if seen.insert(normalize_exact(value)) {
            distinct.push(*value);
        }
    }
    let mut literals = vec![field.to_owned()];
    let count = counted(values.len(), "valor", "valores");
    // Un solo valor distinto ya es una frase corta y clara; no gana nada al
    // convertirse en una lista de un elemento.
    if let [only] = distinct.as_slice() {
        literals.push((*only).to_owned());
        let text = format!("Encontré {count} de «{field}», todos «{only}».");
        return supported(text, &literals, citations, true);
    }
    let shown = distinct.iter().take(MAX_LISTED_VALUES).collect::<Vec<_>>();
    literals.extend(shown.iter().map(|value| (**value).to_owned()));
    let remaining = distinct.len().saturating_sub(shown.len());
    let distinct_count = counted(distinct.len(), "valor distinto", "valores distintos");
    let listed = shown
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut text = format!("{field} — {count}, {distinct_count}\n\n{listed}");
    if remaining > 0 {
        text.push_str(&format!(
            "\n\n+ {}",
            counted(remaining, "valor distinto más", "valores distintos más")
        ));
    }
    supported(text, &literals, citations, true)
}

// -------------------------------------------------------------------------
// Intenciones que hablan del registro completo, no de un campo suyo.
// -------------------------------------------------------------------------

/// `normalize_spanish` no reduce "resume" y "resumen" a una raíz común, así
/// que la detección compara por prefijo en vez de por igualdad.
fn asks_for_summary(terms: &[String]) -> bool {
    terms.iter().any(|term| term.starts_with("resum"))
}

/// Una pregunta por documentos relacionados nombra a la vez la relación y el
/// continente. Exigir ambas señales evita confundirla con una pregunta por un
/// campo que casualmente se llame "algo relacionado".
fn asks_for_related_documents(terms: &[String]) -> bool {
    terms.iter().any(|term| term.starts_with("relacionad"))
        && terms.iter().any(|term| CONTAINER_ROOTS.contains(term))
}

fn summary_answer(
    documents: &[DocumentContext],
    identifier: &str,
    identifier_text: &str,
    hits: &[SearchHit],
) -> Option<Synthesis> {
    let principal = principal_document(documents, identifier)?;
    if principal.values.is_empty() {
        return None;
    }
    let file = file_name(&principal.hit.evidence);
    let shown = principal
        .values
        .iter()
        .take(MAX_SUMMARY_FIELDS)
        .collect::<Vec<_>>();
    let remaining = principal.values.len().saturating_sub(shown.len());

    let mut literals = vec![identifier_text.to_owned()];
    let mut citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    // Cada par campo–valor del resumen se cita: sin su evidencia no podría
    // pasar el candado, y el usuario no podría abrir la línea de la que salió.
    for value in &shown {
        literals.push(value.field.clone());
        literals.push(value.value.clone());
        if !citations.iter().any(|item| item.id == value.evidence.id) {
            citations.push(value.evidence.clone());
        }
    }
    let items = shown
        .iter()
        .map(|value| format!("{}: {}", value.field, value.value))
        .collect::<Vec<_>>();
    let mut text = format!(
        "Resumen de {identifier_text} — {file}\n\n{}",
        bullet_list(&items)
    );
    if remaining > 0 {
        text.push_str(&format!(
            "\n\n+ {}",
            counted(remaining, "campo más", "campos más")
        ));
    }
    supported(text, &literals, citations, true)
}

fn related_documents_answer(identifier_text: &str, hits: &[SearchHit]) -> Option<Synthesis> {
    let mut literals = vec![identifier_text.to_owned()];
    let shown = hits.iter().take(MAX_LISTED_VALUES).collect::<Vec<_>>();
    let remaining = hits.len().saturating_sub(shown.len());
    let items = shown
        .iter()
        .map(|hit| {
            let file = file_name(&hit.evidence);
            match hit.evidence.field.as_deref() {
                // El campo dice bajo qué papel aparece el identificador en cada
                // documento, que es justamente lo que distingue una relación de
                // otra.
                Some(field) => {
                    literals.push(field.to_owned());
                    format!("{file} — {field}")
                }
                None => file,
            }
        })
        .collect::<Vec<_>>();
    let noun = counted(hits.len(), "documento", "documentos");
    let mut text = format!(
        "{identifier_text} aparece en {noun}:\n\n{}",
        numbered_list(&items)
    );
    if remaining > 0 {
        text.push_str(&format!(
            "\n\n+ {}",
            counted(remaining, "documento más", "documentos más")
        ));
    }
    let citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    supported(text, &literals, citations, true)
}

// -------------------------------------------------------------------------
// Tipo B: la pregunta pide un campo que la evidencia encontrada no contiene.
// -------------------------------------------------------------------------

struct DocumentContext<'a> {
    hit: &'a SearchHit,
    values: Vec<DocumentValue>,
}

enum FieldMatch {
    NotRequested,
    Ambiguous,
    Resolved(String),
}

fn load_documents<'a>(
    tools: &ToolEngine,
    hits: &'a [SearchHit],
) -> Result<Vec<DocumentContext<'a>>> {
    let mut documents = Vec::with_capacity(hits.len());
    for hit in hits {
        documents.push(DocumentContext {
            hit,
            values: tools.document_values(hit.evidence.document_id)?,
        });
    }
    Ok(documents)
}

fn identified_field_answer(
    documents: &[DocumentContext],
    terms: &[String],
    type_words: &[String],
    identifier: &str,
    identifier_text: &str,
    hits: &[SearchHit],
) -> Option<Synthesis> {
    // El vocabulario se limita a los documentos ya encontrados. Compararlo
    // contra el índice completo haría que una palabra suelta abriera campos de
    // otros documentos que la búsqueda nunca devolvió.
    let vocabulary = distinct_fields(documents.iter().flat_map(|context| context.values.iter()));
    match resolve_field(&vocabulary, terms, type_words) {
        // La pregunta no nombra ningún campo: una búsqueda simple por
        // identificador se responde con la evidencia tal cual.
        FieldMatch::NotRequested => return None,
        // Un empate en el vocabulario común, antes de elegir documento
        // principal, casi siempre significa que la palabra coincidente aparece
        // por casualidad en varios nombres de campo de documentos distintos, no
        // que se pida un campo concreto. No es una duda que reportar: es una
        // pregunta que no era de campo.
        FieldMatch::Ambiguous => return None,
        FieldMatch::Resolved(name) => {
            // El campo pedido ya está entre la evidencia encontrada: no hace
            // falta ninguna consulta adicional, la síntesis directa lo responde.
            if hits.iter().all(|hit| {
                hit.evidence
                    .field
                    .as_deref()
                    .is_some_and(|field| normalize_exact(field) == normalize_exact(&name))
            }) {
                return None;
            }
        }
    }

    let citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    let Some(principal) = principal_document(documents, identifier) else {
        return unresolved(
            format!(
                "Sin concluir: {} documentos mencionan {identifier_text}, pero ninguno se distingue como el registro principal, así que no puedo atribuirle ese dato con certeza.",
                hits.len()
            ),
            &[identifier_text.to_owned()],
            citations,
        );
    };
    let file = file_name(&principal.hit.evidence);

    // Lección del corpus real: "precio" empata entre campos de documentos
    // distintos. El campo se resuelve dentro del documento principal ya
    // elegido, nunca contra el vocabulario común de todos los encontrados. Aquí
    // una duda sí es real y se reporta.
    let FieldMatch::Resolved(field) =
        resolve_field(&distinct_fields(principal.values.iter()), terms, type_words)
    else {
        return unresolved(
            format!(
                "Sin concluir: {file} es el registro principal de {identifier_text}, pero no puedo determinar con certeza a qué campo suyo se refiere la pregunta."
            ),
            &[identifier_text.to_owned()],
            citations,
        );
    };

    let matches = principal
        .values
        .iter()
        .filter(|value| normalize_exact(&value.field) == normalize_exact(&field))
        .collect::<Vec<_>>();
    let distinct = matches
        .iter()
        .map(|value| normalize_exact(&value.value))
        .collect::<HashSet<_>>();
    // Varios valores del mismo campo en un solo documento significan varios
    // registros dentro de él (un listado): no es posible atribuir uno de ellos
    // a este identificador sin cruzar la fila, así que no se responde.
    let ([chosen], 1) = (matches.as_slice(), distinct.len()) else {
        return unresolved(
            format!(
                "Sin concluir: {file} registra {} de «{field}», así que no puedo señalar cuál corresponde a {identifier_text}.",
                counted(distinct.len(), "valor distinto", "valores distintos")
            ),
            &[identifier_text.to_owned(), field],
            citations,
        );
    };

    let mut citations = citations;
    let text = direct_phrase(
        &chosen.field,
        &chosen.value,
        Some(identifier_text),
        &chosen.evidence,
    );
    let literals = vec![
        chosen.field.clone(),
        chosen.value.clone(),
        identifier_text.to_owned(),
    ];
    // La evidencia decisiva encabeza las citas; debajo se conserva intacto lo
    // que la búsqueda encontró por su cuenta.
    citations.insert(0, chosen.evidence.clone());
    supported(text, &literals, citations, true)
}

/// El identificador consultado se lee de la propia evidencia: sólo la ruta de
/// coincidencia canónica de `search` lo declara, así que esta capa no vuelve a
/// interpretar la pregunta por su cuenta ni amplía a prefijos.
fn queried_identifier(hits: &[SearchHit]) -> Option<(String, String)> {
    hits.iter()
        .filter(|hit| hit.evidence.match_kind == "canónica")
        .find_map(|hit| {
            Some((
                hit.evidence.normalized_value.clone()?,
                hit.evidence.value.clone()?,
            ))
        })
}

/// La palabra pegada al identificador nombra el TIPO de la entidad ("la
/// propiedad PROP-2026-0001"), no el campo que se pregunta. Se descarta para
/// que no coincida por casualidad con un nombre de campo que la contenga: sin
/// esto, "¿cuál es el color de la propiedad X?" respondía el «Estado de la
/// propiedad» sólo porque era el único campo con la palabra "propiedad".
///
/// Sólo se mira la palabra inmediatamente anterior. Si entre ella y el
/// identificador hay una preposición ("la superficie DE PROP-2026-0001"), esa
/// palabra es justamente el campo pedido y debe conservarse.
fn entity_type_words(question: &str, identifier_text: &str) -> Vec<String> {
    let words = question.split_whitespace().collect::<Vec<_>>();
    let Some(position) = words.iter().position(|word| {
        word.trim_matches(|character: char| !character.is_alphanumeric())
            .eq_ignore_ascii_case(identifier_text.trim())
    }) else {
        return Vec::new();
    };
    words
        .get(position.wrapping_sub(1))
        .map(|word| search_terms(word))
        .unwrap_or_default()
}

/// Documento principal de un identificador. Se prefiere aquel donde el
/// identificador aparece antes dentro del propio documento —un registro habla
/// de su entidad desde sus primeros campos, mientras que una referencia cruzada
/// aparece más abajo— y, a igualdad de posición, el que contiene menos
/// identificadores distintos: un listado menciona muchos, una ficha individual
/// casi ninguno. Si nada desempata, no hay documento principal.
fn principal_document<'a>(
    documents: &'a [DocumentContext<'a>],
    identifier: &str,
) -> Option<&'a DocumentContext<'a>> {
    let mut ranked = documents
        .iter()
        .filter_map(|context| {
            let position = context
                .values
                .iter()
                .find(|value| value.identifier_canonical.as_deref() == Some(identifier))
                .map(|value| value.ordinal)?;
            let distinct = context
                .values
                .iter()
                .filter_map(|value| value.identifier_canonical.as_deref())
                .collect::<HashSet<_>>()
                .len();
            Some(((position, distinct), context))
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(rank, _)| *rank);
    match ranked.as_slice() {
        [(_, only)] => Some(only),
        [(best, winner), (runner_up, _), ..] if best < runner_up => Some(winner),
        _ => None,
    }
}

/// Resuelve qué campo pide la pregunta contra el vocabulario que se le pasa.
/// Exige que coincida al menos un término significativo —ni vocabulario de
/// consulta ni el tipo de la entidad identificada— y que un solo campo quede
/// por delante; un empate se declara ambiguo en vez de elegir por azar.
///
/// Las palabras de tipo sí cuentan para puntuar. Así "¿cuál es el estado de la
/// propiedad X?" prefiere «Estado de la propiedad» sobre «Estado de la orden»,
/// mientras que "¿cuál es el color de la propiedad X?" no resuelve ningún campo:
/// su única coincidencia sería el tipo, que no basta por sí solo.
///
/// El vocabulario de consulta (`FILLER_ROOTS`) sólo descarta un campo cuando es
/// TODO lo que ese campo tiene que decir (así "Documento" nunca resuelve solo,
/// porque cualquier pregunta con la palabra "documento" lo activaría). No se
/// resta del resto de la puntuación: un campo compuesto como "Estado del
/// registro" sigue anotando su palabra "registro" cuando la pregunta también
/// la usa, aunque esa palabra por sí sola sea de las que se ignoran al
/// comparar preguntas contra el campo "Registro". Sin esto, "Estado del
/// registro" perdía su palabra distintiva y quedaba indistinguible del campo
/// "estado", cuando en realidad es el más específico de los dos.
fn resolve_field(vocabulary: &[String], terms: &[String], type_words: &[String]) -> FieldMatch {
    let mut best: Option<(usize, usize, String)> = None;
    let mut tied = false;
    for name in vocabulary {
        let field_terms = search_terms(name);
        let has_significant_term = field_terms.iter().any(|term| !FILLER_ROOTS.contains(term));
        if !has_significant_term {
            continue;
        }
        let matched = field_terms
            .iter()
            .filter(|term| terms.iter().any(|query_term| stems_match(query_term, term)))
            .count();
        let has_real_match = field_terms.iter().any(|term| {
            terms.iter().any(|query_term| stems_match(query_term, term))
                && !type_words.contains(term)
                && !FILLER_ROOTS.contains(term)
        });
        if matched == 0 || !has_real_match {
            continue;
        }
        let unmatched = field_terms.len() - matched;
        match &best {
            None => best = Some((matched, unmatched, name.clone())),
            Some((best_matched, best_unmatched, _)) => {
                if matched > *best_matched
                    || (matched == *best_matched && unmatched < *best_unmatched)
                {
                    best = Some((matched, unmatched, name.clone()));
                    tied = false;
                } else if matched == *best_matched && unmatched == *best_unmatched {
                    tied = true;
                }
            }
        }
    }
    match best {
        None => FieldMatch::NotRequested,
        Some(_) if tied => FieldMatch::Ambiguous,
        Some((_, _, name)) => FieldMatch::Resolved(name),
    }
}

fn distinct_fields<'a>(values: impl Iterator<Item = &'a DocumentValue>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut fields = Vec::new();
    for value in values {
        if seen.insert(normalize_exact(&value.field)) {
            fields.push(value.field.clone());
        }
    }
    fields
}

/// Identificador propio de un documento: el primer valor extraído que el motor
/// ya reconoció como identificador. Es el que nombra al registro, no una
/// referencia cruzada aparecida más abajo. Se descarta si coincide con el valor
/// que ya se está reportando, para no repetirlo en la misma frase.
fn document_identifier(
    tools: &ToolEngine,
    document_id: i64,
    reported_value: &str,
) -> Result<Option<DocumentValue>> {
    Ok(tools
        .document_values(document_id)?
        .into_iter()
        .find(|item| {
            item.identifier_canonical.is_some()
                && normalize_exact(&item.value) != normalize_exact(reported_value)
        }))
}

// -------------------------------------------------------------------------
// Redacción y candado
// -------------------------------------------------------------------------

/// Un hit aporta un par campo–valor citable sólo si la recuperación lo produjo
/// desde un valor estructurado. Los metadatos (nombre de archivo, carpeta de
/// origen) y las coincidencias de texto libre no declaran un campo con valor y
/// no se pueden redactar como si lo hicieran.
fn field_value(hit: &SearchHit) -> Option<(&str, &str)> {
    let field = hit.evidence.field.as_deref()?.trim();
    let value = hit.evidence.value.as_deref()?.trim();
    (!field.is_empty() && !value.is_empty()).then_some((field, value))
}

/// La ruta y la ubicación se copian de la cita que acompaña a la frase, así que
/// señalan exactamente el archivo y el punto que el usuario puede abrir.
fn direct_phrase(
    field: &str,
    value: &str,
    identifier: Option<&str>,
    evidence: &Evidence,
) -> String {
    let file = file_name(evidence);
    // "El campo «X»" concuerda con cualquier nombre de campo. Anteponer el
    // artículo al nombre en sí obligaría a conocer su género, que no se puede
    // deducir de un acervo arbitrario sin un diccionario.
    match identifier {
        Some(identifier) => format!(
            "El campo «{field}» de {identifier} es {value} ({file}, {}).",
            evidence.location
        ),
        None => format!(
            "El campo «{field}» es {value} en {file} ({}).",
            evidence.location
        ),
    }
}

/// Los tres formatos que el frontend sabe interpretar: lista con viñetas,
/// lista numerada y tabla. Componerlos aquí evita repetir el formato Markdown
/// en cada función de síntesis.
fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn numbered_list(items: &[String]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| format!("{}. {item}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn markdown_table(header: &[String], rows: &[Vec<String>]) -> String {
    let mut lines = vec![
        format!("| {} |", header.join(" | ")),
        format!(
            "|{}|",
            header.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        ),
    ];
    for row in rows {
        lines.push(format!("| {} |", row.join(" | ")));
    }
    lines.join("\n")
}

/// Concuerda el número con el sustantivo que lo acompaña. Es gramática del
/// idioma de la interfaz, no vocabulario de ningún rubro.
fn counted(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn file_name(evidence: &Evidence) -> String {
    Path::new(&evidence.path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| evidence.path.clone())
}

/// Nota de cálculo local, con el mismo papel que la evidencia que ya genera
/// `aggregate_values`: un total no está escrito en ningún documento, así que se
/// cita explícitamente como una operación de Omega sobre valores que sí tienen
/// evidencia, en vez de presentarse como un dato leído.
fn calculation_note(field: &str, totals: &[String], sample: &Evidence, count: usize) -> Evidence {
    let rendered = totals.join("; ");
    Evidence {
        id: format!(
            "calc-sintesis-{}-{count}",
            normalize_exact(field).replace(' ', "_")
        ),
        document_id: sample.document_id,
        path: sample.path.clone(),
        origin: sample.origin.clone(),
        location: format!("cálculo local exacto sobre {count} valores extraídos"),
        excerpt: format!(
            "Omega sumó los {count} valores de '{field}' con evidencia y obtuvo {rendered}."
        ),
        normalized_value: None,
        value: Some(rendered),
        matched: None,
        field: None,
        match_kind: "campo".into(),
        reliable: true,
        confidence: None,
    }
}

fn unresolved(text: String, literals: &[String], citations: Vec<Evidence>) -> Option<Synthesis> {
    supported(text, literals, citations, false)
}

/// Publica una síntesis sólo si cada valor que menciona aparece literalmente en
/// la evidencia que la acompaña. Si el candado no pasa, la respuesta se
/// descarta y el motor conserva su mensaje genérico: es preferible no
/// sintetizar a sintetizar algo sin respaldo.
///
/// Los conteos no se declaran como valores literales: no proceden de ningún
/// documento, cuentan las citas que se muestran junto a la respuesta y el
/// usuario puede comprobarlos ahí mismo.
fn supported(
    text: String,
    literals: &[String],
    citations: Vec<Evidence>,
    verified: bool,
) -> Option<Synthesis> {
    let borrowed = citations.iter().collect::<Vec<_>>();
    literals
        .iter()
        .all(|literal| value_is_supported(&borrowed, literal))
        .then_some(Synthesis {
            text,
            verified,
            citations,
        })
}

/// El formato del total se deduce de los propios valores citados: si todos
/// llevan símbolo, si usan separador de millares y si declaran decimales. No se
/// impone una convención que el acervo no use.
fn render_total(total: f64, samples: &[&str], currency: Option<&str>) -> String {
    let symbol = samples
        .iter()
        .all(|value| value.trim_start().starts_with('$'));
    let grouped = samples.iter().any(|value| value.contains(','));
    let decimals = samples.iter().any(|value| has_decimals(value));
    let mut rendered = if decimals {
        format!("{total:.2}")
    } else {
        format!("{total:.0}")
    };
    if grouped {
        rendered = group_thousands(&rendered);
    }
    if symbol {
        rendered.insert(0, '$');
    }
    if let Some(currency) = currency {
        rendered.push(' ');
        rendered.push_str(currency);
    }
    rendered
}

fn has_decimals(value: &str) -> bool {
    value
        .rsplit_once('.')
        .is_some_and(|(_, fraction)| fraction.starts_with(|c: char| c.is_ascii_digit()))
}

fn group_thousands(rendered: &str) -> String {
    let (integer, fraction) = match rendered.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (rendered, None),
    };
    let (sign, digits) = match integer.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", integer),
    };
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_copy_the_notation_of_the_values_they_summarize() {
        assert_eq!(
            render_total(87_430.0, &["$1,200 MXN", "$4,850 MXN"], Some("MXN")),
            "$87,430 MXN"
        );
        // Sin símbolo ni separador en el acervo, el total tampoco los inventa.
        assert_eq!(render_total(9.0, &["4", "5"], None), "9");
        assert_eq!(
            render_total(1_234.5, &["1,000.00", "234.50"], None),
            "1,234.50"
        );
    }

    #[test]
    fn a_query_word_never_names_a_field_by_itself() {
        let vocabulary = vec!["Documento".to_owned(), "Estado".to_owned()];
        let requested = resolve_field(
            &vocabulary,
            &search_terms("Busca el documento ABC-123"),
            &[],
        );
        assert!(matches!(requested, FieldMatch::NotRequested));
        let asked = resolve_field(
            &vocabulary,
            &search_terms("¿Cuál es el estado de ABC-123?"),
            &[],
        );
        assert!(matches!(asked, FieldMatch::Resolved(name) if name == "Estado"));
    }

    #[test]
    fn an_equally_scored_field_is_ambiguous_instead_of_arbitrary() {
        let vocabulary = vec!["Precio pactado".to_owned(), "Precio estimado".to_owned()];
        let requested = resolve_field(
            &vocabulary,
            &search_terms("¿Cuál es el precio de ABC-123?"),
            &[],
        );
        assert!(matches!(requested, FieldMatch::Ambiguous));
    }

    #[test]
    fn a_field_fully_named_by_the_question_wins_over_a_broader_one() {
        // Un empate en términos coincidentes se resuelve a favor del campo que
        // la pregunta nombra por completo. Sin esta regla, "¿cuál es el estado
        // de X?" quedaría sin concluir por la mera existencia de un campo más
        // largo que comparte una palabra.
        let vocabulary = vec!["Estado".to_owned(), "Estado de conservación".to_owned()];
        let requested = resolve_field(
            &vocabulary,
            &search_terms("¿Cuál es el estado de ABC-123?"),
            &[],
        );
        assert!(matches!(requested, FieldMatch::Resolved(name) if name == "Estado"));
    }

    #[test]
    fn the_entity_type_breaks_a_tie_but_never_resolves_a_field_alone() {
        let vocabulary = vec![
            "Estado de la propiedad".to_owned(),
            "Estado de la orden".to_owned(),
            "Clave de la propiedad".to_owned(),
        ];
        let type_words = vec!["propiedad".to_owned()];

        // "propiedad" desempata: sin ella los dos "Estado de..." empatarían.
        let asked = resolve_field(
            &vocabulary,
            &search_terms("¿Cuál es el estado de la propiedad ABC-123?"),
            &type_words,
        );
        assert!(matches!(asked, FieldMatch::Resolved(name) if name == "Estado de la propiedad"));

        // "color" no coincide con nada; la única coincidencia sería el tipo, y
        // por sí solo no habilita ningún campo.
        let invented = resolve_field(
            &vocabulary,
            &search_terms("¿Cuál es el color de la propiedad ABC-123?"),
            &type_words,
        );
        assert!(matches!(invented, FieldMatch::NotRequested));
    }

    #[test]
    fn only_the_word_touching_the_identifier_counts_as_its_type() {
        // "la propiedad X" nombra el tipo de la entidad y se descarta.
        assert_eq!(
            entity_type_words("¿Cuál es el color de la propiedad PROP-1?", "PROP-1"),
            vec!["propiedad".to_owned()]
        );
        // "la superficie DE X" no: la preposición separa al campo pedido del
        // identificador, así que "superficie" tiene que conservarse.
        assert!(entity_type_words("¿Cuál es la superficie de PROP-1?", "PROP-1").is_empty());
    }

    #[test]
    fn summary_and_relation_intents_need_their_own_signals() {
        assert!(asks_for_summary(&search_terms(
            "Resume la propiedad PROP-1"
        )));
        assert!(asks_for_summary(&search_terms("Dame un resumen de PROP-1")));
        assert!(!asks_for_summary(&search_terms(
            "¿Cuál es el estado de PROP-1?"
        )));

        assert!(asks_for_related_documents(&search_terms(
            "¿Cuáles son todos los documentos relacionados con PROP-1?"
        )));
        // Sin la palabra que nombra al continente es una pregunta por un campo
        // que se llama "algo relacionado", no por el acervo.
        assert!(!asks_for_related_documents(&search_terms(
            "¿Cuál es el inmueble relacionado de FIN-1?"
        )));
    }
}
