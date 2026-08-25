use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::LazyLock,
};

use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use serde_json::{Value, json};

use crate::{
    db::Database,
    error::{OmegaError, Result},
    model::{
        AggregateRequest, AggregateRow, ConceptSummary, Evidence, SearchHit, ToolFilter, ToolResult,
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
    pub evidence: Evidence,
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
                let rows = self.aggregate(&request)?;
                let mut evidence = calculation_evidence(&request, &rows)
                    .into_iter()
                    .collect::<Vec<_>>();
                evidence.extend(rows.iter().flat_map(|row| row.evidence.clone()));
                Ok(ToolResult {
                    tool: name.into(),
                    data: serde_json::to_value(rows).unwrap(),
                    evidence,
                })
            }
            _ => Err(OmegaError::InvalidArguments(format!(
                "herramienta desconocida: {name}"
            ))),
        }
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
            return self.strict_exact_search(&exact_tokens, limit);
        }
        // Si el acervo reconoce a la vez un campo y uno de sus valores dentro
        // de la pregunta, la combinación es una condición obligatoria. No se
        // permite completar esta respuesta con FTS, metadatos ni otro campo
        // que sólo comparta alguna palabra del nombre.
        if let Some(hits) = self.strict_structured_hits(query, filters, limit)? {
            return Ok(hits);
        }
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
        for hit in self.structured_hits(query, filters, false)?.into_values() {
            keep_best(&mut by_document, hit);
        }
        let fts_query = terms
            .iter()
            .map(|term| format!("\"{}\"*", term.replace('"', "")))
            .collect::<Vec<_>>()
            // Una coincidencia de una palabra común no basta para convertir un
            // membrete repetido en resultado. El FTS solo complementa a los
            // campos extraídos y exige todos los términos útiles.
            .join(" AND ");
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
        sql.push_str(" ORDER BY bm25(chunks_fts) LIMIT ?");
        values.push(Box::new((limit.saturating_mul(20)).min(400) as i64));
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
        if pairs.is_empty() {
            return if self.query_names_field_with_value(query)? {
                // La consulta sí tiene forma campo–valor, pero el valor no
                // existe para ese campo. Es importante cerrar aquí: permitir
                // FTS devolvería documentos con el mismo campo y otro valor.
                Ok(Some(vec![]))
            } else {
                Ok(None)
            };
        }
        let mut required_filters = filters.to_vec();
        required_filters.extend(pairs.iter().map(|pair| ToolFilter {
            concept: pair.field.clone(),
            equals: pair.value.clone(),
        }));
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
            if !pairs.contains(&pair) {
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

    fn structured_pairs_in_query(
        &self,
        query: &str,
        filters: &[ToolFilter],
    ) -> Result<HashSet<FieldValuePair>> {
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
        let mut pairs = HashSet::new();
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
                pairs.insert(pair);
            }
        }
        // Si "Estado" y "Estado del documento" son conceptos distintos,
        // prevalece el nombre de campo más específico para el mismo valor.
        // Así la presencia de una palabra compartida no abre otro concepto.
        let candidates = pairs.clone();
        pairs.retain(|pair| {
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
            let (score, location, excerpt, field) = if title_match {
                (
                    130.0,
                    "metadato: nombre de archivo",
                    title.clone(),
                    "nombre de archivo",
                )
            } else if !exact_only && origin_match {
                (
                    90.0,
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
                Some(76.0)
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
                    d.path, d.origin, d.ocr_status, d.ocr_confidence
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
            Ok(DocumentValue {
                // La posición real la asigna el recorrido, no la consulta.
                ordinal: 0,
                field: field.clone(),
                value: text_value.clone(),
                value_type,
                identifier_canonical,
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
            if field_named {
                explicit_values.push(normalize_spanish(&value));
                explicit.push(ToolFilter {
                    concept: field,
                    equals: value,
                });
            } else if allow_implicit_values && (value_terms.len() >= 2 || value_type == "state") {
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
                explicit.push(ToolFilter { concept, equals });
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
        Ok(explicit)
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
                    confidence,
                });
            }
            for filter in filters {
                let found = connection
                    .query_row(
                        "SELECT v.evidence_id, v.location, v.excerpt, v.text_value,
                                c.display_name
                         FROM extracted_values v
                         JOIN concepts c ON c.id = v.concept_id
                         WHERE v.document_id = ?1 AND c.canonical_key = ?2
                           AND v.normalized_value = ?3 ORDER BY v.id LIMIT 1",
                        params![
                            document_id,
                            canonical_key(&filter.concept),
                            normalize_spanish(&filter.equals)
                        ],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                            ))
                        },
                    )
                    .optional()?;
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
        sql.push_str(" ORDER BY bm25(chunks_fts) LIMIT 4000");
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
            "SELECT d.id, d.path, d.origin, c.location, c.content
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
                    location: row.get(3)?,
                    excerpt: row.get(4)?,
                    normalized_value: None,
                    value: None,
                    matched: None,
                    field: None,
                    match_kind: "campo".into(),
                    reliable: true,
                    confidence: None,
                })
            },
        )?;
        Ok((count, rows.collect::<rusqlite::Result<Vec<_>>>()?))
    }

    pub fn aggregate(&self, request: &AggregateRequest) -> Result<Vec<AggregateRow>> {
        if request.operation != "sum" && request.operation != "count" {
            return Err(OmegaError::InvalidArguments(
                "operation debe ser sum o count".into(),
            ));
        }
        let concept = resolve_concept(&self.database, &request.concept)?.ok_or_else(|| {
            OmegaError::InvalidArguments(format!("el concepto '{}' no existe", request.concept))
        })?;
        let group_concept = match request.group_by.as_deref() {
            Some(value) => Some(resolve_concept(&self.database, value)?.ok_or_else(|| {
                OmegaError::InvalidArguments(format!(
                    "el concepto de agrupación '{value}' no existe"
                ))
            })?),
            None => None,
        };
        let connection = self.database.connect()?;
        let mut sql = String::from(
            "SELECT v.id, v.document_id, v.numeric_value, v.text_value, v.currency,
                    v.location, v.excerpt, v.evidence_id, d.path, d.origin, c.display_name
             FROM extracted_values v
             JOIN documents d ON d.id = v.document_id
             JOIN concepts c ON c.id = v.concept_id
             WHERE v.concept_id = ?",
        );
        let mut values: Vec<Box<dyn ToSql>> = vec![Box::new(concept)];
        if request.operation == "sum" {
            sql.push_str(" AND v.numeric_value IS NOT NULL");
        }
        if let Some(currency) = &request.currency {
            sql.push_str(" AND upper(v.currency) = upper(?)");
            values.push(Box::new(currency.clone()));
        }
        if let Some(from) = &request.date_from {
            sql.push_str(" AND EXISTS (SELECT 1 FROM extracted_values vd WHERE vd.document_id = v.document_id AND vd.date_value >= ?)");
            values.push(Box::new(from.clone()));
        }
        if let Some(to) = &request.date_to {
            sql.push_str(" AND EXISTS (SELECT 1 FROM extracted_values vd WHERE vd.document_id = v.document_id AND vd.date_value <= ?)");
            values.push(Box::new(to.clone()));
        }
        append_origin(&mut sql, &mut values, request.origin.as_deref());
        append_filters(&mut sql, &mut values, &request.filters);
        sql.push_str(" ORDER BY v.document_id, v.id");

        let mut statement = connection.prepare(&sql)?;
        let matches = statement
            .query_map(
                params_from_iter(values.iter().map(|value| value.as_ref())),
                |row| {
                    Ok((
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        Evidence {
                            id: row.get(7)?,
                            document_id: row.get(1)?,
                            path: row.get(8)?,
                            origin: row.get(9)?,
                            location: row.get(5)?,
                            excerpt: row.get(6)?,
                            normalized_value: Some(normalize_exact(&row.get::<_, String>(3)?)),
                            value: row.get(3)?,
                            matched: Some(row.get(3)?),
                            field: Some(row.get(10)?),
                            match_kind: "campo".into(),
                            reliable: true,
                            confidence: None,
                        },
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if matches.is_empty() {
            return Ok(vec![]);
        }
        let mut grouped: BTreeMap<(Option<String>, Option<String>), AggregateRow> = BTreeMap::new();
        for (document_id, numeric, _text, currency, evidence) in matches {
            let groups = if let Some(group_id) = group_concept {
                let mut group_statement = connection.prepare(
                    "SELECT DISTINCT text_value, location FROM extracted_values WHERE document_id = ?1 AND concept_id = ?2",
                )?;
                let found = group_statement
                    .query_map(params![document_id, group_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
                let record_scope = tabular_record_scope(&evidence.location);
                let found = if let Some(scope) = record_scope {
                    found
                        .into_iter()
                        .filter(|(_, location)| {
                            tabular_record_scope(location).as_deref() == Some(&scope)
                        })
                        .map(|(value, _)| value)
                        .collect::<Vec<_>>()
                } else {
                    found
                        .into_iter()
                        .map(|(value, _)| value)
                        .collect::<Vec<_>>()
                };
                if found.is_empty() {
                    vec![Some("Sin valor".into())]
                } else {
                    found.into_iter().map(Some).collect()
                }
            } else {
                vec![None]
            };
            for group in groups {
                // Los conteos no tienen dimensión monetaria. Las sumas sí se
                // separan por moneda cuando la pregunta no fijó una, para no
                // sumar magnitudes incompatibles silenciosamente.
                let row_currency = (request.operation == "sum")
                    .then(|| currency.clone())
                    .flatten();
                let row = grouped
                    .entry((group.clone(), row_currency.clone()))
                    .or_insert_with(|| AggregateRow {
                        group,
                        currency: row_currency,
                        value: 0.0,
                        matched_values: 0,
                        evidence: vec![],
                    });
                row.value += if request.operation == "count" {
                    1.0
                } else {
                    numeric.unwrap_or(0.0)
                };
                row.matched_values += 1;
                if row.evidence.len() < 50 {
                    row.evidence.push(evidence.clone());
                }
            }
        }
        Ok(grouped.into_values().collect())
    }

    pub fn aggregate_calculation_evidence(
        &self,
        request: &AggregateRequest,
        rows: &[AggregateRow],
    ) -> Option<Evidence> {
        calculation_evidence(request, rows)
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

fn calculation_evidence(request: &AggregateRequest, rows: &[AggregateRow]) -> Option<Evidence> {
    let first = rows.iter().flat_map(|row| row.evidence.iter()).next()?;
    let total_matches = rows.iter().map(|row| row.matched_values).sum::<i64>();
    let rendered = if rows.len() == 1 {
        format!(
            "{}{}",
            format_number(rows[0].value),
            rows[0]
                .currency
                .as_deref()
                .map(|value| format!(" {value}"))
                .unwrap_or_default()
        )
    } else {
        rows.iter()
            .map(|row| {
                format!(
                    "{}: {}{}",
                    row.group.as_deref().unwrap_or("Total"),
                    format_number(row.value),
                    row.currency
                        .as_deref()
                        .map(|value| format!(" {value}"))
                        .unwrap_or_default()
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
            total_matches
        ),
        document_id: first.document_id,
        path: first.path.clone(),
        origin: first.origin.clone(),
        location: format!("cálculo local exacto sobre {total_matches} valores extraídos"),
        excerpt: format!(
            "Omega ejecutó {} para el concepto '{}' y obtuvo {} a partir de {total_matches} valores con evidencia.",
            if request.operation == "sum" {
                "una suma"
            } else {
                "un conteo"
            },
            request.concept,
            rendered,
        ),
        normalized_value: None,
        value: Some(rendered.clone()),
        matched: Some(rendered),
        field: None,
        match_kind: "campo".into(),
        reliable: true,
        confidence: None,
    })
}

fn format_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn append_filters(sql: &mut String, values: &mut Vec<Box<dyn ToSql>>, filters: &[ToolFilter]) {
    for filter in filters {
        sql.push_str(
            " AND EXISTS (
                SELECT 1 FROM extracted_values vf JOIN concepts cf ON cf.id = vf.concept_id
                WHERE vf.document_id = d.id AND cf.canonical_key = ? AND vf.normalized_value = ?
              )",
        );
        values.push(Box::new(canonical_key(&filter.concept)));
        values.push(Box::new(normalize_spanish(&filter.equals)));
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
        confidence,
    }
}

fn ocr_is_reliable(status: &str, confidence: Option<f64>) -> bool {
    status != "failed" && confidence.is_none_or(|value| value >= 0.55)
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
