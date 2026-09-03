//! Lectura de los documentos que la respuesta ya citó.
//!
//! El motor contesta con evidencia: dice en qué documento está el dato y en
//! qué línea. Este módulo hace lo otro que hace un lector humano cuando ya
//! localizó el documento: abrirlo, leerlo entero y contar qué dice. Recibe un
//! `Answer` terminado y sólo le añade `answer.reading`; `text` y `citations`
//! salen intactos, de modo que una respuesta correcta nunca se degrada porque
//! su resumen no se pudo componer.
//!
//! No hay modelo de lenguaje: se redacta con reglas, como `report.rs` y
//! `answer.rs`. Y hay exactamente cinco señales con las que una frase puede
//! decidir su forma —el `value_type` del esquema, la bandera de identificador,
//! el solape de términos entre nombres de campo, la posición de un pasaje en
//! el documento y el `Evidence.field` de la cita—. Ninguna de ellas sabe qué
//! *significa* un campo, y ésa es justamente la propiedad que hace que el
//! mismo binario redacte igual de bien una factura, un contrato o un acta sin
//! que nadie lo recompile. Si una frase necesitara saber el significado de un
//! campo para escribirse, esa frase no está aquí.
//!
//! Ningún valor entra al texto sin pasar por `value_is_supported`, el mismo
//! candado de literalidad que protege al resto del motor. Los conectores
//! («Su», «es», «Registra un importe») son constantes de este archivo, no
//! datos del acervo: no se verifican porque no afirman nada.

use std::collections::{BTreeMap, HashSet};

use crate::{
    error::Result,
    model::{Answer, AnswerReading, ReadDocument},
    normalize::{normalize_exact, search_terms, stems_match},
    report::{category_adjective, file_name, plural},
    tools::{DocumentPassage, DocumentValue, ToolEngine},
    verifier::value_is_supported,
};

/// Un pasaje corto y de una sola línea que abre el documento es su título.
/// El umbral es de forma, no de contenido: un párrafo de prosa lo rebasa y
/// una tabla de campos no cabe en una línea.
const MAX_TITLE_CHARS: usize = 120;

/// Cuánto se cita del cierre. Es una cita, no una transcripción.
const MAX_CLOSING_CHARS: usize = 300;

/// Una línea de la que se pueda decir que es prosa y no un resto de tabla.
/// Sólo mide longitud: no mira ni una palabra.
const MIN_PROSE_CHARS: usize = 40;

/// Topes por bloque. Un documento con sesenta campos no se vuelca entero: se
/// cuenta lo que cabe y se declara que se recortó.
const MAX_SIBLINGS: usize = 4;
const MAX_DATES: usize = 4;
const MAX_AMOUNTS: usize = 4;
const MAX_PLAIN_FIELDS: usize = 6;
const MAX_IDENTIFIERS: usize = 3;

/// Topes del resumen de conjunto.
const MAX_COMMON_FIELDS: usize = 3;
const MAX_DIFFERING_FIELDS: usize = 3;
const MAX_DISTINCT_VALUES: usize = 4;

/// A partir de aquí se recorta el detalle de cada documento —nunca cuántos
/// son—. Veinte líneas largas dejan de ser un resumen.
const MAX_DETAILED_DOCUMENTS: usize = 8;

/// Cuántos documentos se enumeran línea a línea. Los que no caben siguen
/// leídos y siguen en `documents`: la interfaz los lista todos debajo del
/// texto, que es donde una lista larga se puede recorrer sin estorbar.
const MAX_LISTED_DOCUMENTS: usize = 24;

/// Cuántos documentos se abren como mucho.
///
/// Leer es abrir el documento y recorrerlo entero, así que el coste crece con
/// el número de citas. Una pregunta acotada cita unas pocas decenas y se lee
/// completa; una palabra suelta sobre un acervo de diez mil documentos llega
/// a citar miles, y leerlos todos añadiría veinte segundos de espera a una
/// respuesta que ya estaba lista, para producir un texto de miles de líneas
/// que nadie llamaría resumen. Por encima de este tope se leen los primeros y
/// se dice cuántos quedaron sin leer. Ninguna cita se pierde: la respuesta y
/// su evidencia siguen completas, es el resumen el que se declara parcial.
const MAX_READ_DOCUMENTS: usize = 250;

