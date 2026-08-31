//! Memoria local de conversación.
//!
//! Guarda hechos estructurados —un predicado, un campo, una moneda, un rango y
//! la evidencia que ya se citó—, nunca una interpretación libre de lo que el
//! usuario quiso decir. Vive en memoria del proceso: no se escribe a disco, no
//! se comparte entre conversaciones y desaparece al cerrar la aplicación.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    dates::DateRange,
    model::{DateConstraint, Evidence, ToolFilter},
    normalize::normalize_exact,
};

/// Conjunto de documentos de un turno, guardado como **predicado** y no como
/// lista de identificadores internos.
///
/// La razón es concreta: reindexar borra y vuelve a insertar las filas de
/// `documents`, así que los rowid cambian. Una memoria basada en ids
/// apuntaría, después de reindexar, a documentos distintos de los que el
/// usuario vio. El predicado se vuelve a evaluar en cada turno, de modo que el
/// conjunto sigue el estado real del índice y las fuentes revocadas
/// desaparecen solas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocumentSet {
    pub filters: Vec<ToolFilter>,
    pub origin: Option<String>,
    /// Conjunto definido por una clave estable (documentos que comparten un
    /// identificador). Se reevalúa igual que el resto del predicado.
    pub identifier: Option<String>,
    pub date: Option<DateConstraint>,
    /// Unidad del rango, para poder retroceder al periodo anterior sin volver
    /// a leer la pregunta que lo creó.
    pub range: Option<DateRange>,
    /// Cuántos documentos cumplían el predicado cuando se creó. Si al
    /// reevaluarlo cambia, la respuesta lo dice en lugar de callarlo.
    pub document_count: i64,
    /// Muestra acotada de rutas reales, sólo para auditar el conjunto.
    pub paths: Vec<String>,
}

const MAX_REMEMBERED_PATHS: usize = 50;
const MAX_REMEMBERED_EVIDENCE: usize = 50;
const MAX_CONVERSATIONS: usize = 32;

impl DocumentSet {
    pub fn with_paths(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.paths = paths.into_iter().take(MAX_REMEMBERED_PATHS).collect();
        self
    }

}

/// Cálculo del turno anterior. Conserva la evidencia exacta que lo respaldó
/// para poder responder «¿qué documentos respaldan ese total?» sin recalcular
/// ni buscar de nuevo.
#[derive(Clone, Debug)]
pub struct ComputationMemory {
    pub operation: String,
    pub concept: String,
    pub rendered: String,
    pub value_count: usize,
    pub evidence: Vec<Evidence>,
    /// Alguno de los operandos del cálculo venía de OCR de baja confianza.
    /// Es indispensable guardarlo aparte: `evidence` se recorta a
    /// `MAX_REMEMBERED_EVIDENCE`, así que el operando débil puede no estar
    /// entre los recordados y la señal se perdería justo en la continuación.
    pub has_unreliable_evidence: bool,
}

impl ComputationMemory {
    pub fn new(
        operation: &str,
        concept: &str,
        rendered: String,
        value_count: usize,
        evidence: Vec<Evidence>,
        has_unreliable_evidence: bool,
    ) -> Self {
        Self {
            operation: operation.to_owned(),
            concept: concept.to_owned(),
            rendered,
            value_count,
            evidence: evidence.into_iter().take(MAX_REMEMBERED_EVIDENCE).collect(),
            has_unreliable_evidence,
        }
    }
}

