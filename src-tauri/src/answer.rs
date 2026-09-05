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
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    sync::LazyLock,
};

use crate::{
    calc::Decimal,
    census,
    error::Result,
    extract::classify_value,
    model::{Evidence, SearchHit, TypedValue, ValueKind},
    normalize::{
        canonical_identifier, normalize_exact, normalize_literal, normalize_spanish, search_terms,
        stems_match,
    },
    tools::{DocumentValue, FieldRole, QuestionFieldRoles, SignRecord, ToolEngine},
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

/// Palabras interrogativas, en los dos idiomas que el resto del archivo ya
/// reconoce. Lo que se usa de ellas es su AUSENCIA: una pregunta que no lleva
/// ninguna es una pregunta de sí/no.
///
/// No es `QUESTION_FILLER` —esa mezcla los interrogativos con otras palabras
/// que tampoco nombran un campo («documento», «valor», «es»)— ni la lista
/// `QUESTION_WORDS` de `tools.rs`, que deja fuera «qué» y «cómo» a propósito
/// porque allí incluir de más marca campos como preguntados por error. Aquí el
/// riesgo corre al revés: incluir de más sólo hace que la regla se abstenga de
/// actuar, así que la lista se toma deliberadamente amplia.
const INTERROGATIVE_WORDS: &[&str] = &[
    "como", "cual", "cuales", "cuando", "cuanta", "cuantas", "cuanto", "cuantos", "donde", "que",
    "quien", "quienes", "how", "what", "when", "where", "which", "who", "whom", "whose", "why",
];

static INTERROGATIVE_ROOTS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    INTERROGATIVE_WORDS
        .iter()
        .map(|word| normalize_spanish(word))
        .collect()
});

/// ¿La pregunta pide el valor de algún campo, o sólo espera un sí o un no?
///
/// Se lee de la forma de la frase, nunca de un vocabulario de negocio: si no
/// aparece ninguna palabra interrogativa, la pregunta no está pidiendo el
/// valor de nada —afirma algo sobre el documento y espera confirmación.
///
/// Se mira la pregunta cruda y no los términos ya filtrados porque
/// `search_terms` descarta «que» como palabra vacía, y con ella se perdería
/// justo uno de los interrogativos que esta lectura necesita ver.
fn asks_for_a_field_value(question: &str) -> bool {
    normalize_spanish(question)
        .split_whitespace()
        .any(|word| INTERROGATIVE_ROOTS.contains(word))
}

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
    /// Ruta del documento del que habla el texto, cuando habla de uno solo.
    ///
    /// No es lo mismo que «el documento de la primera cita»: la síntesis cita
    /// todo lo que la búsqueda encontró —de varios archivos, casi siempre— y sólo
    /// una de esas citas es la que sostiene la afirmación. Este campo nombra
    /// esa, para que la conversación sepa a qué señala «ese documento» en el
    /// turno siguiente. Vacío cuando el texto no habla de un documento
    /// concreto: entonces no hay nada que señalar y no se adivina.
    pub subject: Option<String>,
}

impl Synthesis {
    fn about(mut self, path: &str) -> Self {
        self.subject = Some(path.to_owned());
        self
    }
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
    // Las palabras del propio identificador que el usuario escribió para
    // localizar el documento no pueden nombrar el campo que pregunta. Un folio
    // como «ABC-2024-00063» aporta la palabra «abc», que coincide entera con el
    // campo «ABC» —el que guarda ese mismo folio— y le ganaba por puntuación al
    // campo realmente pedido, cuyo nombre suele traer alguna palabra más que la
    // pregunta no escribe. El resultado era devolverle al usuario, como dato
    // extraído y verificado, el mismo identificador que él acababa de teclear.
    let identifier_words = match &identifier {
        Some((_, text)) => search_terms(text),
        None => Vec::new(),
    };
    // Y el campo por el que se localizó tampoco lo nombra: «cuyo X es ABC-123»
    // escribe «X» para señalar el registro, no para pedirlo.
    let premise = identifier
        .as_ref()
        .map(|(_, text)| LocatorPremise::new(question, text));

    if let Some((canonical, text)) = &identifier {
        // Una pregunta por los documentos relacionados habla del acervo, no de
        // un campo. Se responde con lo que la búsqueda ya encontró, sin leer
        // ningún documento.
        if asks_for_related_documents(question, &terms) {
            return Ok(related_documents_answer(text, hits));
        }
        if hits.len() <= MAX_PRINCIPAL_CANDIDATES {
            let documents = load_documents(tools, hits)?;
            if asks_for_summary(&terms) {
                return Ok(summary_answer(question, &documents, canonical, text, hits));
            }
            // El orden importa: cuando la pregunta pide un campo que la
            // evidencia encontrada no contiene, la síntesis directa
            // respondería con el identificador —el único campo que esos
            // documentos comparten— en lugar del dato pedido.
            if let Some(synthesis) = identified_field_answer(
                tools,
                question,
                &documents,
                &terms,
                &type_words,
                &identifier_words,
                premise.as_ref(),
                canonical,
                text,
                hits,
            )? {
                return Ok(Some(synthesis));
            }
        }
    }
    shared_field_answer(
        tools,
        question,
        &terms,
        &type_words,
        &identifier_words,
        premise.as_ref(),
        hits,
    )
}

// -------------------------------------------------------------------------
// Tipo A: la evidencia encontrada ya es la respuesta, sólo falta redactarla.
// -------------------------------------------------------------------------

/// Qué hacer cuando la pregunta pide, entrecomillado, un campo concreto.
enum QuotedFieldOutcome {
    /// La pregunta no pide ningún campo entrecomillado, o la evidencia
    /// encontrada ya es la de ese campo: la ruta de siempre sigue igual.
    NotApplicable,
    Answered(Synthesis),
    /// Se pide un campo que la evidencia no es. Contestar con lo encontrado
    /// sería devolver otro campo en su lugar, así que esta ruta se corta.
    NotTheFoundField,
}

/// El nombre de campo que la pregunta entrecomilla.
///
/// Dos señales, las dos explícitas: la palabra «campo» o «columna» justo
/// antes de la cita, o una cita que coincide con el nombre de un campo real
/// del acervo. Sin una de las dos, un texto entrecomillado es lo que siempre
/// fue —algo que buscar— y esta ruta no se activa.
fn quoted_field_name(tools: &ToolEngine, question: &str) -> Result<Option<String>> {
    let quoted = ToolEngine::quoted_literals(question);
    if quoted.is_empty() {
        return Ok(None);
    }
    let words = question.split_whitespace().collect::<Vec<_>>();
    let after_keyword = words
        .iter()
        .position(|word| matches!(normalize_exact(word).as_str(), "campo" | "columna"))
        .and_then(|at| words.get(at + 1..).map(|rest| rest.join(" ")))
        .unwrap_or_default();
    let after_keyword = normalize_exact(&after_keyword);
    if let Some(named) = quoted
        .iter()
        .find(|literal| after_keyword.starts_with(&normalize_exact(literal)))
    {
        return Ok(Some(named.clone()));
    }
    let concepts = tools.list_concepts(None)?;
    Ok(quoted
        .into_iter()
        .find(|literal| {
            concepts
                .iter()
                .any(|concept| normalize_exact(&concept.display_name) == normalize_exact(literal))
        }))
}