/// Añade la lectura a una respuesta ya terminada.
///
/// Devuelve el `Answer` intacto ante cualquier problema: la lectura es un
/// añadido y no puede convertir una respuesta buena en un error.
pub(crate) fn attach(tools: &ToolEngine, mut answer: Answer) -> Result<Answer> {
    answer.reading = compose(tools, &answer);
    Ok(answer)
}

fn compose(tools: &ToolEngine, answer: &Answer) -> Option<AnswerReading> {
    let (documents, cited) = gather(tools, answer);
    if documents.is_empty() {
        return None;
    }
    let mut truncated = cited > documents.len();
    let text = if documents.len() == 1 {
        write_single(&documents[0], &mut truncated)
    } else {
        write_many(&documents, cited, &mut truncated)
    };
    let text = text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    Some(AnswerReading {
        text,
        documents: documents
            .iter()
            .map(|document| ReadDocument {
                path: document.path.clone(),
                origin: document.origin.clone(),
                citation_numbers: document.citation_numbers.clone(),
                passages_read: document.passages.len(),
                reliable: document.reliable(),
            })
            .collect(),
        truncated,
    })
}

/// Recorre las citas en su orden y reúne los documentos distintos, guardando
/// para cada uno los números con los que la interfaz numera sus citas y el
/// campo de la cita que lo trajo: ése es el campo que responde la pregunta,
/// y el motor ya lo resolvió. Un documento citado varias veces se lee una
/// sola vez y conserva todos sus números.
///
/// Devuelve los documentos leídos y cuántos citaba la respuesta, que no
/// siempre son lo mismo: el tope de lectura puede dejar fuera a los últimos.
fn gather(tools: &ToolEngine, answer: &Answer) -> (Vec<DocumentReading>, usize) {
    let mut order: Vec<i64> = Vec::new();
    let mut collected: BTreeMap<i64, (Vec<usize>, Option<String>)> = BTreeMap::new();
    for (index, citation) in answer.citations.iter().enumerate() {
        // Una nota de cálculo no es la cita de un documento: su cifra no está
        // escrita en ninguno. Los documentos que sí la sostienen vienen
        // citados aparte y se leen como cualquier otro.
        if citation.match_kind == "cálculo" || citation.document_id <= 0 {
            continue;
        }
        let entry = collected
            .entry(citation.document_id)
            .or_insert_with(|| {
                order.push(citation.document_id);
                (Vec::new(), None)
            });
        entry.0.push(index + 1);
        if entry.1.is_none() {
            entry.1 = citation.field.clone();
        }
    }
    let cited = order.len();
    let documents = order
        .into_iter()
        .take(MAX_READ_DOCUMENTS)
        .filter_map(|document_id| {
            let (citation_numbers, answering_field) = collected.remove(&document_id)?;
            DocumentReading::open(tools, document_id, citation_numbers, answering_field)
        })
        .collect();
    (documents, cited)
}

/// Un documento leído: sus pasajes en orden y sus campos, ya filtrados por el
/// candado de literalidad.
struct DocumentReading {
    path: String,
    origin: String,
    citation_numbers: Vec<usize>,
    /// Campo de la cita que trajo este documento. Puede faltar: una
    /// coincidencia de texto libre no resuelve ningún campo.
    answering_field: Option<String>,
    values: Vec<DocumentValue>,
    passages: Vec<DocumentPassage>,
}

