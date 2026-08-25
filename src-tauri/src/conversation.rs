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
}

impl ComputationMemory {
    pub fn new(
        operation: &str,
        concept: &str,
        rendered: String,
        value_count: usize,
        evidence: Vec<Evidence>,
    ) -> Self {
        Self {
            operation: operation.to_owned(),
            concept: concept.to_owned(),
            rendered,
            value_count,
            evidence: evidence.into_iter().take(MAX_REMEMBERED_EVIDENCE).collect(),
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