/// El campo que la pregunta entrecomilla, leído del documento que la búsqueda
/// encontró, cuando la evidencia encontrada NO es ese campo.
///
/// Esto arregla el «eco de folio»: «En el documento con folio INC-2025-00190
/// (…), ¿cuál es el valor del campo "Fecha"?» encontraba el documento por el
/// folio, y como todas las coincidencias eran del campo «INC», el atajo de un
/// solo grupo de `shared_field_answer` las daba por buenas y contestaba «El
/// campo «INC» … es INC-2025-00190»: le devolvía al usuario, presentado como
/// dato extraído y verificado, el mismo folio que él acababa de escribir.
///
/// Cuando el documento sí registra el campo pedido, se contesta con él. Cuando
/// no lo registra —o la evidencia abarca varios documentos y elegir uno sería
/// adivinar— no se contesta nada por esta vía: es preferible el mensaje
/// genérico de «no encontré evidencia» a una respuesta sobre otro campo.
fn quoted_field_in_located_document(
    tools: &ToolEngine,
    question: &str,
    hits: &[SearchHit],
) -> Result<QuotedFieldOutcome> {
    let Some(asked) = quoted_field_name(tools, question)? else {
        return Ok(QuotedFieldOutcome::NotApplicable);
    };
    let already_that_field = hits.iter().any(|hit| {
        field_value(hit)
            .is_some_and(|(field, _)| normalize_exact(field) == normalize_exact(&asked))
    });
    if already_that_field {
        return Ok(QuotedFieldOutcome::NotApplicable);
    }
    let documents = hits
        .iter()
        .map(|hit| hit.evidence.document_id)
        .collect::<BTreeSet<_>>();
    let [document] = documents.into_iter().collect::<Vec<_>>()[..] else {
        return Ok(QuotedFieldOutcome::NotTheFoundField);
    };
    let values = tools.document_values(document)?;
    let matching = values
        .iter()
        .filter(|value| normalize_exact(&value.field) == normalize_exact(&asked))
        .collect::<Vec<_>>();
    let Some(first) = matching.first() else {
        return Ok(QuotedFieldOutcome::NotTheFoundField);
    };
    let field = first.field.clone();
    let mut distinct: Vec<&str> = Vec::new();
    for value in &matching {
        if !distinct
            .iter()
            .any(|seen| normalize_exact(seen) == normalize_exact(&value.value))
        {
            distinct.push(value.value.as_str());
        }
    }
    // Las citas son las de ese campo, no las de la búsqueda: la evidencia que
    // sostiene la respuesta tiene que ser la del dato pedido.
    let citations = matching
        .iter()
        .map(|value| value.evidence.clone())
        .collect::<Vec<_>>();
    let synthesis = if let [value] = distinct.as_slice() {
        reported_value_synthesis(
            tools,
            &field,
            value,
            None,
            &first.evidence,
            &[field.clone(), (*value).to_owned()],
            citations,
        )?
        .map(|synthesis| synthesis.about(&first.evidence.path))
    } else {
        value_list_summary(&field, &distinct, citations)
    };
    Ok(match synthesis {
        Some(synthesis) => QuotedFieldOutcome::Answered(synthesis),
        None => QuotedFieldOutcome::NotTheFoundField,
    })
}

/// ¿Pide la pregunta un campo distinto del único que la evidencia trajo?
///
/// El atajo de un solo grupo existe para las búsquedas que no nombran ningún
/// campo («Encuentra ABC-123»): ahí lo encontrado ES la respuesta. Pero cuando
/// la pregunta sí nombra un campo del acervo y ese campo no es el que la
/// evidencia trae, responder con lo encontrado le devuelve al usuario otro dato
/// en lugar del pedido, y además verificado, porque la cita es real: prueba que
/// el identificador existe, no lo que se preguntó.
///
/// El campo pedido se busca en TODO el acervo, no sólo en lo recuperado: si
/// existe pero no está en este documento, se dice; si no existe, esta ruta no
/// opina y el atajo sigue su curso.
fn asked_for_another_field(
    tools: &ToolEngine,
    question: &str,
    terms: &[String],
    type_words: &[String],
    identifier_words: &[String],
    premise: Option<&LocatorPremise>,
    groups: &BTreeMap<String, Vec<usize>>,
    hits: &[SearchHit],
) -> Result<Option<Synthesis>> {
    let Some(found) = groups
        .values()
        .next()
        .and_then(|group| group.first())
        .and_then(|index| field_value(&hits[*index]))
        .map(|(field, _)| field.to_owned())
    else {
        return Ok(None);
    };
    let catalogue = tools
        .list_concepts(None)?
        .into_iter()
        .map(|concept| concept.display_name)
        .collect::<Vec<_>>();
    let asked = match explicitly_quoted_field(question, &catalogue)
        .map(FieldMatch::Resolved)
        .unwrap_or_else(|| {
            resolve_field(
                question,
                &catalogue,
                terms,
                type_words,
                identifier_words,
                premise,
            )
        })
    {
        FieldMatch::Resolved(name) => name,
        FieldMatch::NotRequested | FieldMatch::Ambiguous => return Ok(None),
    };
    if normalize_exact(&asked) == normalize_exact(&found) {
        return Ok(None);
    }
    let citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    let file = hits
        .first()
        .map(|hit| file_name(&hit.evidence))
        .unwrap_or_default();
    // Sin literales que comprobar: esta frase no afirma ningún valor extraído
    // —dice justamente que el dato no está—, así que el candado de literalidad
    // no tiene nada que validar. Pasarle los nombres de los dos campos la
    // descartaba siempre, porque el campo AUSENTE no aparece por definición en
    // la evidencia citada, y al descartarse volvía a colarse el eco del folio.
    Ok(unresolved(
        format!(
            "Sin concluir: lo que encontré de {file} es su «{found}», no «{asked}». El acervo tiene un campo «{asked}», pero este documento no lo registra, así que no puedo darte ese dato sin inventarlo."
        ),
        &[],
        citations,
    ))
}