impl DocumentReading {
    fn open(
        tools: &ToolEngine,
        document_id: i64,
        citation_numbers: Vec<usize>,
        answering_field: Option<String>,
    ) -> Option<Self> {
        let passages = tools.document_text(document_id).ok()?;
        let mut values = tools.document_values(document_id).ok()?;
        let (path, origin) = passages
            .first()
            .map(|passage| &passage.evidence)
            .or_else(|| values.first().map(|value| &value.evidence))
            .map(|reference| (reference.path.clone(), reference.origin.clone()))?;
        // El candado se aplica una sola vez, aquí, sobre todos los valores del
        // documento: lo que no esté escrito en él no llega siquiera a los
        // bloques que redactan. Cada valor se contrasta contra su propia
        // evidencia —la misma que la interfaz muestra en su cita—, que es la
        // comprobación más estricta posible: exige que el valor esté en el
        // fragmento del que se extrajo, no en cualquier parte del documento.
        // Al texto libre —el título y el cierre— se le aplica aparte, contra
        // el pasaje íntegro del que sale.
        values.retain(|value| value_is_supported(&[&value.evidence], &value.value));
        Some(Self {
            path,
            origin,
            citation_numbers,
            answering_field,
            values,
            passages,
        })
    }

    fn reliable(&self) -> bool {
        self.passages
            .iter()
            .map(|passage| &passage.evidence)
            .chain(self.values.iter().map(|value| &value.evidence))
            .all(|evidence| evidence.reliable)
    }

    /// El campo que contestó la pregunta, con su valor y su ubicación.
    fn answering(&self) -> Option<&DocumentValue> {
        let field = normalize_exact(self.answering_field.as_deref()?);
        self.values
            .iter()
            .find(|value| normalize_exact(&value.field) == field)
    }

    /// El primer pasaje, cuando su sola forma lo delata como título: corto,
    /// de una línea y sin ser un par campo/valor de los que ya están
    /// extraídos. No se mira ni una palabra suya.
    fn title(&self) -> Option<&str> {
        let first = self.passages.first()?;
        let content = first.content.trim();
        let single_line = content.lines().count() == 1;
        (single_line
            && content.chars().count() <= MAX_TITLE_CHARS
            && !content.is_empty()
            && !self.declares_a_value(content)
            && passage_supports(first, content))
        .then_some(content)
    }

    /// ¿Esta línea es uno de los pares campo/valor que el índice ya extrajo?
    /// La prueba es universal porque no describe ninguna tabla: compara la
    /// línea con los valores que este mismo documento declaró.
    fn declares_a_value(&self, line: &str) -> bool {
        let line = normalize_exact(line);
        if line.is_empty() {
            return true;
        }
        self.values.iter().any(|value| {
            let excerpt = normalize_exact(&value.evidence.excerpt);
            let field = normalize_exact(&value.field);
            let literal = normalize_exact(&value.value);
            excerpt.contains(&line)
                || (!field.is_empty()
                    && !literal.is_empty()
                    && line.contains(&field)
                    && line.contains(&literal))
        })
    }

    /// El cierre en prosa: el último pasaje que no es una tabla de pares, ya
    /// sin las líneas que sí lo son. De lo que queda se cita la última frase
    /// cuando se sostiene sola, y si no, todo el resto.
    fn closing(&self) -> Option<String> {
        for passage in self.passages.iter().rev() {
            let prose = passage
                .content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !self.declares_a_value(line))
                .collect::<Vec<_>>();
            let Some(last) = prose.last() else {
                continue;
            };
            let text = if last.chars().count() >= MIN_PROSE_CHARS {
                (*last).to_owned()
            } else {
                prose.join(" ")
            };
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.chars().count() < MIN_PROSE_CHARS {
                continue;
            }
            let quoted = clip(&text, MAX_CLOSING_CHARS);
            if !passage_supports(passage, quoted.trim_end_matches('…')) {
                continue;
            }
            return Some(quoted);
        }
        None
    }
}