/// Aclaración en curso: qué se preguntó, sobre qué conjunto y qué opciones se
/// ofrecieron.
///
/// Sin esto, elegir una opción sería una consulta nueva y perdería el alcance
/// que la motivó: el usuario respondería «Anticipo recibido» y el motor sumaría
/// todos los anticipos del acervo en vez de los del conjunto que estaba viendo.
#[derive(Clone, Debug)]
pub struct PendingChoice {
    /// Pregunta original, que se vuelve a planificar con la opción elegida.
    pub question: String,
    /// Predicado completo del conjunto sobre el que se calculará.
    pub set: DocumentSet,
    pub options: Vec<String>,
    pub kind: PendingKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingKind {
    /// Falta elegir el campo sobre el que operar.
    Concept,
    /// Falta elegir el campo de fecha al que anclar un periodo.
    DateField,
}

impl PendingChoice {
    /// ¿La respuesta del usuario es una de las opciones ofrecidas?
    ///
    /// Se acepta la opción escrita tal cual o precedida de una fórmula corta
    /// («el de», «usa»); no se acepta una frase larga, que sería una pregunta
    /// nueva y no una elección.
    pub fn chosen(&self, answer: &str) -> Option<String> {
        let written = normalize_exact(answer);
        if written.split_whitespace().count() > 8 {
            return None;
        }
        self.options
            .iter()
            .find(|option| {
                let normalized = normalize_exact(option);
                !normalized.is_empty()
                    && (written == normalized
                        || written.ends_with(&format!(" {normalized}"))
                        || written.starts_with(&format!("{normalized} ")))
            })
            .cloned()
    }