fn shared_field_answer(
    tools: &ToolEngine,
    question: &str,
    terms: &[String],
    type_words: &[String],
    identifier_words: &[String],
    premise: Option<&LocatorPremise>,
    hits: &[SearchHit],
) -> Result<Option<Synthesis>> {
    match quoted_field_in_located_document(tools, question, hits)? {
        QuotedFieldOutcome::Answered(synthesis) => return Ok(Some(synthesis)),
        QuotedFieldOutcome::NotTheFoundField => return Ok(None),
        QuotedFieldOutcome::NotApplicable => {}
    }
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
        // El atajo vale mientras la pregunta no pida otra cosa. Si nombra un
        // campo del acervo que NO es éste, contestar con el único encontrado
        // sería responder otro campo en su lugar —y presentarlo verificado—,
        // que es justo lo que la evidencia no sostiene.
        if let Some(synthesis) = asked_for_another_field(
            tools,
            question,
            terms,
            type_words,
            identifier_words,
            premise,
            &groups,
            hits,
        )? {
            return Ok(Some(synthesis));
        }
        let Some(members) = groups.into_values().next() else {
            return Ok(None);
        };
        members
    } else {
        let vocabulary = groups
            .values()
            .filter_map(|group| field_value(&hits[group[0]]).map(|(field, _)| field.to_owned()))
            .collect::<Vec<_>>();
        let resolved = match explicitly_quoted_field(question, &vocabulary)
            .map(FieldMatch::Resolved)
            .unwrap_or_else(|| {
                resolve_field(
                    question,
                    &vocabulary,
                    terms,
                    type_words,
                    identifier_words,
                    premise,
                )
            }) {
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
    Ok(reported_value_synthesis(
        tools,
        field,
        value,
        identifier.as_ref().map(|item| item.value.as_str()),
        &hit.evidence,
        &literals,
        citations,
    )?
    .map(|synthesis| synthesis.about(&hit.evidence.path)))
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
        // El total se suma en escala fija sobre el literal original, nunca en
        // `f64`: es una cifra visible y debe ser exacta y reproducible. Si
        // algún literal no se puede leer exactamente, no se publica un total
        // aproximado — se muestra sólo el rango, que sí está respaldado.
        let exact_total = summable
            .then(|| {
                group.iter().try_fold(Decimal::ZERO, |accumulated, item| {
                    Decimal::from_text(item.1).map(|value| accumulated.add(value))
                })
            })
            .flatten();
        let total = exact_total.map(|sum| {
            let samples = group.iter().map(|item| item.1).collect::<Vec<_>>();
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
    supported(text, &literals, citations)
}

fn value_list_summary(field: &str, values: &[&str], citations: Vec<Evidence>) -> Option<Synthesis> {
    let mut seen = HashSet::new();
    let mut distinct = Vec::new();
    for value in values {
        // `normalize_exact` borra puntuación: «3.2» y «3:2», o «50» y «50%»,
        // normalizan igual sin ser el mismo valor. `normalize_literal` sólo
        // pliega mayúsculas y acentos, así que un valor distinto nunca
        // desaparece del listado sólo porque comparta dígitos con otro.
        if seen.insert(normalize_literal(value)) {
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
        return supported(text, &literals, citations);
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
    supported(text, &literals, citations)
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
///
/// Las dos señales no se leen del mismo texto: la relación sólo cuenta si se
/// enuncia FUERA de las comillas. «En el documento con folio X, ¿cuál es el
/// valor del campo "Proveedor relacionado"?» nombra el continente y contiene
/// "relacionado", pero esa palabra es parte del nombre del campo pedido, no
/// una petición de documentos emparentados; leerla como tal detenía la
/// respuesta en la localización del documento sin llegar a extraer el campo.
fn asks_for_related_documents(question: &str, terms: &[String]) -> bool {
    search_terms(&ToolEngine::query_without_quoted_literals(question))
        .iter()
        .any(|term| term.starts_with("relacionad"))
        && terms.iter().any(|term| CONTAINER_ROOTS.contains(term))
}

fn summary_answer(
    question: &str,
    documents: &[DocumentContext],
    identifier: &str,
    identifier_text: &str,
    hits: &[SearchHit],
) -> Option<Synthesis> {
    let principal = principal_document(documents, identifier, question)?;
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
    supported(text, &literals, citations)
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
    supported(text, &literals, citations)
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
    tools: &ToolEngine,
    question: &str,
    documents: &[DocumentContext],
    terms: &[String],
    type_words: &[String],
    identifier_words: &[String],
    premise: Option<&LocatorPremise>,
    identifier: &str,
    identifier_text: &str,
    hits: &[SearchHit],
) -> Result<Option<Synthesis>> {
    // El vocabulario se limita a los documentos ya encontrados. Compararlo
    // contra el índice completo haría que una palabra suelta abriera campos de
    // otros documentos que la búsqueda nunca devolvió.
    let vocabulary = distinct_fields(documents.iter().flat_map(|context| context.values.iter()));
    let shared_field = match explicitly_quoted_field(question, &vocabulary)
        .map(FieldMatch::Resolved)
        .unwrap_or_else(|| {
            resolve_field(
                question,
                &vocabulary,
                terms,
                type_words,
                identifier_words,
                premise,
            )
        })
    {
        // La pregunta no nombra ningún campo del acervo. Antes de rendirse:
        // puede que lo entrecomillado no sea un campo sino la ETIQUETA de una
        // fila de tabla, que es como se nombra una partida, un arancel o un
        // artículo de una lista de precios. Si lo es, se contesta con su fila.
        FieldMatch::NotRequested => return Ok(labelled_row_answer(question, documents, hits)),
        // Un empate en el vocabulario común, antes de elegir documento
        // principal, casi siempre significa que la palabra coincidente aparece
        // por casualidad en varios nombres de campo de documentos distintos, no
        // que se pida un campo concreto. No es una duda que reportar: es una
        // pregunta que no era de campo — salvo que nombre una fila.
        FieldMatch::Ambiguous => return Ok(labelled_row_answer(question, documents, hits)),
        FieldMatch::Resolved(name) => {
            // El campo pedido ya está entre la evidencia encontrada: no hace
            // falta ninguna consulta adicional, la síntesis directa lo responde.
            if hits.iter().all(|hit| {
                hit.evidence
                    .field
                    .as_deref()
                    .is_some_and(|field| normalize_exact(field) == normalize_exact(&name))
            }) {
                return Ok(None);
            }
            name
        }
    };

    let citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    let Some(principal) = principal_document(documents, identifier, question) else {
        // Ningún documento se distingue, pero puede que la elección no
        // importe: si todos coinciden en el valor del campo pedido, ese valor
        // es la respuesta y no hace falta elegir documento.
        if let Some(agreed) = value_agreed_by_every_document(documents, &shared_field) {
            let literals = vec![
                agreed.field.clone(),
                agreed.value.clone(),
                identifier_text.to_owned(),
            ];
            let mut text = format!(
                "{} Los {} documentos que mencionan {identifier_text} coinciden en ese valor, así que no depende de cuál se tome como registro principal.",
                direct_phrase(
                    &agreed.field,
                    &agreed.value,
                    Some(identifier_text),
                    &agreed.evidence,
                ),
                documents.len(),
            );
            let mut citations = citations;
            citations.insert(0, agreed.evidence.clone());
            // Que todos coincidan no vuelve bueno a un valor cuyo signo el
            // propio campo desmiente: coincidir en un dato inválido es
            // repetirlo, no confirmarlo.
            let record = anomalous_negative(tools, &agreed.field, &agreed.value)?;
            if let Some(record) = record {
                text.push_str(&sign_warning(&agreed.field, record));
            }
            let synthesis = match record {
                Some(_) => unresolved(text, &literals, citations),
                None => supported(text, &literals, citations),
            };
            return Ok(synthesis.map(|synthesis| synthesis.about(&agreed.evidence.path)));
        }
        return Ok(unresolved(
            format!(
                "Sin concluir: {} documentos mencionan {identifier_text}, pero ninguno se distingue como el registro principal, así que no puedo atribuirle ese dato con certeza.",
                hits.len()
            ),
            &[identifier_text.to_owned()],
            citations,
        ));
    };
    let file = file_name(&principal.hit.evidence);

    // Lección del corpus real: "precio" empata entre campos de documentos
    // distintos. El campo se resuelve dentro del documento principal ya
    // elegido, nunca contra el vocabulario común de todos los encontrados. Aquí
    // una duda sí es real y se reporta.
    let principal_vocabulary = distinct_fields(principal.values.iter());
    let FieldMatch::Resolved(field) = explicitly_quoted_field(question, &principal_vocabulary)
        .map(FieldMatch::Resolved)
        .unwrap_or_else(|| {
            resolve_field(
                question,
                &principal_vocabulary,
                terms,
                type_words,
                identifier_words,
                premise,
            )
        })
    else {
        return Ok(unresolved(
            format!(
                "Sin concluir: {file} es el registro principal de {identifier_text}, pero no puedo determinar con certeza a qué campo suyo se refiere la pregunta."
            ),
            &[identifier_text.to_owned()],
            citations,
        ));
    };

    let matches = principal
        .values
        .iter()
        .filter(|value| normalize_exact(&value.field) == normalize_exact(&field))
        .collect::<Vec<_>>();
    // Igualdad literal, no `normalize_exact`: dos valores del mismo campo que
    // sólo difieren en puntuación («3.2» frente a «3:2») son valores en
    // conflicto, no el mismo valor escrito dos veces. Tratarlos como iguales
    // aquí escondería el conflicto y el motor elegiría uno de los dos al
    // azar (`[chosen]` más abajo) como si no hubiera ambigüedad.
    let distinct = matches
        .iter()
        .map(|value| normalize_literal(&value.value))
        .collect::<HashSet<_>>();
    // Varios valores del mismo campo en un solo documento significan varios
    // registros dentro de él (un listado): no es posible atribuir uno de ellos
    // a este identificador sin cruzar la fila, así que no se responde.
    let ([chosen], 1) = (matches.as_slice(), distinct.len()) else {
        return Ok(unresolved(
            format!(
                "Sin concluir: {file} registra {} de «{field}», así que no puedo señalar cuál corresponde a {identifier_text}.",
                counted(distinct.len(), "valor distinto", "valores distintos")
            ),
            &[identifier_text.to_owned(), field],
            citations,
        ));
    };

    let mut citations = citations;
    let literals = vec![
        chosen.field.clone(),
        chosen.value.clone(),
        identifier_text.to_owned(),
    ];
    // La evidencia decisiva encabeza las citas; debajo se conserva intacto lo
    // que la búsqueda encontró por su cuenta.
    citations.insert(0, chosen.evidence.clone());
    Ok(reported_value_synthesis(
        tools,
        &chosen.field,
        &chosen.value,
        Some(identifier_text),
        &chosen.evidence,
        &literals,
        citations,
    )?
    .map(|synthesis| synthesis.about(&chosen.evidence.path)))
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

/// Una fila de tabla nombrada por su primera celda.
///
/// Una tabla con encabezado se indexa por sus encabezados, que es la lectura
/// correcta: la fila `Tornillo hexagonal | 250 | piezas` bajo
/// `Concepto | Cantidad | Unidad` queda como Concepto=«Tornillo hexagonal»,
/// Cantidad=250, Unidad=piezas. Pero quien pregunta no nombra la columna:
/// nombra la fila por su etiqueta —«¿cuál es el valor de "Tornillo
/// hexagonal"?»—. Las dos formas señalan la misma casilla del mismo papel, y
/// hasta ahora sólo se entendía una.
///
/// Nada de esto depende del giro del negocio ni de cómo se llamen las
/// columnas: la regla se apoya sólo en el orden de extracción y en que una
/// columna etiqueta se repite una vez por fila. Funciona igual en una tabla
/// de partidas, en un arancel de notaría o en una lista de precios de
/// ferretería.
struct LabelledRow<'a> {
    /// La celda que la pregunta nombró.
    label: &'a DocumentValue,
    /// El resto de las celdas de su fila, en orden.
    cells: Vec<&'a DocumentValue>,
}

/// Localiza la fila que una etiqueta encabeza, o no devuelve nada.
///
/// Las tres exigencias son las que impiden adivinar, y cada una descarta un
/// modo distinto de equivocarse:
///
///  1. **La etiqueta aparece una sola vez** en todo el conjunto de documentos
///     candidatos. Si encabeza varias filas —una fecha que se repite en el
///     turno matutino, el vespertino y el nocturno del mismo día— hay varias
///     respuestas posibles y ninguna razón para preferir una.
///  2. **Su columna se repite** dentro del documento. Una columna que sólo
///     aparece una vez no es la columna etiqueta de una tabla: es un campo de
///     una carátula, y su valor no encabeza ninguna fila.
///  3. **La fila termina donde vuelve a aparecer esa misma columna**, que es
///     donde empieza la fila siguiente. No hace falta leer la posición del
///     texto —que cada formato escribe a su manera— ni conocer el ancho de la
///     tabla.
fn row_labelled_by<'a>(
    documents: &'a [DocumentContext<'a>],
    label: &str,
) -> Option<LabelledRow<'a>> {
    let wanted = normalize_exact(label);
    if wanted.is_empty() {
        return None;
    }
    let mut found: Option<(&'a DocumentContext<'a>, usize)> = None;
    for context in documents {
        for (at, value) in context.values.iter().enumerate() {
            if normalize_exact(&value.value) == wanted {
                if found.is_some() {
                    // Aparece más de una vez: hay más de una fila candidata.
                    return None;
                }
                found = Some((context, at));
            }
        }
    }
    let (context, at) = found?;
    let anchor = context.values.get(at)?;
    let column = normalize_exact(&anchor.field);
    let repeats = context
        .values
        .iter()
        .filter(|value| normalize_exact(&value.field) == column)
        .count();
    if repeats < 2 {
        return None;
    }
    let cells = context
        .values
        .get(at + 1..)?
        .iter()
        .take_while(|value| normalize_exact(&value.field) != column)
        .collect::<Vec<_>>();
    if cells.is_empty() {
        return None;
    }
    // 4. La columna etiqueta tiene que ser la PRIMERA de su tabla, o la fila
    //    que se recoge no es una fila.
    //
    //    Cortar «hasta la siguiente aparición de la columna» sólo delimita una
    //    fila real si la etiqueta la encabeza. Si la columna fuera la tercera
    //    de cinco, lo recogido serían las columnas 4 y 5 de esta fila y las 1 y
    //    2 de la SIGUIENTE, presentadas como si fueran una sola — un error que
    //    ninguna tabla de este acervo produce, pero que otra sí produciría.
    //
    //    Se detecta sin leer posiciones: si la columna etiqueta encabeza la
    //    tabla, lo que hay justo antes de su primera aparición pertenece a otra
    //    cosa (una carátula, otra tabla) y su nombre no puede estar entre las
    //    celdas recogidas. Si la columna fuera interior, ahí estaría justamente
    //    una de las columnas que la preceden en cada fila.
    let first = context
        .values
        .iter()
        .position(|value| normalize_exact(&value.field) == column)?;
    if let Some(before) = first.checked_sub(1).and_then(|at| context.values.get(at)) {
        let preceding = normalize_exact(&before.field);
        if cells
            .iter()
            .any(|cell| normalize_exact(&cell.field) == preceding)
        {
            return None;
        }
    }
    Some(LabelledRow {
        label: anchor,
        cells,
    })
}

/// Contesta nombrando la fila entera, sin elegir columna por el usuario.
///
/// La pregunta pide «el valor» en singular, pero una fila puede tener varias
/// celdas y nada en la pregunta dice cuál. Devolverlas todas, cada una con el
/// nombre de su columna, contesta sin suponer: si la tabla es de dos columnas
/// —el caso más común de un arancel o una lista de precios— queda una sola
/// celda y la respuesta es exactamente el valor pedido.
///
/// El texto dice además que lo pedido no era un campo sino una fila. Quien
/// pregunta tiene derecho a saber que su forma de nombrar la casilla no es la
/// que el documento usa.
fn labelled_row_answer(
    question: &str,
    documents: &[DocumentContext],
    hits: &[SearchHit],
) -> Option<Synthesis> {
    let quoted = ToolEngine::quoted_literals(question);
    let [asked] = quoted.as_slice() else {
        return None;
    };
    let row = row_labelled_by(documents, asked)?;
    let file = file_name(&row.label.evidence);
    let shown = row.cells.iter().take(MAX_SUMMARY_FIELDS).collect::<Vec<_>>();
    let remaining = row.cells.len().saturating_sub(shown.len());

    let mut literals = vec![row.label.value.clone(), row.label.field.clone()];
    let mut citations = hits
        .iter()
        .map(|hit| hit.evidence.clone())
        .collect::<Vec<_>>();
    citations.insert(0, row.label.evidence.clone());
    for cell in &shown {
        literals.push(cell.field.clone());
        literals.push(cell.value.clone());
        if !citations.iter().any(|item| item.id == cell.evidence.id) {
            citations.push(cell.evidence.clone());
        }
    }
    let items = shown
        .iter()
        .map(|cell| format!("{}: {}", cell.field, cell.value))
        .collect::<Vec<_>>();
    let mut text = format!(
        "«{}» no es un campo de {file}, sino la etiqueta de una fila de su tabla (columna «{}»). Esa fila registra:\n\n{}",
        row.label.value,
        row.label.field,
        bullet_list(&items)
    );
    if remaining > 0 {
        text.push_str(&format!(
            "\n\nLa fila tiene {} más.",
            counted(remaining, "celda", "celdas")
        ));
    }
    supported(text, &literals, citations)
        .map(|synthesis| synthesis.about(&row.label.evidence.path))
}

/// La misma vía de fila etiquetada, para la ruta que localiza el documento por
/// su **clave interna de indexación** (`D#####`) en vez de por un folio escrito
/// en la pregunta.
///
/// La ronda 8 construyó esta capacidad dentro de `identified_field_answer`, que
/// es la ruta del folio, y allí se quedó: preguntar «¿cuál es el valor de
/// "Lubricantes - insumo #2" en el documento D05975?» no la alcanzaba, aunque
/// es exactamente la misma pregunta sobre exactamente la misma casilla. La
/// asimetría no la producía ninguna diferencia real entre las dos rutas —sólo
/// que una se escribió después—.
///
/// Se reutiliza `row_labelled_by` sin tocarla, con sus cuatro exigencias
/// intactas: etiqueta única, columna repetida, corte en la siguiente aparición
/// de esa columna, y columna etiqueta primera de su tabla. Una segunda puerta
/// con garantías propias sería una segunda forma de equivocarse.
pub fn labelled_row_in_document(question: &str, values: &[DocumentValue]) -> Option<Synthesis> {
    let hits = values
        .iter()
        .map(|value| SearchHit {
            title: value.field.clone(),
            score: 1.0,
            evidence: value.evidence.clone(),
        })
        .collect::<Vec<_>>();
    let [hit, ..] = hits.as_slice() else {
        return None;
    };
    let documents = vec![DocumentContext {
        hit,
        values: values.to_vec(),
    }];
    labelled_row_answer(question, &documents, &hits)
}

/// Documento principal de un identificador. Se prefiere aquel donde el
/// identificador aparece antes dentro del propio documento —un registro habla
/// de su entidad desde sus primeros campos, mientras que una referencia cruzada
/// aparece más abajo— y, a igualdad de posición, el que contiene menos
/// identificadores distintos: un listado menciona muchos, una ficha individual
/// casi ninguno.
///
/// Cuando ni el orden de aparición ni el número de identificadores separan a
/// los candidatos —dos carátulas que registran el mismo folio en la misma
/// posición— queda una última señal, y sólo una: el tipo de documento que la
/// pregunta nombra (`document_named_by_kind`). Si tampoco ésa desempata, no
/// hay documento principal y no se contesta.
fn principal_document<'a>(
    documents: &'a [DocumentContext<'a>],
    identifier: &str,
    question: &str,
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
        // Empate en la cabeza: se desempata sólo entre los empatados, nunca
        // contra un candidato que el orden de aparición ya dejó atrás.
        [(best, _), ..] => {
            let tied = ranked
                .iter()
                .filter(|(rank, _)| rank == best)
                .map(|(_, context)| *context)
                .collect::<Vec<_>>();
            document_named_by_kind(&tied, question)
        }
        [] => None,
    }
}

