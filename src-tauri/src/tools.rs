use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::LazyLock,
};

use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    calc::{self, Operand, Operation},
    census,
    db::Database,
    error::{OmegaError, Result},
    extract::classify_value,
    model::{
        AggregateRequest, AggregateResult, AggregateRow, ConceptSummary, DateConstraint, Evidence,
        OcrStatus, SearchHit, ToolFilter, ToolResult, ValueKind,
    },
    normalize::{
        canonical_identifier, canonical_key, normalize_exact, normalize_literal, normalize_spanish,
        search_terms, stems_match,
    },
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FieldValuePair {
    field: String,
    value: String,
}

/// Documento alcanzado por una clave de localización (ID interno de
/// indexación o ruta). Deliberadamente no lleva `Evidence`: la clave que lo
/// encontró no es citable, así que la respuesta debe tomar su evidencia de los
/// valores realmente extraídos del documento.
#[derive(Clone, Debug)]
pub struct LocatedDocument {
    pub id: i64,
    pub path: String,
    pub origin: String,
}

/// Un valor estructurado leído de un documento concreto, con su posición
/// dentro de ese documento. Nunca cruza la frontera con la interfaz: sólo lo
/// consume la capa de síntesis para resolver un campo puntual.
#[derive(Clone, Debug)]
pub struct DocumentValue {
    /// Posición del valor dentro de su propio documento, en orden de
    /// extracción. Que un identificador aparezca entre los primeros campos
    /// distingue al documento que trata de esa entidad de otro que sólo la
    /// menciona de pasada.
    pub ordinal: usize,
    pub field: String,
    pub value: String,
    pub value_type: String,
    pub identifier_canonical: Option<String>,
    /// La capa de entidades ya reconoció este valor como el nombre de una
    /// entidad. Es la única marca del esquema que distingue a quién nombra un
    /// campo de lo que ese campo mide, y la usa la resolución de «¿quién?»,
    /// que no tiene ninguna categoría de valor a la que traducirse.
    pub is_entity: bool,
    pub evidence: Evidence,
}

/// Lo que el acervo tiene escrito sobre el signo de un campo numérico.
#[derive(Clone, Copy, Debug)]
pub struct SignRecord {
    /// Valores del campo que el índice pudo leer como número.
    pub numeric: i64,
    /// Cuántos de ellos son negativos.
    pub negative: i64,
}

#[derive(Clone, Debug)]
pub struct OriginSummary {
    pub origin: String,
    pub document_count: i64,
    pub evidence: Evidence,
}

#[derive(Clone, Debug)]
pub struct DocumentQueryResult {
    pub document_count: i64,
    /// Evidencia de una cantidad acotada de documentos. Para intersecciones
    /// contiene una cita por cada filtro aplicado al mismo `document_id`.
    pub evidence: Vec<Evidence>,
}

#[derive(Clone, Debug)]
pub struct TextQueryResult {
    pub document_count: usize,
    pub hits: Vec<SearchHit>,
}

#[derive(Clone, Copy)]
enum IdentifierMode {
    Exact,
    Prefix,
    Contains,
}

impl IdentifierMode {
    fn label(self) -> &'static str {
        match self {
            Self::Exact => "canónica",
            Self::Prefix => "prefijo",
            Self::Contains => "contiene",
        }
    }

    fn matches(self, candidate: &str, query: &str) -> bool {
        match self {
            Self::Exact => candidate == query,
            Self::Prefix => candidate.starts_with(query),
            Self::Contains => candidate.contains(query),
        }
    }
}

#[derive(Clone)]
pub struct ToolEngine {
    database: Database,
}

impl ToolEngine {
    pub fn new(database: Database) -> Self {
        Self { database }
    }