    /// ¿La respuesta pide explícitamente todas las opciones en vez de elegir
    /// una?
    ///
    /// Es tan literal como `chosen`: sólo reconoce la palabra genérica de
    /// «todo/ambos», nunca una frase larga que podría ser una pregunta nueva
    /// sin relación con la aclaración.
    pub fn wants_all(&self, answer: &str) -> bool {
        const ALL_WORDS: &[&str] = &["todos", "todas", "todo", "ambos", "ambas"];
        let normalized = normalize_exact(answer);
        let words = normalized.split_whitespace().collect::<Vec<_>>();
        words.len() <= 8 && words.iter().any(|word| ALL_WORDS.contains(word))
    }
}

/// Estado de una conversación. Todo campo es opcional: una conversación nueva
/// empieza vacía y ninguna rama del motor puede rellenarla por suposición.
#[derive(Clone, Debug, Default)]
pub struct ConversationState {
    pub last_question: Option<String>,
    pub set: Option<DocumentSet>,
    /// Campo sobre el que se calculó por última vez.
    pub concept: Option<String>,
    pub group_by: Option<String>,
    pub currency: Option<String>,
    pub last_result: Option<ComputationMemory>,
    /// Identificador consultado por última vez, en su forma canónica.
    pub identifier: Option<String>,
    /// Aclaración a la espera de respuesta.
    pub pending: Option<PendingChoice>,
    /// Última comparación entre dos grupos o periodos, para poder responder
    /// «¿cuál es la diferencia?» sin recalcular ni volver a preguntar.
    pub comparison: Option<ComparisonMemory>,
    /// Documento del que habló la respuesta anterior, cuando fue exactamente
    /// uno: lo que «ese documento» señala en la continuación siguiente.
    ///
    /// Se guarda por su **ruta** y no por su identificador de fila, por la
    /// misma razón por la que el conjunto se guarda como predicado: reindexar
    /// reasigna los rowid, y entonces el mismo número apuntaría a otro
    /// archivo. La ruta sigue siendo el mismo archivo, y si desaparece del
    /// índice la referencia deja de resolver — que es lo correcto.
    pub document: Option<String>,
}

/// Comparación ya calculada, con sus dos lados y su evidencia.
#[derive(Clone, Debug)]
pub struct ComparisonMemory {
    pub concept: String,
    pub dimension: String,
    pub left_label: String,
    pub right_label: String,
    pub left: Option<ComparisonSide>,
    pub right: Option<ComparisonSide>,
    pub evidence: Vec<Evidence>,
    /// Alguno de los operandos que produjeron esta comparación venía de OCR
    /// de baja confianza. Se guarda con la comparación porque `evidence` sólo
    /// contiene la muestra visible: una continuación («¿cuál es la
    /// diferencia?») no puede recuperar esa señal mirando las citas.
    pub has_unreliable_evidence: bool,
}

#[derive(Clone, Debug)]
pub struct ComparisonSide {
    pub rendered: String,
    pub value_count: usize,
    pub currency: Option<String>,
    /// Valor en la escala fija del motor aritmético, para poder derivar la
    /// diferencia y el porcentaje sin volver a consultar el acervo.
    pub units: i128,
}

impl ConversationState {
    pub fn has_context(&self) -> bool {
        self.set.is_some()
            || self.last_result.is_some()
            || self.identifier.is_some()
            || self.comparison.is_some()
            || self.document.is_some()
    }
}

/// Almacén de conversaciones vivas. La clave la elige la interfaz; el motor
/// nunca fusiona dos claves ni consulta una conversación distinta de la que
/// recibió.
#[derive(Clone, Default)]
pub struct ConversationMemory {
    inner: Arc<Mutex<Store>>,
}

#[derive(Default)]
struct Store {
    states: HashMap<String, ConversationState>,
    /// Orden de llegada, para acotar cuántas conversaciones se recuerdan.
    order: Vec<String>,
}

impl ConversationMemory {
    pub fn state(&self, conversation: &str) -> ConversationState {
        self.inner
            .lock()
            .map(|store| store.states.get(conversation).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn store(&self, conversation: &str, state: ConversationState) {
        let Ok(mut store) = self.inner.lock() else {
            return;
        };
        if !store.states.contains_key(conversation) {
            store.order.push(conversation.to_owned());
            while store.order.len() > MAX_CONVERSATIONS {
                let oldest = store.order.remove(0);
                store.states.remove(&oldest);
            }
        }
        store.states.insert(conversation.to_owned(), state);
    }

    /// Descarta la evidencia y los cálculos recordados de todas las
    /// conversaciones, conservando el predicado de cada conjunto.
    ///
    /// Se invoca al reindexar o revocar una fuente: las citas guardadas dejan
    /// de existir en ese momento, mientras que el predicado sigue siendo
    /// válido porque se reevalúa contra el índice nuevo.
    pub fn invalidate_results(&self) {
        if let Ok(mut store) = self.inner.lock() {
            for state in store.states.values_mut() {
                state.last_result = None;
            }
        }
    }

    /// Borra una conversación. Es lo que ejecuta «Nueva conversación»: el
    /// contexto no se degrada ni se hereda, desaparece.
    pub fn reset(&self, conversation: &str) {
        if let Ok(mut store) = self.inner.lock() {
            store.states.remove(conversation);
            store.order.retain(|id| id != conversation);
        }
    }
}

/// Cómo alude una pregunta al turno anterior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reference {
    /// No alude a nada: la pregunta se sostiene sola.
    None,
    /// Alude explícitamente («esos», «compáralo», «los anteriores»).
    Explicit,
}

/// Palabras que sólo pueden referirse a algo dicho antes. No incluye nombres
/// de campos ni de negocio: son deícticos del español.
const DEICTIC: &[&str] = &[
    "eso",
    "esos",
    "esa",
    "esas",
    "ese",
    "estos",
    "estas",
    "ellos",
    "ellas",
    "aquellos",
    "aquellas",
    "dichos",
    "dichas",
    "mismo",
    "misma",
    "mismos",
    "mismas",
];

/// Palabras que aluden a lo anterior sólo cuando no forman parte de una
/// expresión de calendario («el mes anterior» es un periodo, no una anáfora).
const ORDINAL_REFERENCE: &[&str] = &["anterior", "anteriores", "previo", "previos"];

/// Raíces de operación que, con un pronombre pegado, forman una referencia
/// («compáralo», «súmalos», «cuéntalos»).
const CLITIC_ROOTS: &[&str] = &[
    "compar", "sum", "cuent", "promedi", "rest", "agrup", "desglos", "orden", "list", "muestr",
];

/// Decide si la pregunta alude al turno anterior.
///
/// Es deliberadamente literal: reconoce pronombres y verbos con pronombre
/// pegado, no intenciones. Una pregunta que no contiene ninguna de esas marcas
/// jamás hereda el contexto por su cuenta.
pub fn reference_in(question: &str) -> Reference {
    let normalized = normalize_exact(question);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let deictic = words.iter().any(|word| DEICTIC.contains(word));
    let clitic = words.iter().any(|word| is_clitic_reference(word));
    let ordinal = words.iter().enumerate().any(|(index, word)| {
        ORDINAL_REFERENCE.contains(word)
            && !matches!(
                index.checked_sub(1).and_then(|previous| words.get(previous)),
                Some(&"mes") | Some(&"ano") | Some(&"periodo") | Some(&"trimestre")
            )
    });
    if deictic || clitic || ordinal {
        Reference::Explicit
    } else {
        Reference::None
    }
}

/// Posición dentro del conjunto que el turno anterior dejó delante, nombrada
/// por un ordinal («el primero», «la última», «el primer documento»).
///
/// No es un deíctico ni una operación: es una forma más de aludir al turno
/// anterior, y como las demás sólo significa algo cuando la conversación ya
/// tiene un conjunto. Sin contexto no se adivina nada — quien la consulta
/// comprueba el contexto antes de usarla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrdinalPosition {
    /// Índice contado desde el principio, empezando en 0.
    Nth(usize),
    /// El último del conjunto, cualquiera que sea su tamaño.
    Last,
}

/// Ordinales del español, con la posición que nombran. La lista se detiene en
/// el quinto a propósito: más allá nadie cuenta de memoria, y el usuario
/// escribe el dato que busca en vez de su posición.
const ORDINAL_WORDS: &[(&str, usize)] = &[
    ("primero", 0),
    ("primer", 0),
    ("primera", 0),
    ("segundo", 1),
    ("segunda", 1),
    ("tercero", 2),
    ("tercer", 2),
    ("tercera", 2),
    ("cuarto", 3),
    ("cuarta", 3),
    ("quinto", 4),
    ("quinta", 4),
];

const LAST_WORDS: &[&str] = &["ultimo", "ultima"];

/// Sustantivos genéricos de continente que un ordinal puede modificar sin
/// dejar de señalar una posición del resultado anterior («el primer
/// documento»). No nombran ningún giro de negocio.
const ORDINAL_HEADS: &[&str] = &[
    "documento",
    "documentos",
    "archivo",
    "archivos",
    "expediente",
    "expedientes",
    "registro",
    "registros",
    "resultado",
    "resultados",
];

/// Palabras de calendario que un ordinal describe en vez de señalar una
/// posición: «el primer trimestre» es un periodo, no el primer documento.
const ORDINAL_CALENDAR: &[&str] = &[
    "trimestre",
    "semestre",
    "mes",
    "ano",
    "dia",
    "semana",
    "bimestre",
    "cuatrimestre",
];

/// Posición ordinal que la pregunta señala del resultado anterior.
///
/// Deliberadamente literal, igual que `reference_in`: sólo se reconoce el
/// ordinal **nominalizado** —el que no modifica a ningún sustantivo, «¿cuál es
/// el Responsable del primero?»— y el que modifica a un continente genérico
/// («el primer documento»). Un ordinal pegado a cualquier otro sustantivo
/// describe ese sustantivo y no se toca: «el primer trimestre» sigue siendo un
/// periodo.
pub fn ordinal_position_in(question: &str) -> Option<OrdinalPosition> {
    let normalized = normalize_exact(question);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    words.iter().enumerate().find_map(|(index, word)| {
        let position = if LAST_WORDS.contains(word) {
            OrdinalPosition::Last
        } else {
            OrdinalPosition::Nth(
                ORDINAL_WORDS
                    .iter()
                    .find_map(|(name, at)| (name == word).then_some(*at))?,
            )
        };
        match words.get(index + 1) {
            // Ordinal nominalizado: no modifica a nada, así que sólo puede
            // señalar una posición de lo que ya está delante.
            None => Some(position),
            Some(next) if ORDINAL_HEADS.contains(next) => Some(position),
            Some(next) if ORDINAL_CALENDAR.contains(next) => None,
            // Cualquier otro sustantivo: el ordinal lo describe a él.
            Some(_) => None,
        }
    })
}

fn is_clitic_reference(word: &str) -> bool {
    let stem = ["los", "las", "lo", "la"]
        .iter()
        .find_map(|suffix| word.strip_suffix(suffix))
        .unwrap_or("");
    stem.len() >= 4 && CLITIC_ROOTS.iter().any(|root| stem.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pending() -> PendingChoice {
        PendingChoice {
            question: "¿Cuánto suman?".into(),
            set: DocumentSet::default(),
            options: vec!["Cantidad".into(), "Precio unitario".into(), "Monto principal".into()],
            kind: PendingKind::Concept,
        }
    }

    #[test]
    fn wants_all_recognizes_the_generic_word_but_not_a_valid_option() {
        let pending = sample_pending();
        assert!(pending.wants_all("todos"));
        assert!(pending.wants_all("ponme todos"));
        assert!(pending.wants_all("dame todas"));
        assert!(pending.wants_all("ambos"));
        assert!(!pending.wants_all("Monto principal"));
        assert!(!pending.wants_all("ninguno"));
        assert!(!pending.wants_all("una frase larga que no tiene relación con las opciones ofrecidas"));
    }

    #[test]
    fn deictic_and_clitic_questions_point_at_the_previous_turn() {
        assert_eq!(reference_in("¿cuánto suman esos?"), Reference::Explicit);
        assert_eq!(reference_in("compáralo con marzo"), Reference::Explicit);
        assert_eq!(
            reference_in("de esos contratos, ¿cuáles son locales?"),
            Reference::Explicit
        );
        assert_eq!(
            reference_in("¿qué documentos respaldan ese total?"),
            Reference::Explicit
        );
        assert_eq!(reference_in("los anteriores"), Reference::Explicit);
    }

    #[test]
    fn a_calendar_phrase_is_not_an_anaphora_by_itself() {
        // "el mes anterior" nombra un periodo; sin un pronombre no convierte
        // una pregunta autónoma en una continuación.
        assert_eq!(
            reference_in("¿cuántos documentos hay del mes anterior?"),
            Reference::None
        );
        assert_eq!(
            reference_in("¿cuántas actas hay con estado abierto?"),
            Reference::None
        );
    }

    #[test]
    fn a_new_conversation_starts_without_context() {
        let memory = ConversationMemory::default();
        let mut state = ConversationState::default();
        state.concept = Some("Importe".into());
        memory.store("c1", state);
        assert!(memory.state("c1").has_context() || memory.state("c1").concept.is_some());
        memory.reset("c1");
        assert!(memory.state("c1").concept.is_none());
        // Otra conversación nunca ve el estado de la primera.
        assert!(memory.state("c2").concept.is_none());
    }

    #[test]
    fn a_nominalized_ordinal_points_at_a_position_of_the_previous_set() {
        assert_eq!(
            ordinal_position_in("¿Cuál es el Responsable del primero?"),
            Some(OrdinalPosition::Nth(0))
        );
        assert_eq!(
            ordinal_position_in("¿y el segundo?"),
            Some(OrdinalPosition::Nth(1))
        );
        assert_eq!(
            ordinal_position_in("dame el último"),
            Some(OrdinalPosition::Last)
        );
        // Modificando un continente genérico sigue siendo una posición.
        assert_eq!(
            ordinal_position_in("¿cuál es el Folio del primer documento?"),
            Some(OrdinalPosition::Nth(0))
        );
    }

    #[test]
    fn an_ordinal_that_describes_another_noun_is_not_a_reference() {
        // Calendario: nombra un periodo, no una posición.
        assert_eq!(ordinal_position_in("¿cuánto se gastó en el primer trimestre?"), None);
        assert_eq!(ordinal_position_in("el segundo semestre de 2024"), None);
        // Cualquier otro sustantivo: el ordinal lo describe a él.
        assert_eq!(ordinal_position_in("¿cuál es el primer proveedor de la lista de compras?"), None);
    }

    #[test]
    fn the_store_forgets_the_oldest_conversations() {
        let memory = ConversationMemory::default();
        for index in 0..(MAX_CONVERSATIONS + 5) {
            let mut state = ConversationState::default();
            state.concept = Some(format!("campo-{index}"));
            memory.store(&format!("c{index}"), state);
        }
        assert!(memory.state("c0").concept.is_none());
        assert_eq!(
            memory
                .state(&format!("c{}", MAX_CONVERSATIONS + 4))
                .concept
                .as_deref(),
            Some(&format!("campo-{}", MAX_CONVERSATIONS + 4)[..])
        );
    }
}