/// Entre documentos que el orden de aparición no separa, el que es del tipo
/// que la pregunta nombra.
///
/// El tipo no sale de ninguna lista escrita en el código: se lee del nombre
/// del propio archivo con `census::kind_of_path`, el mismo criterio con el
/// que el censo reparte el acervo por tipo. Así la señal existe para
/// cualquier acervo, sin conocer de antemano qué tipos contiene.
///
/// La exigencia es simétrica y en los dos sentidos, como en
/// `origin_identified_by_value`: hace falta que la pregunta nombre el tipo de
/// **exactamente uno** de los empatados. Si no nombra el de ninguno, no hay
/// señal; si nombra el de varios, la señal no distingue. En ambos casos el
/// empate sigue siendo un empate y la respuesta correcta es no concluir.
fn document_named_by_kind<'a>(
    tied: &[&'a DocumentContext<'a>],
    question: &str,
) -> Option<&'a DocumentContext<'a>> {
    let asked = normalize_spanish(question);
    let words = asked.split_whitespace().collect::<Vec<_>>();
    let mut named = tied
        .iter()
        .filter(|context| question_names_kind(&words, &context.hit.evidence.path));
    let first = *named.next()?;
    named.next().is_none().then_some(first)
}

/// ¿Nombra la pregunta el tipo de este archivo?
///
/// El tipo puede ser de varias palabras («orden_mantenimiento»), así que se
/// busca como secuencia de palabras dentro de la pregunta ya normalizada, no
/// como subcadena: sin límites de palabra, el tipo «pago» coincidiría dentro
/// de «pagos anticipados» o de cualquier palabra que lo contenga.
fn question_names_kind(question_words: &[&str], path: &str) -> bool {
    let Some(kind) = census::kind_of_path(path) else {
        return false;
    };
    let normalized = normalize_spanish(&kind.replace('_', " "));
    let length = normalized.split_whitespace().count();
    if length == 0 || length > question_words.len() {
        return false;
    }
    question_words
        .windows(length)
        .any(|window| census::kind_matches(&kind, &window.join(" ")))
}

