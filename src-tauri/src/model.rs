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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrStatus {
    NotRequired,
    Pending,
    Complete,
    LowConfidence,
    Failed,
}

impl OcrStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Complete => "complete",
            // Las bases creadas antes de esta versión validan los estados con
            // un CHECK más estrecho. La confianza persiste en su propia
            // columna y conserva la marca sin romper índices existentes.
            Self::LowConfidence => "complete",
            Self::Failed => "failed",
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub values: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub value: f64,
    pub matched_values: i64,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool: String,
    pub data: serde_json::Value,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
    pub mode: String,
    pub verified: bool,
    pub citations: Vec<Evidence>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub sources: i64,
    pub documents: i64,
    pub concepts: i64,
    pub values: i64,
}