    pub fn definitions() -> Value {
        json!([
            {
                "type": "function",
                "name": "list_concepts",
                "description": "Lista los conceptos que realmente existen en el acervo local. Úsala antes de asumir vocabulario.",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": ["string", "null"] } },
                    "required": ["query"],
                    "additionalProperties": false
                }
            },
            {
                "type": "function",
                "name": "search_documents",
                "description": "Busca texto y valores en los documentos locales y devuelve evidencia citable.",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 20 },
                        "filters": { "type": "array", "items": { "$ref": "#/$defs/filter" } }
                    },
                    "required": ["query", "limit", "filters"],
                    "additionalProperties": false,
                    "$defs": { "filter": {
                        "type": "object",
                        "properties": { "concept": { "type": "string" }, "equals": { "type": "string" } },
                        "required": ["concept", "equals"],
                        "additionalProperties": false
                    }}
                }
            },
            {
                "type": "function",
                "name": "exact_lookup",
                "description": "Identifica un expediente, folio, nombre o identificador exacto y devuelve evidencia.",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": { "value": { "type": "string" } },
                    "required": ["value"],
                    "additionalProperties": false
                }
            },
            {
                "type": "function",
                "name": "aggregate_values",
                "description": "Suma, cuenta o agrupa valores tipados con filtros y evidencia exacta.",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "concept": { "type": "string" },
                        "operation": { "type": "string", "enum": ["sum", "count"] },
                        "filters": { "type": "array", "items": { "$ref": "#/$defs/filter" } },
                        "origin": { "type": ["string", "null"] },
                        "currency": { "type": ["string", "null"] },
                        "date_from": { "type": ["string", "null"] },
                        "date_to": { "type": ["string", "null"] },
                        "group_by": { "type": ["string", "null"] }
                    },
                    "required": ["concept", "operation", "filters", "origin", "currency", "date_from", "date_to", "group_by"],
                    "additionalProperties": false,
                    "$defs": { "filter": {
                        "type": "object",
                        "properties": { "concept": { "type": "string" }, "equals": { "type": "string" } },
                        "required": ["concept", "equals"],
                        "additionalProperties": false
                    }}
                }
            },
            {
                "type": "function",
                "name": "count_documents",
                "description": "Cuenta documentos distintos que cumplen filtros semánticos y devuelve muestras de evidencia.",
                "strict": true,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filters": { "type": "array", "items": { "$ref": "#/$defs/filter" } }
                    },
                    "required": ["filters"],
                    "additionalProperties": false,
                    "$defs": { "filter": {
                        "type": "object",
                        "properties": { "concept": { "type": "string" }, "equals": { "type": "string" } },
                        "required": ["concept", "equals"],
                        "additionalProperties": false
                    }}
                }
            }
        ])
    }

    pub fn execute(&self, name: &str, arguments: &Value) -> Result<ToolResult> {
        match name {
            "list_concepts" => {
                let query = arguments.get("query").and_then(Value::as_str);
                let concepts = self.list_concepts(query)?;
                Ok(ToolResult {
                    tool: name.into(),
                    data: serde_json::to_value(concepts).unwrap(),
                    evidence: vec![],
                })
            }
            "search_documents" => {
                let query = required_string(arguments, "query")?;
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(8)
                    .clamp(1, 20) as usize;
                let filters = parse_filters(arguments.get("filters"))?;
                let hits = self.search(query, &filters, limit)?;
                let evidence = hits.iter().map(|hit| hit.evidence.clone()).collect();
                Ok(ToolResult {
                    tool: name.into(),
                    data: serde_json::to_value(hits).unwrap(),
                    evidence,
                })
            }
            "exact_lookup" => {
                let hits = self.exact_lookup(required_string(arguments, "value")?, 20)?;
                let evidence = hits.iter().map(|hit| hit.evidence.clone()).collect();
                Ok(ToolResult {
                    tool: name.into(),
                    data: serde_json::to_value(hits).unwrap(),
                    evidence,
                })
            }
            "count_documents" => {
                let filters = parse_filters(arguments.get("filters"))?;
                let (count, evidence) = self.count_documents(&filters)?;
                Ok(ToolResult {
                    tool: name.into(),
                    data: json!({ "count": count, "filters": filters }),
                    evidence,
                })
            }
            "aggregate_values" => {
                let request: AggregateRequest = serde_json::from_value(arguments.clone())
                    .map_err(|error| OmegaError::InvalidArguments(error.to_string()))?;
                let result = self.aggregate(&request)?;
                let mut evidence = calculation_evidence(&request, &result)
                    .into_iter()
                    .collect::<Vec<_>>();
                evidence.extend(result.rows.iter().flat_map(|row| row.evidence.clone()));
                Ok(ToolResult {
                    tool: name.into(),
                    data: serde_json::to_value(result).unwrap(),
                    evidence,
                })
            }
            _ => Err(OmegaError::InvalidArguments(format!(
                "herramienta desconocida: {name}"
            ))),
        }
    }

    /// Cuántos valores numéricos registra el acervo para un campo, y cuántos
    /// de ellos son negativos.
    ///
    /// Es el único dato que hace falta para saber si un signo negativo es una
    /// forma normal de usar ese campo —una nota de crédito, un ajuste, una
    /// devolución, una desviación— o una rareza que no se parece a nada de lo
    /// que el acervo tiene escrito. No se consulta ningún vocabulario ni se
    /// supone nada sobre el giro del negocio: se cuenta lo que el índice ya
    /// contiene, en el momento de responder.
    ///
    /// El campo se busca por su clave canónica, la misma con la que el índice
    /// agrupa un concepto, para que dos escrituras del mismo nombre no
    /// cuenten por separado.
    pub fn field_sign_record(&self, field: &str) -> Result<SignRecord> {
        let connection = self.database.connect()?;
        let key = canonical_key(field);
        let (numeric, negative) = connection.query_row(
            "SELECT COUNT(v.numeric_value),
                    COALESCE(SUM(CASE WHEN v.numeric_value < 0 THEN 1 ELSE 0 END), 0)
             FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             WHERE c.canonical_key = ?1 AND v.numeric_value IS NOT NULL",
            params![key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        Ok(SignRecord { numeric, negative })
    }

    pub fn list_concepts(&self, query: Option<&str>) -> Result<Vec<ConceptSummary>> {
        let connection = self.database.connect()?;
        let normalized = query.map(normalize_spanish).unwrap_or_default();
        let pattern = format!("%{normalized}%");
        let mut statement = connection.prepare(
            "SELECT c.canonical_key, c.display_name, c.value_type, COUNT(v.id) occurrences
             FROM concepts c
             LEFT JOIN extracted_values v ON v.concept_id = c.id
             WHERE ?1 = '' OR c.canonical_key LIKE ?2 OR EXISTS (
                SELECT 1 FROM concept_aliases a
                WHERE a.concept_id = c.id AND a.normalized_alias LIKE ?2 AND a.status != 'rejected'
             )
             GROUP BY c.id
             ORDER BY occurrences DESC, c.display_name",
        )?;
        let rows = statement.query_map(params![normalized, pattern], |row| {
            Ok(ConceptSummary {
                key: row.get(0)?,
                display_name: row.get(1)?,
                value_type: row.get(2)?,
                occurrences: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn search(
        &self,
        query: &str,
        filters: &[ToolFilter],
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        if let Some((mode, value)) = explicit_identifier_mode(query) {
            if let Some(canonical) = canonical_identifier(&value) {
                return self.canonical_identifier_hits(&canonical, mode, limit);
            }
            if matches!(mode, IdentifierMode::Contains) {
                return self.explicit_text_contains(&value, limit);
            }
            return Ok(vec![]);
        }
        if query_contains_filename(query) {
            crate::trace!("g) search(): ruta strict_exact_search (la pregunta trae un nombre de archivo)");
            return self.strict_exact_search(&exact_query_tokens(query), limit);
        }
        let canonical_identifiers = canonical_identifier_candidates(query);
        if !canonical_identifiers.is_empty() {
            return self.canonical_identifier_hits(
                &canonical_identifiers[0],
                IdentifierMode::Exact,
                limit,
            );
        }
        // Una consulta natural puede incluir un valor que debe resolverse de
        // forma literal. En ese caso no es válido completar los resultados
        // con el nombre del campo ni con coincidencias FTS por prefijo: la
        // intención exacta domina toda la recuperación.
        let exact_tokens = exact_query_tokens(query);
        if !exact_tokens.is_empty() {
            crate::trace!("g) search(): ruta strict_exact_search por tokens exactos {exact_tokens:?}");
            return self.strict_exact_search(&exact_tokens, limit);
        }
        // Si el acervo reconoce a la vez un campo y uno de sus valores dentro
        // de la pregunta, la combinación es una condición obligatoria. No se
        // permite completar esta respuesta con FTS, metadatos ni otro campo
        // que sólo comparta alguna palabra del nombre.
        if let Some(hits) = self.strict_structured_hits(query, filters, limit)? {
            crate::trace!(
                "g) search(): ruta strict_structured_hits MANDA y devuelve {} hits (corta FTS y metadatos)",
                hits.len()
            );
            for hit in hits.iter().take(8) {
                crate::trace!("g)   strict #: score={:.2} campo={:?} doc={}", hit.score, hit.evidence.field, hit.evidence.path.rsplit('/').next().unwrap_or(""));
            }
            return Ok(hits);
        }
        crate::trace!("g) search(): strict_structured_hits no manda; sigue por metadatos+campos+FTS");
        // "exactamente AB" expresa una intención literal pero AB no es un
        // identificador completo (ni un archivo ni una frase citada). En vez
        // de ampliar a prefijos, se devuelve cero evidencia.
        if requests_exact_but_incomplete(query) {
            return Ok(vec![]);
        }
        let terms = search_terms(query);
        if terms.is_empty() {
            return Err(OmegaError::InvalidArguments(
                "la búsqueda está vacía".into(),
            ));
        }
        let mut by_document = self.metadata_hits(query, false)?;
        crate::trace!("g) search(): metadata_hits -> {} documentos", by_document.len());
        let structured = self.structured_hits(query, filters, false)?;
        crate::trace!("g) search(): structured_hits -> {} documentos", structured.len());
        for hit in structured.into_values() {
            keep_best(&mut by_document, hit);
        }
        // Los términos obligatorios son los de CONTENIDO. La gramática de la
        // consulta —«estoy buscando…», «¿cuándo…?»— no describe el documento
        // y exigirla dentro del AND anulaba búsquedas cuyas palabras de
        // contenido coincidían todas. Si la pregunta fuera sólo gramática no
        // quedaría nada que exigir, así que en ese caso se conservan tal cual.
        let required = {
            let filtered = content_terms(query);
            if filtered.is_empty() {
                terms.clone()
            } else {
                filtered
            }
        };
        let fts_query = required
            .iter()
            .map(|term| format!("\"{}\"*", term.replace('"', "")))
            .collect::<Vec<_>>()
            // Una coincidencia de una palabra común no basta para convertir un
            // membrete repetido en resultado. El FTS solo complementa a los
            // campos extraídos y exige todos los términos útiles.
            .join(" AND ");
        crate::trace!("g) search(): FTS query = {fts_query:?}");
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    c.location, c.content, bm25(chunks_fts), c.id
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE chunks_fts MATCH ?",
        );
        let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(fts_query)];
        append_filters(&mut sql, &mut values, filters);
        // Mismo motivo que en `search_text`: el orden final lo decide el
        // ranking de más abajo, no el `bm25` de un fragmento suelto. Un corte
        // aquí elegiría por adelantado qué documentos pueden competir. Se
        // conserva un candidato por documento (`keep_best`), así que recorrer
        // todas las coincidencias no crece con el número de fragmentos.
        sql.push_str(" ORDER BY bm25(chunks_fts)");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| {
                let document_id: i64 = row.get(0)?;
                let chunk_id: i64 = row.get(9)?;
                let content: String = row.get(7)?;
                let location: String = row.get(6)?;
                Ok(SearchHit {
                    title: row.get(1)?,
                    score: 20.0 + row.get::<_, f64>(8)?.abs(),
                    evidence: chunk_evidence(
                        format!("c-{chunk_id}"),
                        document_id,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        &location,
                        &content,
                        &terms,
                    ),
                })
            },
        )?;
        for hit in rows.collect::<rusqlite::Result<Vec<_>>>()? {
            keep_best(&mut by_document, hit);
        }
        let mut hits = by_document.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn exact_lookup(&self, value: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.strict_exact_search(&[value.trim().to_owned()], limit)
    }

    /// Recuperación cerrada para una o más claves literales. No reutiliza la
    /// ruta normal de FTS (que permite prefijos) y sólo admite un documento si
    /// contiene el valor completo como metadato, campo extraído o texto.
    fn strict_exact_search(&self, values: &[String], limit: usize) -> Result<Vec<SearchHit>> {
        let mut by_document = HashMap::new();
        for value in values.iter().filter(|value| !value.trim().is_empty()) {
            for hit in self.metadata_hits(value, true)?.into_values() {
                keep_best(&mut by_document, hit);
            }
            for hit in self.structured_hits(value, &[], true)?.into_values() {
                keep_best(&mut by_document, hit);
            }
            for hit in self.exact_text_hits(value)?.into_values() {
                keep_best(&mut by_document, hit);
            }
        }
        let mut hits = by_document.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// Igualdad canónica para identificadores mixtos. La comparación elimina
    /// sólo separadores equivalentes y se realiza contra el valor completo;
    /// no usa FTS ni coincidencias por prefijo salvo que el modo sea explícito.
    fn canonical_identifier_hits(
        &self,
        identifier: &str,
        mode: IdentifierMode,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let connection = self.database.connect()?;
        let mut by_document = HashMap::new();

        let mut values_sql = String::from(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    v.location, v.excerpt, v.text_value, v.evidence_id, c.display_name,
                    v.identifier_canonical
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE v.identifier_canonical IS NOT NULL",
        );
        let value_parameter = match mode {
            IdentifierMode::Exact => {
                values_sql.push_str(" AND v.identifier_canonical = ?");
                identifier.to_owned()
            }
            IdentifierMode::Prefix => {
                values_sql.push_str(" AND v.identifier_canonical LIKE ?");
                format!("{identifier}%")
            }
            IdentifierMode::Contains => {
                values_sql.push_str(" AND v.identifier_canonical LIKE ?");
                format!("%{identifier}%")
            }
        };
        let mut values_statement = connection.prepare(&values_sql)?;
        let value_rows = values_statement.query_map([value_parameter], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?;
        for row in value_rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                excerpt,
                text_value,
                evidence_id,
                field,
                canonical,
            ) = row?;
            if !mode.matches(&canonical, identifier) {
                continue;
            }
            keep_best(
                &mut by_document,
                SearchHit {
                    title,
                    score: 140.0,
                    evidence: Evidence {
                        id: evidence_id,
                        document_id,
                        path,
                        origin,
                        location,
                        excerpt: brief_excerpt(&excerpt, Some(&text_value)),
                        normalized_value: Some(canonical),
                        value: Some(text_value.clone()),
                        matched: Some(text_value),
                        field: Some(field),
                        match_kind: mode.label().into(),
                        reliable: ocr_is_reliable(&ocr_status, confidence),
                        ocr_status: Some(ocr_status),
                        ocr_confidence: confidence,
                        confidence,
                    },
                },
            );
        }

        let mut chunk_statement = connection.prepare(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    c.location, c.content, c.id
             FROM chunks c JOIN documents d ON d.id = c.document_id",
        )?;
        let chunk_rows = chunk_statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        for row in chunk_rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                content,
                chunk_id,
            ) = row?;
            let Some(original) = identifiers_in_text(&content).into_iter().find(|candidate| {
                canonical_identifier(candidate)
                    .as_deref()
                    .is_some_and(|candidate| mode.matches(candidate, identifier))
            }) else {
                continue;
            };
            let mut evidence = chunk_evidence(
                format!("c-{chunk_id}"),
                document_id,
                path,
                origin,
                ocr_status,
                confidence,
                &location,
                &content,
                &search_terms(&original),
            );
            evidence.value = Some(original.clone());
            evidence.matched = Some(original);
            evidence.normalized_value = Some(identifier.to_owned());
            evidence.match_kind = mode.label().into();
            keep_best(
                &mut by_document,
                SearchHit {
                    title,
                    score: 130.0,
                    evidence,
                },
            );
        }
        let mut hits = by_document.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn explicit_text_contains(&self, value: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let normalized = normalize_literal(value);
        if normalized.trim().is_empty() {
            return Ok(vec![]);
        }
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    c.location, c.content, c.id
             FROM chunks c JOIN documents d ON d.id = c.document_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        let terms = search_terms(value);
        let mut by_document = HashMap::new();
        for row in rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                content,
                chunk_id,
            ) = row?;
            if !normalize_literal(&content).contains(&normalized) {
                continue;
            }
            let mut evidence = chunk_evidence(
                format!("c-{chunk_id}"),
                document_id,
                path,
                origin,
                ocr_status,
                confidence,
                &location,
                &content,
                &terms,
            );
            if !normalize_literal(&evidence.excerpt).contains(&normalized) {
                continue;
            }
            evidence.value = Some(value.to_owned());
            evidence.matched = Some(value.to_owned());
            evidence.normalized_value = Some(normalized.clone());
            evidence.match_kind = IdentifierMode::Contains.label().into();
            keep_best(
                &mut by_document,
                SearchHit {
                    title,
                    score: 115.0,
                    evidence,
                },
            );
        }
        let mut hits = by_document.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| left.title.cmp(&right.title));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Fallback literal para valores que no llegaron a convertirse en un
    /// campo. FTS se usa sólo para localizar candidatos; se valida después la
    /// frase completa con límites de palabra, por lo que un prefijo, sufijo o
    /// una coincidencia aislada del nombre de un campo queda descartada.
    fn exact_text_hits(&self, value: &str) -> Result<HashMap<i64, SearchHit>> {
        if normalize_literal(value).trim().is_empty() {
            return Ok(HashMap::new());
        }
        // Se conserva la frase original para la consulta FTS. La
        // normalización se reserva para la comprobación posterior, ya que
        // aplicar stemming antes de FTS rompería frases literales plurales o
        // acentuadas.
        let fts_phrase = format!("\"{}\"", value.replace('"', ""));
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    c.location, c.content, bm25(chunks_fts), c.id
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE chunks_fts MATCH ?
             ORDER BY bm25(chunks_fts)",
        )?;
        let rows = statement.query_map([fts_phrase], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, f64>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })?;
        let terms = search_terms(value);
        let mut hits = HashMap::new();
        for row in rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                content,
                rank,
                chunk_id,
            ) = row?;
            if !literal_occurs_as_complete_value(&content, value) {
                continue;
            }
            let mut evidence = chunk_evidence(
                format!("c-{chunk_id}"),
                document_id,
                path,
                origin,
                ocr_status,
                confidence,
                &location,
                &content,
                &terms,
            );
            // La cita debe contener el valor completo, no sólo uno de los
            // términos con los que FTS tokenizó el identificador.
            if !literal_occurs_as_complete_value(&evidence.excerpt, value) {
                continue;
            }
            evidence.matched = Some(value.to_owned());
            evidence.match_kind = "exacta".into();
            keep_best(
                &mut hits,
                SearchHit {
                    title,
                    score: 110.0 + rank.abs(),
                    evidence,
                },
            );
        }
        Ok(hits)
    }

    /// Devuelve `Some` cuando la consulta contiene al menos un par
    /// campo–valor existente. Los dos componentes se comprueban en la misma
    /// fila de `extracted_values`, con normalización literal (sin stemming),
    /// para no equiparar valores como "vencida" y "vencido".
    fn strict_structured_hits(
        &self,
        query: &str,
        filters: &[ToolFilter],
        limit: usize,
    ) -> Result<Option<Vec<SearchHit>>> {
        let pairs = self.structured_pairs_in_query(query, filters)?;
        crate::trace!("g) structured_pairs_in_query -> {:?}", pairs.values().collect::<Vec<_>>());
        if pairs.is_empty() {
            return if self.query_names_field_with_value(query)? {
                crate::trace!("g) strict_structured_hits: forma campo-valor sin valor existente -> CIERRA con 0 hits");
                // La consulta sí tiene forma campo–valor, pero el valor no
                // existe para ese campo. Es importante cerrar aquí: permitir
                // FTS devolvería documentos con el mismo campo y otro valor.
                Ok(Some(vec![]))
            } else {
                Ok(None)
            };
        }
        let mut required_filters = filters.to_vec();
        // El filtro exige el valor literal, no su forma normalizada: la
        // normalización quita puntuación («$3,300 MXN» pierde el signo, la
        // coma y queda «3 300 mxn»), y un filtro construido con esa forma ya
        // no clasifica al mismo tipo que la fila real, así que nunca la
        // encontraría.
        required_filters.extend(pairs.values().map(|(field, value)| ToolFilter {
            concept: field.clone(),
            equals: value.clone(),
        }));
        crate::trace!("g) strict_structured_hits: filtros OBLIGATORIOS (AND en el mismo documento) = {required_filters:?}");
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence, v.location, v.excerpt,
                    v.text_value, v.evidence_id, c.display_name
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE 1 = 1",
        );
        let mut values: Vec<Box<dyn ToSql>> = vec![];
        // Cada par campo-valor es obligatorio en el mismo documento. La
        // versión anterior filtraba las filas después del SELECT y terminaba
        // uniendo documentos que cumplían sólo una de las condiciones.
        append_filters(&mut sql, &mut values, &required_filters);
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )?;
        let mut by_document = HashMap::new();
        for row in rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                excerpt,
                text_value,
                evidence_id,
                field,
            ) = row?;
            let pair = FieldValuePair {
                field: normalize_exact(&field),
                value: normalize_exact(&text_value),
            };
            if !pairs.contains_key(&pair) {
                continue;
            }
            keep_best(
                &mut by_document,
                SearchHit {
                    title,
                    score: 125.0,
                    evidence: Evidence {
                        id: evidence_id,
                        document_id,
                        path,
                        origin,
                        location,
                        excerpt: brief_excerpt(&excerpt, Some(&text_value)),
                        normalized_value: Some(normalize_exact(&text_value)),
                        value: Some(text_value.clone()),
                        matched: Some(text_value),
                        field: Some(field),
                        match_kind: "campo".into(),
                        reliable: ocr_is_reliable(&ocr_status, confidence),
                        ocr_status: Some(ocr_status),
                        ocr_confidence: confidence,
                        confidence,
                    },
                },
            );
        }
        let mut hits = by_document.into_values().collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        hits.truncate(limit);
        Ok(Some(hits))
    }

    /// Pares campo–valor que la pregunta nombra literalmente, ya confirmados
    /// contra el acervo. La clave es la forma normalizada (para comparar sin
    /// puntuación ni mayúsculas); el valor guardado es la pareja tal como está
    /// escrita en el documento, porque un filtro construido a partir de la
    /// forma normalizada perdería la puntuación que distingue su tipo (por
    /// ejemplo, entre un importe y el mismo número sin moneda).
    fn structured_pairs_in_query(
        &self,
        query: &str,
        filters: &[ToolFilter],
    ) -> Result<HashMap<FieldValuePair, (String, String)>> {
        let normalized_query = normalize_exact(query);
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT DISTINCT c.display_name, v.text_value
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE 1 = 1",
        );
        let mut values: Vec<Box<dyn ToSql>> = vec![];
        append_filters(&mut sql, &mut values, filters);
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut pairs = HashMap::new();
        for row in rows {
            let (field, value) = row?;
            let pair = FieldValuePair {
                field: normalize_exact(&field),
                value: normalize_exact(&value),
            };
            if pair.field != pair.value
                && whole_phrase_in(&normalized_query, &pair.field)
                && whole_phrase_in(&normalized_query, &pair.value)
            {
                pairs.entry(pair).or_insert((field, value));
            }
        }
        // Si "Estado" y "Estado del documento" son conceptos distintos,
        // prevalece el nombre de campo más específico para el mismo valor.
        // Así la presencia de una palabra compartida no abre otro concepto.
        let candidates = pairs.keys().cloned().collect::<HashSet<_>>();
        pairs.retain(|pair, _| {
            !candidates.iter().any(|other| {
                other.value == pair.value
                    && other.field != pair.field
                    && other.field.split_whitespace().count()
                        > pair.field.split_whitespace().count()
                    && whole_phrase_in(&other.field, &pair.field)
            })
        });
        Ok(pairs)
    }

    /// Reconoce una petición campo–valor aun cuando el valor no exista. Se
    /// basa en los nombres de concepto ya extraídos, en la posición del texto
    /// que sigue al campo y en las carpetas de origen que el propio acervo ya
    /// autorizó; no contiene vocabulario de un rubro de negocio.
    fn query_names_field_with_value(&self, query: &str) -> Result<bool> {
        const QUERY_FILLER: &[&str] = &[
            "busca",
            "buscar",
            "encuentra",
            "encontrar",
            "muestra",
            "mostrar",
            "lista",
            "listar",
            "documento",
            "documentos",
            "archivo",
            "archivos",
            "campo",
            "campos",
            "cual",
            "cuales",
            "es",
            "son",
            "del",
            "de",
            "la",
            "las",
            "el",
            "los",
            "un",
            "una",
            "con",
            "para",
            "por",
            "en",
            "sobre",
            "the",
            "a",
            "an",
            "of",
            "for",
            "with",
            "find",
            "show",
            "list",
            "document",
            "documents",
            "field",
            "fields",
        ];
        let normalized_query = normalize_exact(query);
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT display_name, value_type FROM concepts
             ORDER BY length(display_name) DESC",
        )?;
        let fields = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Las carpetas de origen ya autorizadas son metadato del propio
        // acervo, igual que en `metadata_hits`/`structured_hits`. Sirven para
        // reconocer cuándo lo que sigue al campo describe una categoría de
        // documentos ("de mantenimiento", "de propiedades") en vez de un
        // intento de valor.
        let origins = connection
            .prepare("SELECT DISTINCT origin FROM documents")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|origin| normalize_exact(&origin))
            .collect::<Vec<_>>();
        // Los campos se recorren del nombre más largo al más corto. Cuando uno
        // de ellos ya explica su tramo de la pregunta sin dejar nada pendiente
        // -o con un pendiente que no es un valor real, ver más abajo-, ese
        // tramo queda resuelto: un campo más corto contenido por completo en
        // él (p.ej. "estado", dentro de "Estado de la propiedad") no puede
        // reabrir el cierre sólo porque, aislado, su propio resto de frase ya
        // no es puro relleno. Sin esto, "¿cuál es el estado de la propiedad?"
        // se cerraba a cero: el campo correcto y más específico no dejaba
        // ningún valor pendiente, pero el campo corto "estado" —mero prefijo
        // del anterior— sí veía "de la propiedad" como un intento de valor.
        let mut resolved_spans: Vec<(usize, usize)> = Vec::new();
        for (field, value_type) in fields {
            let normalized_field = normalize_exact(&field);
            let Some(start) = phrase_position(&normalized_query, &normalized_field) else {
                continue;
            };
            let end = start + normalized_field.len();
            if resolved_spans
                .iter()
                .any(|&(resolved_start, resolved_end)| {
                    resolved_start <= start && end <= resolved_end
                })
            {
                continue;
            }
            let after = &normalized_query[end..];
            if after
                .split_whitespace()
                .any(|word| !QUERY_FILLER.contains(&word))
            {
                // Un campo numérico o de fecha sólo se cierra si lo que sigue
                // podría ser en sí mismo un valor de ese tipo (un dígito). Sin
                // este filtro, "el costo estimado de mantenimiento" se leía
                // como un intento de valor fallido -"de mantenimiento"-, en
                // vez de cómo lo que realmente es: cómo la pregunta describe
                // el campo en lenguaje natural.
                if requires_numeric_shape(&value_type) && !after.chars().any(|c| c.is_ascii_digit())
                {
                    resolved_spans.push((start, end));
                    continue;
                }
                // Lo mismo para un campo de texto o estado cuando lo que sigue
                // nombra una carpeta de origen real ("de propiedades", "de
                // mantenimiento"): sigue describiendo el campo, no proponiendo
                // un valor. Fuera de ese caso, cualquier palabra sí puede ser
                // un valor legítimo (p.ej. "cuyo estado sea Vendida").
                let significant = after
                    .split_whitespace()
                    .filter(|word| !QUERY_FILLER.contains(word))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !significant.is_empty()
                    && origins
                        .iter()
                        .any(|origin| whole_phrase_in(origin, &significant))
                {
                    resolved_spans.push((start, end));
                    continue;
                }
                return Ok(true);
            }
            resolved_spans.push((start, end));
        }
        Ok(false)
    }

    /// Los nombres de archivo y las carpetas son metadatos autorizados del
    /// documento. Se consultan sin convertirlos en texto inventado dentro del
    /// archivo y funcionan incluso si el parser no extrajo campos.
    fn metadata_hits(&self, query: &str, exact_only: bool) -> Result<HashMap<i64, SearchHit>> {
        let exact = exact_fragments(query);
        let normalized_query = normalize_spanish(query);
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, title, path, origin, ocr_status, ocr_confidence FROM documents ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<f64>>(5)?,
            ))
        })?;
        let mut hits = HashMap::new();
        for row in rows {
            let (document_id, title, path, origin, ocr_status, ocr_confidence) = row?;
            let title_match = exact
                .iter()
                .any(|needle| normalize_spanish(&title) == *needle);
            let origin_match = phrase_in(&normalized_query, &normalize_spanish(&origin));
            let origin_says_it_all =
                origin_match && !query_says_more_than_the_origin(query, &origin);
            let (score, location, excerpt, field) = if title_match {
                (
                    130.0,
                    "metadato: nombre de archivo",
                    title.clone(),
                    "nombre de archivo",
                )
            } else if !exact_only && origin_match {
                (
                    // El nombre de la carpeta sólo puntúa como evidencia
                    // cuando la pregunta no dice nada más: entonces la
                    // procedencia ES lo consultado. Si la pregunta añade
                    // cualquier palabra de contenido, la carpeta pasa por
                    // debajo de una coincidencia real en el texto (20 + bm25),
                    // porque compartir carpeta con lo preguntado no dice nada
                    // de lo que el documento contiene — y una carpeta grande
                    // arrastraba miles de documentos por delante del que sí
                    // tenía el contenido pedido.
                    if origin_says_it_all { 90.0 } else { 15.0 },
                    "metadato: carpeta de origen",
                    origin.clone(),
                    "carpeta de origen",
                )
            } else {
                continue;
            };
            keep_best(
                &mut hits,
                SearchHit {
                    title,
                    score,
                    evidence: Evidence {
                        id: format!("m-{document_id}-{field}"),
                        document_id,
                        path,
                        origin,
                        location: location.into(),
                        excerpt: excerpt.clone(),
                        normalized_value: None,
                        value: None,
                        matched: Some(excerpt.clone()),
                        field: Some(field.into()),
                        match_kind: if title_match { "exacta" } else { "campo" }.into(),
                        reliable: ocr_is_reliable(&ocr_status, ocr_confidence),
                        ocr_status: Some(ocr_status),
                        ocr_confidence,
                        confidence: ocr_confidence,
                    },
                },
            );
        }
        Ok(hits)
    }

    /// Recupera por metadatos y campos antes de consultar texto libre. Los
    /// resultados siempre se reducen a una evidencia por documento.
    fn structured_hits(
        &self,
        query: &str,
        filters: &[ToolFilter],
        exact_only: bool,
    ) -> Result<HashMap<i64, SearchHit>> {
        let normalized_query = normalize_spanish(query);
        let terms = search_terms(query);
        let exact = exact_fragments(query);
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence, v.location, v.excerpt,
                    v.text_value, v.normalized_value, v.evidence_id, c.display_name,
                    c.canonical_key, v.value_type
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE 1 = 1",
        );
        let mut values: Vec<Box<dyn ToSql>> = vec![];
        append_filters(&mut sql, &mut values, filters);
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )?;
        let mut hits = HashMap::new();
        for row in rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                ocr_confidence,
                location,
                excerpt,
                text_value,
                normalized_value,
                evidence_id,
                field,
                _canonical_field,
                value_type,
            ) = row?;
            let title_match = exact
                .iter()
                .any(|needle| normalize_spanish(&title) == *needle);
            let value_match = exact.iter().any(|needle| normalized_value == *needle);
            let field_terms = search_terms(&field);
            let field_match = !field_terms.is_empty()
                && field_terms
                    .iter()
                    .all(|term| terms.iter().any(|query_term| stems_match(query_term, term)));
            let field_overlap = field_terms
                .iter()
                .any(|term| terms.iter().any(|query_term| stems_match(query_term, term)));
            let value_phrase = phrase_in(&normalized_query, &normalized_value)
                && normalized_value.split_whitespace().count() > 0;
            let origin_match = phrase_in(&normalized_query, &normalize_spanish(&origin));

            let score = if value_match || title_match {
                Some(120.0)
            } else if value_phrase
                && (field_match
                    || field_overlap
                    || value_type == "state"
                    || normalized_value.split_whitespace().count() >= 2)
            {
                Some(105.0)
            } else if field_match {
                Some(80.0)
            } else if origin_match {
                // Misma razón que en `metadata_hits`: coincidir con el nombre
                // de la carpeta no es evidencia de contenido en cuanto la
                // pregunta dice algo más que ese nombre.
                Some(if query_says_more_than_the_origin(query, &origin) {
                    15.0
                } else {
                    76.0
                })
            } else {
                None
            };
            if let Some(score) = score {
                let evidence = Evidence {
                    id: evidence_id,
                    document_id,
                    path,
                    origin,
                    location,
                    excerpt: brief_excerpt(&excerpt, Some(&text_value)),
                    normalized_value: Some(normalize_exact(&text_value)),
                    value: Some(text_value.clone()),
                    matched: Some(text_value.clone()),
                    field: Some(field),
                    match_kind: if value_match || title_match {
                        "exacta"
                    } else {
                        "campo"
                    }
                    .into(),
                    reliable: ocr_is_reliable(&ocr_status, ocr_confidence),
                    ocr_status: Some(ocr_status),
                    ocr_confidence,
                    confidence: ocr_confidence,
                };
                keep_best(
                    &mut hits,
                    SearchHit {
                        title,
                        score,
                        evidence,
                    },
                );
            }
        }
        if exact_only {
            hits.retain(|_, hit| hit.score >= 120.0);
        }
        Ok(hits)
    }

    pub fn concept_values(&self, concept: &str) -> Result<Vec<String>> {
        let concept_id = resolve_concept(&self.database, concept)?.ok_or_else(|| {
            OmegaError::InvalidArguments(format!("el concepto '{concept}' no existe"))
        })?;
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT text_value FROM extracted_values
             WHERE concept_id = ?1 ORDER BY text_value LIMIT 200",
        )?;
        let rows = statement.query_map([concept_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Valores estructurados de un documento en el orden en que el parser los
    /// encontró. Es una lectura puntual y aislada: no participa en la
    /// recuperación, no altera qué documentos encuentra `search` ni cómo se
    /// ordenan sus resultados.
    ///
    /// El orden se toma del propio `id` de la fila. El indexado reconstruye
    /// cada documento completo en una sola transacción e inserta sus registros
    /// en el orden en que el parser los leyó, así que la posición dentro del
    /// documento ya está disponible sin añadir una columna al esquema (y sin
    /// obligar a reindexar una base ya existente).
    pub fn document_values(&self, document_id: i64) -> Result<Vec<DocumentValue>> {
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT v.evidence_id, c.display_name, v.text_value, v.value_type,
                    v.identifier_canonical, v.location, v.excerpt,
                    d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    EXISTS(SELECT 1 FROM entities e WHERE e.evidence_id = v.evidence_id)
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE v.document_id = ?1
             ORDER BY v.id",
        )?;
        let rows = statement.query_map([document_id], |row| {
            let evidence_id: String = row.get(0)?;
            let field: String = row.get(1)?;
            let text_value: String = row.get(2)?;
            let value_type: String = row.get(3)?;
            let identifier_canonical: Option<String> = row.get(4)?;
            let location: String = row.get(5)?;
            let excerpt: String = row.get(6)?;
            let ocr_status: String = row.get(9)?;
            let confidence: Option<f64> = row.get(10)?;
            let is_entity: bool = row.get(11)?;
            Ok(DocumentValue {
                // La posición real la asigna el recorrido, no la consulta.
                ordinal: 0,
                field: field.clone(),
                value: text_value.clone(),
                value_type,
                identifier_canonical,
                is_entity,
                evidence: Evidence {
                    id: evidence_id,
                    document_id,
                    path: row.get(7)?,
                    origin: row.get(8)?,
                    location,
                    excerpt: brief_excerpt(&excerpt, Some(&text_value)),
                    normalized_value: Some(normalize_exact(&text_value)),
                    value: Some(text_value.clone()),
                    matched: Some(text_value),
                    field: Some(field),
                    match_kind: "campo".into(),
                    reliable: ocr_is_reliable(&ocr_status, confidence),
                    ocr_status: Some(ocr_status),
                    ocr_confidence: confidence,
                    confidence,
                },
            })
        })?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .enumerate()
            .map(|(ordinal, mut value)| {
                value.ordinal = ordinal;
                value
            })
            .collect())
    }

    /// ¿La pregunta nombra este valor, palabra por palabra?
    ///
    /// Mismo criterio que las anclas de `pinned_document`: hace falta un tramo
    /// de al menos dos palabras consecutivas del valor escrito en la pregunta.
    /// Una palabra suelta no lo nombra, sólo coincide con él.
    pub fn value_named_by(question: &str, value: &str) -> bool {
        let normalized_value = normalize_exact(value);
        let words = normalized_value.split_whitespace().collect::<Vec<_>>();
        longest_run_named_by(&normalize_exact(question), &words).is_some()
    }

    /// Documento único al que apuntan, combinadas, las pistas de la pregunta.
    ///
    /// Ninguna pista sale de una lista escrita en el código: son (a) valores
    /// del propio acervo que la pregunta nombra y que pertenecen a un solo
    /// campo —el ancla— y (b) el resto de las palabras de contenido de la
    /// pregunta, comprobadas contra el texto, el nombre y la carpeta de cada
    /// candidato. Un acervo distinto produce otras anclas sin tocar el motor.
    ///
    /// Sólo devuelve un documento cuando gana **solo**: un empate no elige
    /// —devuelve `None` y la pregunta sigue exactamente el camino de antes—,
    /// porque quedarse con uno de dos candidatos indistinguibles sería
    /// adivinar cuál lee.
    ///
    /// `context` es la ruta del documento del que ya hablaba la conversación.
    /// Sin ancla, la única forma de fijar uno es que la pregunta lo vuelva a
    /// describir entero: **todas** sus palabras de contenido tienen que estar
    /// en ese documento. Basta una que no esté para que la continuación deje
    /// de hablar de él y la pregunta vuelva a la búsqueda normal.
    pub fn pinned_document(&self, query: &str, context: Option<&str>) -> Result<Option<i64>> {
        // Con una clave escrita en la pregunta no hace falta ninguna de estas
        // pistas: el identificador ya localiza el documento por sí solo y su
        // ruta —cerrada y literal— es más precisa que cualquier cruce de
        // palabras. Esta ruta es justo para cuando esa clave no está.
        if !canonical_identifier_candidates(query).is_empty()
            || explicit_identifier_mode(query).is_some()
        {
            crate::trace!("f) pinned_document: ABORTA, la pregunta trae clave/identificador escrito");
            return Ok(None);
        }
        let clues = content_terms(query);
        crate::trace!("f) pinned_document: pistas (content_terms) = {:?}", clues);
        if clues.is_empty() {
            crate::trace!("f) pinned_document: ABORTA, sin pistas");
            return Ok(None);
        }
        let inherited = match context {
            Some(path) => self.document_by_path(path)?.map(|document| document.id),
            None => None,
        };
        let anchored = self.anchored_documents(query)?;
        crate::trace!(
            "f) anchored_documents -> {}",
            match &anchored {
                None => "None (la pregunta NO anclo nada)".to_string(),
                Some(set) => format!("{} candidatos", set.len()),
            }
        );
        if let Some(mut candidates) = anchored {
            // El documento del que ya hablaba la conversación compite como un
            // candidato más. No gana por estar ahí: gana sólo si cubre más
            // pistas que cualquier otro, que es justo lo que significa que la
            // pregunta lo siga describiendo a él.
            if let Some(document_id) = inherited {
                candidates.insert(document_id);
            }
            let decision = match candidates.len() {
                0 => Ok(None),
                1 => Ok(candidates.iter().copied().next()),
                _ => self.best_covered(&candidates, &clues, inherited),
            };
            crate::trace!("f) pinned_document DECIDE: {:?}", decision.as_ref().ok());
            return decision;
        }
        // Sin ancla, la única forma de seguir hablando del mismo documento es
        // que la pregunta lo vuelva a describir entero. Una sola palabra de
        // contenido no describe un documento: describe una categoría.
        if clues.len() < 2 {
            crate::trace!("f) pinned_document: sin ancla y <2 pistas -> None");
            return Ok(None);
        }
        let Some(document_id) = inherited else {
            crate::trace!("f) pinned_document: sin ancla y sin documento heredado -> None");
            return Ok(None);
        };
        let covered = self.covered_clues(document_id, &clues)?;
        crate::trace!(
            "f) pinned_document: sin ancla, heredado cubre {covered}/{} pistas",
            clues.len()
        );
        Ok((covered == clues.len()).then_some(document_id))
    }

    /// Documentos que cumplen todas las anclas de la pregunta.
    ///
    /// Un ancla es un tramo de la pregunta que nombra, palabra por palabra, a
    /// un valor ya extraído del acervo —«Roble Grupo» dentro de «Roble Grupo
    /// (CLI-2020-0056)»— y que en todo el acervo pertenece a **un solo**
    /// campo. Esa unicidad es lo que la vuelve una condición y no una
    /// coincidencia: si el mismo texto fuera valor de dos campos distintos, no
    /// se sabría cuál está nombrando la pregunta y no se ancla nada.
    ///
    /// Se exige un tramo de dos palabras como mínimo. Una palabra suelta
    /// («ventas») aparece dentro de demasiados valores como para señalar a
    /// ninguno.
    ///
    /// `None` significa «la pregunta no ancló nada», que no es lo mismo que
    /// «ancló y no hay documentos» (conjunto vacío).
    fn anchored_documents(&self, query: &str) -> Result<Option<HashSet<i64>>> {
        let normalized_query = normalize_exact(query);
        if normalized_query.is_empty() {
            return Ok(None);
        }
        let query_words = normalized_query
            .split_whitespace()
            .collect::<HashSet<&str>>();
        let connection = self.database.connect()?;
        let mut statement =
            connection.prepare("SELECT document_id, concept_id, text_value FROM extracted_values")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut by_run: HashMap<String, (HashSet<i64>, HashSet<i64>)> = HashMap::new();
        for row in rows {
            let (document_id, concept_id, text_value) = row?;
            let normalized = normalize_exact(&text_value);
            let words = normalized.split_whitespace().collect::<Vec<_>>();
            if words.len() < 2 || !words.iter().any(|word| query_words.contains(word)) {
                continue;
            }
            let Some(run) = longest_run_named_by(&normalized_query, &words) else {
                continue;
            };
            let entry = by_run.entry(run).or_default();
            entry.0.insert(concept_id);
            entry.1.insert(document_id);
        }
        // Las anclas se suman, no se intersecan. Una pregunta puede nombrar de
        // pasada un valor de otro documento —«una minuta de ventas» es, en
        // algún archivo, el título literal de otra minuta— y exigir que TODAS
        // se cumplan a la vez dejaba el conjunto vacío justo cuando una de
        // ellas sí era la buena. Cumplir varias no se pierde: las palabras de
        // cada ancla son también pistas, así que el documento que las cumple
        // todas cubre más que el que sólo cumple una y gana por su cuenta.
        let mut candidates: Option<HashSet<i64>> = None;
        for (run, (concepts, documents)) in by_run.into_iter() {
            if concepts.len() != 1 {
                crate::trace!(
                    "f) ancla DESCARTADA por regla `concepts.len() != 1`: tramo {:?} pertenece a {} campos, en {} documentos",
                    run, concepts.len(), documents.len()
                );
                continue;
            }
            crate::trace!(
                "f) ancla ACEPTADA: tramo {:?} (1 campo), aporta {} documentos",
                run, documents.len()
            );
            candidates
                .get_or_insert_with(HashSet::new)
                .extend(documents);
        }
        // Un ancla que señala a media carpeta no es un ancla: leer cada
        // candidato para compararlos cuesta, y con tantos la pregunta no está
        // señalando a ninguno en particular.
        Ok(candidates.filter(|documents| documents.len() <= MAX_PINNED_CANDIDATES))
    }

    /// El candidato que cubre más pistas de la pregunta, si hay uno solo que
    /// las cubra. Un empate no elige por su cuenta: dos documentos que
    /// responden igual de bien a lo escrito no se distinguen, y quedarse con
    /// uno sería inventar la diferencia.
    ///
    /// La única cosa que desempata es la conversación. Si uno de los
    /// candidatos empatados es el documento del que ya se estaba hablando, la
    /// continuación habla de ése: eso es lo que significa «esa minuta» o «la
    /// minuta de ventas de Tijuana» dicho dos turnos seguidos. No es una
    /// preferencia por lo reciente —el documento heredado sólo entra en el
    /// desempate si cubre TANTAS pistas como el mejor—, y sin conversación
    /// previa no hay nada que desempatar y se devuelve `None`.
    fn best_covered(
        &self,
        candidates: &HashSet<i64>,
        clues: &[String],
        inherited: Option<i64>,
    ) -> Result<Option<i64>> {
        let mut ordered = candidates.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut best = 0;
        let mut winners: Vec<i64> = Vec::new();
        for document_id in ordered {
            let covered = self.covered_clues(document_id, clues)?;
            crate::trace!("f) best_covered: doc {document_id} cubre {covered}/{} pistas", clues.len());
            if covered > best {
                best = covered;
                winners.clear();
            }
            if covered == best {
                winners.push(document_id);
            }
        }
        if best == 0 {
            return Ok(None);
        }
        crate::trace!("f) best_covered: mejor={best}, empatados={winners:?}, heredado={inherited:?}");
        Ok(match winners.as_slice() {
            [only] => Some(*only),
            several => inherited.filter(|document_id| several.contains(document_id)),
        })
    }

    /// Cuántas de las pistas están escritas en el documento. Se miran las tres
    /// procedencias que el índice ya tiene por documento —su texto, su nombre
    /// de archivo y su carpeta—, con la misma comparación por raíz que usa la
    /// recuperación.
    fn covered_clues(&self, document_id: i64, clues: &[String]) -> Result<usize> {
        let connection = self.database.connect()?;
        let (title, origin): (String, String) = connection.query_row(
            "SELECT title, origin FROM documents WHERE id = ?1",
            [document_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut statement =
            connection.prepare("SELECT content FROM chunks WHERE document_id = ?1")?;
        let mut words = search_terms(&format!("{title} {origin}"));
        for content in statement
            .query_map([document_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
        {
            words.extend(search_terms(&content));
        }
        Ok(clues
            .iter()
            .filter(|clue| words.iter().any(|word| stems_match(word, clue)))
            .count())
    }

    pub fn origin_summaries(&self) -> Result<Vec<OriginSummary>> {
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT origin, COUNT(*), MIN(id)
             FROM documents GROUP BY origin ORDER BY origin",
        )?;
        let grouped = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut summaries = Vec::with_capacity(grouped.len());
        for (origin, document_count, document_id) in grouped {
            let (path, ocr_status, confidence): (String, String, Option<f64>) = connection
                .query_row(
                    "SELECT path, ocr_status, ocr_confidence FROM documents WHERE id = ?1",
                    [document_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            summaries.push(OriginSummary {
                origin: origin.clone(),
                document_count,
                evidence: Evidence {
                    id: format!("m-{document_id}-carpeta de origen"),
                    document_id,
                    path,
                    origin: origin.clone(),
                    location: "metadato: carpeta de origen".into(),
                    excerpt: origin.clone(),
                    normalized_value: Some(normalize_exact(&origin)),
                    value: Some(origin.clone()),
                    matched: Some(origin),
                    field: Some("carpeta de origen".into()),
                    match_kind: "campo".into(),
                    reliable: ocr_is_reliable(&ocr_status, confidence),
                    ocr_status: Some(ocr_status),
                    ocr_confidence: confidence,
                    confidence,
                },
            });
        }
        Ok(summaries)
    }

    /// Resuelve una carpeta exclusivamente contra los orígenes descubiertos.
    /// Acepta el nombre completo y también la parte descriptiva posterior a
    /// un prefijo ordinal (`02_reportes` -> `reportes`).
    pub fn match_origin(&self, query: &str) -> Result<Option<String>> {
        // Un ordinal es un nombre de carpeta explícito. Aceptamos sólo la
        // coincidencia completa; `02_reportes` no puede convertirse en
        // `01_reportes` sólo porque comparten la parte descriptiva, y lo
        // mismo vale escrito `02 reportes`, `02-reportes` o entrecomillado.
        match self.resolve_explicit_origin(query)? {
            ExplicitOrigin::Found(origin) => return Ok(Some(origin)),
            ExplicitOrigin::Missing(_) => return Ok(None),
            ExplicitOrigin::NotNamed => {}
        }
        let normalized_query = normalize_exact(query);
        let query_terms = search_terms(query);
        let mut matches = self
            .origin_summaries()?
            .into_iter()
            .filter_map(|summary| {
                let full = normalize_exact(&summary.origin);
                let descriptive = full
                    .split_whitespace()
                    .skip_while(|part| part.chars().all(char::is_numeric))
                    .collect::<Vec<_>>()
                    .join(" ");
                let score = if whole_phrase_in(&normalized_query, &full) {
                    full.split_whitespace().count() + 200
                } else if !descriptive.is_empty()
                    && whole_phrase_in(&normalized_query, &descriptive)
                {
                    descriptive.split_whitespace().count() + 100
                } else {
                    // En lenguaje natural es habitual omitir parte del nombre
                    // de la carpeta ("facturas" frente a
                    // "07_facturas_emitidas"). Sólo se acepta el mejor match
                    // descubierto en el índice; los empates se descartan más
                    // abajo para no adivinar una categoría.
                    let origin_terms = search_terms(&descriptive);
                    origin_terms
                        .iter()
                        .filter(|candidate| {
                            query_terms.iter().any(|term| stems_match(term, candidate))
                        })
                        .count()
                };
                (score > 0).then_some((score, summary.origin))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        if matches.len() > 1 && matches[0].0 == matches[1].0 {
            return Ok(None);
        }
        Ok(matches.into_iter().next().map(|(_, origin)| origin))
    }

    /// Indica que la pregunta fijó un origen ordinal inexistente. Se expone al
    /// planificador para distinguir «no nombró origen» de «nombró uno que no
    /// está en el índice»; ambos devolvían `None` antes y el segundo caía sobre
    /// todo el acervo.
    /// Todas las carpetas de origen que la pregunta nombra, en el orden en que
    /// aparecen escritas.
    ///
    /// `match_origin` responde a otra pregunta —«¿cuál es LA carpeta de este
    /// alcance?»— y por eso devuelve una sola y descarta los empates. Comparar
    /// dos carpetas necesita justamente lo contrario: reconocerlas todas. Se
    /// exige la coincidencia **completa** del nombre de la carpeta o de su
    /// parte descriptiva; nunca una coincidencia parcial por palabras sueltas,
    /// que es lo que permitiría comparar dos carpetas que el usuario no
    /// nombró.
    pub fn origins_mentioned(&self, query: &str) -> Result<Vec<String>> {
        let normalized = normalize_exact(query);
        let mut named = self
            .origin_summaries()?
            .into_iter()
            .filter_map(|summary| {
                let full = normalize_exact(&summary.origin);
                let descriptive = full
                    .split_whitespace()
                    .skip_while(|part| part.chars().all(char::is_numeric))
                    .collect::<Vec<_>>()
                    .join(" ");
                let matched = if whole_phrase_in(&normalized, &full) {
                    full
                } else if !descriptive.is_empty() && whole_phrase_in(&normalized, &descriptive) {
                    descriptive
                } else {
                    return None;
                };
                Some((
                    phrase_position(&normalized, &matched).unwrap_or(usize::MAX),
                    summary.origin,
                ))
            })
            .collect::<Vec<_>>();
        named.sort();
        Ok(named.into_iter().map(|(_, origin)| origin).collect())
    }

    pub fn explicit_origin_is_missing(&self, query: &str) -> Result<bool> {
        Ok(matches!(
            self.resolve_explicit_origin(query)?,
            ExplicitOrigin::Missing(_)
        ))
    }

    /// Resuelve la carpeta que la pregunta nombró explícitamente contra los
    /// orígenes realmente indexados.
    ///
    /// Prueba prefijos de mayor a menor (`02 reportes internos`, luego
    /// `02 reportes`), porque la captura admite palabras de más: «en 02
    /// reportes de mayo» debe resolver a `02_reportes` y no fallar por
    /// arrastrar «de mayo». Si ningún prefijo coincide, el origen se declara
    /// ausente — nunca se degrada a la carpeta más parecida.
    pub fn resolve_explicit_origin(&self, query: &str) -> Result<ExplicitOrigin> {
        let Some(tokens) = explicit_origin_tokens(query) else {
            return Ok(ExplicitOrigin::NotNamed);
        };
        let summaries = self.origin_summaries()?;
        for length in (2..=tokens.len()).rev() {
            let candidate = tokens[..length].join(" ");
            if let Some(summary) = summaries
                .iter()
                .find(|summary| normalize_exact(&summary.origin) == candidate)
            {
                return Ok(ExplicitOrigin::Found(summary.origin.clone()));
            }
        }
        // Se informa con el ordinal y su primera palabra descriptiva: es lo
        // que el usuario nombró, sin las palabras que la captura arrastró.
        Ok(ExplicitOrigin::Missing(tokens[..2].join(" ")))
    }

    /// Una pregunta que fija un literal entre comillas es una búsqueda literal
    /// y no una consulta de razonamiento. El plan estructurado la deja pasar.
    pub fn query_has_quoted_literal(query: &str) -> bool {
        QUOTED_LITERAL
            .captures_iter(query)
            .any(|capture| !normalize_spanish(&capture[1]).is_empty())
    }

    /// Cuando lo único entrecomillado es el nombre de una carpeta ordinal, la
    /// pregunta no pide encontrar ese texto dentro de los documentos: pide
    /// acotar el alcance a esa carpeta. «Suma el Importe en “01 reportes”» es
    /// una suma sobre `01_reportes`, no la búsqueda de la cadena
    /// «01 reportes» escrita en algún archivo.
    pub fn quoted_literal_is_only_an_origin(query: &str) -> bool {
        let mut quoted = QUOTED_LITERAL
            .captures_iter(query)
            .map(|capture| capture[1].to_owned())
            .filter(|literal| !normalize_spanish(literal).is_empty())
            .peekable();
        quoted.peek().is_some()
            && quoted.all(|literal| explicit_origin_tokens(&literal).is_some())
    }

    /// Documentos que la pregunta señala por una **clave de localización**: el
    /// identificador interno de indexación (`D#####`, que corresponde al
    /// prefijo numérico del nombre de archivo) o la ruta/nombre del archivo.
    ///
    /// Estas claves NO son contenido citable: no aparecen escritas dentro del
    /// documento. Sirven sólo para llegar al documento; lo que se afirme
    /// después tiene que apoyarse en la evidencia textual que sí se extrajo de
    /// él. Por eso esta función devuelve documentos, nunca `Evidence`: quien
    /// llame debe construir su respuesta con `document_values`.
    ///
    /// Ante varios candidatos no adivina: los devuelve todos y deja que la
    /// capa de respuesta enumere, igual que ya hace con un folio ambiguo.
    pub fn locate_documents_by_key(&self, query: &str) -> Result<Vec<LocatedDocument>> {
        let mut located: Vec<LocatedDocument> = Vec::new();
        let mut seen = HashSet::new();
        let connection = self.database.connect()?;

        let mut push = |rows: Vec<(i64, String, String)>, seen: &mut HashSet<i64>| {
            for (id, path, origin) in rows {
                if seen.insert(id) {
                    located.push(LocatedDocument { id, path, origin });
                }
            }
        };

        // `D07550` designa al archivo cuyo nombre empieza por `07550_`. Es un
        // metadato del propio índice, no un dato que haya que creerle a nadie.
        for capture in INTERNAL_DOCUMENT_ID.captures_iter(query) {
            let digits = capture[1].to_owned();
            let mut statement = connection.prepare(
                r"SELECT id, path, origin FROM documents
                  WHERE path LIKE '%/' || ?1 || '\_%' ESCAPE '\'
                     OR path LIKE ?1 || '\_%' ESCAPE '\'",
            )?;
            let rows = statement
                .query_map([&digits], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(i64, String, String)>>>()?;
            push(rows, &mut seen);
        }

        // Ruta o nombre de archivo escritos en la pregunta.
        for name in document_path_candidates(query) {
            let mut statement = connection.prepare(
                "SELECT id, path, origin FROM documents
                 WHERE path = ?1 OR path LIKE '%/' || ?1",
            )?;
            let rows = statement
                .query_map([&name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<(i64, String, String)>>>()?;
            push(rows, &mut seen);
        }
        Ok(located)
    }

    /// Un documento del índice por su identificador interno de fila.
    ///
    /// Lo usa la resolución de referencias ordinales: el conjunto anterior se
    /// reevalúa como predicado y devuelve identificadores, y para responder
    /// sobre uno de ellos hace falta su ruta y su carpeta. Devuelve `None`
    /// cuando la fila ya no existe —reindexar la reasigna—, que es justo el
    /// caso en el que no se puede responder.
    pub fn document_by_id(&self, id: i64) -> Result<Option<LocatedDocument>> {
        let connection = self.database.connect()?;
        Ok(connection
            .query_row(
                "SELECT id, path, origin FROM documents WHERE id = ?1",
                params![id],
                |row| {
                    Ok(LocatedDocument {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        origin: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Un documento del índice por su ruta.
    ///
    /// Es la vuelta del camino que recuerda la conversación: la respuesta
    /// anterior habló de un archivo y guardó su ruta, y el turno siguiente
    /// —«¿y cuál es la Moneda de ese documento?»— la resuelve otra vez contra
    /// el índice actual. Devuelve `None` si ese archivo ya no está indexado,
    /// que es justo cuando la referencia no debe resolver a nada.
    pub fn document_by_path(&self, path: &str) -> Result<Option<LocatedDocument>> {
        let connection = self.database.connect()?;
        Ok(connection
            .query_row(
                "SELECT id, path, origin FROM documents WHERE path = ?1",
                params![path],
                |row| {
                    Ok(LocatedDocument {
                        id: row.get(0)?,
                        path: row.get(1)?,
                        origin: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// El SHA-256 del contenido de un documento, calculado por el indexador
    /// al leer el archivo por primera vez (`Sha256::digest` sobre los bytes
    /// crudos). No pasa por el texto extraído ni por OCR, así que compararlo
    /// entre dos documentos es un hecho mecánico y verificable, no una
    /// inferencia sobre su contenido.
    pub fn content_hash(&self, document_id: i64) -> Result<Option<String>> {
        let connection = self.database.connect()?;
        connection
            .query_row(
                "SELECT content_hash FROM documents WHERE id = ?1",
                [document_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Los demás documentos del acervo —cualquiera, sin acotar a un
    /// subconjunto— que comparten el SHA-256 de `document_id`: copias byte a
    /// byte idénticas de él. Vacío si no tiene ninguna.
    pub fn documents_sharing_hash(&self, document_id: i64) -> Result<Vec<LocatedDocument>> {
        let Some(hash) = self.content_hash(document_id)? else {
            return Ok(vec![]);
        };
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, path, origin FROM documents
             WHERE content_hash = ?1 AND id != ?2
             ORDER BY id",
        )?;
        let rows = statement
            .query_map(params![hash, document_id], |row| {
                Ok(LocatedDocument {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    origin: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Los literales entrecomillados de la pregunta, tal como se escribieron.
    pub fn quoted_literals(query: &str) -> Vec<String> {
        QUOTED_LITERAL
            .captures_iter(query)
            .map(|capture| capture[1].trim().to_owned())
            .filter(|literal| !literal.is_empty())
            .collect()
    }

    /// La pregunta sin sus citas entrecomilladas. El nombre de un campo suele
    /// ir entrecomillado («el valor del campo "Proveedor relacionado"»), y sus
    /// palabras describen al campo pedido, no a la pregunta que lo envuelve:
    /// quien necesite leer la intención de la frase debe mirar lo que queda
    /// fuera de las comillas.
    pub fn query_without_quoted_literals(query: &str) -> String {
        QUOTED_LITERAL.replace_all(query, " ").into_owned()
    }

    /// Señales que obligan a conservar la semántica exacta de la búsqueda.
    /// El planificador la consulta antes de cualquier expansión textual.
    pub fn query_has_exact_signal(query: &str) -> bool {
        explicit_identifier_mode(query).is_some()
            || query_contains_filename(query)
            || !canonical_identifier_candidates(query).is_empty()
            || !exact_query_tokens(query).is_empty()
            || requests_exact_but_incomplete(query)
    }

    /// Descubre filtros en la pregunta usando únicamente conceptos y valores
    /// existentes. Los valores implícitos se aceptan sólo para intenciones de
    /// lista/conteo y cuando identifican un único concepto dentro del alcance.
    pub fn filters_from_query(
        &self,
        query: &str,
        origin: Option<&str>,
        allow_implicit_values: bool,
    ) -> Result<Vec<ToolFilter>> {
        let query_terms = search_terms(query);
        let exact_query = normalize_exact(query);
        let field_name_tokens = written_field_name_tokens(query);
        // Las palabras que van dentro de unas comillas describen el valor
        // entrecomillado, no la pregunta que lo envuelve. Un valor de OTRO
        // campo que sólo coincida con palabras de dentro de las comillas no es
        // algo que el usuario haya pedido: en «¿Cuántos documentos del área
        // "Operaciones, producción, almacén, mantenimiento" están en formato
        // PDF_SCAN?» aparecía un filtro «Actividad = Producción estándar»
        // sacado de «producción» (dentro de las comillas) y de «están»
        // (que casa con «estándar» por prefijo). El valor entrecomillado
        // completo sí sigue resolviéndose: eso lo escribió el usuario entero.
        let unquoted_terms = search_terms(&Self::query_without_quoted_literals(query));
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT DISTINCT c.display_name, v.text_value, v.value_type
             FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             JOIN documents d ON d.id = v.document_id WHERE 1 = 1",
        );
        let mut parameters: Vec<Box<dyn ToSql>> = Vec::new();
        append_origin(&mut sql, &mut parameters, origin);
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map(
                params_from_iter(parameters.iter().map(|value| value.as_ref())),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut explicit = Vec::new();
        let mut explicit_values = Vec::new();
        let mut implicit: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (field, value, value_type) in rows {
            let field_terms = search_terms(&field);
            let value_terms = search_terms(&value);
            if value_terms.is_empty() || value_terms.len() > 8 {
                continue;
            }
            let field_named = terms_contain_all(&query_terms, &field_terms);
            let value_named = if value.chars().any(char::is_numeric) {
                whole_phrase_in(&exact_query, &normalize_exact(&value))
            } else {
                terms_contain_all(&query_terms, &value_terms)
            };
            if !value_named {
                continue;
            }
            // La pregunta usa esa palabra como NOMBRE de campo, no como
            // valor: la escribió pegada a un separador «campo: valor» o
            // «campo=valor». Un acervo cuya carátula de dos columnas dejó los
            // propios nombres de campo como valores de un concepto
            // («Documento» = «Área», «Documento» = «Moneda») convertía
            // «area=calidad, moneda=MXN» en dos filtros contradictorios que
            // vaciaban el alcance, y con el alcance vacío la respuesta era una
            // negativa que parecía «no hay datos» cuando en realidad era «me
            // inventé el filtro».
            let value_is_a_written_field_name = value_terms.iter().all(|value_term| {
                field_name_tokens
                    .iter()
                    .any(|token| stems_match(token, value_term))
            });
            if value_is_a_written_field_name {
                continue;
            }
            if field_named {
                crate::trace!("c)   filtro EXPLICITO (la pregunta nombra el campo): {field} = {value}");
                explicit_values.push(normalize_spanish(&value));
                explicit.push(ToolFilter {
                    concept: field,
                    equals: value,
                });
            } else if allow_implicit_values
                && (value_terms.len() >= 2 || value_type == "state")
                && terms_contain_all(&unquoted_terms, &value_terms)
            {
                implicit
                    .entry(normalize_spanish(&value))
                    .or_default()
                    .push((field, value));
            }
        }

        for (implicit_value, candidates) in implicit {
            if explicit_values
                .iter()
                .any(|explicit_value| whole_phrase_in(explicit_value, &implicit_value))
            {
                continue;
            }
            let distinct_fields = candidates
                .iter()
                .map(|(field, _)| canonical_key(field))
                .collect::<HashSet<_>>();
            if distinct_fields.len() == 1 {
                let (concept, equals) = candidates[0].clone();
                crate::trace!(
                    "c)   filtro IMPLICITO (valor {:?} nombrado sin nombrar el campo, unico campo posible): {concept} = {equals}",
                    implicit_value
                );
                explicit.push(ToolFilter { concept, equals });
            } else {
                crate::trace!(
                    "c)   valor implicito {:?} DESCARTADO: {} campos posibles",
                    implicit_value, distinct_fields.len()
                );
            }
        }
        explicit.sort_by(|left, right| {
            canonical_key(&left.concept)
                .cmp(&canonical_key(&right.concept))
                .then_with(|| {
                    normalize_spanish(&left.equals).cmp(&normalize_spanish(&right.equals))
                })
        });
        explicit.dedup_by(|left, right| {
            canonical_key(&left.concept) == canonical_key(&right.concept)
                && normalize_spanish(&left.equals) == normalize_spanish(&right.equals)
        });
        let explicit = drop_values_that_name_a_filtered_field(explicit);
        let explicit = collapse_spelling_variants(explicit, &self.list_concepts(None)?);
        let explicit = self.drop_values_that_the_acervo_reads_as_a_field(explicit)?;
        Ok(prefer_literal_values(explicit, &exact_query))
    }

    /// Descarta el filtro **inferido** cuyo valor es, para el propio acervo,
    /// el nombre de un campo y no un dato.
    ///
    /// Es el mismo artefacto de la carátula de dos columnas que ya conocía
    /// `drop_values_that_name_a_filtered_field`, pero por la otra puerta: ahí
    /// el nombre aparecía como campo filtrado en la misma pregunta, y aquí la
    /// pregunta sólo lo **nombra** («¿y cuál es la Moneda de ese documento?»).
    /// Sin esta guarda, el planificador armaba `Documento = Moneda` —«Moneda»
    /// es un valor del concepto «Documento» en los 14 archivos cuya carátula
    /// se leyó como tabla— y el alcance salía vacío: una negativa que parecía
    /// «no hay datos» cuando en realidad era «me inventé el filtro».
    ///
    /// Quién gana lo decide el acervo, no una lista ni un umbral escrito aquí:
    /// se comparan los documentos en los que esa cadena es el **nombre** de un
    /// campo con aquéllos en los que es el **valor** de este otro campo. Sólo
    /// se descarta cuando la lectura como nombre de campo es la más extendida
    /// de las dos. Así «Empresa = Grupo Nexo Industrial, S.A. de C.V.» (7.113
    /// documentos como valor frente a 139 como campo) se conserva intacto,
    /// mientras que «Documento = Moneda» (14 frente a 4.810) desaparece.
    ///
    /// Nunca toca un par «Campo: valor» escrito por el usuario: `resolved_filters`
    /// devuelve los escritos antes de llegar aquí, y esta función sólo se
    /// invoca desde la ruta de inferencia.
    fn drop_values_that_the_acervo_reads_as_a_field(
        &self,
        filters: Vec<ToolFilter>,
    ) -> Result<Vec<ToolFilter>> {
        if filters.is_empty() {
            return Ok(filters);
        }
        let connection = self.database.connect()?;
        let mut documents_for_field = connection.prepare(
            "SELECT COUNT(DISTINCT v.document_id)
             FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             WHERE c.canonical_key = ?1",
        )?;
        let mut documents_for_value = connection.prepare(
            "SELECT COUNT(DISTINCT v.document_id)
             FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             WHERE c.canonical_key = ?1 AND v.literal_value = ?2",
        )?;
        let mut kept = Vec::new();
        for filter in filters {
            // Un número no es el nombre de un campo: la colisión sólo puede
            // darse entre cadenas literales.
            let FilterKey::Literal(literal) = filter_key(&filter.concept, &filter.equals) else {
                kept.push(filter);
                continue;
            };
            let value_as_field = canonical_key(&filter.equals);
            if value_as_field == canonical_key(&filter.concept) {
                kept.push(filter);
                continue;
            }
            let as_field: i64 =
                documents_for_field.query_row(params![value_as_field], |row| row.get(0))?;
            if as_field == 0 {
                kept.push(filter);
                continue;
            }
            let as_value: i64 = documents_for_value
                .query_row(params![canonical_key(&filter.concept), literal], |row| {
                    row.get(0)
                })?;
            if as_value >= as_field {
                kept.push(filter);
            }
        }
        Ok(kept)
    }

    /// Filtros de una pregunta, dando prioridad absoluta a los pares
    /// «Campo: valor» escritos por el usuario.
    ///
    /// Cuando la pregunta nombra explícitamente un campo y un valor, ningún
    /// otro campo entra por inferencia: el usuario ya dijo qué quería, y
    /// añadirle un filtro adivinado cambia la respuesta sin avisar.
    pub fn resolved_filters(
        &self,
        query: &str,
        origin: Option<&str>,
        allow_implicit_values: bool,
    ) -> Result<Vec<ToolFilter>> {
        let written = self.written_filters(query)?;
        if !written.is_empty() {
            crate::trace!("c) resolved_filters: ESCRITOS (campo: valor) -> {:?}", written.filters);
            return Ok(written.filters);
        }
        let inferred = self.filters_from_query(query, origin, allow_implicit_values)?;
        crate::trace!(
            "c) resolved_filters: INFERIDOS (allow_implicit_values={allow_implicit_values}, origin={origin:?}) -> {inferred:?}"
        );
        Ok(inferred)
    }

    pub fn query_documents(
        &self,
        filters: &[ToolFilter],
        origin: Option<&str>,
        evidence_document_limit: usize,
    ) -> Result<DocumentQueryResult> {
        let connection = self.database.connect()?;
        let mut count_sql = String::from("SELECT COUNT(*) FROM documents d WHERE 1 = 1");
        let mut count_parameters: Vec<Box<dyn ToSql>> = Vec::new();
        append_origin(&mut count_sql, &mut count_parameters, origin);
        append_filters(&mut count_sql, &mut count_parameters, filters);
        let document_count = connection.query_row(
            &count_sql,
            params_from_iter(count_parameters.iter().map(|value| value.as_ref())),
            |row| row.get(0),
        )?;

        let mut sample_sql = String::from(
            "SELECT d.id, d.path, d.origin, d.ocr_status, d.ocr_confidence
             FROM documents d WHERE 1 = 1",
        );
        let mut sample_parameters: Vec<Box<dyn ToSql>> = Vec::new();
        append_origin(&mut sample_sql, &mut sample_parameters, origin);
        append_filters(&mut sample_sql, &mut sample_parameters, filters);
        sample_sql.push_str(" ORDER BY d.id LIMIT ?");
        sample_parameters.push(Box::new(evidence_document_limit.min(50) as i64));
        let mut statement = connection.prepare(&sample_sql)?;
        let documents = statement
            .query_map(
                params_from_iter(sample_parameters.iter().map(|value| value.as_ref())),
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut evidence = Vec::new();
        for (document_id, path, document_origin, ocr_status, confidence) in documents {
            if origin.is_some() {
                evidence.push(Evidence {
                    id: format!("m-{document_id}-carpeta de origen"),
                    document_id,
                    path: path.clone(),
                    origin: document_origin.clone(),
                    location: "metadato: carpeta de origen".into(),
                    excerpt: document_origin.clone(),
                    normalized_value: Some(normalize_exact(&document_origin)),
                    value: Some(document_origin.clone()),
                    matched: Some(document_origin.clone()),
                    field: Some("carpeta de origen".into()),
                    match_kind: "campo".into(),
                    reliable: ocr_is_reliable(&ocr_status, confidence),
                    ocr_status: Some(ocr_status.clone()),
                    ocr_confidence: confidence,
                    confidence,
                });
            }
            for filter in filters {
                const COLUMNS: &str = "v.evidence_id, v.location, v.excerpt, v.text_value, c.display_name";
                let row_mapper = |row: &rusqlite::Row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                };
                let found = match filter_key(&filter.concept, &filter.equals) {
                    FilterKey::Numeric { kind, value } => connection
                        .query_row(
                            &format!(
                                "SELECT {COLUMNS}
                                 FROM extracted_values v
                                 JOIN concepts c ON c.id = v.concept_id
                                 WHERE v.document_id = ?1 AND c.canonical_key = ?2
                                   AND v.value_type = ?3 AND v.numeric_value = ?4
                                 ORDER BY v.id LIMIT 1"
                            ),
                            params![document_id, canonical_key(&filter.concept), kind, value],
                            row_mapper,
                        )
                        .optional()?,
                    FilterKey::Literal(literal) => connection
                        .query_row(
                            &format!(
                                "SELECT {COLUMNS}
                                 FROM extracted_values v
                                 JOIN concepts c ON c.id = v.concept_id
                                 WHERE v.document_id = ?1 AND c.canonical_key = ?2
                                   AND v.literal_value = ?3
                                 ORDER BY v.id LIMIT 1"
                            ),
                            params![document_id, canonical_key(&filter.concept), literal],
                            row_mapper,
                        )
                        .optional()?,
                };
                if let Some((id, location, excerpt, value, field)) = found {
                    evidence.push(Evidence {
                        id,
                        document_id,
                        path: path.clone(),
                        origin: document_origin.clone(),
                        location,
                        excerpt: brief_excerpt(&excerpt, Some(&value)),
                        normalized_value: Some(normalize_exact(&value)),
                        value: Some(value.clone()),
                        matched: Some(value),
                        field: Some(field),
                        match_kind: "campo".into(),
                        reliable: ocr_is_reliable(&ocr_status, confidence),
                        ocr_status: Some(ocr_status.clone()),
                        ocr_confidence: confidence,
                        confidence,
                    });
                }
            }
        }
        Ok(DocumentQueryResult {
            document_count,
            evidence,
        })
    }

    /// Recuperación textual extractiva con FTS OR y ranking por cobertura del
    /// documento y del fragmento. No genera sinónimos ni conclusiones.
    pub fn search_text(
        &self,
        query: &str,
        origin: Option<&str>,
        limit: usize,
    ) -> Result<TextQueryResult> {
        let terms = content_terms(query);
        if terms.is_empty() {
            return Ok(TextQueryResult {
                document_count: 0,
                hits: Vec::new(),
            });
        }
        let fts_query = terms
            .iter()
            .map(|term| format!("\"{}\"*", term.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT d.id, d.title, d.path, d.origin, d.ocr_status, d.ocr_confidence,
                    c.location, c.content, bm25(chunks_fts), c.id
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE chunks_fts MATCH ?",
        );
        let mut parameters: Vec<Box<dyn ToSql>> = vec![Box::new(fts_query)];
        append_origin(&mut sql, &mut parameters, origin);
        // Sin corte: el ranking real —cobertura de términos, especificidad y
        // longitud del fragmento— corre después de esta consulta, así que
        // cualquier `LIMIT` aquí decide qué documentos llegan a evaluarse por
        // un criterio (`bm25` de un fragmento suelto) que no es el que manda.
        // Con más de cuatro mil fragmentos coincidentes eso borraba el
        // documento relevante antes de mirarlo, y además truncaba el número de
        // documentos que la respuesta declara como alcance.
        //
        // No materializa nada grande: las filas se recorren en streaming y lo
        // que se conserva es un candidato por documento, no por fragmento, así
        // que la memoria queda acotada por el número de documentos del acervo.
        sql.push_str(" ORDER BY bm25(chunks_fts)");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(parameters.iter().map(|value| value.as_ref())),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, f64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )?;

        struct Candidate {
            covered: HashSet<usize>,
            best_coverage: usize,
            best_specificity: usize,
            best_length: usize,
            best_rank: f64,
            hit: SearchHit,
        }
        let mut candidates: HashMap<i64, Candidate> = HashMap::new();
        for row in rows {
            let (
                document_id,
                title,
                path,
                origin,
                ocr_status,
                confidence,
                location,
                content,
                rank,
                chunk_id,
            ) = row?;
            let normalized = search_terms(&content);
            let covered = terms
                .iter()
                .enumerate()
                .filter(|(_, term)| {
                    normalized
                        .iter()
                        .any(|word| stems_match(word, term) || prefix_terms_match(word, term))
                })
                .map(|(index, _)| index)
                .collect::<HashSet<_>>();
            if covered.is_empty() {
                continue;
            }
            let coverage = covered.len();
            let specificity = covered
                .iter()
                .map(|index| terms[*index].len())
                .sum::<usize>();
            let content_length = content.chars().count();
            let hit = SearchHit {
                title,
                score: coverage as f64 * 50.0
                    + specificity as f64 * 3.0
                    + content_length.min(1000) as f64 / 100.0
                    + rank.abs(),
                evidence: chunk_evidence(
                    format!("c-{chunk_id}"),
                    document_id,
                    path,
                    origin,
                    ocr_status,
                    confidence,
                    &location,
                    &content,
                    &terms,
                ),
            };
            match candidates.get_mut(&document_id) {
                Some(candidate) => {
                    candidate.covered.extend(covered);
                    if coverage > candidate.best_coverage
                        || (coverage == candidate.best_coverage
                            && specificity > candidate.best_specificity)
                        || (coverage == candidate.best_coverage
                            && specificity == candidate.best_specificity
                            && content_length > candidate.best_length)
                        || (coverage == candidate.best_coverage
                            && specificity == candidate.best_specificity
                            && content_length == candidate.best_length
                            && rank.abs() > candidate.best_rank)
                    {
                        candidate.best_coverage = coverage;
                        candidate.best_specificity = specificity;
                        candidate.best_length = content_length;
                        candidate.best_rank = rank.abs();
                        candidate.hit = hit;
                    }
                }
                None => {
                    candidates.insert(
                        document_id,
                        Candidate {
                            covered,
                            best_coverage: coverage,
                            best_specificity: specificity,
                            best_length: content_length,
                            best_rank: rank.abs(),
                            hit,
                        },
                    );
                }
            }
        }
        let minimum = if terms.len() <= 2 {
            terms.len()
        } else {
            ((terms.len() + 2) / 3).max(2)
        };
        let mut ranked = candidates
            .into_values()
            .filter(|candidate| candidate.covered.len() >= minimum)
            .map(|mut candidate| {
                candidate.hit.score += candidate.covered.len() as f64 * 100.0;
                candidate.hit
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
        });
        let document_count = ranked.len();
        let mut seen_excerpts = HashSet::new();
        ranked.retain(|hit| seen_excerpts.insert(normalize_exact(&hit.evidence.excerpt)));
        ranked.truncate(limit.min(20));
        Ok(TextQueryResult {
            document_count,
            hits: ranked,
        })
    }

    pub fn indexed_document_count(&self) -> Result<i64> {
        let connection = self.database.connect()?;
        Ok(connection.query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))?)
    }

    /// Extensiones realmente presentes en el índice. El formato no se
    /// inventa ni se enumera desde una lista fija: se descubre igual que
    /// cualquier otro dato del acervo.
    pub fn available_extensions(&self) -> Result<BTreeSet<String>> {
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT lower(extension) FROM documents
             WHERE extension IS NOT NULL AND trim(extension) != ''
             UNION
             SELECT DISTINCT lower(extension) FROM unindexed_documents
             WHERE extension IS NOT NULL AND trim(extension) != ''",
        )?;
        let values = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(values.collect::<rusqlite::Result<BTreeSet<_>>>()?)
    }

    /// Todos los archivos que el indexador descubrió en el acervo, estén o no
    /// indexados.
    ///
    /// Es la base del censo (`census`): los que se pudieron leer viven en
    /// `documents` y los que no, en `unindexed_documents`. Juntar los dos es
    /// lo que permite dar un total de archivos completo en vez de uno
    /// silenciosamente recortado a lo legible. Ningún dato de contenido sale
    /// de aquí: sólo ruta, carpeta y si se logró indexar.
    pub fn census_files(&self) -> Result<Vec<census::CensusFile>> {
        let connection = self.database.connect()?;
        let mut files = Vec::new();
        let mut indexed = connection.prepare("SELECT id, path, origin FROM documents")?;
        let rows = indexed.query_map([], |row| {
            Ok(census::CensusFile {
                document_id: Some(row.get(0)?),
                path: row.get(1)?,
                origin: row.get(2)?,
                indexed: true,
            })
        })?;
        files.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        let mut missing =
            connection.prepare("SELECT path, origin FROM unindexed_documents")?;
        let rows = missing.query_map([], |row| {
            Ok(census::CensusFile {
                document_id: None,
                path: row.get(0)?,
                origin: row.get(1)?,
                indexed: false,
            })
        })?;
        files.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    /// La carpeta que un valor de campo identifica, cuando lo identifica sin
    /// ambigüedad y sin dejarse nada fuera.
    ///
    /// «¿Cuántos documentos hay en el área "Recursos humanos y capacitación"?»
    /// nombra un valor extraído, no una carpeta, y el nombre de la carpeta
    /// (`rh`) no se parece en nada a ese texto. La correspondencia no se
    /// adivina: se comprueba contra el índice, y en las **dos** direcciones:
    ///
    ///  1. Todos los documentos indexados que escriben ese valor exacto están
    ///     en una misma carpeta.
    ///  2. Todos los documentos indexados de esa carpeta escriben ese valor.
    ///
    /// La segunda condición es la que hace sólida la respuesta, y se aprendió
    /// rompiendo una prueba: con sólo la primera, una carpeta con tres
    /// archivos de los que dos dicen «Área: Norte» contestaba «3 documentos»
    /// a «¿cuántos hay en el área Norte?», contando un archivo que no lo dice.
    /// Un valor que la mayoría de la carpeta comparte NO identifica la
    /// carpeta: la identifica el que la comparte entera. Sin umbrales ni
    /// mayorías — un umbral aquí sería una cifra afirmada con confianza y
    /// equivocada en los casos que no lo cumplen, que es justo lo que este
    /// motor no hace.
    ///
    /// Devuelve además cuántos documentos indexados escriben el valor, para
    /// que la respuesta pueda declarar por separado los archivos de la carpeta
    /// y los documentos que se pudieron leer.
    pub fn origin_identified_by_value(&self, value: &str) -> Result<Option<(String, i64)>> {
        // `normalized_value` guarda lo que produce `normalize_spanish`; la
        // pregunta tiene que normalizarse igual o no coincide con nada.
        let normalized = normalize_spanish(value);
        if normalized.is_empty() {
            return Ok(None);
        }
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT d.origin, COUNT(DISTINCT d.id) FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             WHERE v.normalized_value = ?1
             GROUP BY d.origin",
        )?;
        let rows = statement
            .query_map([&normalized], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let [(origin, documents)] = rows.as_slice() else {
            return Ok(None);
        };
        let indexed_in_origin: i64 = connection.query_row(
            "SELECT COUNT(*) FROM documents WHERE origin = ?1",
            params![origin],
            |row| row.get(0),
        )?;
        Ok((*documents == indexed_in_origin).then(|| (origin.clone(), *documents)))
    }

    /// ¿Todos los documentos indexados que escriben este valor están en esta
    /// carpeta?
    ///
    /// Es la mitad débil de `origin_identified_by_value`, y responde a otra
    /// pregunta. Allí hay que **deducir** de qué carpeta habla el usuario a
    /// partir de un valor, y por eso hace falta la equivalencia en las dos
    /// direcciones. Aquí la carpeta ya está resuelta por su propio nombre y lo
    /// único que se comprueba es si el filtro de contenido que la pregunta
    /// escribió («…del área "Operaciones, producción…"») es otra forma de
    /// nombrar esa misma carpeta o un recorte adicional dentro de ella. Para
    /// eso basta con que el valor no viva en ninguna otra.
    pub fn value_lives_only_in_origin(&self, origin: &str, value: &str) -> Result<bool> {
        let normalized = normalize_spanish(value);
        if normalized.is_empty() {
            return Ok(false);
        }
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT d.origin FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             WHERE v.normalized_value = ?1",
        )?;
        let origins = statement
            .query_map([&normalized], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(matches!(origins.as_slice(), [only] if only == origin))
    }

    /// Conteo real de documentos por formato dentro de un alcance, con lo que
    /// el índice **no** pudo leer declarado al lado.
    ///
    /// Los documentos no indexados sólo se pueden acotar por carpeta: no
    /// tienen valores extraídos, así que ningún filtro de campo puede
    /// alcanzarlos. Cuando el alcance se define por filtros y no por carpeta,
    /// el conteo de no indexados es el del acervo entero y la respuesta tiene
    /// que decirlo así, no fingir que corresponde al alcance.
    pub fn count_by_format(
        &self,
        filters: &[ToolFilter],
        origin: Option<&str>,
        request: &FormatRequest,
        evidence_limit: usize,
    ) -> Result<FormatCount> {
        let connection = self.database.connect()?;
        let count_by = |scanned: Option<bool>, filters: &[ToolFilter]| -> Result<i64> {
            let mut sql =
                String::from("SELECT COUNT(*) FROM documents d WHERE lower(d.extension) = ?");
            let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(request.extension.clone())];
            match scanned {
                Some(true) => sql.push_str(" AND d.ocr_status != 'not_required'"),
                Some(false) => sql.push_str(" AND d.ocr_status = 'not_required'"),
                None => {}
            }
            append_origin(&mut sql, &mut values, origin);
            append_filters(&mut sql, &mut values, filters);
            Ok(connection.query_row(
                &sql,
                params_from_iter(values.iter().map(|value| value.as_ref())),
                |row| row.get(0),
            )?)
        };
        let scanned = count_by(Some(true), filters)?;
        let with_text_layer = count_by(Some(false), filters)?;
        let matching = if request.scanned_only {
            scanned
        } else {
            scanned + with_text_layer
        };
        // El mismo alcance visto sólo por carpeta. Cuando la pregunta nombra
        // el ámbito una vez y el motor lo reconoce por dos vías —la carpeta y
        // un campo del documento—, aplicar las dos es una conjunción que el
        // usuario no escribió: un documento escaneado cuyo campo quedó mal
        // leído por OCR vive en la carpeta correcta y aun así se caía del
        // conteo, en silencio. No se elige por el usuario cuál de las dos
        // lecturas vale: se cuenta la estricta y se declara la diferencia.
        let broader = if origin.is_some() && !filters.is_empty() {
            let scanned_only_broader = count_by(Some(true), &[])?;
            if request.scanned_only {
                scanned_only_broader
            } else {
                scanned_only_broader + count_by(Some(false), &[])?
            }
        } else {
            matching
        };

        let mut sample_sql = String::from(
            "SELECT d.id, d.path, d.origin, d.extension, d.ocr_status, d.ocr_confidence
             FROM documents d WHERE lower(d.extension) = ?",
        );
        let mut sample_values: Vec<Box<dyn ToSql>> = vec![Box::new(request.extension.clone())];
        if request.scanned_only {
            sample_sql.push_str(" AND d.ocr_status != 'not_required'");
        }
        append_origin(&mut sample_sql, &mut sample_values, origin);
        append_filters(&mut sample_sql, &mut sample_values, filters);
        sample_sql.push_str(" ORDER BY d.id LIMIT ?");
        sample_values.push(Box::new(evidence_limit as i64));
        let mut statement = connection.prepare(&sample_sql)?;
        let evidence = statement
            .query_map(
                params_from_iter(sample_values.iter().map(|value| value.as_ref())),
                |row| {
                    let document_id: i64 = row.get(0)?;
                    let extension: String = row.get(3)?;
                    let ocr_status: String = row.get(4)?;
                    let confidence: Option<f64> = row.get(5)?;
                    let label = extension.to_uppercase();
                    Ok(Evidence {
                        id: format!("m-{document_id}-formato"),
                        document_id,
                        path: row.get(1)?,
                        origin: row.get(2)?,
                        location: "metadato: extensión del archivo".into(),
                        excerpt: label.clone(),
                        normalized_value: Some(normalize_exact(&label)),
                        value: Some(label.clone()),
                        matched: Some(label),
                        field: Some("formato".into()),
                        match_kind: "campo".into(),
                        reliable: ocr_is_reliable(&ocr_status, confidence),
                        ocr_status: Some(ocr_status),
                        ocr_confidence: confidence,
                        confidence,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut unindexed_sql = String::from(
            "SELECT COUNT(*) FROM unindexed_documents u WHERE lower(u.extension) = ?",
        );
        let mut unindexed_values: Vec<Box<dyn ToSql>> =
            vec![Box::new(request.extension.clone())];
        if let Some(origin) = origin {
            unindexed_sql.push_str(" AND u.origin = ?");
            unindexed_values.push(Box::new(origin.to_owned()));
        }
        let unindexed = connection.query_row(
            &unindexed_sql,
            params_from_iter(unindexed_values.iter().map(|value| value.as_ref())),
            |row| row.get(0),
        )?;

        Ok(FormatCount {
            matching,
            only_in_origin: broader - matching,
            scanned,
            with_text_layer,
            unindexed,
            unindexed_is_scoped: origin.is_some(),
            evidence,
        })
    }

    /// Documentos citados cuya extensión declarada no corresponde a su
    /// contenido real. Devuelve la ruta y qué resultó ser el contenido.
    pub fn declared_format_mismatches(&self, documents: &[i64]) -> Result<Vec<(String, String)>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }
        let connection = self.database.connect()?;
        let mut rows = Vec::new();
        for chunk in documents.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT DISTINCT d.path, d.extension, d.declared_format_mismatch
                 FROM documents d
                 WHERE d.declared_format_mismatch IS NOT NULL
                   AND d.id IN ({placeholders})
                 ORDER BY d.path"
            );
            let mut statement = connection.prepare(&sql)?;
            let found = statement
                .query_map(params_from_iter(chunk.iter()), |row| {
                    Ok((
                        format!("{} (.{})", row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.extend(found);
        }
        Ok(rows)
    }

    /// Cómo se leyó un documento: si hizo falta OCR, con qué confianza salió y
    /// cuánto texto quedó. Es un hecho del índice, no una interpretación.
    pub fn document_reading(&self, document_id: i64) -> Result<DocumentReading> {
        let connection = self.database.connect()?;
        let (path, origin, extension, status, confidence) = connection.query_row(
            "SELECT path, origin, extension, ocr_status, ocr_confidence
             FROM documents WHERE id = ?1",
            [document_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                ))
            },
        )?;
        let values: i64 = connection.query_row(
            "SELECT COUNT(*) FROM extracted_values WHERE document_id = ?1",
            [document_id],
            |row| row.get(0),
        )?;
        Ok(DocumentReading {
            document_id,
            path,
            origin,
            extension,
            status: OcrStatus::from_stored(&status),
            stored_status: status,
            confidence,
            values,
        })
    }

    pub fn available_currencies(&self) -> Result<BTreeSet<String>> {
        let connection = self.database.connect()?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT upper(currency) FROM extracted_values
             WHERE currency IS NOT NULL AND trim(currency) != '' ORDER BY 1",
        )?;
        let values = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(values.collect::<rusqlite::Result<BTreeSet<_>>>()?)
    }

    pub fn count_documents(&self, filters: &[ToolFilter]) -> Result<(i64, Vec<Evidence>)> {
        let connection = self.database.connect()?;
        let mut count_sql = String::from("SELECT COUNT(*) FROM documents d WHERE 1 = 1");
        let mut count_values: Vec<Box<dyn ToSql>> = vec![];
        append_filters(&mut count_sql, &mut count_values, filters);
        let count: i64 = connection.query_row(
            &count_sql,
            params_from_iter(count_values.iter().map(|value| value.as_ref())),
            |row| row.get(0),
        )?;

        let mut evidence_sql = String::from(
            "SELECT d.id, d.path, d.origin, d.ocr_status, d.ocr_confidence, c.location, c.content
             FROM documents d JOIN chunks c ON c.document_id = d.id
             WHERE 1 = 1",
        );
        let mut evidence_values: Vec<Box<dyn ToSql>> = vec![];
        append_filters(&mut evidence_sql, &mut evidence_values, filters);
        evidence_sql.push_str(" GROUP BY d.id ORDER BY d.id LIMIT 20");
        let mut statement = connection.prepare(&evidence_sql)?;
        let rows = statement.query_map(
            params_from_iter(evidence_values.iter().map(|value| value.as_ref())),
            |row| {
                let document_id: i64 = row.get(0)?;
                Ok(Evidence {
                    id: format!("doc-{document_id}"),
                    document_id,
                    path: row.get(1)?,
                    origin: row.get(2)?,
                    location: row.get(5)?,
                    excerpt: row.get(6)?,
                    normalized_value: None,
                    value: None,
                    matched: None,
                    field: None,
                    match_kind: "campo".into(),
                    reliable: ocr_is_reliable(
                        &row.get::<_, String>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                    ),
                    ocr_status: Some(row.get(3)?),
                    ocr_confidence: row.get(4)?,
                    confidence: row.get(4)?,
                })
            },
        )?;
        Ok((count, rows.collect::<rusqlite::Result<Vec<_>>>()?))
    }

    /// Única ruta pública de agregación. Primero fija todos los documentos del
    /// alcance y después clasifica valores válidos, inválidos, ausentes y de
    /// moneda incompatible. Ninguna suma visible pasa por `f64`.
    pub fn aggregate(&self, request: &AggregateRequest) -> Result<AggregateResult> {
        if request.operation != "sum" && request.operation != "count" {
            return Err(OmegaError::InvalidArguments(
                "operation debe ser sum o count".into(),
            ));
        }
        resolve_concept(&self.database, &request.concept)?.ok_or_else(|| {
            OmegaError::InvalidArguments(format!("el concepto '{}' no existe", request.concept))
        })?;
        if let Some(group) = request.group_by.as_deref() {
            resolve_concept(&self.database, group)?.ok_or_else(|| {
                OmegaError::InvalidArguments(format!(
                    "el concepto de agrupación '{group}' no existe"
                ))
            })?;
        }
        let documents = self.aggregate_scope_documents(request)?;
        let operands = self.collect_operands(&ValueQuery {
            concept: &request.concept,
            documents: Some(&documents),
            group_by: request.group_by.as_deref(),
            ..ValueQuery::default()
        })?;
        let operation = if request.operation == "sum" {
            Operation::Sum
        } else {
            Operation::Count
        };
        let all_buckets = calc::compute(operation, &operands);
        let (buckets, currency_excluded) = split_aggregate_currency(all_buckets, request.currency.as_deref());
        let used_documents = buckets
            .iter()
            .flat_map(|bucket| bucket.document_ids.iter().copied())
            .collect::<HashSet<_>>();
        let with_field = operands
            .iter()
            .map(|operand| operand.document_id)
            .collect::<HashSet<_>>();
        let currency_only = currency_excluded
            .into_iter()
            .filter(|id| !used_documents.contains(id))
            .collect::<HashSet<_>>();
        let missing_field_count = documents
            .iter()
            .filter(|id| !with_field.contains(id))
            .count();
        let invalid_value_count = operation
            .needs_numbers()
            .then(|| {
                with_field
                    .iter()
                    .filter(|id| !used_documents.contains(id) && !currency_only.contains(id))
                    .count()
            })
            .unwrap_or_default();
        let currency_mismatch_count = currency_only.len();
        let excluded_count = missing_field_count + invalid_value_count + currency_mismatch_count;
        let value_count = buckets.iter().map(|bucket| bucket.value_count as i64).sum();
        let has_unreliable_evidence = buckets
            .iter()
            .any(|bucket| bucket.has_unreliable_evidence);
        let verified = !buckets.is_empty() && excluded_count == 0 && !has_unreliable_evidence;
        let warning = (!verified).then(|| aggregate_warning(
            missing_field_count,
            invalid_value_count,
            currency_mismatch_count,
            has_unreliable_evidence,
        ));
        let rows = buckets
            .iter()
            .map(|bucket| AggregateRow {
                group: bucket.group.clone(),
                currency: bucket.currency.clone(),
                value: aggregate_value(operation, bucket),
                matched_values: bucket.value_count as i64,
                evidence: bucket.evidence.clone(),
                has_unreliable_evidence: bucket.has_unreliable_evidence,
            })
            .collect();
        Ok(AggregateResult {
            rows,
            document_count: documents.len() as i64,
            value_count,
            excluded_count: excluded_count as i64,
            missing_field_count: missing_field_count as i64,
            invalid_value_count: invalid_value_count as i64,
            currency_mismatch_count: currency_mismatch_count as i64,
            verified,
            warning,
            has_unreliable_evidence,
        })
    }

    pub fn aggregate_calculation_evidence(
        &self,
        request: &AggregateRequest,
        result: &AggregateResult,
    ) -> Option<Evidence> {
        calculation_evidence(request, result)
    }

    fn aggregate_scope_documents(&self, request: &AggregateRequest) -> Result<Vec<i64>> {
        let connection = self.database.connect()?;
        let mut sql = String::from("SELECT d.id FROM documents d WHERE 1 = 1");
        let mut values: Vec<Box<dyn ToSql>> = vec![];
        append_origin(&mut sql, &mut values, request.origin.as_deref());
        append_filters(&mut sql, &mut values, &request.filters);
        if let Some(from) = &request.date_from {
            sql.push_str(" AND EXISTS (SELECT 1 FROM extracted_values vd WHERE vd.document_id = d.id AND vd.date_value >= ?)");
            values.push(Box::new(from.clone()));
        }
        if let Some(to) = &request.date_to {
            sql.push_str(" AND EXISTS (SELECT 1 FROM extracted_values vd WHERE vd.document_id = d.id AND vd.date_value <= ?)");
            values.push(Box::new(to.clone()));
        }
        sql.push_str(" ORDER BY d.id");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| row.get::<_, i64>(0),
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Devuelve la unidad de registro de ubicaciones tabulares. Así una suma por
/// Estado asocia el importe de una fila con el Estado de esa misma fila y no
/// con todos los estados presentes en el archivo.
fn tabular_record_scope(location: &str) -> Option<String> {
    if location.starts_with("fila ") {
        return location
            .split_once(", celda")
            .map(|(row, _)| row.to_owned());
    }
    if location.starts_with("hoja ") {
        let (sheet, cell_part) = location.split_once(", celda ")?;
        let cell = cell_part.split_whitespace().next()?;
        let row = cell.trim_matches(|character: char| !character.is_ascii_digit());
        if !row.is_empty() {
            return Some(format!("{sheet}, fila {row}"));
        }
    }
    if location.starts_with("tabla ") {
        let (table_row, _) = location.split_once(", celda")?;
        return Some(table_row.to_owned());
    }
    None
}

fn calculation_evidence(request: &AggregateRequest, result: &AggregateResult) -> Option<Evidence> {
    let first = result.rows.iter().flat_map(|row| row.evidence.iter()).next()?;
    let rendered = if result.rows.len() == 1 {
        result.rows[0].value.clone()
    } else {
        result.rows.iter()
            .map(|row| {
                format!(
                    "{}: {}",
                    row.group.as_deref().unwrap_or("Total"),
                    row.value,
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    Some(Evidence {
        id: format!(
            "calc-{}-{}-{}",
            request.operation,
            canonical_key(&request.concept),
            result.value_count
        ),
        document_id: first.document_id,
        path: first.path.clone(),
        origin: first.origin.clone(),
        location: format!("cálculo local exacto sobre {} valores extraídos", result.value_count),
        excerpt: format!(
            "Omega ejecutó {} para el concepto '{}' y obtuvo {} a partir de {} valores con evidencia.",
            if request.operation == "sum" {
                "una suma"
            } else {
                "un conteo"
            },
            request.concept,
            rendered,
            result.value_count,
        ),
        normalized_value: None,
        value: Some(rendered.clone()),
        matched: Some(rendered),
        field: None,
        match_kind: "campo".into(),
        reliable: first.reliable,
        ocr_status: first.ocr_status.clone(),
        ocr_confidence: first.ocr_confidence,
        confidence: first.ocr_confidence,
    })
}

fn split_aggregate_currency(
    buckets: Vec<calc::Bucket>,
    wanted: Option<&str>,
) -> (Vec<calc::Bucket>, HashSet<i64>) {
    let Some(wanted) = wanted else {
        return (buckets, HashSet::new());
    };
    let mut matching = Vec::new();
    let mut excluded = HashSet::new();
    for bucket in buckets {
        if bucket
            .currency
            .as_deref()
            .is_some_and(|currency| currency.eq_ignore_ascii_case(wanted))
        {
            matching.push(bucket);
        } else {
            excluded.extend(bucket.document_ids.iter().copied());
        }
    }
    (matching, excluded)
}

fn aggregate_value(operation: Operation, bucket: &calc::Bucket) -> String {
    let value = calc::render_amount(bucket.value, bucket.currency.as_deref());
    if operation == Operation::Sum && bucket.currency.is_none() && !value.contains('.') {
        format!("{value}.00")
    } else {
        value
    }
}

fn aggregate_warning(
    missing: usize,
    invalid: usize,
    currency: usize,
    unreliable: bool,
) -> String {
    let mut reasons = Vec::new();
    if missing > 0 {
        reasons.push(format!("{missing} documentos sin el campo"));
    }
    if invalid > 0 {
        reasons.push(format!("{invalid} valores inválidos"));
    }
    if currency > 0 {
        reasons.push(format!("{currency} documentos con moneda incompatible"));
    }
    if unreliable {
        reasons.push("OCR de baja confianza en un operando".into());
    }
    if reasons.is_empty() {
        "Sin evidencia suficiente para verificar la agregación.".into()
    } else {
        format!("Resultado parcial o no verificado: {}.", reasons.join("; "))
    }
}

/// Dos valores distintos del mismo campo no pueden cumplirse a la vez dentro de
/// un documento: aplicados como intersección dan siempre cero. Cuando eso pasa,
/// se conserva el valor que la pregunta escribe literalmente y se descartan los
/// que sólo coincidieron por raíces compartidas —«Documentación pendiente»
/// emparejaba con una pregunta que decía «documentos» y «Pendiente de emisión»,
/// y entre los dos apagaban la consulta entera—.
///
/// Si más de un valor aparece literalmente, se conservan todos: eso ya no es
/// una coincidencia accidental sino una comparación explícita entre dos grupos.
/// Una palabra que en esta misma pregunta ya es el NOMBRE de un campo con su
/// propio filtro no puede además ser el VALOR de otro campo.
///
/// «¿Cuántos documentos del área "Ventas, …" están en formato DOCX?» resolvía
/// dos filtros: el que el usuario pidió («Área = Ventas, …») y otro sacado de
/// la palabra «área» —«Documento = Área»— porque la carátula de dos columnas
/// del acervo dejó los propios nombres de campo como valores del concepto
/// «Documento». El segundo recortaba el alcance a los pocos documentos que
/// tuvieran esa carátula, y el conteo salía vacío.
///
/// La condición es estrecha a propósito: sólo se descarta el valor cuando ese
/// mismo nombre YA aparece en la pregunta como campo filtrado. Un valor que
/// por casualidad coincida con el nombre de un concepto que la pregunta no
/// está filtrando («Tipo = Factura», con «Factura» existiendo también como
/// campo) no se toca.
fn drop_values_that_name_a_filtered_field(filters: Vec<ToolFilter>) -> Vec<ToolFilter> {
    let filtered_concepts = filters
        .iter()
        .map(|filter| canonical_key(&filter.concept))
        .collect::<HashSet<_>>();
    filters
        .into_iter()
        .filter(|filter| {
            let value_as_field = canonical_key(&filter.equals);
            value_as_field == canonical_key(&filter.concept)
                || !filtered_concepts.contains(&value_as_field)
        })
        .collect()
}

/// Varias grafías del MISMO campo no son varias condiciones.
///
/// Un acervo con OCR contiene el mismo rótulo escrito de varias formas
/// («Moneda», «Monede», «Monedie» — 4.810, 8 y 1 documentos respectivamente en
/// el corpus de auditoría). Cuando la pregunta nombra ese campo una sola vez,
/// la inferencia lo reconocía en las tres y devolvía tres filtros, que se
/// aplican **en conjunción**: un documento tendría que tener los tres campos a
/// la vez, así que el alcance quedaba vacío y la respuesta parecía «no hay
/// datos» cuando en realidad era «pedí algo imposible».
///
/// Ante varias grafías del mismo nombre con el mismo valor se conserva una
/// sola: la del concepto con más valores en el acervo, que es el rótulo bien
/// escrito. Los pocos documentos que sólo tienen la grafía corrupta quedan
/// fuera del alcance; es una pérdida acotada y visible en la cobertura de la
/// respuesta, y siempre preferible a un alcance vacío.
///
/// «Mismo nombre» se exige en los dos sentidos: cada término de un nombre
/// tiene que corresponder a uno del otro y al revés, así que «Importe» e
/// «Importe total» —dos campos realmente distintos— nunca se colapsan.
fn collapse_spelling_variants(
    filters: Vec<ToolFilter>,
    catalogue: &[ConceptSummary],
) -> Vec<ToolFilter> {
    let occurrences = |concept: &str| {
        let key = canonical_key(concept);
        catalogue
            .iter()
            .find(|item| item.key == key)
            .map_or(0, |item| item.occurrences)
    };
    let mut kept: Vec<ToolFilter> = Vec::new();
    for filter in filters {
        let twin = kept.iter_mut().find(|existing| {
            normalize_spanish(&existing.equals) == normalize_spanish(&filter.equals)
                && same_field_name(&existing.concept, &filter.concept)
        });
        match twin {
            Some(existing) => {
                if occurrences(&filter.concept) > occurrences(&existing.concept) {
                    *existing = filter;
                }
            }
            None => kept.push(filter),
        }
    }
    kept
}

/// ¿Son dos formas de escribir el mismo nombre de campo? Se compara término a
/// término y en ambas direcciones, con la misma tolerancia que usa la
/// inferencia para reconocer el campo dentro de la pregunta.
fn same_field_name(left: &str, right: &str) -> bool {
    let left_terms = search_terms(left);
    let right_terms = search_terms(right);
    if left_terms.is_empty() || right_terms.is_empty() {
        return false;
    }
    terms_contain_all(&left_terms, &right_terms) && terms_contain_all(&right_terms, &left_terms)
}

fn prefer_literal_values(filters: Vec<ToolFilter>, exact_query: &str) -> Vec<ToolFilter> {
    let mut competing: HashMap<String, usize> = HashMap::new();
    for filter in &filters {
        *competing.entry(canonical_key(&filter.concept)).or_insert(0) += 1;
    }
    let literal_by_concept = filters
        .iter()
        .filter(|filter| whole_phrase_in(exact_query, &normalize_exact(&filter.equals)))
        .fold(HashMap::<String, usize>::new(), |mut counts, filter| {
            *counts.entry(canonical_key(&filter.concept)).or_insert(0) += 1;
            counts
        });
    filters
        .into_iter()
        .filter(|filter| {
            let key = canonical_key(&filter.concept);
            if competing.get(&key).copied().unwrap_or(0) < 2 {
                return true;
            }
            if literal_by_concept.get(&key).copied().unwrap_or(0) != 1 {
                return true;
            }
            whole_phrase_in(exact_query, &normalize_exact(&filter.equals))
        })
        .collect()
}

/// Clave de comparación tipada para un valor de filtro. `normalized_value`
/// (que borra puntuación y aplica raíces) o incluso el texto literal por sí
/// solos no bastan: «1000» y «1,000» son el mismo número escrito distinto,
/// pero «1,000» y «1,000%» NO son el mismo valor aunque compartan cifras. La
/// comparación tiene que conocer el tipo del valor para saber cuál de las
/// dos cosas es cierta en cada caso — no basta con preservar más o menos
/// puntuación en una cadena y esperar que eso baste.
///
/// - Número y porcentaje comparan por su **valor numérico exacto**
///   (tolerante al formato: «1000» y «1,000» son el mismo número) pero nunca
///   se mezclan entre sí, porque el tipo también entra en la comparación:
///   «1,000» (número) y «1,000%» (porcentaje) tienen el mismo
///   `numeric_value` y aun así no coinciden.
/// - Cualquier otro tipo (texto, estado, fecha, dinero) compara por su forma
///   **literal**: sólo pliega mayúsculas y acentos, nunca puntuación, así
///   que «Pendiente» y ««Pendiente»» no colapsan en el mismo valor.
#[derive(Debug, Clone, PartialEq)]
enum FilterKey {
    Numeric { kind: &'static str, value: f64 },
    Literal(String),
}

/// El valor crudo (`ToolFilter::equals`) es el que se guarda, se muestra y
/// se cita; esta función sólo lo clasifica para decidir CÓMO compararlo, sin
/// alterarlo. Reclasificar aquí es seguro porque `equals` siempre es el
/// valor literal tal como está en el acervo (nunca una forma ya normalizada)
/// en todos los sitios que construyen un `ToolFilter`.
fn filter_key(concept: &str, equals: &str) -> FilterKey {
    let typed = classify_value(concept, equals);
    match (typed.kind, typed.numeric_value) {
        (ValueKind::Number | ValueKind::Percentage, Some(value)) => FilterKey::Numeric {
            kind: typed.kind.as_str(),
            value,
        },
        _ => FilterKey::Literal(typed.literal_value),
    }
}

fn append_filters(sql: &mut String, values: &mut Vec<Box<dyn ToSql>>, filters: &[ToolFilter]) {
    for filter in filters {
        match filter_key(&filter.concept, &filter.equals) {
            FilterKey::Numeric { kind, value } => {
                sql.push_str(
                    " AND d.id IN (
                        SELECT vf.document_id FROM extracted_values vf
                        JOIN concepts cf ON cf.id = vf.concept_id
                        WHERE cf.canonical_key = ? AND vf.value_type = ? AND vf.numeric_value = ?
                      )",
                );
                values.push(Box::new(canonical_key(&filter.concept)));
                values.push(Box::new(kind));
                values.push(Box::new(value));
            }
            FilterKey::Literal(literal) => {
                sql.push_str(
                    " AND d.id IN (
                        SELECT vf.document_id FROM extracted_values vf
                        JOIN concepts cf ON cf.id = vf.concept_id
                        WHERE cf.canonical_key = ? AND vf.literal_value = ?
                      )",
                );
                values.push(Box::new(canonical_key(&filter.concept)));
                values.push(Box::new(literal));
            }
        }
    }
}

fn append_origin(sql: &mut String, values: &mut Vec<Box<dyn ToSql>>, origin: Option<&str>) {
    if let Some(origin) = origin {
        sql.push_str(" AND d.origin = ?");
        values.push(Box::new(origin.to_owned()));
    }
}

fn terms_contain_all(haystack: &[String], needles: &[String]) -> bool {
    !needles.is_empty()
        && needles.iter().all(|needle| {
            haystack
                .iter()
                .any(|term| stems_match(term, needle) || prefix_terms_match(term, needle))
        })
}

fn prefix_terms_match(left: &str, right: &str) -> bool {
    left.len().min(right.len()) >= 4 && (left.starts_with(right) || right.starts_with(left))
}

/// Conserva sólo términos de contenido. La lista contiene palabras de
/// formulación de consultas, nunca nombres propios de un dominio documental.
fn content_terms(query: &str) -> Vec<String> {
    const FILLER: &[&str] = &[
        "a",
        "al",
        "ante",
        "aplica",
        "aplican",
        "aplicar",
        "como",
        "con",
        "cual",
        "cuales",
        "cuando",
        "cuanto",
        "cuantos",
        "de",
        "debe",
        "del",
        "despues",
        "dime",
        "donde",
        "el",
        "en",
        "es",
        "esta",
        "este",
        "existe",
        "hacer",
        "hay",
        "la",
        "las",
        "lo",
        "los",
        "me",
        "menciona",
        "mencionan",
        "mention",
        "mentions",
        "muestra",
        "para",
        "por",
        "que",
        "quien",
        "quienes",
        "se",
        "sobre",
        "son",
        "su",
        "sus",
        "tiene",
        "tienen",
        "un",
        "una",
        "y",
        "busca",
        "buscar",
        "buscando",
        "buscamos",
        "busco",
        "estoy",
        "estamos",
        "estaba",
        "estuvo",
        "necesito",
        "necesitamos",
        "quiero",
        "quisiera",
        "queria",
        "podrias",
        "puedes",
        "puede",
        "dame",
        "muestrame",
        "mostrar",
        "revisar",
        "revisando",
        "reviso",
        "encontrar",
        "encuentro",
        "saber",
        "sabes",
        "ayudame",
        "ayuda",
        "favor",
        "gracias",
        "hola",
        "era",
        "eran",
        "fue",
        "fueron",
        "sea",
        "ser",
        "estan",
        "existen",
        "campo",
        "campos",
        "dato",
        "datos",
        "documentacion",
        "documento",
        "documentos",
        "informacion",
        "registros",
        "resume",
        "the",
        "what",
        "which",
        "who",
        "where",
        "when",
        "show",
        "find",
        "documents",
    ];
    let filler = FILLER
        .iter()
        .map(|word| normalize_spanish(word))
        .collect::<HashSet<_>>();
    let mut terms = search_terms(query)
        .into_iter()
        .filter(|term| term.len() >= 3 && !filler.contains(term))
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

fn resolve_concept(database: &Database, value: &str) -> Result<Option<i64>> {
    let connection = database.connect()?;
    let key = canonical_key(value);
    let normalized = normalize_spanish(value);
    Ok(connection
        .query_row(
            "SELECT c.id FROM concepts c
         WHERE c.canonical_key = ?1 OR EXISTS (
            SELECT 1 FROM concept_aliases a
            WHERE a.concept_id = c.id AND a.normalized_alias = ?2 AND a.status != 'rejected'
         ) LIMIT 1",
            params![key, normalized],
            |row| row.get(0),
        )
        .optional()?)
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| OmegaError::InvalidArguments(format!("falta {field}")))
}

fn parse_filters(value: Option<&Value>) -> Result<Vec<ToolFilter>> {
    match value {
        None => Ok(vec![]),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|error| OmegaError::InvalidArguments(error.to_string())),
    }
}

fn keep_best(hits: &mut HashMap<i64, SearchHit>, candidate: SearchHit) {
    match hits.get(&candidate.evidence.document_id) {
        Some(existing) if existing.score >= candidate.score => {}
        _ => {
            hits.insert(candidate.evidence.document_id, candidate);
        }
    }
}

fn exact_fragments(query: &str) -> Vec<String> {
    let mut fragments = vec![normalize_spanish(query)];
    fragments.extend(
        exact_query_tokens(query)
            .iter()
            .map(|value| normalize_spanish(value)),
    );
    fragments.retain(|value| !value.is_empty());
    fragments.sort();
    fragments.dedup();
    fragments
}

/// Extrae únicamente señales explícitas de una búsqueda literal. El detector
/// no presupone prefijos ni vocabulario de un dominio: reconoce nombres de
/// archivo, citas entre comillas e identificadores que mezclan letras y
/// números (con o sin separadores).
fn exact_query_tokens(query: &str) -> Vec<String> {
    let quoted = regex::Regex::new(r#"[\"“”']([^\"“”']+)[\"“”']"#).expect("valid quote regex");
    let filename = regex::Regex::new(r"(?u)\b[\p{L}\p{N}][\p{L}\p{N}._-]*\.[\p{L}\p{N}]{1,12}\b")
        .expect("valid filename regex");
    let candidate = regex::Regex::new(r"(?u)\b[\p{L}\p{N}][\p{L}\p{N}._/-]*\b")
        .expect("valid identifier regex");
    let ordinal_folder = regex::Regex::new(r"(?u)^\d{1,3}_[\p{L}]+(?:_[\p{L}]+)*$")
        .expect("valid ordinal folder regex");
    let quoted_tokens = quoted
        .captures_iter(query)
        .map(|capture| capture[1].trim().to_owned())
        .filter(|value| !normalize_spanish(value).is_empty())
        .collect::<Vec<_>>();
    // Las comillas expresan exactamente qué secuencia debe aparecer. No se
    // mezclan con subidentificadores que pudieran existir dentro de la frase.
    if !quoted_tokens.is_empty() {
        return deduplicate_exact_tokens(quoted_tokens);
    }

    let filename_tokens = filename
        .find_iter(query)
        .map(|item| item.as_str().to_owned())
        .collect::<Vec<_>>();
    // Un nombre de archivo es un metadato literal más específico que el
    // posible identificador contenido en su base (por ejemplo, ID-7.pdf).
    if !filename_tokens.is_empty() {
        return deduplicate_exact_tokens(filename_tokens);
    }

    let identifier_tokens = candidate.find_iter(query).filter_map(|item| {
        let value = item.as_str();
        let has_letter = value.chars().any(char::is_alphabetic);
        let has_number = value.chars().any(char::is_numeric);
        // Un ordinal de carpeta como "01_reportes" no representa por sí solo
        // un identificador: es una convención de ordenamiento común y debe
        // seguir pudiendo consultarse como categoría. La regla se limita a
        // números iniciales y separadores de carpeta, sin conocer ninguna
        // categoría concreta.
        (has_letter && has_number && !ordinal_folder.is_match(value)).then(|| value.to_owned())
    });
    deduplicate_exact_tokens(identifier_tokens.collect())
}

fn canonical_identifier_candidates(query: &str) -> Vec<String> {
    let compact = regex::Regex::new(r"(?u)\b[\p{L}\p{N}][\p{L}\p{N}._/-]*[\p{L}\p{N}]\b")
        .expect("valid canonical identifier regex");
    let spaced = regex::Regex::new(r"(?u)\b\p{L}[\p{L}\p{N}._/-]*\s+\p{N}[\p{L}\p{N}._/-]*\b")
        .expect("valid spaced identifier regex");
    let mut values = compact
        .find_iter(query)
        .chain(spaced.find_iter(query))
        .filter(|match_| !looks_like_ordinal_folder(match_.as_str()))
        .filter(|match_| !looks_like_labeled_ordinal_folder(match_.as_str()))
        .filter_map(|match_| canonical_identifier(match_.as_str()))
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn query_contains_filename(query: &str) -> bool {
    static FILENAME: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?u)\b[\p{L}\p{N}][\p{L}\p{N}._-]*\.[\p{L}\p{N}]{1,12}\b")
            .expect("valid filename regex")
    });
    FILENAME.is_match(query)
}

fn looks_like_ordinal_folder(value: &str) -> bool {
    static ORDINAL_FOLDER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?u)^\d{1,3}_[\p{L}]+(?:_[\p{L}]+)*$")
            .expect("valid ordinal folder regex")
    });
    ORDINAL_FOLDER.is_match(value)
}

fn looks_like_labeled_ordinal_folder(value: &str) -> bool {
    let mut words = value.split_whitespace();
    let Some(label) = words.next() else {
        return false;
    };
    let Some(candidate) = words.next() else {
        return false;
    };
    words.next().is_none()
        && [
            "carpeta",
            "categoria",
            "categoría",
            "origen",
            "fuente",
            "folder",
            "category",
            "source",
        ]
        .contains(&label.to_lowercase().as_str())
        && looks_like_ordinal_folder(candidate)
}

fn explicit_identifier_mode(query: &str) -> Option<(IdentifierMode, String)> {
    let prefix =
        regex::Regex::new(r#"(?iu)^\s*(?:empieza\s+con|starts\s+with)\s+[\"“”']?([^\"“”']+)"#)
            .expect("valid prefix query regex");
    if let Some(capture) = prefix.captures(query) {
        return Some((IdentifierMode::Prefix, capture[1].trim().to_owned()));
    }
    let contains = regex::Regex::new(
        r#"(?iu)^\s*(?:contiene|menciona|contains|mentions)\s+[\"“”']?([^\"“”']+)"#,
    )
    .expect("valid contains query regex");
    contains
        .captures(query)
        .map(|capture| (IdentifierMode::Contains, capture[1].trim().to_owned()))
}

fn identifiers_in_text(value: &str) -> Vec<String> {
    static IDENTIFIER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?u)\b[\p{L}\p{N}][\p{L}\p{N}._/-]*[\p{L}\p{N}]\b")
            .expect("valid identifier extraction regex")
    });
    IDENTIFIER
        .find_iter(value)
        .map(|match_| match_.as_str().to_owned())
        .collect()
}

fn deduplicate_exact_tokens(mut tokens: Vec<String>) -> Vec<String> {
    // Se deduplica por normalización para que "CODE-7" entre comillas no
    // produzca dos recorridos distintos del mismo valor.
    tokens.sort_by_key(|value| normalize_spanish(value));
    tokens.dedup_by(|left, right| normalize_spanish(left) == normalize_spanish(right));
    tokens
}

fn requests_exact_but_incomplete(query: &str) -> bool {
    let words = normalize_exact(query);
    let requests_exactness = words
        .split_whitespace()
        .any(|word| matches!(word, "exactamente" | "exactly"));
    requests_exactness && exact_query_tokens(query).is_empty()
}

/// ¿La pregunta dice algo más que el nombre de la carpeta que coincidió?
///
/// El nombre de una carpeta es metadato del índice, no prueba de que el
/// documento sea el pedido. Cuando la pregunta se agota en ese nombre
/// («¿qué hay en ventas?») la procedencia ES lo consultado; en cuanto la
/// pregunta añade cualquier otra palabra de contenido, compartir carpeta con
/// lo preguntado deja de decir nada sobre lo que el documento contiene.
fn query_says_more_than_the_origin(query: &str, origin: &str) -> bool {
    let origin_terms = search_terms(origin);
    content_terms(query).iter().any(|term| {
        !origin_terms
            .iter()
            .any(|origin_term| stems_match(origin_term, term))
    })
}

/// El tramo más largo de palabras consecutivas de un valor que la pregunta
/// escribe tal cual. Devuelve `None` si no llega a dos palabras: con una sola
/// no se está nombrando ese valor, se está usando una palabra que además
/// aparece en él.
fn longest_run_named_by(normalized_query: &str, words: &[&str]) -> Option<String> {
    let mut best: Option<String> = None;
    let mut best_length = 0;
    for start in 0..words.len() {
        for end in ((start + 2)..=words.len()).rev() {
            if end - start <= best_length {
                break;
            }
            let run = words[start..end].join(" ");
            // Dos palabras, pero dos palabras CON CONTENIDO: «de ventas» no
            // nombra ningún valor —es una preposición y una palabra que está
            // en cientos de campos—, y como ancla arrastraba media carpeta.
            if search_terms(&run).len() >= 2 && whole_phrase_in(normalized_query, &run) {
                best_length = end - start;
                best = Some(run);
                break;
            }
        }
    }
    best
}

/// Cuántos candidatos como mucho compara `pinned_document` leyéndolos. Por
/// encima de este número la pregunta no está señalando a un documento sino a
/// un conjunto, y la respuesta correcta sigue siendo la lista de siempre.
const MAX_PINNED_CANDIDATES: usize = 120;

fn phrase_in(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || needle.len() < 3 {
        return false;
    }
    whole_phrase_in(haystack, needle)
}

/// Los tipos con forma reconocible (`extract::classify_value` ya sabe
/// distinguirlos) sólo pueden tomar un valor literal que contenga un dígito:
/// un importe, una cantidad, un porcentaje o una fecha. Un campo de texto o
/// estado no tiene esa restricción, así que cualquier palabra puede seguir
/// siendo un intento de valor legítimo.
fn requires_numeric_shape(value_type: &str) -> bool {
    matches!(value_type, "money" | "number" | "percentage" | "date")
}

fn whole_phrase_in(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let padded_haystack = format!(" {haystack} ");
    let padded_needle = format!(" {needle} ");
    padded_haystack.contains(&padded_needle)
}

fn phrase_position(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.match_indices(needle).find_map(|(start, _)| {
        let end = start + needle.len();
        let starts_at_boundary = start == 0
            || haystack[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let ends_at_boundary = end == haystack.len()
            || haystack[end..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
        (starts_at_boundary && ends_at_boundary).then_some(start)
    })
}

/// Comprueba una aparición literal sin permitir que el valor consultado sea
/// sólo el inicio, final o tramo de otro identificador. Los separadores que
/// suelen formar parte de identificadores se consideran continuaciones, no
/// límites válidos.
fn literal_occurs_as_complete_value(haystack: &str, needle: &str) -> bool {
    let normalized_haystack = normalize_literal(haystack);
    let normalized_needle = normalize_literal(needle);
    if normalized_needle.trim().is_empty() {
        return false;
    }
    normalized_haystack
        .match_indices(&normalized_needle)
        .any(|(start, _)| {
            let end = start + normalized_needle.len();
            let before = normalized_haystack[..start].chars().next_back();
            let after = normalized_haystack[end..].chars().next();
            !before.is_some_and(is_identifier_continuation)
                && !after.is_some_and(is_identifier_continuation)
        })
}

fn is_identifier_continuation(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '-' | '_' | '/' | '.')
}

fn chunk_evidence(
    id: String,
    document_id: i64,
    path: String,
    origin: String,
    ocr_status: String,
    confidence: Option<f64>,
    location: &str,
    content: &str,
    terms: &[String],
) -> Evidence {
    let start_line = location
        .strip_prefix("líneas ")
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<usize>().ok())
        .unwrap_or(1);
    let best = content
        .lines()
        .enumerate()
        .map(|(offset, line)| {
            let normalized = search_terms(line);
            let score = terms
                .iter()
                .filter(|term| {
                    normalized
                        .iter()
                        .any(|word| stems_match(word, term) || prefix_terms_match(word, term))
                })
                .count();
            (score, offset, line.trim())
        })
        .max_by_key(|(score, _, _)| *score)
        .unwrap_or((0, 0, content.trim()));
    let best_terms = search_terms(best.2);
    let matched = terms
        .iter()
        .filter(|term| {
            best_terms
                .iter()
                .any(|word| stems_match(word, term) || prefix_terms_match(word, term))
        })
        .max_by_key(|term| term.len())
        .cloned();
    Evidence {
        id,
        document_id,
        path,
        origin,
        location: if location.starts_with("líneas ") {
            format!("línea {}", start_line + best.1)
        } else {
            location.to_owned()
        },
        excerpt: brief_excerpt(best.2, matched.as_deref()),
        normalized_value: None,
        value: None,
        matched,
        field: None,
        match_kind: "texto".into(),
        reliable: ocr_is_reliable(&ocr_status, confidence),
        ocr_status: Some(ocr_status),
        ocr_confidence: confidence,
        confidence,
    }
}

/// Una evidencia sólo es fiable cuando su documento llegó al índice por una
/// lectura completa. El estado manda: sin él, un documento pendiente, omitido
/// o de baja confianza pasaba por fiable con sólo tener la columna de
/// confianza en NULL. La cifra, cuando existe, sigue pudiendo degradarlo.
fn ocr_is_reliable(status: &str, confidence: Option<f64>) -> bool {
    OcrStatus::from_stored(status).is_reliable()
        && confidence.is_none_or(|value| value >= crate::ocr::RELIABLE_CONFIDENCE)
}

/// Extrae un nombre de carpeta ordinal escrito literalmente por el usuario.
/// No normaliza el prefijo: éste distingue `02_reportes` de `01_reportes`.
/// Literales entrecomillados de una pregunta. Lo comparten la detección de
/// búsqueda literal y la de carpeta entrecomillada, que deben leer exactamente
/// las mismas comillas para no discrepar.
/// Identificador interno de indexación: `D` seguido de cinco dígitos, que
/// corresponden al prefijo del nombre de archivo (`D07550` -> `07550_...`).
/// No es contenido del documento; sólo sirve para localizarlo.
static INTERNAL_DOCUMENT_ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\bD(\d{5})\b").expect("valid internal id regex"));

/// Rutas y nombres de archivo mencionados en la pregunta, en la forma en que
/// aparecen escritos («ventas/08529_cotizacion.pdf» o «08529_cotizacion.pdf»).
fn document_path_candidates(query: &str) -> Vec<String> {
    static CANDIDATE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?u)[\p{L}\p{N}_-]+(?:/[\p{L}\p{N}._-]+)*\.[\p{L}\p{N}]{1,12}")
            .expect("valid path regex")
    });
    let mut seen = HashSet::new();
    CANDIDATE
        .find_iter(query)
        .map(|item| item.as_str().trim_matches(|c| c == '(' || c == ')').to_owned())
        .filter(|candidate| seen.insert(candidate.clone()))
        .collect()
}

static QUOTED_LITERAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"["\u{201c}\u{201d}']([^"\u{201c}\u{201d}']+)["\u{201c}\u{201d}']"#)
        .expect("valid quote regex")
});

/// Carpeta ordinal nombrada explícitamente por el usuario, en cualquiera de
/// las formas en que se escribe de verdad:
///
/// - `02_reportes` y `02-reportes` — separador inequívoco;
/// - `«02 reportes»` / `"02 reportes"` — entrecomillado;
/// - `carpeta 02 reportes` / `origen 02 reportes` — con la palabra que nombra
///   la carpeta;
/// - `… en 02 reportes` — preposición locativa más un ordinal con cero a la
///   izquierda, que es la convención de estas carpetas.
///
/// Devuelve los tokens ya normalizados (`["02", "reportes", …]`). Se limita a
/// cuatro palabras descriptivas: quien resuelve prueba prefijos de mayor a
/// menor, así que capturar de más no rompe la coincidencia.
///
/// El cero a la izquierda importa: sin él, `en 12 documentos` se leería como
/// una carpeta inexistente y la pregunta se rechazaría por un origen que
/// nadie nombró. Una carpeta real sin cero (`12_reportes`) sigue llegando por
/// el separador inequívoco o por la coincidencia normal de `match_origin`.
fn explicit_origin_tokens(query: &str) -> Option<Vec<String>> {
    const MAX_DESCRIPTIVE_WORDS: usize = 4;
    // Una palabra descriptiva y hasta tres más unidas por espacio, `_` o `-`.
    // No exige separador ni fin de texto después de la última: `02 reportes?`
    // termina en signo de interrogación y debe reconocerse igual.
    const DESCRIPTIVE: &str = r"[\p{L}][\p{L}\p{N}]*(?:[\s_-]+[\p{L}][\p{L}\p{N}]*){0,3}";
    const OPENING_QUOTE: &str = r#"["\u{201c}\u{201d}'«]"#;
    static JOINED: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?u)\b(\d{1,3})[_-]([\p{L}][\p{L}\p{N}_-]*)")
            .expect("valid joined-origin regex")
    });
    static QUOTED: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(&format!(
            r#"(?u){OPENING_QUOTE}\s*(\d{{1,3}})[\s_-]+({DESCRIPTIVE})\s*["\u{{201c}}\u{{201d}}'»]"#
        ))
        .expect("valid quoted-origin regex")
    });
    static CUED: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(&format!(
            r"(?iu)\b(?:carpetas?|origen|or[ií]genes|directorio|subcarpeta)\s+{OPENING_QUOTE}?\s*(\d{{1,3}})[\s_-]+({DESCRIPTIVE})"
        ))
        .expect("valid cued-origin regex")
    });
    static LOCATIVE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(&format!(
            r"(?iu)\b(?:en|del|de)\s+(0\d{{1,2}})[\s_-]+({DESCRIPTIVE})"
        ))
        .expect("valid locative-origin regex")
    });
    // El texto completo es sólo el nombre de la carpeta. Es el caso de un
    // literal ya extraído de sus comillas, donde no queda ninguna pista
    // alrededor. Al exigir que ocupe toda la cadena no puede confundirse con
    // un número suelto dentro de una frase.
    static BARE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(&format!(
            r"(?iu)^\s*(\d{{1,3}})[\s_-]+({DESCRIPTIVE})\s*$"
        ))
        .expect("valid bare-origin regex")
    });

    let captured = JOINED
        .captures(query)
        .or_else(|| QUOTED.captures(query))
        .or_else(|| CUED.captures(query))
        .or_else(|| LOCATIVE.captures(query))
        .or_else(|| BARE.captures(query))?;
    let ordinal = normalize_exact(&captured[1]);
    let descriptive = normalize_exact(&captured[2]);
    if ordinal.is_empty() || descriptive.is_empty() {
        return None;
    }
    let mut tokens = vec![ordinal];
    tokens.extend(
        descriptive
            .split_whitespace()
            .take(MAX_DESCRIPTIVE_WORDS)
            .map(str::to_owned),
    );
    Some(tokens)
}

/// Qué hizo la pregunta con el origen. Distinguir «no nombró ninguno» de
/// «nombró uno que no existe» es lo que impide sustituir en silencio una
/// carpeta pedida por otra parecida.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplicitOrigin {
    /// La pregunta no nombra ninguna carpeta de forma explícita.
    NotNamed,
    /// Nombró una carpeta que existe en el índice.
    Found(String),
    /// Nombró una carpeta que no existe. Nunca se sustituye por una parecida:
    /// el texto guardado es lo que el usuario escribió, para poder decírselo.
    Missing(String),
}

/// Mantiene una sola unidad de evidencia en un máximo de 360 caracteres. Si
/// la coincidencia está en medio, conserva contexto a ambos lados sin añadir
/// texto que no proceda del archivo.
fn brief_excerpt(value: &str, matched: Option<&str>) -> String {
    const MAX: usize = 360;
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX {
        return compact;
    }
    let start = matched
        .and_then(|needle| {
            let lower = compact.to_lowercase();
            lower
                .find(&needle.to_lowercase())
                .map(|byte_index| lower[..byte_index].chars().count())
        })
        .map(|index| index.saturating_sub(70))
        .unwrap_or(0);
    let chars = compact.chars().collect::<Vec<_>>();
    let end = (start + MAX.saturating_sub(1)).min(chars.len());
    let mut result = chars[start..end].iter().collect::<String>();
    if start > 0 {
        result.insert(0, '…');
    }
    if end < chars.len() {
        result.push('…');
    }
    result
}

/// `filter_key` es la comparación tipada de la que depende `append_filters`
/// (y sus dos hermanas): un número y un porcentaje nunca deben producir la
/// misma clave aunque compartan cifras, y el valor crudo nunca se altera al
/// clasificarlo — sólo decide cómo compararlo.
#[cfg(test)]
mod filter_key_tests {
    use super::*;

    #[test]
    fn fifty_and_fifty_percent_never_produce_the_same_key() {
        let number = filter_key("Descuento", "50");
        let percent = filter_key("Descuento", "50%");
        assert_ne!(
            number, percent,
            "«50» y «50%» comparten dígitos pero no son el mismo valor"
        );
        assert_eq!(number, FilterKey::Numeric { kind: "number", value: 50.0 });
        assert_eq!(
            percent,
            FilterKey::Numeric {
                kind: "percentage",
                value: 50.0
            }
        );
    }

    #[test]
    fn a_thousand_and_a_thousand_percent_never_produce_the_same_key() {
        let number = filter_key("Existencias", "1,000");
        let percent = filter_key("Existencias", "1,000%");
        assert_ne!(number, percent);
        // Mismo valor numérico (1000.0) en los dos, pero el tipo los separa:
        // si `filter_key` sólo comparara por número, aquí colisionarían.
        assert_eq!(number, FilterKey::Numeric { kind: "number", value: 1000.0 });
        assert_eq!(
            percent,
            FilterKey::Numeric {
                kind: "percentage",
                value: 1000.0
            }
        );
    }

    #[test]
    fn a_number_tolerates_thousands_formatting_but_a_percentage_does_not_leak_into_it() {
        // Tolerancia deseada: «1000» y «1,000» son el mismo número escrito
        // distinto, así que sí deben compartir clave.
        assert_eq!(filter_key("Existencias", "1000"), filter_key("Existencias", "1,000"));
        // Pero ninguno de los dos comparte clave con el porcentaje homónimo.
        assert_ne!(filter_key("Existencias", "1000"), filter_key("Existencias", "1,000%"));
    }

    #[test]
    fn a_quoted_text_value_keeps_its_full_literal_form() {
        let bare = filter_key("Estado", "Pendiente");
        let quoted = filter_key("Estado", "«Pendiente»");
        assert_ne!(bare, quoted);
        assert_eq!(bare, FilterKey::Literal("pendiente".into()));
        assert_eq!(quoted, FilterKey::Literal("«pendiente»".into()));
    }

    #[test]
    fn classifying_a_value_for_comparison_never_mutates_the_raw_text() {
        // La clasificación sólo decide CÓMO comparar: el texto que un
        // ToolFilter guarda y muestra (`equals`) no pasa por esta función en
        // ningún punto de escritura, sólo de lectura.
        let raw = "50%";
        let _ = filter_key("Descuento", raw);
        assert_eq!(raw, "50%", "filter_key no debe alterar el valor de origen");
    }

    #[test]
    fn a_colon_separated_value_is_not_the_same_key_as_a_hyphen_separated_look_alike() {
        assert_ne!(filter_key("Marcador", "3:2"), filter_key("Marcador", "3-2"));
    }

    #[test]
    fn filter_predicates_are_driven_from_matching_values_not_every_document() {
        let mut sql = String::from("SELECT COUNT(*) FROM documents d WHERE 1 = 1");
        let mut values = Vec::new();
        append_filters(
            &mut sql,
            &mut values,
            &[ToolFilter {
                concept: "Estado".into(),
                equals: "Cerrada".into(),
            }],
        );

        assert!(
            sql.contains("d.id IN (\n                        SELECT vf.document_id"),
            "el filtro debe empezar por los valores coincidentes para que un acervo grande no haga una subconsulta por documento: {sql}"
        );
        assert!(!sql.contains("EXISTS"), "{sql}");
        assert_eq!(values.len(), 2);
    }
}

/// Un filtro escrito «Campo: 50» nunca debe devolver documentos con «Campo:
/// 50%», «Campo: «50»» o «Campo: 3:2»: la normalización de texto quita
/// puntuación y los volvería indistinguibles si el tipo del valor no se
/// exigiera también.
#[cfg(test)]
mod numeric_filter_tests {
    use std::fs;

    use super::*;
    use crate::{db::Database, indexer::Indexer, parser::LocalDocumentParser};

    fn engine_with_fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, ToolEngine) {
        let fixture = tempfile::tempdir().unwrap();
        let documents = fixture.path().join("documentos");
        fs::create_dir_all(&documents).unwrap();
        for (name, content) in files {
            fs::write(documents.join(name), content).unwrap();
        }
        let database = Database::open(fixture.path().join("omega.db3")).unwrap();
        let parser = LocalDocumentParser::default();
        let indexer = Indexer::new(&database, &parser);
        let source_id = indexer.authorize(&documents).unwrap();
        indexer.index_source(source_id).unwrap();
        (fixture, ToolEngine::new(database))
    }

    fn filter(concept: &str, equals: &str) -> Vec<ToolFilter> {
        vec![ToolFilter {
            concept: concept.to_owned(),
            equals: equals.to_owned(),
        }]
    }


    #[test]
    fn a_plain_number_and_its_percentage_of_the_same_digits_stay_distinct() {
        let (_fixture, engine) = engine_with_fixture(&[
            (
                "a.md",
                "Folio: A-1\nDescuento: 50\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
            (
                "b.md",
                "Folio: B-1\nDescuento: 50%\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
        ]);
        let number = engine
            .query_documents(&filter("Descuento", "50"), None, 10)
            .unwrap();
        assert_eq!(number.document_count, 1);
        assert!(
            number
                .evidence
                .iter()
                .any(|item| item.value.as_deref() == Some("50"))
        );
        let percent = engine
            .query_documents(&filter("Descuento", "50%"), None, 10)
            .unwrap();
        assert_eq!(percent.document_count, 1);
        assert!(
            percent
                .evidence
                .iter()
                .any(|item| item.value.as_deref() == Some("50%"))
        );
    }

    #[test]
    fn a_grouped_thousand_and_its_percentage_stay_distinct() {
        let (_fixture, engine) = engine_with_fixture(&[
            (
                "a.md",
                "Folio: A-1\nExistencias: 1,000\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
            (
                "b.md",
                "Folio: B-1\nExistencias: 1,000%\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
        ]);
        let number = engine
            .query_documents(&filter("Existencias", "1,000"), None, 10)
            .unwrap();
        assert_eq!(number.document_count, 1);
        let percent = engine
            .query_documents(&filter("Existencias", "1,000%"), None, 10)
            .unwrap();
        assert_eq!(percent.document_count, 1);
    }

    #[test]
    fn a_value_quoted_in_guillemets_is_not_the_same_as_its_bare_number() {
        let (_fixture, engine) = engine_with_fixture(&[
            (
                "a.md",
                "Folio: A-1\nCodigo: 50\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
            (
                "b.md",
                "Folio: B-1\nCodigo: «50»\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
        ]);
        let bare = engine
            .query_documents(&filter("Codigo", "50"), None, 10)
            .unwrap();
        assert_eq!(bare.document_count, 1);
        let quoted = engine
            .query_documents(&filter("Codigo", "«50»"), None, 10)
            .unwrap();
        assert_eq!(quoted.document_count, 1);
    }

    #[test]
    fn a_value_with_a_colon_is_not_the_same_as_its_decimal_look_alike() {
        let (_fixture, engine) = engine_with_fixture(&[
            (
                "a.md",
                "Folio: A-1\nProporcion: 3.2\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
            (
                "b.md",
                "Folio: B-1\nProporcion: 3:2\n\nRegistro de inventario de prueba, sin relación con ningún giro concreto.\n",
            ),
        ]);
        let decimal = engine
            .query_documents(&filter("Proporcion", "3.2"), None, 10)
            .unwrap();
        assert_eq!(decimal.document_count, 1);
        let ratio = engine
            .query_documents(&filter("Proporcion", "3:2"), None, 10)
            .unwrap();
        assert_eq!(ratio.document_count, 1);
    }

    #[test]
    fn documents_with_values_preserves_a_scope_larger_than_sqlite_variable_limit() {
        let fixture = tempfile::tempdir().unwrap();
        let documents = fixture.path().join("documentos");
        fs::create_dir_all(&documents).unwrap();
        let total = 1_100;
        for index in 1..=total {
            let folio = if index == total { 1 } else { index };
            fs::write(
                documents.join(format!("{index:04}.md")),
                format!("Folio: BENCH-{folio:04}\nEstado: Cerrada\n"),
            )
            .unwrap();
        }
        let database = Database::open(fixture.path().join("omega.db3")).unwrap();
        let parser = LocalDocumentParser::default();
        let indexer = Indexer::new(&database, &parser);
        let source_id = indexer.authorize(&documents).unwrap();
        assert_eq!(indexer.index_source(source_id).unwrap().indexed, total);
        let engine = ToolEngine::new(database);

        let scope = (1..=total as i64).collect::<Vec<_>>();
        let matched = engine.documents_with_values(&scope, "Estado").unwrap();
        assert_eq!(matched.len(), total);
        assert_eq!(matched, scope);
        let duplicates = engine.duplicate_groups(&scope).unwrap();
        assert_eq!(duplicates.len(), 1);
        assert_eq!(duplicates[0].paths.len(), 2);
    }
}

#[cfg(test)]
mod tabular_tests {
    use super::tabular_record_scope;

    #[test]
    fn csv_and_workbook_cells_share_only_their_own_row_scope() {
        assert_eq!(
            tabular_record_scope("fila 8, celda F8 (Importe total)").as_deref(),
            Some("fila 8")
        );
        assert_eq!(
            tabular_record_scope("hoja Registros, celda C12 (Estado)").as_deref(),
            Some("hoja Registros, fila 12")
        );
    }
}

/// Referencia mínima a un documento del alcance. No lleva contenido: sirve
/// para definir un conjunto y para citarlo por su ruta real.
#[derive(Clone, Debug)]
pub struct DocumentRef {
    pub id: i64,
    pub path: String,
    pub origin: String,
    pub title: String,
}

/// Consulta de valores para el motor aritmético. Todos los límites del alcance
/// viajan juntos, de modo que un cálculo nunca puede ejecutarse sobre un
/// conjunto distinto del que la respuesta declara.
#[derive(Clone, Debug, Default)]
pub struct ValueQuery<'a> {
    pub concept: &'a str,
    pub filters: &'a [ToolFilter],
    pub origin: Option<&'a str>,
    /// Conjunto explícito de documentos. Cuando está presente manda sobre los
    /// filtros: es el «conjunto anterior» de una conversación.
    pub documents: Option<&'a [i64]>,
    pub date: Option<&'a DateConstraint>,
    pub group_by: Option<&'a str>,
    pub currency: Option<&'a str>,
}

/// Resultado de `collect_category_operands`: los operandos determinados y, por
/// separado, los dos motivos por los que un documento del alcance no aportó
/// ninguno. Se devuelven juntos porque la respuesta tiene que declararlos
/// juntos: una cifra sin su cobertura no es interpretable.
#[derive(Clone, Debug, Default)]
pub struct CategoryOperands {
    pub operands: Vec<Operand>,
    /// Documentos con más de un valor de la categoría: cuál es «el principal»
    /// no lo dice el documento, así que no se elige.
    pub ambiguous_documents: usize,
    /// Documentos sin ningún valor de la categoría.
    pub without_documents: usize,
    /// Campos realmente usados y en cuántos documentos, para poder nombrarlos
    /// en la respuesta en vez de hablar de «un campo monetario» en abstracto.
    pub fields: Vec<(String, usize)>,
}

/// Formato de archivo nombrado explícitamente en una pregunta de conteo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatRequest {
    /// Tal como lo escribió el usuario («DOCX», «PDF_SCAN»).
    pub label: String,
    /// Extensión real del índice a la que corresponde.
    pub extension: String,
    /// La pregunta pidió sólo los leídos por OCR (un PDF escaneado).
    pub scanned_only: bool,
}

/// Conteo por formato con su cobertura: cuántos documentos del alcance tienen
/// ese formato, cómo se reparten entre los que traen texto y los que hubo que
/// leer por OCR, y cuántos archivos del alcance no se pudieron indexar y por
/// tanto no están en ninguna de esas cifras.
#[derive(Clone, Debug, Default)]
pub struct FormatCount {
    pub matching: i64,
    /// Documentos de ese formato que están en la carpeta del alcance pero que
    /// los filtros de campo dejaron fuera. En un acervo con escaneos suele ser
    /// el mismo documento con su campo mal leído por OCR, no un documento de
    /// otro ámbito: se declara en vez de perderse.
    pub only_in_origin: i64,
    pub scanned: i64,
    pub with_text_layer: i64,
    pub unindexed: i64,
    /// El conteo de no indexados corresponde al alcance (había carpeta) o a
    /// todo el acervo (el alcance eran filtros de campo, que no pueden
    /// alcanzar un documento sin valores extraídos).
    pub unindexed_is_scoped: bool,
    pub evidence: Vec<Evidence>,
}

/// Estado de lectura de un documento concreto: el dato con el que Omega puede
/// responder por la fiabilidad de su **propia** lectura, en vez de callarla.
#[derive(Clone, Debug)]
pub struct DocumentReading {
    pub document_id: i64,
    pub path: String,
    pub origin: String,
    pub extension: String,
    pub status: OcrStatus,
    /// El estado tal como está escrito en el índice, para poder citarlo sin
    /// volver a traducirlo.
    pub stored_status: String,
    pub confidence: Option<f64>,
    /// Valores extraídos del documento. Un escaneo del que no salió nada
    /// utilizable no tiene ninguno.
    pub values: i64,
}

const ID_CHUNK: usize = 400;

impl ToolEngine {
    /// Acceso de sólo lectura a la base para los módulos que construyen
    /// relaciones. Ningún módulo abre la base por su cuenta.
    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    pub fn concept_by_name(&self, name: &str) -> Result<Option<ConceptSummary>> {
        let Some(id) = resolve_concept(&self.database, name)? else {
            return Ok(None);
        };
        let connection = self.database.connect()?;
        Ok(connection
            .query_row(
                "SELECT c.canonical_key, c.display_name, c.value_type,
                        (SELECT COUNT(*) FROM extracted_values v WHERE v.concept_id = c.id)
                 FROM concepts c WHERE c.id = ?1",
                [id],
                |row| {
                    Ok(ConceptSummary {
                        key: row.get(0)?,
                        display_name: row.get(1)?,
                        value_type: row.get(2)?,
                        occurrences: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Conceptos presentes en un conjunto concreto de documentos, ordenados por
    /// cuántos documentos del conjunto los contienen. Permite elegir el campo
    /// fecha o el campo numérico de un conjunto sin conocer el negocio.
    pub fn concepts_in_documents(&self, documents: &[i64]) -> Result<Vec<ConceptSummary>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }
        let connection = self.database.connect()?;
        let mut totals: BTreeMap<String, ConceptSummary> = BTreeMap::new();
        for chunk in documents.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT c.canonical_key, c.display_name, c.value_type, COUNT(DISTINCT v.document_id)
                 FROM extracted_values v JOIN concepts c ON c.id = v.concept_id
                 WHERE v.document_id IN ({placeholders})
                 GROUP BY c.id"
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(chunk.iter()), |row| {
                Ok(ConceptSummary {
                    key: row.get(0)?,
                    display_name: row.get(1)?,
                    value_type: row.get(2)?,
                    occurrences: row.get(3)?,
                })
            })?;
            for concept in rows {
                let concept = concept?;
                totals
                    .entry(concept.key.clone())
                    .and_modify(|stored| stored.occurrences += concept.occurrences)
                    .or_insert(concept);
            }
        }
        let mut concepts = totals.into_values().collect::<Vec<_>>();
        concepts.sort_by(|left, right| {
            right
                .occurrences
                .cmp(&left.occurrences)
                .then_with(|| left.key.cmp(&right.key))
        });
        Ok(concepts)
    }

    /// Documentos que cumplen simultáneamente el alcance completo.
    ///
    /// El rango de fechas se ancla a un campo: ambos extremos se comprueban
    /// contra el mismo valor, de modo que un documento con dos fechas
    /// distintas no puede satisfacer un extremo con cada una.
    pub fn documents_matching(
        &self,
        filters: &[ToolFilter],
        origin: Option<&str>,
        date: Option<&DateConstraint>,
    ) -> Result<Vec<DocumentRef>> {
        let connection = self.database.connect()?;
        let mut sql =
            String::from("SELECT d.id, d.path, d.origin, d.title FROM documents d WHERE 1 = 1");
        let mut values: Vec<Box<dyn ToSql>> = Vec::new();
        append_origin(&mut sql, &mut values, origin);
        append_filters(&mut sql, &mut values, filters);
        append_date(&mut sql, &mut values, date);
        sql.push_str(" ORDER BY d.id");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| {
                Ok(DocumentRef {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    origin: row.get(2)?,
                    title: row.get(3)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Recupera los operandos de un cálculo con su evidencia y, si se pidió,
    /// la etiqueta del grupo al que pertenece cada valor dentro de su propio
    /// registro.
    /// Operandos de una CATEGORÍA de valor (p. ej. todo lo monetario), no de
    /// un campo nombrado.
    ///
    /// Un documento sólo aporta operando cuando tiene **exactamente un** valor
    /// de esa categoría: entonces «el campo monetario del documento» está
    /// determinado por el propio documento y no hay nada que elegir. Un
    /// documento con dos o más se excluye y se cuenta aparte —decidir cuál de
    /// ellos es «el principal» sería adivinar—, y un documento sin ninguno se
    /// cuenta por su propio motivo. Los tres números salen de aquí para que la
    /// respuesta pueda declarar su cobertura sin recalcular nada.
    pub fn collect_category_operands(
        &self,
        value_type: &str,
        documents: &[i64],
    ) -> Result<CategoryOperands> {
        let mut result = CategoryOperands::default();
        if documents.is_empty() {
            return Ok(result);
        }
        let connection = self.database.connect()?;
        // Por documento, en el orden de inserción, para que «exactamente uno»
        // se decida sobre el conjunto completo de sus valores y no sobre el
        // trozo que tocó a un chunk.
        let mut per_document: BTreeMap<i64, Vec<(Operand, String)>> = BTreeMap::new();
        for chunk in documents.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT v.document_id, v.numeric_value, v.currency, v.location, v.excerpt,
                        v.evidence_id, v.text_value, d.path, d.origin, c.display_name,
                        d.ocr_status, d.ocr_confidence
                 FROM extracted_values v
                 JOIN documents d ON d.id = v.document_id
                 JOIN concepts c ON c.id = v.concept_id
                 WHERE v.value_type = ? AND v.document_id IN ({placeholders})
                 ORDER BY v.document_id, v.id"
            );
            let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(value_type.to_owned())];
            for id in chunk {
                values.push(Box::new(*id));
            }
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map(
                    params_from_iter(values.iter().map(|value| value.as_ref())),
                    |row| {
                        let document_id: i64 = row.get(0)?;
                        let text: String = row.get(6)?;
                        let field: String = row.get(9)?;
                        Ok((
                            Operand {
                                document_id,
                                numeric: row.get::<_, Option<f64>>(1)?,
                                currency: row.get::<_, Option<String>>(2)?,
                                group: None,
                                evidence: Evidence {
                                    id: row.get(5)?,
                                    document_id,
                                    path: row.get(7)?,
                                    origin: row.get(8)?,
                                    location: row.get(3)?,
                                    excerpt: row.get(4)?,
                                    normalized_value: Some(normalize_exact(&text)),
                                    value: Some(text.clone()),
                                    matched: Some(text),
                                    field: Some(field.clone()),
                                    match_kind: "campo".into(),
                                    reliable: ocr_is_reliable(
                                        &row.get::<_, String>(10)?,
                                        row.get::<_, Option<f64>>(11)?,
                                    ),
                                    ocr_status: Some(row.get(10)?),
                                    ocr_confidence: row.get(11)?,
                                    confidence: row.get(11)?,
                                },
                            },
                            field,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (operand, field) in rows {
                per_document
                    .entry(operand.document_id)
                    .or_default()
                    .push((operand, field));
            }
        }
        let mut fields: BTreeMap<String, usize> = BTreeMap::new();
        for document in documents {
            match per_document.get(document).map(Vec::as_slice) {
                None | Some([]) => result.without_documents += 1,
                Some([(operand, field)]) => {
                    *fields.entry(field.clone()).or_default() += 1;
                    result.operands.push(operand.clone());
                }
                Some(_) => result.ambiguous_documents += 1,
            }
        }
        result.fields = fields.into_iter().collect();
        result
            .fields
            .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        Ok(result)
    }

    pub fn collect_operands(&self, query: &ValueQuery<'_>) -> Result<Vec<Operand>> {
        let Some(concept_id) = resolve_concept(&self.database, query.concept)? else {
            return Ok(vec![]);
        };
        let group_id = match query.group_by {
            Some(name) => match resolve_concept(&self.database, name)? {
                Some(id) => Some(id),
                None => return Ok(vec![]),
            },
            None => None,
        };
        let connection = self.database.connect()?;
        let mut operands = Vec::new();
        let chunks: Vec<Vec<i64>> = match query.documents {
            Some(documents) => {
                if documents.is_empty() {
                    return Ok(vec![]);
                }
                documents
                    .chunks(ID_CHUNK)
                    .map(<[i64]>::to_vec)
                    .collect::<Vec<_>>()
            }
            None => vec![vec![]],
        };
        for chunk in chunks {
            let mut sql = String::from(
                "SELECT v.document_id, v.numeric_value, v.currency, v.location, v.excerpt,
                        v.evidence_id, v.text_value, d.path, d.origin, c.display_name,
                        d.ocr_status, d.ocr_confidence
                 FROM extracted_values v
                 JOIN documents d ON d.id = v.document_id
                 JOIN concepts c ON c.id = v.concept_id
                 WHERE v.concept_id = ?",
            );
            let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(concept_id)];
            if !chunk.is_empty() {
                let placeholders = vec!["?"; chunk.len()].join(",");
                sql.push_str(&format!(" AND v.document_id IN ({placeholders})"));
                for id in &chunk {
                    values.push(Box::new(*id));
                }
            } else {
                append_origin(&mut sql, &mut values, query.origin);
                append_filters(&mut sql, &mut values, query.filters);
                append_date(&mut sql, &mut values, query.date);
            }
            if let Some(currency) = query.currency {
                sql.push_str(" AND upper(v.currency) = upper(?)");
                values.push(Box::new(currency.to_owned()));
            }
            sql.push_str(" ORDER BY v.document_id, v.id");
            let mut statement = connection.prepare(&sql)?;
            let rows = statement
                .query_map(
                    params_from_iter(values.iter().map(|value| value.as_ref())),
                    |row| {
                        let document_id: i64 = row.get(0)?;
                        let text: String = row.get(6)?;
                        Ok((
                            document_id,
                            row.get::<_, Option<f64>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            Evidence {
                                id: row.get(5)?,
                                document_id,
                                path: row.get(7)?,
                                origin: row.get(8)?,
                                location: row.get(3)?,
                                excerpt: row.get(4)?,
                                normalized_value: Some(normalize_exact(&text)),
                                value: Some(text.clone()),
                                matched: Some(text),
                                field: Some(row.get(9)?),
                                match_kind: "campo".into(),
                                reliable: ocr_is_reliable(
                                    &row.get::<_, String>(10)?,
                                    row.get::<_, Option<f64>>(11)?,
                                ),
                                ocr_status: Some(row.get(10)?),
                                ocr_confidence: row.get(11)?,
                                confidence: row.get(11)?,
                            },
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (document_id, numeric, currency, location, evidence) in rows {
                let groups = match group_id {
                    Some(group) => self.group_labels(&connection, document_id, group, &location)?,
                    None => vec![None],
                };
                for group in groups {
                    operands.push(Operand {
                        document_id,
                        numeric,
                        currency: currency.clone(),
                        group,
                        evidence: evidence.clone(),
                    });
                }
            }
        }
        Ok(operands)
    }

    /// Etiquetas de grupo aplicables a un valor. Reutiliza la unidad de
    /// registro tabular para que el importe de una fila se asocie al grupo de
    /// esa misma fila y no a todos los del archivo.
    fn group_labels(
        &self,
        connection: &rusqlite::Connection,
        document_id: i64,
        group_concept: i64,
        location: &str,
    ) -> Result<Vec<Option<String>>> {
        let mut statement = connection.prepare(
            "SELECT DISTINCT text_value, location FROM extracted_values
             WHERE document_id = ?1 AND concept_id = ?2",
        )?;
        let found = statement
            .query_map(params![document_id, group_concept], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        let scope = tabular_record_scope(location);
        let labels = match scope {
            Some(scope) => found
                .into_iter()
                .filter(|(_, place)| tabular_record_scope(place).as_deref() == Some(&scope))
                .map(|(value, _)| value)
                .collect::<Vec<_>>(),
            None => found.into_iter().map(|(value, _)| value).collect(),
        };
        Ok(if labels.is_empty() {
            vec![Some(NO_GROUP_VALUE.to_owned())]
        } else {
            labels.into_iter().map(Some).collect()
        })
    }
}

/// Etiqueta para el grupo de documentos que no declaran el campo de
/// agrupación. Nombrarlo evita presentarlos como si pertenecieran a otro grupo.
pub const NO_GROUP_VALUE: &str = "Sin valor";

fn append_date(sql: &mut String, values: &mut Vec<Box<dyn ToSql>>, date: Option<&DateConstraint>) {
    if let Some(date) = date {
        sql.push_str(
            " AND EXISTS (
                SELECT 1 FROM extracted_values vd JOIN concepts cd ON cd.id = vd.concept_id
                WHERE vd.document_id = d.id AND cd.canonical_key = ?
                  AND vd.date_value IS NOT NULL
                  AND vd.date_value >= ? AND vd.date_value <= ?
              )",
        );
        values.push(Box::new(canonical_key(&date.concept)));
        values.push(Box::new(date.from.clone()));
        values.push(Box::new(date.to.clone()));
    }
}

/// Documentos del acervo con contenido byte a byte idéntico.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub content_hash: String,
    /// Rutas de todas las copias, en orden estable.
    pub paths: Vec<String>,
}

impl ToolEngine {
    /// Grupos de contenido idéntico dentro de un conjunto de documentos.
    ///
    /// No altera nada: sólo dice cuáles de esos documentos son copias exactas
    /// entre sí, para que una respuesta que se apoye en ellos pueda decirlo.
    /// Un documento cuya copia está fuera del conjunto no cuenta aquí: lo que
    /// importa es si el propio cálculo sumó el mismo contenido dos veces.
    pub fn duplicate_groups(&self, documents: &[i64]) -> Result<Vec<DuplicateGroup>> {
        if documents.len() < 2 {
            return Ok(vec![]);
        }
        let unique = documents.iter().copied().collect::<BTreeSet<_>>();
        let connection = self.database.connect()?;
        // Sólo los hashes que ya tienen más de una copia global pueden
        // afectar la respuesta. Se descubren una vez en SQLite y el alcance
        // se cruza en memoria: no se forma un `IN` gigante ni se insertan
        // decenas de miles de IDs temporales por cada respuesta.
        let mut statement = connection.prepare(
            "SELECT d.id, d.content_hash, d.path FROM documents d
             WHERE d.content_hash IN (
                 SELECT content_hash FROM documents
                 GROUP BY content_hash HAVING COUNT(*) > 1
             ) ORDER BY d.content_hash, d.path",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut paths_by_hash: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for row in rows {
            let (id, hash, path) = row?;
            if unique.contains(&id) {
                paths_by_hash.entry(hash).or_default().push(path);
            }
        }
        Ok(paths_by_hash
            .into_iter()
            .filter_map(|(content_hash, paths)| {
                (paths.len() > 1).then_some(DuplicateGroup {
                    content_hash,
                    paths,
                })
            })
            .collect())
    }

    /// De un conjunto de documentos, los que tienen algún valor extraído del
    /// campo indicado. Es la forma barata de saber cuáles participaron de
    /// verdad en un cálculo sobre ese campo, sin repetir la agregación.
    pub fn documents_with_values(&self, documents: &[i64], concept: &str) -> Result<Vec<i64>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }
        let unique = documents.iter().copied().collect::<BTreeSet<_>>();
        let connection = self.database.connect()?;
        let key = canonical_key(concept);
        // Se parte de los documentos que realmente tienen el concepto y se
        // cruza el alcance en memoria. Así no hay un `IN` de miles de
        // variables ni una consulta por lote cuando el alcance es grande.
        let mut statement = connection.prepare(
            "SELECT DISTINCT v.document_id FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             WHERE c.canonical_key = ?1 ORDER BY v.document_id",
        )?;
        let rows = statement.query_map([key], |row| row.get::<_, i64>(0))?;
        let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|id| unique.contains(id))
            .collect())
    }

    /// Una cita por documento del conjunto: su primer fragmento real. Permite
    /// respaldar un conteo con los documentos que lo produjeron.
    pub fn evidence_for_documents(&self, documents: &[i64], limit: usize) -> Result<Vec<Evidence>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }
        let connection = self.database.connect()?;
        let selected = documents.iter().take(limit.min(50)).collect::<Vec<_>>();
        let placeholders = vec!["?"; selected.len()].join(",");
        let sql = format!(
            "SELECT d.id, d.path, d.origin, d.ocr_status, d.ocr_confidence, c.location, c.content
             FROM documents d JOIN chunks c ON c.document_id = d.id
             WHERE d.id IN ({placeholders})
             GROUP BY d.id ORDER BY d.id"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(selected.iter()), |row| {
            let document_id: i64 = row.get(0)?;
            Ok(Evidence {
                id: format!("doc-{document_id}"),
                document_id,
                path: row.get(1)?,
                origin: row.get(2)?,
                location: row.get(5)?,
                excerpt: row.get(6)?,
                normalized_value: None,
                value: None,
                matched: None,
                field: None,
                match_kind: "campo".into(),
                reliable: ocr_is_reliable(
                    &row.get::<_, String>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                ),
                ocr_status: Some(row.get(3)?),
                ocr_confidence: row.get(4)?,
                confidence: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Un par «Campo: valor» escrito por el usuario y ya resuelto contra el acervo.
#[derive(Clone, Debug)]
pub struct WrittenFilters {
    /// Pares cuyo campo **y** valor existen tal cual.
    pub filters: Vec<ToolFilter>,
    /// Pares cuyo campo existe pero cuyo valor no aparece completo en el
    /// acervo. Nunca se degradan a una coincidencia parcial.
    pub unresolved: Vec<UnresolvedPair>,
}

#[derive(Clone, Debug)]
pub struct UnresolvedPair {
    /// Nombre real del concepto que el usuario nombró.
    pub concept: String,
    /// Valor tal como lo escribió el usuario.
    pub written: String,
    /// Valores existentes emparentados con el escrito. Se ofrecen como
    /// aclaración; jamás se aplican solos.
    pub near: Vec<String>,
}

impl WrittenFilters {
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty() && self.unresolved.is_empty()
    }
}

impl ToolEngine {
    /// Resuelve los pares «Campo: valor» que la pregunta escribe literalmente.
    ///
    /// Un valor escrito por el usuario se exige **completo**: si pide
    /// «Estado: Pendiente de emisión» y el acervo sólo tiene «Pendiente», el
    /// par queda sin resolver y el motor tendrá que preguntar, nunca responder
    /// con el valor recortado como si fuera el pedido.
    pub fn written_filters(&self, question: &str) -> Result<WrittenFilters> {
        let concepts = self.list_concepts(None)?;
        let mut filters = Vec::new();
        let mut unresolved = Vec::new();
        let pairs = written_pairs(question);
        for (index, (field_text, raw_value)) in pairs.iter().enumerate() {
            let Some(concept) = concept_named_in(&concepts, field_text) else {
                // Sin un campo real detrás, los dos puntos son puntuación, no
                // un filtro. La pregunta sigue su curso normal.
                continue;
            };
            let next_field = pairs
                .get(index + 1)
                .and_then(|(next, _)| concept_named_in(&concepts, next));
            let value_text = trim_next_field(raw_value, next_field.as_ref());
            if value_text.is_empty() {
                continue;
            }
            let values = self.concept_values(&concept.display_name)?;
            // Comparación tipada, no `normalize_spanish` ni un texto crudo:
            // un número y un porcentaje comparan por su valor numérico
            // exacto (tolerante al formato) sin mezclarse entre sí, y todo
            // lo demás por su forma literal. Así «Campo: 50» nunca resuelve
            // al «50%» existente en el acervo sólo porque compartan dígitos.
            let wanted_key = filter_key(&concept.display_name, &value_text);
            if let Some(exact) = values
                .iter()
                .find(|value| filter_key(&concept.display_name, value) == wanted_key)
            {
                filters.push(ToolFilter {
                    concept: concept.display_name.clone(),
                    equals: exact.clone(),
                });
                continue;
            }
            // Las sugerencias emparentadas sí usan una comparación difusa
            // (raíces, sin puntuación): son una aclaración que el usuario
            // confirma, nunca un valor que el motor aplica solo, así que
            // aproximarse de más aquí no reintroduce el riesgo de confundir
            // «50» con «50%».
            let wanted = normalize_spanish(&value_text);
            let near = values
                .iter()
                .filter(|value| {
                    let candidate = normalize_spanish(value);
                    candidate.contains(&wanted) || wanted.contains(&candidate)
                })
                .take(6)
                .cloned()
                .collect::<Vec<_>>();
            unresolved.push(UnresolvedPair {
                concept: concept.display_name.clone(),
                written: value_text.clone(),
                near,
            });
        }
        Ok(WrittenFilters {
            filters,
            unresolved,
        })
    }

    /// Valores de campos de texto o estado que la pregunta menciona
    /// literalmente, con el campo al que pertenecen.
    ///
    /// A diferencia de la inferencia de filtros, aquí se exige que el valor
    /// completo aparezca como frase en la pregunta: es lo que permite
    /// reconocer los dos grupos de una comparación sin adivinar.
    pub fn values_mentioned(&self, question: &str, origin: Option<&str>) -> Result<Vec<(String, String)>> {
        let exact_query = normalize_exact(question);
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT DISTINCT c.display_name, v.text_value
             FROM extracted_values v
             JOIN concepts c ON c.id = v.concept_id
             JOIN documents d ON d.id = v.document_id
             WHERE v.value_type IN ('text', 'state')",
        );
        let mut parameters: Vec<Box<dyn ToSql>> = Vec::new();
        append_origin(&mut sql, &mut parameters, origin);
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map(
                params_from_iter(parameters.iter().map(|value| value.as_ref())),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|(_, value)| {
                let normalized = normalize_exact(value);
                !normalized.is_empty()
                    && normalized.split_whitespace().count() <= 8
                    && whole_phrase_in(&exact_query, &normalized)
            })
            .collect())
    }

    /// Posición de un valor dentro de la pregunta, para conservar el orden en
    /// que el usuario nombró los grupos de una comparación.
    pub fn mention_position(question: &str, value: &str) -> usize {
        phrase_position(&normalize_exact(question), &normalize_exact(value)).unwrap_or(usize::MAX)
    }
}

/// Palabras que la pregunta escribe como NOMBRE de campo: las que están
/// pegadas por la izquierda a un separador «campo: valor» o «campo=valor».
///
/// No decide ningún filtro por sí sola; sirve para lo contrario, para que la
/// inferencia de filtros no vuelva a usar como VALOR una palabra que el
/// usuario acaba de escribir como nombre de campo.
fn written_field_name_tokens(question: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    for (index, character) in question.char_indices() {
        if character != ':' && character != '=' {
            continue;
        }
        let Some(word) = question[..index].split_whitespace().next_back() else {
            continue;
        };
        tokens.extend(search_terms(word));
    }
    tokens
}

/// Extrae los pares «texto: texto» de una pregunta.
///
/// El valor de cada par llega hasta los siguientes dos puntos —o hasta el final
/// de la pregunta—, sin cortarlo por conectores: un valor puede contener « y »
/// («Evaluación de seguridad y notificación al responsable de turno») y
/// partirlo ahí convertiría el filtro en otro más corto que el usuario no pidió.
/// Separar dos filtros escritos seguidos es trabajo de quien conoce los nombres
/// de campo reales.
fn written_pairs(question: &str) -> Vec<(String, String)> {
    let chars = question.char_indices().collect::<Vec<_>>();
    let colons = chars
        .iter()
        .enumerate()
        .filter(|(_, (_, character))| *character == ':')
        .filter(|(position, _)| !is_time_like_colon(&chars, *position))
        .map(|(_, (index, _))| *index)
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for (position, colon) in colons.iter().enumerate() {
        let start = position
            .checked_sub(1)
            .map(|previous| colons[previous] + 1)
            .unwrap_or(0);
        let end = colons.get(position + 1).copied().unwrap_or(question.len());
        let field = question[start..*colon].trim();
        let value = question[colon + 1..end].trim();
        if field.is_empty() || value.is_empty() {
            continue;
        }
        // Unos dos puntos que siguen a un identificador son puntuación de la
        // frase, no el separador de un par «Campo: valor». «Revisa el pedido
        // ABC-2023-00116: ¿cuál es su importe?» no declara ningún filtro: el
        // identificador ya nombra al documento y lo que sigue es la pregunta.
        // Sin esta distinción el resto de la frase se tomaba como el valor
        // buscado —y la respuesta era una negativa sobre un valor que nadie
        // escribió— antes siquiera de intentar localizar el documento.
        if ends_with_identifier(field) {
            continue;
        }
        pairs.push((field.to_owned(), value.to_owned()));
    }
    pairs
}

/// ¿Termina este texto en algo que ya es un identificador por sí mismo?
///
/// Se mira sólo la última palabra —la que quedaría pegada a los dos puntos— y
/// se le quitan los adornos con que se suele citar («`ABC-1`», "ABC-1"). El
/// criterio es el mismo `canonical_identifier` que usa la recuperación, así que
/// exige letras **y** dígitos: un campo con número suelto («Turno 2») o un
/// nombre normal («Moneda») nunca se confunden con un identificador.
fn ends_with_identifier(field: &str) -> bool {
    field
        .split_whitespace()
        .next_back()
        .map(|word| word.trim_matches(|character: char| !character.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .and_then(canonical_identifier)
        .is_some()
}

/// ¿Es este colon parte de una hora u otro valor «dígito:dígito», como
/// «10:30», en vez de un separador «Campo: valor»?
///
/// El nombre de un campo siempre termina en una letra, nunca en un dígito
/// pegado al colon sin espacio: por eso basta con mirar los dos caracteres
/// que rodean al colon. Sin esta distinción, «Hora: 10:30» se partía en dos
/// pares por el colon interno y el valor quedaba truncado en «10».
fn is_time_like_colon(chars: &[(usize, char)], position: usize) -> bool {
    let before = position.checked_sub(1).map(|index| chars[index].1);
    let after = chars.get(position + 1).map(|(_, character)| *character);
    matches!(before, Some(character) if character.is_ascii_digit())
        && matches!(after, Some(character) if character.is_ascii_digit())
}

/// Recorta del final de un valor el nombre de campo del par siguiente.
///
/// «Ciudad base: Norte y Estado: Abierto» deja «Norte y Estado» como valor del
/// primer par; aquí se quitan las palabras justas que forman «Estado» y el
/// conector que las unía, sin tocar valores que legítimamente contienen « y ».
fn trim_next_field(value: &str, next: Option<&ConceptSummary>) -> String {
    let cleaned = |text: &str| {
        text.trim()
            .trim_end_matches(['?', '.', '!', ',', ';'])
            .trim()
            .trim_end_matches(" y")
            .trim_end_matches(" e")
            .trim()
            .to_owned()
    };
    let Some(concept) = next else {
        return cleaned(value);
    };
    let wanted = search_terms(&concept.display_name);
    let words = value.split_whitespace().collect::<Vec<_>>();
    let limit = words.len().min(wanted.len() + 3);
    for dropped in 1..=limit {
        let tail = words[words.len() - dropped..].join(" ");
        let tail_terms = search_terms(&tail);
        if wanted.iter().all(|term| {
            tail_terms
                .iter()
                .any(|candidate| stems_match(candidate, term))
        }) {
            return cleaned(&words[..words.len() - dropped].join(" "));
        }
    }
    cleaned(value)
}

/// Concepto nombrado al final de un texto: se prueba con las últimas palabras,
/// de la frase más larga a la más corta, y gana el nombre más específico.
fn concept_named_in(concepts: &[ConceptSummary], text: &str) -> Option<ConceptSummary> {
    let words = normalize_exact(text)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut best: Option<ConceptSummary> = None;
    for concept in concepts {
        let concept_terms = search_terms(&concept.display_name);
        if concept_terms.is_empty() {
            continue;
        }
        // El nombre del campo debe aparecer completo y pegado al final del
        // texto que precede a los dos puntos.
        let tail = words
            .iter()
            .rev()
            .take(concept_terms.len() + 2)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let tail_terms = search_terms(&tail);
        if concept_terms
            .iter()
            .all(|term| tail_terms.iter().any(|candidate| stems_match(candidate, term)))
            && best
                .as_ref()
                .is_none_or(|current| search_terms(&current.display_name).len() < concept_terms.len())
        {
            best = Some(concept.clone());
        }
    }
    best
}