/// Valor que no depende de qué documento se elija.
///
/// Cuando varios documentos mencionan el mismo identificador y ninguno se
/// distingue como el principal, todavía se puede contestar sin elegir: si
/// **todos** registran el campo pedido y **todos** registran para él el mismo
/// valor, la respuesta es la misma se mire el documento que se mire, y elegir
/// deja de hacer falta.
///
/// No es una mayoría. Una sola discrepancia —o un solo documento que no
/// registre el campo— cancela la vía: lo que autoriza a contestar es la
/// ausencia de desacuerdo comprobada en todos los candidatos, no que la mayor
/// parte coincida. Un documento con varios valores del mismo campo tampoco
/// cuenta, por la misma razón que más abajo: son varios registros dentro de
/// él y no se puede atribuir uno al identificador sin cruzar la fila.
fn value_agreed_by_every_document<'a>(
    documents: &'a [DocumentContext<'a>],
    field: &str,
) -> Option<&'a DocumentValue> {
    let mut agreed: Option<&DocumentValue> = None;
    for context in documents {
        let mut matches = context
            .values
            .iter()
            .filter(|value| normalize_exact(&value.field) == normalize_exact(field));
        let only = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        match agreed {
            None => agreed = Some(only),
            Some(previous) if normalize_literal(&previous.value) == normalize_literal(&only.value) => {}
            Some(_) => return None,
        }
    }
    agreed
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
/// ¿La pregunta nombra alguno de los campos que este documento registra?
///
/// Localizar un documento no autoriza a contestar cualquier cosa sobre él. Si
/// la pregunta pide un campo que el documento no tiene —el caso típico de una
/// pregunta por un dato ausente— la respuesta correcta es «no encontré
/// evidencia», nunca el valor de otro campo que sí estaba a mano.
pub fn question_names_a_field(question: &str, vocabulary: &[String]) -> bool {
    if explicitly_quoted_field(question, vocabulary).is_some() {
        return true;
    }
    let terms = search_terms(question);
    matches!(
        resolve_field(
            question,
            vocabulary,
            &terms,
            &[],
            &identifier_terms_in(question),
            None
        ),
        FieldMatch::Resolved(_)
    )
}