/// Un documento leído: identidad, la respuesta, y después lo que el documento
/// declara, agrupado por lo único que el esquema sabe de cada valor.
fn write_single(document: &DocumentReading, truncated: &mut bool) -> String {
    let mut published = Published::default();
    let mut paragraphs: Vec<String> = Vec::new();

    if let Some(title) = document.title() {
        paragraphs.push(format!(
            "«{title}». Leí sus {} {}.",
            document.passages.len(),
            plural(document.passages.len(), "pasaje", "pasajes")
        ));
    }

    let answer = answer_block(document, &mut published);
    let siblings = siblings_block(document, &mut published, truncated);
    let middle = [answer, siblings]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !middle.is_empty() {
        paragraphs.push(middle.join(" "));
    }

    let body = [
        dates_block(document, &mut published, truncated),
        amounts_block(document, &mut published, truncated),
        plain_block(document, &mut published, truncated),
        identifiers_block(document, &mut published, truncated),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !body.is_empty() {
        paragraphs.push(body.join(" "));
    }

    if let Some(closing) = document.closing() {
        // El cierre ya trae su propia puntuación final —o la elipsis del
        // recorte—; añadirle un punto detrás de la comilla lo duplicaría.
        let stop = if closing.ends_with(['.', '!', '?', '…']) {
            ""
        } else {
            "."
        };
        paragraphs.push(format!("Cierra con: «{closing}»{stop}"));
    }

    paragraphs.join("\n\n")
}

/// Campos ya publicados, por par campo/valor. Un mismo dato declarado dos
/// veces en el documento se cuenta una sola vez.
#[derive(Default)]
struct Published {
    seen: HashSet<String>,
}

impl Published {
    fn key(value: &DocumentValue) -> String {
        format!(
            "{}\u{1}{}",
            normalize_exact(&value.field),
            normalize_exact(&value.value)
        )
    }

    fn contains(&self, value: &DocumentValue) -> bool {
        self.seen.contains(&Self::key(value))
    }

    fn insert(&mut self, value: &DocumentValue) -> bool {
        self.seen.insert(Self::key(value))
    }
}

/// Los valores que todavía no se han publicado y que este documento respalda,
/// sin repetir un mismo par campo/valor.
///
/// La deduplicación se hace aquí y no al publicar porque un bloque reúne sus
/// valores antes de escribir ninguno: un formato que repite el mismo campo en
/// la carátula y en el cuerpo declara dos veces lo mismo, y decirlo dos veces
/// en la misma frase no informa de nada.
fn pending<'a>(
    document: &'a DocumentReading,
    published: &Published,
) -> impl Iterator<Item = &'a DocumentValue> {
    let mut repeated = HashSet::new();
    document
        .values
        .iter()
        .filter(|value| !value.value.trim().is_empty())
        .filter(move |value| !published.contains(value))
        .filter(move |value| repeated.insert(Published::key(value)))
        .collect::<Vec<_>>()
        .into_iter()
}

fn answer_block(document: &DocumentReading, published: &mut Published) -> Option<String> {
    let value = document.answering()?;
    published.insert(value);
    Some(format!(
        "Responde tu pregunta en {}: {}, en {}.",
        value.field, value.value, value.evidence.location
    ))
}

/// Campos cuyo nombre comparte al menos un término con el del campo que
/// respondió. El parentesco lo decide `search_terms`, no un diccionario: si
/// dos rótulos comparten una raíz, el acervo mismo los emparentó.
fn siblings_block(
    document: &DocumentReading,
    published: &mut Published,
    truncated: &mut bool,
) -> Option<String> {
    let answering = document.answering()?;
    let terms = search_terms(&answering.field);
    if terms.is_empty() {
        return None;
    }
    let field = normalize_exact(&answering.field);
    let mut related = Vec::new();
    for value in pending(document, published) {
        if normalize_exact(&value.field) == field {
            continue;
        }
        let shares = search_terms(&value.field)
            .iter()
            .any(|word| terms.iter().any(|term| stems_match(word, term)));
        if shares {
            related.push(value);
        }
    }
    if related.is_empty() {
        return None;
    }
    let total = related.len();
    if total > MAX_SIBLINGS {
        *truncated = true;
    }
    let shown = related
        .into_iter()
        .take(MAX_SIBLINGS)
        .map(|value| {
            published.insert(value);
            format!("{}, {}", value.field, value.value)
        })
        .collect::<Vec<_>>();
    let list = join_with(&shown, "; ", "; y ");
    Some(if total == 1 {
        format!("Otro campo comparte ese nombre: {list}.")
    } else {
        format!("Otros {total} campos comparten ese nombre: {list}.")
    })
}

