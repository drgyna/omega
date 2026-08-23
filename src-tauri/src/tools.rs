use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
        search_terms,
    },
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FieldValuePair {
    field: String,
    value: String,
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
                        "currency": { "type": ["string", "null"] },
                        "date_from": { "type": ["string", "null"] },
                        "date_to": { "type": ["string", "null"] },
                        "group_by": { "type": ["string", "null"] }
                    },
                    "required": ["concept", "operation", "filters", "currency", "date_from", "date_to", "group_by"],
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
    /// basa exclusivamente en los nombres de concepto ya extraídos y en la
    /// posición del texto que sigue al campo; no contiene vocabulario de un
    /// rubro de negocio.
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
            "SELECT DISTINCT display_name FROM concepts ORDER BY length(display_name) DESC",
        )?;
        let fields = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for field in fields {
            let normalized_field = normalize_exact(&field);
            let Some(start) = phrase_position(&normalized_query, &normalized_field) else {
                continue;
            };
            let after = &normalized_query[start + normalized_field.len()..];
            if after
                .split_whitespace()
                .any(|word| !QUERY_FILLER.contains(&word))
            {
                return Ok(true);
            }
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
                    .all(|term| terms.iter().any(|query_term| query_term == term));
            let field_overlap = field_terms
                .iter()
                .any(|term| terms.iter().any(|query_term| query_term == term));
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

    pub fn indexed_document_count(&self) -> Result<i64> {
        let connection = self.database.connect()?;
        Ok(connection.query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))?)
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
        let mut grouped: BTreeMap<Option<String>, AggregateRow> = BTreeMap::new();
        for (document_id, numeric, _text, evidence) in matches {
            let groups = if let Some(group_id) = group_concept {
                let mut group_statement = connection.prepare(
                    "SELECT DISTINCT text_value FROM extracted_values WHERE document_id = ?1 AND concept_id = ?2",
                )?;
                let found = group_statement
                    .query_map(params![document_id, group_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                if found.is_empty() {
                    vec![Some("Sin valor".into())]
                } else {
                    found.into_iter().map(Some).collect()
                }
            } else {
                vec![None]
            };
            for group in groups {
                let row = grouped
                    .entry(group.clone())
                    .or_insert_with(|| AggregateRow {
                        group,
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
}

fn calculation_evidence(request: &AggregateRequest, rows: &[AggregateRow]) -> Option<Evidence> {
    let first = rows.iter().flat_map(|row| row.evidence.iter()).next()?;
    let total_matches = rows.iter().map(|row| row.matched_values).sum::<i64>();
    let rendered = if rows.len() == 1 {
        format_number(rows[0].value)
    } else {
        rows.iter()
            .map(|row| {
                format!(
                    "{}: {}",
                    row.group.as_deref().unwrap_or("Total"),
                    format_number(row.value)
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let currency = request
        .currency
        .as_deref()
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
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
            "Omega ejecutó {} para el concepto '{}' y obtuvo {}{} a partir de {total_matches} valores con evidencia.",
            if request.operation == "sum" {
                "una suma"
            } else {
                "un conteo"
            },
            request.concept,
            rendered,
            currency,
        ),
        normalized_value: None,
        value: Some(format!("{rendered}{currency}")),
        matched: Some(format!("{rendered}{currency}")),
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
            let normalized = normalize_spanish(line);
            let score = terms
                .iter()
                .filter(|term| normalized.split_whitespace().any(|word| word == *term))
                .count();
            (score, offset, line.trim())
        })
        .max_by_key(|(score, _, _)| *score)
        .unwrap_or((0, 0, content.trim()));
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
        excerpt: brief_excerpt(best.2, terms.first().map(String::as_str)),
        normalized_value: None,
        value: None,
        matched: terms.first().cloned(),
        field: None,
        match_kind: "texto".into(),
        reliable: ocr_is_reliable(&ocr_status, confidence),
        confidence,
    }
}

fn ocr_is_reliable(status: &str, confidence: Option<f64>) -> bool {
    status != "failed" && confidence.is_none_or(|value| value >= 0.55)
}

/// Mantiene una sola unidad de evidencia en un máximo de 220 caracteres. Si
/// la coincidencia está en medio, conserva contexto a ambos lados sin añadir
/// texto que no proceda del archivo.
fn brief_excerpt(value: &str, matched: Option<&str>) -> String {
    const MAX: usize = 220;
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
