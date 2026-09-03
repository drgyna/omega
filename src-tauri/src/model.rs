use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub text: String,
    /// Fragmentos ya ubicados por el parser cuando el formato ofrece una
    /// unidad más precisa que líneas de texto plano (celda, párrafo o página).
    pub chunks: Vec<ParsedChunk>,
    pub records: Vec<ParsedRecord>,
    pub parser: String,
    pub ocr_status: OcrStatus,
    pub ocr_confidence: Option<f64>,
    /// Lo que el parser no pudo dar por bueno: una fórmula sin resultado, un
    /// resultado en caché que el propio archivo desautoriza, una parte
    /// ilegible. Viaja al reporte de indexación para que la omisión quede
    /// visible en vez de desaparecer.
    pub warnings: Vec<String>,
    /// El contenido real del archivo no corresponde a su extensión declarada.
    /// Lleva el nombre de lo que el contenido resultó ser («texto plano»,
    /// «PDF», «ZIP/OOXML»). `None` es el caso normal: la extensión dice la
    /// verdad, o el formato no tiene una firma con la que contrastarla.
    pub declared_format_mismatch: Option<String>,
}

/// Estado OCR de un documento. Son categorías disjuntas y ninguna puede
/// sustituir a otra: la diferencia entre «no hizo falta», «no hay motor»,
/// «el motor falló» y «salió con poca confianza» es justo lo que permite
/// distinguir un documento leído de uno que nadie pudo leer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrStatus {
    /// El formato trae texto propio; no se intentó OCR.
    NotRequired,
    /// Hay motor OCR, pero este archivo todavía no se procesó.
    Pending,
    /// El motor leyó el archivo con confianza suficiente.
    Complete,
    /// El motor leyó el archivo por debajo del umbral de confianza.
    LowConfidence,
    /// El motor corrió y no entregó texto utilizable.
    Failed,
    /// No hay motor OCR disponible en este equipo: el archivo queda omitido.
    Unavailable,
}