fn dates_block(
    document: &DocumentReading,
    published: &mut Published,
    truncated: &mut bool,
) -> Option<String> {
    let mut distinct = Vec::new();
    let mut seen = HashSet::new();
    for value in pending(document, published) {
        if value.value_type != "date" {
            continue;
        }
        if seen.insert(normalize_exact(&value.value)) {
            distinct.push(value);
        }
    }
    if distinct.is_empty() {
        return None;
    }
    let total = distinct.len();
    if total == 1 {
        let value = distinct[0];
        published.insert(value);
        return Some(format!("El documento se fecha el {}.", value.value));
    }
    if total > MAX_DATES {
        *truncated = true;
    }
    let shown = distinct
        .into_iter()
        .take(MAX_DATES)
        .map(|value| {
            published.insert(value);
            value.value.clone()
        })
        .collect::<Vec<_>>();
    let list = join_with(&shown, ", ", " y ");
    Some(if total > shown.len() {
        format!("Registra {total} fechas; las primeras son {list}.")
    } else {
        format!("Registra las fechas {list}.")
    })
}

/// Importes, porcentajes y números. La categoría la da el esquema; el nombre
/// de la categoría, `report::category_adjective`.
fn amounts_block(
    document: &DocumentReading,
    published: &mut Published,
    truncated: &mut bool,
) -> Option<String> {
    let mut sentences = Vec::new();
    for kind in ["money", "percentage", "number"] {
        let group = pending(document, published)
            .filter(|value| value.value_type == kind)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        let total = group.len();
        if total > MAX_AMOUNTS {
            *truncated = true;
        }
        let shown = group
            .into_iter()
            .take(MAX_AMOUNTS)
            .map(|value| {
                published.insert(value);
                format!("{}, {}", value.field, value.value)
            })
            .collect::<Vec<_>>();
        let list = join_with(&shown, "; ", "; y ");
        // El importe tiene nombre propio en español; el resto de las
        // categorías se nombra con su adjetivo, que es lo único que el
        // esquema sabe de ellas.
        let noun = if kind == "money" {
            (
                "un importe".to_owned(),
                format!("{total} importes"),
            )
        } else {
            let adjective = category_adjective(kind);
            (
                format!("un valor {adjective}"),
                format!("{total} valores {}", adjective_plural(adjective)),
            )
        };
        sentences.push(format!(
            "Registra {}: {list}.",
            if total == 1 { noun.0 } else { noun.1 }
        ));
    }
    (!sentences.is_empty()).then(|| sentences.join(" "))
}