/// Las palabras que aporta un identificador escrito en la pregunta.
///
/// Se reconoce con el mismo criterio que la recuperación (`canonical_identifier`:
/// letras **y** dígitos), así que una palabra normal o un número suelto nunca
/// entran aquí. Sirven para no dejar que el folio que el usuario tuvo que
/// teclear para localizar el documento cuente como el nombre del campo que
/// pregunta.
fn identifier_terms_in(question: &str) -> Vec<String> {
    question
        .split_whitespace()
        .map(|word| word.trim_matches(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .filter(|word| canonical_identifier(word).is_some())
        .flat_map(search_terms)
        .collect()
}

/// Campo nombrado explícitamente entre comillas por la pregunta.
///
/// Cuando el usuario escribe «el valor del campo "Cliente relacionado"» está
/// nombrando el campo, no describiéndolo: eso manda sobre cualquier palabra
/// suelta del resto de la frase. Sin esta preferencia, el contexto que
/// acompaña a la pregunta —«(orden_compra, área Compras, **proveedores**,
/// logística)»— empataba «Cliente relacionado» con «Proveedor relacionado» y
/// la resolución quedaba ambigua, respondiendo con un campo que nadie pidió.
fn explicitly_quoted_field(question: &str, vocabulary: &[String]) -> Option<String> {
    let quoted = ToolEngine::quoted_literals(question);
    vocabulary
        .iter()
        .find(|name| {
            quoted
                .iter()
                .any(|literal| normalize_exact(literal) == normalize_exact(name))
        })
        .cloned()
}

/// Lo que la pregunta pide de un documento ya fijado, cuando lo nombra por su
/// categoría y no por el nombre del campo.
#[derive(Clone, Debug)]
pub enum FieldRequest {
    /// La pregunta no nombra ninguna categoría, o el documento no registra
    /// ningún valor de la que nombra.
    NotRequested,
    Resolved(String),
    /// Más de un campo del documento podría responderla. No se elige: quien
    /// recibe esto pregunta.
    Ambiguous(Vec<String>),
}

/// Categorías de valor que la palabra interrogativa de la pregunta nombra.
///
/// Es gramática del español —clase cerrada, sin vocabulario de ningún giro de
/// negocio— traducida a la taxonomía que el esquema ya distingue en
/// `concepts.value_type`. Ese puente hace falta porque «cuándo» no comparte
/// ninguna raíz con «Fecha» ni «cuánto» con «Importe»: emparejar por raíz
/// léxica, que es lo único que hace `resolve_field`, no puede resolverlas.
///
/// No hay categoría de persona ni de lugar en el esquema, así que «quién» no
/// se traduce a ninguna: se resuelve aparte, entre los valores que la capa de
/// entidades ya marcó como nombres.
fn asked_value_categories(question: &str) -> Vec<&'static str> {
    let mut categories = Vec::new();
    if question_says(question, &["cuando"]) {
        categories.push("date");
    }
    if question_says(question, &["cuanto", "cuanta", "cuantos", "cuantas"]) {
        categories.extend(["money", "number", "percentage"]);
    }
    categories
}

fn asks_who(question: &str) -> bool {
    question_says(question, &["quien", "quienes"])
}

fn question_says(question: &str, words: &[&str]) -> bool {
    normalize_exact(question)
        .split_whitespace()
        .any(|word| words.contains(&word))
}

/// ¿La pregunta nombra una categoría de valor en vez de un campo?
///
/// Lo consulta el planificador para saber que una continuación como «y en esa
/// minuta, ¿cuándo se registró?» sí está pidiendo un dato del documento del
/// que se hablaba, aunque no escriba el nombre de ningún campo.
pub fn question_names_a_value_category(question: &str) -> bool {
    !asked_value_categories(question).is_empty() || asks_who(question)
}

/// Campo de ESTE documento que responde a la categoría que nombra la pregunta.
///
/// Se resuelve dentro del documento ya fijado, nunca contra el acervo: la
/// categoría («una fecha») sólo identifica un campo si este documento registra
/// exactamente uno de ella. Con dos o más no se elige ninguno.
pub fn field_asked_by_category(question: &str, values: &[DocumentValue]) -> FieldRequest {
    let categories = asked_value_categories(question);
    let mut candidates = if !categories.is_empty() {
        distinct_fields(
            values
                .iter()
                .filter(|value| categories.contains(&value.value_type.as_str())),
        )
    } else if asks_who(question) {
        // «Quién» se resuelve entre los valores que la capa de entidades ya
        // reconoció como nombres, descartando los que otra pista de la
        // pregunta ya reclamó: el valor que la pregunta escribe para localizar
        // el documento no puede ser, además, el que pregunta.
        distinct_fields(
            values
                .iter()
                .filter(|value| value.is_entity && !ToolEngine::value_named_by(question, &value.value)),
        )
    } else {
        Vec::new()
    };
    match candidates.len() {
        0 => FieldRequest::NotRequested,
        1 => FieldRequest::Resolved(candidates.remove(0)),
        _ => FieldRequest::Ambiguous(candidates),
    }
}

/// El campo que la pregunta nombra POR COMPLETO: todas las palabras
/// significativas de su nombre están escritas en la pregunta.
///
/// Es la forma fuerte de `resolve_field`, que se conforma con una coincidencia
/// parcial. La distinción importa porque una coincidencia parcial puede ser
/// accidental —«minuta de **ventas**» toca «Meta de **ventas**» sin pedirlo— y
/// no debe ganarle a una pregunta que dice explícitamente qué categoría busca.
/// Con el nombre completo escrito no hay accidente posible, y manda.
pub fn field_named_in_full(question: &str, vocabulary: &[String]) -> Option<String> {
    let terms = search_terms(question);
    let identifier_words = identifier_terms_in(question);
    vocabulary
        .iter()
        .filter(|name| {
            let field_terms = search_terms(name);
            !field_terms.is_empty()
                && field_terms.iter().any(|term| {
                    !FILLER_ROOTS.contains(term) && !identifier_words.contains(term)
                })
                && field_terms.iter().all(|field_term| {
                    terms
                        .iter()
                        .any(|query_term| stems_match(query_term, field_term))
                })
        })
        .max_by_key(|name| search_terms(name).len())
        .cloned()
}

/// La premisa que localizó el documento: el identificador que el usuario
/// escribió, más la lectura gramatical de la pregunta que dice qué papel juega
/// cada nombre de campo respecto de él.
///
/// `resolve_field` puntúa un campo por cuántas palabras suyas están escritas en
/// la pregunta, y ya descuenta las que sólo nombran el TIPO de la entidad y las
/// del VALOR del identificador. Faltaba el tercer caso: la palabra que nombra
/// al campo POR EL QUE se localizó el documento («…cuyo X es ABC-123»). Esa
/// palabra está en la pregunta para señalar el registro, igual que el propio
/// identificador, no para pedir un dato. Sin descontarla, el campo localizador
/// empataba con el pedido y el motor terminaba devolviendo, sellado como dato
/// extraído, el mismo identificador que el usuario acababa de teclear.
struct LocatorPremise {
    roles: QuestionFieldRoles,
    identifier_text: String,
}

impl LocatorPremise {
    fn new(question: &str, identifier_text: &str) -> Self {
        Self {
            roles: QuestionFieldRoles::new(question),
            identifier_text: identifier_text.to_owned(),
        }
    }

    /// Este nombre de campo sólo sirve para señalar el documento.
    ///
    /// Dos condiciones de forma, ninguna de vocabulario:
    ///
    /// * el campo está escrito PEGADO al identificador —con nada entre medio
    ///   salvo cópulas y artículos—, que es exactamente lo que
    ///   `QuestionFieldRoles` ya llama condición; y
    /// * ninguna palabra interrogativa alcanza a ese campo.
    ///
    /// La segunda condición es la que mantiene viva «¿cuál es el estado de
    /// ABC-123?»: ahí «estado» también queda pegado al identificador (sólo
    /// «de» en medio), pero «cuál» lo señala, así que se pide, no localiza.
    /// El papel de campo preguntado se consulta con un valor vacío a
    /// propósito: sin valor que emparejar no hay condición posible, y `role`
    /// devuelve entonces la marca interrogativa sola.
    fn only_locates(&self, field: &str) -> bool {
        self.roles.role(field, &self.identifier_text) == FieldRole::Restriction
            && self.roles.role(field, "") != FieldRole::Asked
    }
}

fn resolve_field(
    question: &str,
    vocabulary: &[String],
    terms: &[String],
    type_words: &[String],
    identifier_words: &[String],
    premise: Option<&LocatorPremise>,
) -> FieldMatch {
    let asks_for_a_value = asks_for_a_field_value(question);
    let mut best: Option<(usize, usize, String)> = None;
    let mut tied = false;
    for name in vocabulary {
        // Un campo que la pregunta sólo escribe para señalar el documento no
        // compite por ser el campo pedido, aunque sus palabras coincidan.
        if premise.is_some_and(|premise| premise.only_locates(name)) {
            continue;
        }
        let field_terms = search_terms(name);
        let has_significant_term = field_terms.iter().any(|term| !FILLER_ROOTS.contains(term));
        if !has_significant_term {
            continue;
        }
        let matched = field_terms
            .iter()
            .filter(|term| terms.iter().any(|query_term| stems_match(query_term, term)))
            .count();
        // Un campo sólo está pedido si algo que la pregunta escribió POR SU
        // CUENTA lo nombra. Las palabras del identificador citado no cuentan,
        // igual que no cuentan las que sólo nombran el tipo de la entidad ni
        // las de relleno: están en la pregunta porque hacían falta para
        // localizar el documento, no porque describan el dato que se busca.
        let has_real_match = field_terms.iter().any(|term| {
            terms.iter().any(|query_term| stems_match(query_term, term))
                && !type_words.contains(term)
                && !identifier_words.contains(term)
                && !FILLER_ROOTS.contains(term)
        });
        if matched == 0 || !has_real_match {
            continue;
        }
        let unmatched = field_terms.len() - matched;
        // Un nombre de varias palabras queda nombrado cuando la pregunta
        // escribe el nombre entero o, al menos, la palabra que lo encabeza: en
        // español el sintagma se lee por su primera palabra y las de detrás la
        // especifican («Forma de X» es una forma; «X de Y» sigue siendo X).
        // Si de un nombre compuesto sólo calzó alguna de las palabras de
        // detrás, la primera no, y encima la pregunta no pide el valor de nada
        // —no lleva ninguna interrogativa—, lo que hubo fue un choque de
        // raíces entre el verbo de la pregunta y una palabra suelta del
        // nombre, no una petición de ese campo. Sin ninguna interrogativa que
        // respalde la lectura, esa coincidencia parcial no basta para
        // adjudicar el campo, y quedarse sin candidato es la salida honesta.
        let head_matched = field_terms
            .first()
            .is_some_and(|head| terms.iter().any(|term| stems_match(term, head)));
        if !asks_for_a_value && unmatched > 0 && !head_matched {
            continue;
        }
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
            // Una fecha escrita en palabras («26 de enero de 2023») mezcla
            // letras y dígitos, así que `canonical_identifier` la acepta como
            // clave. No lo es: presentarla como el identificador del documento
            // hacía leer «El campo «Moneda» de 26 de enero de 2023 es USD»
            // como si la fecha nombrara al registro. El tipo del valor ya
            // distingue las dos cosas y aquí basta con respetarlo.
            item.value_type != "date"
                && item.identifier_canonical.is_some()
                // Mismo defecto que la fecha en palabras, y por el mismo
                // mecanismo: `canonical_identifier` acepta cualquier mezcla de
                // letras y dígitos, así que una cantidad con unidad («318 kg»),
                // un rango de fechas («2026-03-12 a 2027-03-12») o el nombre de
                // un modelo («Azimut 55 Flybridge») quedan marcados como clave
                // y podían acabar nombrando al registro: «El campo «Folio» de
                // 318 kg es EMB-26-0001».
                //
                // El tipo del valor NO separa estos casos —una cantidad con
                // unidad se clasifica `text`, igual que un folio, porque
                // `parse_number` no acepta el sufijo— así que la condición se
                // apoya en la forma: un identificador de negocio se escribe en
                // una sola palabra («NOT-26-0001», «FER-01000», «LOT-RES-26-0004»)
                // y lo que trae un espacio es prosa, no una clave. Medido sobre
                // los seis corpus de prueba, la regla separa las dos clases sin
                // excepciones: los 28 campos identificadores del acervo (folio,
                // instrumento, SKU, lote, póliza, expediente…) no llevan
                // espacios, y los 7 que sí los llevan son cantidades, rangos y
                // nombres de modelo.
                //
                // Sólo filtra cómo se REDACTA la frase. La búsqueda por folio
                // (`canonical_identifier_hits`) y los vínculos entre documentos
                // (`relations.rs`) leen la misma columna y no se ven afectados.
                && !item.value.trim().contains(char::is_whitespace)
                && normalize_literal(&item.value) != normalize_literal(reported_value)
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

/// Mínimo de valores numéricos que un campo tiene que tener registrados en el
/// acervo para que su historial signifique algo. Por debajo de esto no hay
/// costumbre que contradecir: un valor negativo no sería una rareza sino uno
/// de los primeros datos del campo, y llamarlo sospechoso sería inventar una
/// norma que el acervo todavía no tiene.
const MIN_SIGN_OBSERVATIONS: i64 = 20;

/// Uno de cada veinte. Por encima de esa proporción los negativos no son una
/// rareza sino una forma de usar el campo —una nota de crédito, un ajuste, una
/// devolución, una desviación contra presupuesto— y señalarlos sería acusar de
/// inválido a un dato correcto.
const EXCEPTIONAL_NEGATIVE_SHARE: i64 = 20;

/// ¿El signo de este valor contradice lo que el acervo tiene escrito en su
/// propio campo?
///
/// Un importe negativo en una orden de compra o una factura es casi siempre un
/// dato inválido —un signo que se coló en la captura, una exportación que
/// invirtió la columna, un OCR que leyó un guion de más—. Pero «negativo» a
/// secas no basta para decirlo: hay campos donde el signo es parte del oficio
/// y un valor negativo es exactamente el dato correcto. El criterio, entonces,
/// no puede ser el signo suelto.
///
/// Lo que sí se puede comprobar sin suponer nada es si ese campo, **en este
/// acervo y en este momento**, se usa alguna vez en negativo. Si de sus
/// cientos de valores registrados ninguno más lo es, el signo es una rareza
/// que merece decirse; si una parte apreciable de ellos lo es, es su forma
/// normal de uso y Omega se calla. La regla no consulta ningún vocabulario ni
/// sabe qué es una nota de crédito: lee el índice al responder, así que
/// funciona igual en un acervo cuyos campos nadie conozca de antemano.
fn anomalous_negative(tools: &ToolEngine, field: &str, value: &str) -> Result<Option<SignRecord>> {
    let typed = classify_value(field, value);
    if !typed.numeric_value.is_some_and(|number| number < 0.0) {
        return Ok(None);
    }
    let record = tools.field_sign_record(field)?;
    let exceptional = record.numeric >= MIN_SIGN_OBSERVATIONS
        && record.negative.saturating_mul(EXCEPTIONAL_NEGATIVE_SHARE) <= record.numeric;
    Ok(exceptional.then_some(record))
}

/// Reporta un valor leído de un documento — y, cuando su signo contradice al
/// propio campo, lo reporta **como sospechoso**, no como el dato.
///
/// La respuesta sigue mostrando el valor y su cita: ocultarlo sería peor que
/// darlo por bueno, porque quien pregunta no podría ir a corregirlo. Lo que
/// cambia es lo que Omega afirma sobre él —dice que no lo respalda, y por qué—
/// y que la síntesis sale **sin marcar como verificada** (`unresolved`), que
/// es el mismo candado que usa cualquier otra ruta cuando la evidencia no
/// alcanza para afirmar.
fn reported_value_synthesis(
    tools: &ToolEngine,
    field: &str,
    value: &str,
    identifier: Option<&str>,
    evidence: &Evidence,
    literals: &[String],
    citations: Vec<Evidence>,
) -> Result<Option<Synthesis>> {
    let phrase = direct_phrase(field, value, identifier, evidence);
    let Some(record) = anomalous_negative(tools, field, value)? else {
        return Ok(supported(phrase, literals, citations));
    };
    Ok(unresolved(
        format!("{phrase}{}", sign_warning(field, record)),
        literals,
        citations,
    ))
}

/// Lo que se le dice a quien preguntó, cuando el signo del valor contradice al
/// campo. Va detrás del valor y de su cita, nunca en su lugar: el dato tiene
/// que quedar a la vista para que se pueda ir a corregirlo al documento.
fn sign_warning(field: &str, record: SignRecord) -> String {
    let rarity = if record.negative <= 1 {
        "éste es el único negativo".to_owned()
    } else {
        format!("sólo {} son negativos, éste entre ellos", record.negative)
    };
    format!(
        " Pero no te lo puedo dar por bueno: es un valor negativo, y de los {} valores numéricos que el acervo registra en «{field}», {rarity}. Lo señalo como dato inválido o sospechoso del documento fuente, no como el valor del campo.",
        record.numeric
    )
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
        reliable: sample.reliable,
        ocr_status: sample.ocr_status.clone(),
        ocr_confidence: sample.ocr_confidence,
        confidence: sample.ocr_confidence,
    }
}

fn unresolved(text: String, literals: &[String], citations: Vec<Evidence>) -> Option<Synthesis> {
    synthesis_if_supported(text, literals, citations, false)
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
) -> Option<Synthesis> {
    synthesis_if_supported(text, literals, citations, true)
}

/// Núcleo compartido por `supported` y `unresolved`. `resolved` distingue la
/// síntesis que sí responde de la que declara que la evidencia no alcanza;
/// la confiabilidad de la evidencia NO es un parámetro: se deriva siempre de
/// las propias citas, igual que en `Answer::verified`. Así ninguna ruta de
/// síntesis puede declararse verificada apoyándose en OCR de baja confianza.
fn synthesis_if_supported(
    text: String,
    literals: &[String],
    citations: Vec<Evidence>,
    resolved: bool,
) -> Option<Synthesis> {
    let borrowed = citations.iter().collect::<Vec<_>>();
    let reliable = citations.iter().all(|evidence| evidence.reliable);
    literals
        .iter()
        .all(|literal| value_is_supported(&borrowed, literal))
        .then_some(Synthesis {
            text,
            verified: resolved && reliable,
            citations,
            subject: None,
        })
}

/// El formato del total se deduce de los propios valores citados: si todos
/// llevan símbolo, si usan separador de millares y si declaran decimales. No se
/// impone una convención que el acervo no use.
fn render_total(total: Decimal, samples: &[&str], currency: Option<&str>) -> String {
    let symbol = samples
        .iter()
        .all(|value| value.trim_start().starts_with('$'));
    let grouped = samples.iter().any(|value| value.contains(','));
    let decimals = samples.iter().any(|value| has_decimals(value));
    let mut rendered = fixed_decimals(total, if decimals { 2 } else { 0 });
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

/// Reemplazo exacto de `format!("{total:.N}")` sobre un `f64`: rinde una
/// cantidad de escala fija con `decimals` decimales y sin separador de miles,
/// que es lo que `render_total` espera antes de aplicar su propio formato.
/// Redondea la mitad hacia arriba en magnitud, como hace la presentación
/// monetaria del resto del motor.
fn fixed_decimals(total: Decimal, decimals: u32) -> String {
    // La escala interna de `Decimal` son cuatro dígitos fraccionarios.
    const SCALE_DIGITS: u32 = 4;
    debug_assert!(decimals <= SCALE_DIGITS);
    let raw = total.raw();
    let negative = raw < 0;
    let divisor = 10u128.pow(SCALE_DIGITS - decimals);
    let magnitude = raw.unsigned_abs();
    let quotient = magnitude / divisor;
    let remainder = magnitude % divisor;
    let scaled = if remainder * 2 >= divisor {
        quotient + 1
    } else {
        quotient
    };
    let unit = 10u128.pow(decimals);
    let mut rendered = if decimals == 0 {
        scaled.to_string()
    } else {
        format!(
            "{}.{:0width$}",
            scaled / unit,
            scaled % unit,
            width = decimals as usize
        )
    };
    if negative && scaled > 0 {
        rendered.insert(0, '-');
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
        let exact = |literal: &str| Decimal::from_text(literal).expect("literal exacto");
        assert_eq!(
            render_total(exact("87430"), &["$1,200 MXN", "$4,850 MXN"], Some("MXN")),
            "$87,430 MXN"
        );
        // Sin símbolo ni separador en el acervo, el total tampoco los inventa.
        assert_eq!(render_total(exact("9"), &["4", "5"], None), "9");
        assert_eq!(
            render_total(exact("1234.50"), &["1,000.00", "234.50"], None),
            "1,234.50"
        );
    }

    /// El total visible se suma en escala fija. En `f64`, 0.1 + 0.2 no da
    /// exactamente 0.3 y la cifra publicada arrastraría ese error.
    #[test]
    fn a_visible_total_is_summed_exactly_and_never_as_a_float() {
        let cents = ["0.10", "0.20"];
        let total = cents
            .iter()
            .fold(Decimal::ZERO, |acc, literal| {
                acc.add(Decimal::from_text(literal).unwrap())
            });
        assert_eq!(render_total(total, &cents, None), "0.30");
        // Y la escala fija no pierde un céntimo en una suma larga.
        let many = (0..1_000).fold(Decimal::ZERO, |acc, _| {
            acc.add(Decimal::from_text("0.07").unwrap())
        });
        assert_eq!(render_total(many, &["0.07"], None), "70.00");
    }

    #[test]
    fn a_query_word_never_names_a_field_by_itself() {
        let vocabulary = vec!["Documento".to_owned(), "Estado".to_owned()];
        let requested = resolve_field(
            "Busca el documento ABC-123",
            &vocabulary,
            &search_terms("Busca el documento ABC-123"),
            &[],
            &[],
            None,
        );
        assert!(matches!(requested, FieldMatch::NotRequested));
        let asked = resolve_field(
            "¿Cuál es el estado de ABC-123?",
            &vocabulary,
            &search_terms("¿Cuál es el estado de ABC-123?"),
            &[],
            &[],
            None,
        );
        assert!(matches!(asked, FieldMatch::Resolved(name) if name == "Estado"));
    }

    #[test]
    fn an_equally_scored_field_is_ambiguous_instead_of_arbitrary() {
        let vocabulary = vec!["Precio pactado".to_owned(), "Precio estimado".to_owned()];
        let requested = resolve_field(
            "¿Cuál es el precio de ABC-123?",
            &vocabulary,
            &search_terms("¿Cuál es el precio de ABC-123?"),
            &[],
            &[],
            None,
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
            "¿Cuál es el estado de ABC-123?",
            &vocabulary,
            &search_terms("¿Cuál es el estado de ABC-123?"),
            &[],
            &[],
            None,
        );
        assert!(matches!(requested, FieldMatch::Resolved(name) if name == "Estado"));
    }

    #[test]
    fn a_question_without_an_interrogative_needs_the_head_of_a_compound_name() {
        // «¿Ya se …?» no pide el valor de ningún campo: no lleva interrogativa.
        // Si de un nombre de dos palabras sólo calza la de detrás —porque su
        // raíz choca con la del verbo— no se ha nombrado ese campo, y elegirlo
        // publicaría un dato ajeno a lo preguntado con sello de verificado.
        let vocabulary = vec!["Modo de traslado".to_owned(), "Situación".to_owned()];
        let collision = "¿Ya se trasladó el bulto ABC-123?";
        let resolved = resolve_field(
            collision,
            &vocabulary,
            &search_terms(collision),
            &[],
            &[],
            None,
        );
        assert!(matches!(resolved, FieldMatch::NotRequested));

        // Con una interrogativa delante, la misma coincidencia parcial vuelve
        // a valer: ahí sí se está pidiendo el valor de un campo.
        let asked = "¿Cuál es el traslado del bulto ABC-123?";
        let resolved = resolve_field(asked, &vocabulary, &search_terms(asked), &[], &[], None);
        assert!(matches!(resolved, FieldMatch::Resolved(name) if name == "Modo de traslado"));

        // Y sin interrogativa, nombrar la PRIMERA palabra sigue bastando: la
        // regla sólo descarta las coincidencias que dejan el núcleo sin
        // nombrar, no toda coincidencia parcial.
        let vocabulary = vec!["Situación declarada".to_owned()];
        let head = "¿Ya cambió la situación de ABC-123?";
        let resolved = resolve_field(head, &vocabulary, &search_terms(head), &[], &[], None);
        assert!(matches!(resolved, FieldMatch::Resolved(name) if name == "Situación declarada"));
    }

    #[test]
    fn the_field_that_only_locates_the_document_never_competes_with_the_asked_one() {
        // «cuya X es ABC-123» escribe X pegado al identificador: es la premisa
        // que señala el documento, no el dato pedido. Sin descontarla empata
        // con el campo preguntado y la respuesta termina siendo el mismo
        // identificador que el usuario acababa de escribir.
        let question = "¿Quién es el responsable del registro cuya referencia es ABC-123?";
        let vocabulary = vec!["Responsable".to_owned(), "Referencia".to_owned()];
        let terms = search_terms(question);
        let premise = LocatorPremise::new(question, "ABC-123");
        let resolved = resolve_field(question, &vocabulary, &terms, &[], &[], Some(&premise));
        assert!(matches!(resolved, FieldMatch::Resolved(name) if name == "Responsable"));
        // Sin premisa los dos campos empatan: ése era exactamente el defecto.
        assert!(matches!(
            resolve_field(question, &vocabulary, &terms, &[], &[], None),
            FieldMatch::Ambiguous
        ));

        // Estar pegado al identificador no basta: si una interrogativa alcanza
        // al campo, se pide. «¿Cuál es el estado de ABC-123?» sólo tiene «de»
        // entre los dos y sigue preguntando por «Estado».
        let asked = "¿Cuál es el estado de ABC-123?";
        let vocabulary = vec!["Estado".to_owned()];
        let premise = LocatorPremise::new(asked, "ABC-123");
        let resolved = resolve_field(asked, &vocabulary, &search_terms(asked), &[], &[], Some(&premise));
        assert!(matches!(resolved, FieldMatch::Resolved(name) if name == "Estado"));
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
            "¿Cuál es el estado de la propiedad ABC-123?",
            &vocabulary,
            &search_terms("¿Cuál es el estado de la propiedad ABC-123?"),
            &type_words,
            &[],
            None,
        );
        assert!(matches!(asked, FieldMatch::Resolved(name) if name == "Estado de la propiedad"));

        // "color" no coincide con nada; la única coincidencia sería el tipo, y
        // por sí solo no habilita ningún campo.
        let invented = resolve_field(
            "¿Cuál es el color de la propiedad ABC-123?",
            &vocabulary,
            &search_terms("¿Cuál es el color de la propiedad ABC-123?"),
            &type_words,
            &[],
            None,
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

        // El nombre del campo va entrecomillado: manda sobre las palabras
        // sueltas del contexto que lo rodea. Sin esta preferencia, la palabra
        // «proveedores» del nombre del área empataba con «Cliente relacionado»
        // y la resolución elegía un campo que nadie pidió.
        let vocabulary = vec![
            "OC".to_owned(),
            "Cliente relacionado".to_owned(),
            "Proveedor relacionado".to_owned(),
        ];
        let ambigua = "En el documento con folio OC-2024-00114 (orden_compra, área Compras, proveedores, logística), ¿cuál es el valor del campo \"Cliente relacionado\"?";
        assert_eq!(
            explicitly_quoted_field(ambigua, &vocabulary).as_deref(),
            Some("Cliente relacionado")
        );
        // Sin comillas no inventa una preferencia: resuelve como siempre.
        assert!(explicitly_quoted_field("¿cuál es el cliente relacionado?", &vocabulary).is_none());

        let related = "¿Cuáles son todos los documentos relacionados con PROP-1?";
        assert!(asks_for_related_documents(related, &search_terms(related)));
        // Sin la palabra que nombra al continente es una pregunta por un campo
        // que se llama "algo relacionado", no por el acervo.
        let field = "¿Cuál es el inmueble relacionado de FIN-1?";
        assert!(!asks_for_related_documents(field, &search_terms(field)));
        // Nombra el continente y contiene "relacionado", pero esa palabra está
        // entrecomillada porque es el nombre del campo pedido: la pregunta
        // quiere ese valor, no la lista de documentos emparentados.
        let quoted_field =
            "En el documento con folio AUD-1, ¿cuál es el valor del campo \"Proveedor relacionado\"?";
        assert!(!asks_for_related_documents(
            quoted_field,
            &search_terms(quoted_field)
        ));
    }
}