impl OcrStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Complete => "complete",
            // Una lectura dudosa se guarda como tal. Colapsarla en «complete»
            // borraba la única marca que distingue un documento leído de uno
            // que nadie pudo leer, y dejaba la confianza como único rastro —
            // un rastro que se pierde en cuanto la columna es NULL.
            Self::LowConfidence => "low_confidence",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }

    /// Un estado sólo es fiable cuando el texto del documento procede de una
    /// lectura completa: ni pendiente, ni fallida, ni omitida, ni de baja
    /// confianza. La ausencia de un número de confianza no vuelve fiable un
    /// estado que ya dice que nadie pudo leer el archivo.
    pub fn is_reliable(self) -> bool {
        matches!(self, Self::NotRequired | Self::Complete)
    }

    /// Estado tal como quedó persistido en el índice. Un valor desconocido
    /// nunca se degrada a «completo»: se trata como fallido.
    pub fn from_stored(value: &str) -> Self {
        match value {
            "not_required" => Self::NotRequired,
            "pending" => Self::Pending,
            "complete" => Self::Complete,
            "low_confidence" => Self::LowConfidence,
            "unavailable" => Self::Unavailable,
            _ => Self::Failed,
        }
    }

    /// Texto para avisos y reportes dirigidos a una persona.
    pub fn description(self) -> &'static str {
        match self {
            Self::NotRequired => "OCR no requerido",
            Self::Pending => "OCR pendiente",
            Self::Complete => "OCR completo",
            Self::LowConfidence => "OCR de baja confianza",
            Self::Failed => "OCR fallido",
            Self::Unavailable => "OCR no disponible en este equipo",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedChunk {
    pub location: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ParsedRecord {
    pub label: String,
    pub value: String,
    pub location: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Money,
    Date,
    Percentage,
    Number,
    State,
    Text,
}

impl ValueKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Money => "money",
            Self::Date => "date",
            Self::Percentage => "percentage",
            Self::Number => "number",
            Self::State => "state",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedValue {
    pub kind: ValueKind,
    pub text_value: String,
    pub normalized_value: String,
    /// Clave de igualdad literal: sólo pliega mayúsculas y acentos, nunca
    /// puntuación. Es la que decide si dos filtros son «el mismo valor» — a
    /// diferencia de `normalized_value`, no borra el «%» ni las comillas, así
    /// que «50» y «50%», o «Pendiente» y ««Pendiente»», nunca colapsan en la
    /// misma clave sólo porque compartan dígitos o letras.
    pub literal_value: String,
    pub numeric_value: Option<f64>,
    pub currency: Option<String>,
    pub date_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSummary {
    pub id: i64,
    pub path: String,
    pub document_count: i64,
    pub indexed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexReport {
    pub source_id: i64,
    pub discovered: usize,
    pub indexed: usize,
    #[serde(default)]
    pub modified: usize,
    pub skipped: usize,
    pub ocr_pending: usize,
    /// Documentos indexados cuyo texto salió de un OCR por debajo del umbral
    /// de confianza. Su evidencia existe, pero nunca se declara verificada.
    #[serde(default)]
    pub ocr_low_confidence: usize,
    /// Archivos en los que el motor OCR corrió y no entregó texto utilizable.
    #[serde(default)]
    pub ocr_failed: usize,
    /// Archivos que habrían necesitado OCR y quedaron sin procesar porque no
    /// hay motor en este equipo. Se cuentan aparte de un fallo real: no es lo
    /// mismo que el motor fallara a que no exista.
    #[serde(default)]
    pub ocr_unavailable: usize,
    /// Grupos de documentos con contenido byte a byte idéntico.
    ///
    /// Política: los duplicados **no** se descartan ni alteran ningún conteo.
    /// Dos copias de un archivo pueden ser un error de archivo o dos entregas
    /// reales, y el índice no puede decidirlo. Se conservan, se cuentan y se
    /// nombran, y toda respuesta que se apoye en ellos lo advierte.
    #[serde(default)]
    pub duplicate_groups: usize,
    /// Documentos que pertenecen a algún grupo duplicado (todas las copias,
    /// no sólo las «sobrantes»: ninguna es más original que otra).
    #[serde(default)]
    pub duplicate_documents: usize,
    pub values: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Reparto del tiempo de la indexación por fase, en milisegundos. No es
    /// telemetría: es el único dato con el que se puede decidir qué optimizar
    /// sin adivinar. Se mide siempre —un reloj por fase es más barato que
    /// cualquiera de las fases— y viaja en el mismo reporte que el resto.
    #[serde(default)]
    pub phases: IndexPhases,
    pub elapsed_ms: u128,
}

/// Tiempo por fase de una indexación. Las fases son disjuntas y su suma es
/// prácticamente `elapsed_ms`: lo que no cae en ninguna es el recorrido del
/// bucle, que no hace trabajo propio.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexPhases {
    /// Recorrer la carpeta autorizada y filtrar por extensión.
    pub discover_ms: u128,
    /// Borrar el índice anterior de la fuente (y su cascada) antes de
    /// reindexar. Es cero en una base limpia.
    pub purge_ms: u128,
    /// Analizar el archivo: incluye el OCR cuando el documento lo necesita.
    /// El desglose por parser dice cuánto de esto es OCR.
    pub parse_ms: u128,
    /// Calcular el SHA-256 del archivo para detectar cambios.
    pub hash_ms: u128,
    /// Escribir documento, fragmentos, conceptos y valores en SQLite.
    pub insert_ms: u128,
    /// Segunda pasada sobre la carátula de los PDF con capa de texto, ya con
    /// el vocabulario de rótulos completo del acervo. No vuelve a abrir
    /// ningún archivo: relee los fragmentos ya guardados.
    #[serde(default)]
    pub cover_pass_ms: u128,
    /// Campos que aportó esa segunda pasada, aparte de los de la primera.
    #[serde(default)]
    pub cover_pass_values: usize,
    /// Limpieza de conceptos, retipado, detección de duplicados y `COMMIT`.
    pub finalize_ms: u128,
    /// Tiempo de análisis agrupado por el parser que lo hizo, ordenado de más
    /// a menos. Es lo que distingue «el OCR domina» de «el OCR es un detalle».
    #[serde(default)]
    pub parse_ms_by_parser: Vec<(String, u128)>,
    /// Documentos analizados por cada parser, para poder leer el coste por
    /// documento y no sólo el total.
    #[serde(default)]
    pub documents_by_parser: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    pub id: String,
    pub document_id: i64,
    pub path: String,
    /// Carpeta relativa a la fuente autorizada de la que procede el documento.
    /// Se calcula al indexar; nunca se infiere a partir de una ruta no autorizada.
    pub origin: String,
    pub location: String,
    pub excerpt: String,
    /// Representación usada para comparar. En identificadores conserva una
    /// clave canónica separada del valor original mostrado al usuario.
    #[serde(default)]
    pub normalized_value: Option<String>,
    pub value: Option<String>,
    /// Texto literal usado para resaltar la coincidencia en la interfaz.
    pub matched: Option<String>,
    /// Etiqueta del campo extraído cuando la coincidencia procede de un valor
    /// estructurado. Las coincidencias FTS no inventan un campo.
    pub field: Option<String>,
    /// exacta, campo o texto. No es una interpretación del contenido.
    pub match_kind: String,
    /// OCR de baja confianza se expone, pero queda explícitamente marcado.
    pub reliable: bool,
    /// Estado OCR del documento del que procede la evidencia. Una nota de
    /// cálculo hereda el de su operando; nunca se inventa un estado nuevo.
    #[serde(default)]
    pub ocr_status: Option<String>,
    /// Confianza OCR del documento del que procede la evidencia.
    #[serde(default)]
    pub ocr_confidence: Option<f64>,
    /// Alias histórico para clientes ya existentes. Debe ser igual a
    /// `ocr_confidence` cuando la evidencia procede de OCR.
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub score: f64,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptSummary {
    pub key: String,
    pub display_name: String,
    pub value_type: String,
    pub occurrences: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToolFilter {
    pub concept: String,
    pub equals: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateRequest {
    pub concept: String,
    pub operation: String,
    #[serde(default)]
    pub filters: Vec<ToolFilter>,
    /// Carpeta de origen descubierta en el propio índice. No es una etiqueta
    /// de negocio: permite limitar cualquier agregación a una fuente real.
    #[serde(default)]
    pub origin: Option<String>,
    pub currency: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub group_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateRow {
    pub group: Option<String>,
    /// Moneda de esta fila. Una suma sin moneda explícita se separa por esta
    /// dimensión para impedir que importes incompatibles se combinen.
    #[serde(default)]
    pub currency: Option<String>,
    /// Resultado decimal ya renderizado por el motor local. Nunca se expone
    /// una suma como `f64`: perdería precisión y no expresa su moneda.
    pub value: String,
    pub matched_values: i64,
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub has_unreliable_evidence: bool,
}

/// Resultado de la única política pública de agregación. El alcance y sus
/// exclusiones se publican junto con las filas para que nadie pueda interpretar
/// una suma parcial como un total completo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateResult {
    pub rows: Vec<AggregateRow>,
    pub document_count: i64,
    pub value_count: i64,
    pub excluded_count: i64,
    pub missing_field_count: i64,
    pub invalid_value_count: i64,
    pub currency_mismatch_count: i64,
    pub verified: bool,
    pub warning: Option<String>,
    pub has_unreliable_evidence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub data: serde_json::Value,
    pub evidence: Vec<Evidence>,
}

/// Prefijo con el que se identifica una cita que no procede del contenido del
/// documento sino de un metadato del índice (nombre de archivo, carpeta,
/// extensión). Es una convención de `location` que ya usaban las tres rutas
/// que las construyen; aquí se le da nombre para poder razonar sobre ella.
pub const METADATA_LOCATION_PREFIX: &str = "metadato:";

impl Evidence {
    /// ¿Esta cita aporta algo legible, o sólo dice que el documento existe?
    ///
    /// Un metadato **con valor** sí aporta: «carpeta de origen = calidad» o
    /// «formato = DOCX» son hechos del acervo que sostienen un conteo. Un
    /// metadato **sin valor** —el nombre de archivo que la propia pregunta
    /// acaba de escribir— no sostiene nada: repetirlo no es haber encontrado
    /// información. Una respuesta cuyas únicas citas sean de esa clase no
    /// puede presentarse como verificada ni como una respuesta con hallazgos.
    pub fn is_substantive(&self) -> bool {
        !self.location.trim_start().starts_with(METADATA_LOCATION_PREFIX)
            || self
                .value
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}

/// Restricción de fecha anclada a un campo concreto del acervo.
///
/// El ancla importa: sin ella, un documento con varias fechas puede satisfacer
/// el extremo inferior con una y el superior con otra, y quedar dentro de un
/// rango al que no pertenece por ninguna de las dos.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DateConstraint {
    pub concept: String,
    pub from: String,
    pub to: String,
}

impl DateConstraint {
    pub fn label(&self) -> String {
        format!("{} entre {} y {}", self.concept, self.from, self.to)
    }
}

/// Alcance efectivo de una respuesta. Es lo que la interfaz muestra como
/// «filtros y alcance aplicados»: no es prosa, son los mismos datos que el
/// motor usó para consultar.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AnswerScope {
    #[serde(default)]
    pub filters: Vec<ToolFilter>,
    pub origin: Option<String>,
    /// Campo sobre el que se calculó, cuando la respuesta es un cálculo.
    pub concept: Option<String>,
    pub group_by: Option<String>,
    pub date: Option<DateConstraint>,
    pub currency: Option<String>,
    /// Documentos que entraron en el alcance, cuando el motor los contó.
    pub document_count: Option<i64>,
    /// Cuántos valores individuales alimentaron el cálculo.
    pub value_count: Option<i64>,
    /// Documentos del alcance que no participaron en el cálculo (por campo
    /// faltante, valor inválido, moneda incompatible o división entre cero).
    /// El motivo de cada exclusión va en el texto de la respuesta; este
    /// número es sólo el total, para que alcance/usados/excluidos se puedan
    /// leer por separado sin tener que restar.
    #[serde(default)]
    pub excluded_count: Option<i64>,
    /// El alcance se heredó del turno anterior de la conversación.
    #[serde(default)]
    pub inherited: bool,
}

impl AnswerScope {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Pregunta de aclaración. Omega la emite en lugar de adivinar a qué se
/// refiere una referencia ambigua o qué campo debe usar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clarification {
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// Motivo legible por máquina; permite a la interfaz y a las pruebas
    /// distinguir una aclaración de otra sin leer el texto.
    pub reason: String,
}

/// Un documento que Omega abrió y leyó completo para redactar el resumen.
/// No sustituye a la cita: la acompaña. `citation_numbers` son los mismos
/// números con los que la interfaz numera la evidencia, para que el lector
/// pueda saltar de una frase del resumen a la cita que la sostiene.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadDocument {
    pub path: String,
    pub origin: String,
    pub citation_numbers: Vec<usize>,
    pub passages_read: usize,
    /// Falso cuando el documento llegó por OCR de baja confianza. Se publica
    /// junto a la lectura por el mismo motivo que en la evidencia: lo que se
    /// leyó mal no puede presentarse como si se hubiera leído bien.
    pub reliable: bool,
}

/// Un hecho publicado por la lectura, unido a la evidencia exacta de la que
/// salió. No se reutilizan las citas generales de la respuesta: el resumen
/// puede mencionar otros campos del documento y cada uno necesita su propio
/// rastro navegable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadingClaim {
    pub text: String,
    pub evidence: Evidence,
}

/// Lectura de los documentos ya citados, redactada con reglas. Vive aparte de
/// `Answer::text` a propósito: la respuesta verificada no cambia porque su
/// resumen se pueda componer o no.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerReading {
    pub text: String,
    pub documents: Vec<ReadDocument>,
    /// Hechos atómicos que aparecen en el resumen, cada uno con su evidencia.
    #[serde(default)]
    pub claims: Vec<ReadingClaim>,
    /// Cobertura declarada, separada de la lista potencialmente recortada.
    pub documents_matched: usize,
    pub documents_read: usize,
    /// El detalle por documento se recortó. Nunca se recorta la cantidad de
    /// documentos leídos; sólo cuánto se cuenta de cada uno.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Answer {
    pub text: String,
    pub mode: String,
    pub verified: bool,
    pub citations: Vec<Evidence>,
    pub warning: Option<String>,
    /// La respuesta se apoyó en el resultado del turno anterior.
    #[serde(default)]
    pub used_context: bool,
    #[serde(default)]
    pub scope: Option<AnswerScope>,
    #[serde(default)]
    pub clarification: Option<Clarification>,
    /// Resumen de los documentos citados, leídos completos. Es un añadido:
    /// `text` y `citations` son idénticos con lectura y sin ella.
    #[serde(default)]
    pub reading: Option<AnswerReading>,
}

impl Answer {
    /// Respuesta local con evidencia. Es el único constructor que marca una
    /// respuesta como verificada.
    pub fn verified(text: impl Into<String>, citations: Vec<Evidence>) -> Self {
        let has_unreliable_ocr = citations.iter().any(|evidence| !evidence.reliable);
        // Citas que sólo señalan la existencia del documento (el nombre de
        // archivo que la pregunta ya traía) no sostienen ninguna afirmación
        // sobre su contenido. El candado vive aquí, junto al de OCR débil,
        // porque éste es el único constructor que puede marcar `verified`:
        // así ninguna ruta futura puede saltárselo por descuido.
        let only_existence = !citations.is_empty()
            && !citations.iter().any(Evidence::is_substantive);
        Self {
            text: text.into(),
            mode: "local".into(),
            verified: !has_unreliable_ocr && !only_existence,
            citations,
            warning: if has_unreliable_ocr {
                Some(
                    "Resultado no verificado: la evidencia incluye OCR de baja confianza."
                        .to_owned(),
                )
            } else if only_existence {
                Some(
                    "Resultado no verificado: la única evidencia es la existencia del documento, no su contenido."
                        .to_owned(),
                )
            } else {
                None
            },
            ..Self::default()
        }
    }

    /// Respuesta que no afirma nada del acervo: sin evidencia no hay hecho.
    pub fn unverified(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            mode: "local".into(),
            verified: false,
            ..Self::default()
        }
    }

    pub fn with_scope(mut self, scope: AnswerScope) -> Self {
        self.used_context = scope.inherited;
        self.scope = (!scope.is_empty()).then_some(scope);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub sources: i64,
    pub documents: i64,
    pub concepts: i64,
    pub values: i64,
}