/// El resto de los campos, encadenados con elisión del verbo: así se escribe
/// una enumeración de atributos en español. Los identificadores no entran
/// aquí porque tienen su propia frase: son claves, no descripciones.
fn plain_block(
    document: &DocumentReading,
    published: &mut Published,
    truncated: &mut bool,
) -> Option<String> {
    let rest = pending(document, published)
        .filter(|value| matches!(value.value_type.as_str(), "text" | "state"))
        .filter(|value| value.identifier_canonical.is_none())
        .collect::<Vec<_>>();
    if rest.is_empty() {
        return None;
    }
    let total = rest.len();
    let shown = rest
        .into_iter()
        .take(MAX_PLAIN_FIELDS)
        .map(|value| {
            published.insert(value);
            (value.field.clone(), value.value.clone())
        })
        .collect::<Vec<_>>();
    let mut parts = shown
        .iter()
        .enumerate()
        .map(|(index, (field, value))| {
            if index == 0 {
                format!("Su {field} es {value}")
            } else {
                format!("su {field}, {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    let omitted = total - shown.len();
    if omitted > 0 {
        *truncated = true;
        parts.push_str(&format!(
            "; y {omitted} {} más que no detallo",
            plural(omitted, "campo", "campos")
        ));
    }
    parts.push('.');
    Some(parts)
}

/// Identificadores: valores que mezclan letras y dígitos y funcionan como
/// clave estable. La bandera la puso el índice al extraerlos.
fn identifiers_block(
    document: &DocumentReading,
    published: &mut Published,
    truncated: &mut bool,
) -> Option<String> {
    let keys = pending(document, published)
        .filter(|value| value.identifier_canonical.is_some())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return None;
    }
    let total = keys.len();
    if total > MAX_IDENTIFIERS {
        *truncated = true;
    }
    let shown = keys
        .into_iter()
        .take(MAX_IDENTIFIERS)
        .map(|value| {
            published.insert(value);
            format!("{}: {}", value.field, value.value)
        })
        .collect::<Vec<_>>();
    let list = join_with(&shown, ", ", " y ");
    Some(if shown.len() == 1 {
        format!("Lo identifica {list}.")
    } else {
        format!("Lo identifican {list}.")
    })
}

/// Varios documentos: primero el conjunto —en qué coinciden y en qué no— y
/// después cada uno en forma breve. Con muchos se recorta el detalle de cada
/// documento antes que su número; lo que se recorte se dice en el propio
/// texto, para que nadie lea un resumen parcial como si fuera completo.
fn write_many(documents: &[DocumentReading], cited: usize, truncated: &mut bool) -> String {
    let total = documents.len();
    let fields = FieldsAcross::of(documents);
    let mut heading = vec![if cited > total {
        format!("Leí {total} de los {cited} documentos citados.")
    } else {
        format!("Leí {total} documentos.")
    }];
    heading.extend(fields.common(total));
    heading.extend(fields.differing());

    let detailed = total <= MAX_DETAILED_DOCUMENTS;
    if !detailed {
        *truncated = true;
        heading.push(format!(
            "Con {total} documentos doy de cada uno su nombre, sus citas y el campo que responde."
        ));
    }
    let listed = total.min(MAX_LISTED_DOCUMENTS);
    if listed < total {
        heading.push(format!(
            "Enumero {listed}; los {} restantes quedan en la lista de documentos leídos.",
            total - listed
        ));
    }
    if cited > total {
        heading.push(format!(
            "Los {} que faltan siguen citados, con su evidencia, pero no entran en este resumen.",
            cited - total
        ));
    }

    let lines = documents
        .iter()
        .take(listed)
        .map(|document| format!("- {}", document_line(document, detailed)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\n{lines}", heading.join(" "))
}

/// Una línea por documento: cómo se llama, con qué números está citado y qué
/// dice en el campo que responde la pregunta.
fn document_line(document: &DocumentReading, detailed: bool) -> String {
    let name = document
        .title()
        .filter(|_| detailed)
        .map(|title| format!("«{title}»"))
        .unwrap_or_else(|| format!("`{}`", file_name(&document.path)));
    let numbers = document
        .citation_numbers
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>();
    let citations = format!(
        "{} {}",
        plural(numbers.len(), "cita", "citas"),
        join_with(&numbers, ", ", " y ")
    );
    match document.answering() {
        Some(value) => format!("{name}, {citations}: {}, {}.", value.field, value.value),
        None => format!("{name}, {citations}."),
    }
}

/// Los campos vistos desde el conjunto: qué documento declara cada uno y con
/// qué valor. La clave es el nombre normalizado del campo, así que dos
/// documentos que escriben el mismo rótulo distinto siguen encontrándose.
struct FieldsAcross {
    entries: Vec<FieldAcross>,
}

struct FieldAcross {
    display: String,
    /// Posición más temprana del campo dentro de un documento. Ordenar por
    /// ella pone primero lo que los documentos ponen primero, sin que este
    /// código tenga que saber qué campo es más importante.
    ordinal: usize,
    /// Un valor por documento: el primero que ese documento declara. Los
    /// valores ya pasaron el candado al abrirse su documento.
    per_document: Vec<String>,
}

impl FieldsAcross {
    fn of(documents: &[DocumentReading]) -> Self {
        let mut index: BTreeMap<String, FieldAcross> = BTreeMap::new();
        for document in documents {
            let mut taken: HashSet<String> = HashSet::new();
            for value in &document.values {
                if value.value.trim().is_empty() {
                    continue;
                }
                let key = normalize_exact(&value.field);
                if key.is_empty() || !taken.insert(key.clone()) {
                    continue;
                }
                let entry = index.entry(key).or_insert_with(|| FieldAcross {
                    display: value.field.clone(),
                    ordinal: value.ordinal,
                    per_document: Vec::new(),
                });
                entry.ordinal = entry.ordinal.min(value.ordinal);
                entry.per_document.push(value.value.clone());
            }
        }
        let mut entries = index.into_values().collect::<Vec<_>>();
        entries.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then(a.display.cmp(&b.display)));
        Self { entries }
    }

    /// Campos en los que los documentos leídos declaran todos el mismo valor.
    fn common(&self, total: usize) -> Option<String> {
        let shared = self
            .entries
            .iter()
            .filter(|field| field.per_document.len() == total)
            .filter_map(|field| {
                let first = field.per_document.first()?;
                let same = field
                    .per_document
                    .iter()
                    .all(|value| normalize_exact(value) == normalize_exact(first));
                same.then(|| format!("{}: {first}", field.display))
            })
            .take(MAX_COMMON_FIELDS)
            .collect::<Vec<_>>();
        (!shared.is_empty())
            .then(|| format!("Los {total} coinciden en {}.", join_with(&shared, "; ", "; y ")))
    }

    /// Campos en los que no coinciden, con cuántos documentos sostienen cada
    /// valor.
    fn differing(&self) -> Option<String> {
        let split = self
            .entries
            .iter()
            .filter(|field| field.per_document.len() >= 2)
            .filter_map(|field| {
                let mut counted: BTreeMap<String, (String, usize)> = BTreeMap::new();
                for value in &field.per_document {
                    let entry = counted
                        .entry(normalize_exact(value))
                        .or_insert_with(|| (value.clone(), 0));
                    entry.1 += 1;
                }
                if counted.len() < 2 {
                    return None;
                }
                // Cuando los valores son demasiados para enumerarlos se dice
                // cuántos hay: es el mismo dato, y cabe.
                if counted.len() > MAX_DISTINCT_VALUES {
                    return Some(format!(
                        "{}: {} valores distintos",
                        field.display,
                        counted.len()
                    ));
                }
                let mut values = counted.into_values().collect::<Vec<_>>();
                values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let list = values
                    .into_iter()
                    .map(|(value, count)| format!("{value} ({count})"))
                    .collect::<Vec<_>>();
                Some(format!("{}: {}", field.display, join_with(&list, ", ", ", ")))
            })
            .take(MAX_DIFFERING_FIELDS)
            .collect::<Vec<_>>();
        (!split.is_empty())
            .then(|| format!("Se diferencian en {}.", join_with(&split, "; ", "; y ")))
    }
}

/// El candado para el texto que no sale de un valor extraído: el título y el
/// cierre se contrastan contra el pasaje íntegro del que se leyeron. El
/// extracto de la evidencia va abreviado para la interfaz, así que aquí se
/// usa el contenido completo; si no, un cierre largo no se podría verificar
/// contra su propio pasaje.
fn passage_supports(passage: &DocumentPassage, text: &str) -> bool {
    let mut evidence = passage.evidence.clone();
    evidence.excerpt = passage.content.clone();
    value_is_supported(&[&evidence], text)
}

/// Concordancia de número del adjetivo de categoría. Es la regla del español
/// —vocal más «s», consonante más «es»—, no una tabla de palabras: cubre
/// cualquier adjetivo que `report::category_adjective` devuelva hoy o mañana.
fn adjective_plural(adjective: &str) -> String {
    if adjective.ends_with(['a', 'e', 'i', 'o', 'u', 'á', 'é', 'í', 'ó', 'ú']) {
        format!("{adjective}s")
    } else {
        format!("{adjective}es")
    }
}

/// Enumeración con separador propio para el último elemento, que es como se
/// enumera en español.
fn join_with(items: &[String], separator: &str, last: &str) -> String {
    match items.split_last() {
        None => String::new(),
        Some((only, [])) => only.clone(),
        Some((tail, head)) => format!("{}{last}{tail}", head.join(separator)),
    }
}

/// Recorte por caracteres con elipsis. Corta por palabra para no partir una
/// en dos.
fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut clipped = String::new();
    for word in text.split_whitespace() {
        if clipped.chars().count() + word.chars().count() + 1 > limit {
            break;
        }
        if !clipped.is_empty() {
            clipped.push(' ');
        }
        clipped.push_str(word);
    }
    format!("{}…", clipped.trim_end_matches([',', ';', '.', ' ']))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{
        agent::Agent, dates::Clock, db::Database, indexer::Indexer,
        parser::LocalDocumentParser,
    };

    /// La invariante que manda sobre todo el módulo —`text` y `citations`
    /// intactos— se comprueba aquí, y no en la suite de integración, porque
    /// sólo desde dentro del crate se puede tener en la mano el mismo
    /// `Answer` antes y después de `attach`: fuera, el motor ya la trae
    /// puesta.
    #[test]
    fn attaching_a_reading_leaves_the_answer_untouched() {
        let fixture = tempfile::tempdir().unwrap();
        let corpus = fixture.path().join("acervo");
        fs::create_dir_all(&corpus).unwrap();
        fs::write(
            corpus.join("registro.md"),
            "# Registro de prueba\n\nAlfa: REG-77-0001\nBeta: Activo\nGamma: $1,200.00 MXN\n\nEl responsable conserva la evidencia del movimiento hasta su cierre formal.\n",
        )
        .unwrap();

        let database = Database::open(fixture.path().join("omega.db")).unwrap();
        let parser = LocalDocumentParser::default();
        let source = Indexer::new(&database, &parser).authorize(&corpus).unwrap();
        Indexer::new(&database, &parser).index_source(source).unwrap();
        let tools = ToolEngine::new(database);

        let question = "¿Qué Alfa aparece en el registro REG-77-0001?";
        let original = Agent::new(tools.clone(), Clock::fixed("2026-08-26").unwrap())
            .answer(question)
            .unwrap();
        let attached = attach(&tools, original.clone()).unwrap();

        assert_eq!(attached.text, original.text);
        assert_eq!(attached.citations.len(), original.citations.len());
        for (after, before) in attached.citations.iter().zip(&original.citations) {
            assert_eq!(after.id, before.id);
            assert_eq!(after.excerpt, before.excerpt);
            assert_eq!(after.value, before.value);
            assert_eq!(after.location, before.location);
        }
        assert!(original.reading.is_none());
        let reading = attached.reading.expect("la lectura debe componerse");
        assert!(
            !reading.text.is_empty() && reading.documents.len() == 1,
            "{reading:?}"
        );
    }

    #[test]
    fn an_enumeration_closes_with_its_own_connector() {
        let items = ["uno".to_owned(), "dos".to_owned(), "tres".to_owned()];
        assert_eq!(join_with(&items, ", ", " y "), "uno, dos y tres");
        assert_eq!(join_with(&items[..1], ", ", " y "), "uno");
        assert_eq!(join_with(&[], ", ", " y "), "");
    }

    #[test]
    fn the_category_adjective_agrees_in_number_by_rule() {
        assert_eq!(adjective_plural("monetario"), "monetarios");
        assert_eq!(adjective_plural("porcentual"), "porcentuales");
        assert_eq!(adjective_plural("numérico"), "numéricos");
    }

    #[test]
    fn clipping_never_splits_a_word() {
        let text = "una frase larga que no cabe entera en el hueco disponible";
        let clipped = clip(text, 20);
        assert!(clipped.ends_with('…'), "{clipped}");
        assert!(text.starts_with(clipped.trim_end_matches('…').trim()));
        assert_eq!(clip("corta", 20), "corta");
    }
}
