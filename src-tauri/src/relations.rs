//! Relaciones entre documentos, contradicciones y expedientes.
//!
//! Una relación sólo existe cuando dos documentos comparten una **clave
//! estable**: el valor canónico de un identificador, que `normalize` sólo
//! produce cuando el valor mezcla letras y dígitos. Dos razones sociales
//! parecidas, dos nombres de persona casi iguales o dos títulos similares no
//! generan clave y por tanto nunca pueden vincularse aquí. El módulo no conoce
//! ningún tipo de documento: no sabe qué es una factura, un contrato ni una
//! póliza; sabe que dos archivos escriben el mismo identificador.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, params};

use crate::{
    error::Result,
    model::Evidence,
    normalize::{canonical_identifier, normalize_exact, search_terms},
    tools::ToolEngine,
};

/// Longitud mínima de una clave canónica. Descarta restos como «a1» que
/// coincidirían por casualidad.
const MIN_KEY_LENGTH: usize = 5;

/// Cuántos documentos puede abarcar una clave antes de dejar de ser un
/// expediente. Un valor compartido por medio acervo no identifica a nadie.
const MAX_LINKED_DOCUMENTS: usize = 50;

/// Raíces de nombres de campo con semántica de identificador. Son palabras de
/// gestión documental —no de ningún giro— y sólo se usan para decidir si un
/// valor con espacios puede ser una clave.
const IDENTIFIER_FIELD_ROOTS: &[&str] = &[
    "folio",
    "expedient",
    "contrat",
    "poliz",
    "codig",
    "clave",
    "identificador",
    "referenc",
    "matricul",
    "serie",
    "registr",
    "numero",
    "num",
    "caso",
    "tramit",
    "orden",
    "guia",
    "control",
    "id",
];

/// ¿El nombre del campo anuncia un identificador?
pub fn field_names_an_identifier(field: &str) -> bool {
    search_terms(field).iter().any(|term| {
        IDENTIFIER_FIELD_ROOTS
            .iter()
            .any(|root| term.starts_with(root))
    })
}

/// Clave estable con la que dos documentos pueden vincularse.
///
/// Las reglas son deliberadamente estrictas y genéricas, porque el coste de
/// equivocarse es inventar una relación:
///
/// - Un importe, una fecha, un porcentaje o un número suelto nunca identifican
///   a nadie, por mucho que dos documentos compartan la cifra.
/// - Un identificador no lleva espacios, salvo que el propio campo se llame
///   como un identificador; así «10 pasajeros» en «Capacidad autorizada» queda
///   fuera, mientras que «Folio: SEG 26 0024» sigue dentro.
/// - El valor debe mezclar letras y dígitos y tener cuerpo suficiente: un
///   nombre de ciudad, de persona o de producto no produce clave.
pub fn stable_key(field: &str, value: &str, value_type: &str) -> Option<String> {
    if matches!(value_type, "money" | "date" | "percentage" | "number") {
        return None;
    }
    let trimmed = value.trim();
    if trimmed.split_whitespace().count() > 1 && !field_names_an_identifier(field) {
        return None;
    }
    canonical_identifier(trimmed).filter(|canonical| canonical.len() >= MIN_KEY_LENGTH)
}

/// Documento vinculado, con el campo que creó el vínculo.
#[derive(Clone, Debug)]
pub struct LinkedDocument {
    pub document_id: i64,
    pub path: String,
    /// Campo cuyo valor produjo la clave compartida.
    pub field: String,
    pub value: String,
    pub evidence: Evidence,
}

/// Conjunto de documentos que comparten una clave.
#[derive(Clone, Debug)]
pub struct RelationGroup {
    pub canonical: String,
    /// El identificador tal como está escrito en el acervo.
    pub display: String,
    pub documents: Vec<LinkedDocument>,
}

impl RelationGroup {
    pub fn document_ids(&self) -> Vec<i64> {
        self.documents
            .iter()
            .map(|document| document.document_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Campos que crearon el vínculo, sin repetir.
    pub fn linking_fields(&self) -> Vec<String> {
        let mut fields = self
            .documents
            .iter()
            .map(|document| document.field.clone())
            .collect::<Vec<_>>();
        fields.dedup();
        fields.sort();
        fields.dedup();
        fields
    }
}

/// Un valor concreto de un campo dentro de un documento.
#[derive(Clone, Debug)]
pub struct FieldValue {
    pub document_id: i64,
    pub path: String,
    pub value: String,
    pub normalized: String,
    pub evidence: Evidence,
}

/// Campo que dos o más documentos vinculados escriben de forma distinta.
#[derive(Clone, Debug)]
pub struct Contradiction {
    /// El identificador tal como está escrito en los documentos.
    pub display: String,
    /// Campos que crearon el vínculo entre los documentos comparados.
    pub linking_fields: Vec<String>,
    /// Campo cuyos valores no coinciden.
    pub concept: String,
    pub entries: Vec<FieldValue>,
}

/// Ficha extractiva de un identificador: campos, conflictos y ausencias.
#[derive(Clone, Debug)]
pub struct Dossier {
    pub group: RelationGroup,
    pub fields: Vec<DossierField>,
    /// Campos que unos documentos del expediente declaran y otros no.
    pub missing: Vec<DossierGap>,
}

#[derive(Clone, Debug)]
pub struct DossierField {
    pub concept: String,
    pub values: Vec<FieldValue>,
    /// Dos documentos vinculados escriben valores distintos del mismo campo.
    pub conflicting: bool,
}

#[derive(Clone, Debug)]
pub struct DossierGap {
    pub concept: String,
    /// Rutas de los documentos del expediente que no declaran ese campo.
    pub absent_in: Vec<String>,
}

const MAX_SCANNED_GROUPS: usize = 400;
const MAX_REPORTED_CONTRADICTIONS: usize = 20;

/// Identificadores canónicos que existen en el índice y coinciden con el texto.
///
/// Devuelve más de uno sólo cuando la coincidencia es realmente ambigua; el
/// motor debe entonces preguntar en vez de elegir.
pub fn identifier_candidates(tools: &ToolEngine, text: &str) -> Result<Vec<String>> {
    let connection = tools.database().connect()?;
    let mut exact = Vec::new();
    for token in tokens_of(text) {
        let Some(canonical) = canonical_identifier(&token) else {
            continue;
        };
        let found: Option<String> = connection
            .query_row(
                "SELECT identifier_canonical FROM extracted_values
                 WHERE identifier_canonical = ?1 LIMIT 1",
                [&canonical],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(value) = found {
            exact.push(value);
            continue;
        }
        // Sin coincidencia exacta, un prefijo puede identificar al expediente,
        // pero sólo si es inequívoco. Varias coincidencias son una ambigüedad
        // que el motor debe exponer, no resolver.
        let mut statement = connection.prepare(
            "SELECT DISTINCT identifier_canonical FROM extracted_values
             WHERE identifier_canonical LIKE ?1 ORDER BY identifier_canonical LIMIT 6",
        )?;
        let prefixes = statement
            .query_map([format!("{canonical}%")], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        exact.extend(prefixes);
    }
    exact.sort();
    exact.dedup();
    Ok(exact)
}

fn tokens_of(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                !character.is_alphanumeric() && !matches!(character, '-' | '_' | '/' | '.')
            })
            .to_owned()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Documentos que comparten una clave canónica, con la cita de cada uno.
pub fn documents_for(tools: &ToolEngine, canonical: &str) -> Result<Option<RelationGroup>> {
    let connection = tools.database().connect()?;
    let mut statement = connection.prepare(
        "SELECT v.document_id, d.path, d.origin, d.title, c.display_name, v.text_value,
                v.location, v.excerpt, v.evidence_id, v.value_type
         FROM extracted_values v
         JOIN documents d ON d.id = v.document_id
         JOIN concepts c ON c.id = v.concept_id
         WHERE v.identifier_canonical = ?1
         ORDER BY v.document_id, v.id",
    )?;
    let rows = statement
        .query_map([canonical], |row| {
            let value_type: String = row.get(9)?;
            let document_id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let origin: String = row.get(2)?;
            let field: String = row.get(4)?;
            let value: String = row.get(5)?;
            Ok((
                stable_key(&field, &value, &value_type),
                LinkedDocument {
                document_id,
                path: path.clone(),
                field: field.clone(),
                value: value.clone(),
                evidence: Evidence {
                    id: row.get(8)?,
                    document_id,
                    path,
                    origin,
                    location: row.get(6)?,
                    excerpt: row.get(7)?,
                    normalized_value: Some(canonical.to_owned()),
                    value: Some(value.clone()),
                    matched: Some(value),
                    field: Some(field),
                    match_kind: "canónica".into(),
                    reliable: true,
                    confidence: None,
                },
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Sólo sobreviven las filas cuya combinación de campo y valor produce una
    // clave estable con esta misma canónica. Una coincidencia que venga de una
    // capacidad, un importe o un texto descriptivo no vincula nada.
    let rows = rows
        .into_iter()
        .filter(|(key, _)| key.as_deref() == Some(canonical))
        .map(|(_, document)| document)
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(None);
    }
    // Un documento puede escribir el identificador en varios campos; para la
    // lista de documentos vinculados basta la primera aparición de cada uno.
    let mut seen = BTreeSet::new();
    let display = rows[0].value.clone();
    let documents = rows
        .into_iter()
        .filter(|document| seen.insert(document.document_id))
        .collect::<Vec<_>>();
    // Un valor compartido por medio acervo no es un expediente. Si además
    // ningún campo del vínculo se llama como un identificador, la coincidencia
    // es de vocabulario, no de identidad.
    if documents.len() > MAX_LINKED_DOCUMENTS
        || (documents.len() > 5
            && !documents
                .iter()
                .any(|document| field_names_an_identifier(&document.field)))
    {
        return Ok(None);
    }
    Ok(Some(RelationGroup {
        canonical: canonical.to_owned(),
        display,
        documents,
    }))
}

/// Valores por campo de un conjunto de documentos, con su evidencia.
fn field_values(tools: &ToolEngine, documents: &[i64]) -> Result<BTreeMap<String, Vec<FieldValue>>> {
    if documents.is_empty() {
        return Ok(BTreeMap::new());
    }
    let connection = tools.database().connect()?;
    let placeholders = vec!["?"; documents.len()].join(",");
    let sql = format!(
        "SELECT c.display_name, v.document_id, d.path, v.text_value, v.normalized_value,
                v.location, v.excerpt, v.evidence_id, d.origin
         FROM extracted_values v
         JOIN documents d ON d.id = v.document_id
         JOIN concepts c ON c.id = v.concept_id
         WHERE v.document_id IN ({placeholders})
         ORDER BY c.display_name, v.document_id, v.id"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(documents.iter()), |row| {
            let concept: String = row.get(0)?;
            let document_id: i64 = row.get(1)?;
            let path: String = row.get(2)?;
            let value: String = row.get(3)?;
            Ok((
                concept.clone(),
                FieldValue {
                    document_id,
                    path: path.clone(),
                    value: value.clone(),
                    normalized: row.get(4)?,
                    evidence: Evidence {
                        id: row.get(7)?,
                        document_id,
                        path,
                        origin: row.get(8)?,
                        location: row.get(5)?,
                        excerpt: row.get(6)?,
                        normalized_value: Some(normalize_exact(&value)),
                        value: Some(value.clone()),
                        matched: Some(value),
                        field: Some(concept),
                        match_kind: "campo".into(),
                        reliable: true,
                        confidence: None,
                    },
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: BTreeMap<String, Vec<FieldValue>> = BTreeMap::new();
    for (concept, value) in rows {
        grouped.entry(concept).or_default().push(value);
    }
    Ok(grouped)
}

/// Contradicciones dentro de un grupo ya vinculado.
///
/// Un campo entra en conflicto sólo si cada documento comparado declara **un
/// solo** valor para él y esos valores difieren. Un documento con varios
/// valores del mismo campo (una tabla con muchas filas) no es una
/// contradicción: es un documento con varios registros, y confundir ambos
/// llenaría el informe de falsos positivos.
pub fn contradictions_in(tools: &ToolEngine, group: &RelationGroup) -> Result<Vec<Contradiction>> {
    let ids = group.document_ids();
    if ids.len() < 2 {
        return Ok(vec![]);
    }
    let values = field_values(tools, &ids)?;
    let linking = group.linking_fields();
    let mut contradictions = Vec::new();
    for (concept, entries) in values {
        // El campo que creó el vínculo no puede contradecirse consigo mismo.
        if entries
            .iter()
            .all(|entry| canonical_identifier(&entry.value).as_deref() == Some(&group.canonical))
        {
            continue;
        }
        let mut by_document: BTreeMap<i64, Vec<&FieldValue>> = BTreeMap::new();
        for entry in &entries {
            by_document.entry(entry.document_id).or_default().push(entry);
        }
        let single: Vec<&FieldValue> = by_document
            .values()
            .filter_map(|values| {
                let distinct = values
                    .iter()
                    .map(|value| value.normalized.as_str())
                    .collect::<BTreeSet<_>>();
                (distinct.len() == 1).then(|| values[0])
            })
            .collect();
        if single.len() < 2 {
            continue;
        }
        let distinct = single
            .iter()
            .map(|value| value.normalized.as_str())
            .collect::<BTreeSet<_>>();
        if distinct.len() < 2 {
            continue;
        }
        contradictions.push(Contradiction {
            display: group.display.clone(),
            linking_fields: linking.clone(),
            concept,
            entries: single.into_iter().cloned().collect(),
        });
    }
    Ok(contradictions)
}

/// Recorre el acervo buscando claves compartidas con valores incompatibles.
pub fn contradictions(
    tools: &ToolEngine,
    key_concept: Option<&str>,
    compared_concept: Option<&str>,
) -> Result<Vec<Contradiction>> {
    let connection = tools.database().connect()?;
    let mut sql = String::from(
        "SELECT v.identifier_canonical
         FROM extracted_values v
         JOIN concepts c ON c.id = v.concept_id
         WHERE v.identifier_canonical IS NOT NULL",
    );
    if key_concept.is_some() {
        sql.push_str(" AND c.canonical_key = ?2");
    }
    sql.push_str(
        " GROUP BY v.identifier_canonical
          HAVING COUNT(DISTINCT v.document_id) >= 2
          ORDER BY v.identifier_canonical
          LIMIT ?1",
    );
    let mut statement = connection.prepare(&sql)?;
    let canonicals: Vec<String> = match key_concept {
        Some(concept) => statement
            .query_map(
                params![
                    MAX_SCANNED_GROUPS as i64,
                    crate::normalize::canonical_key(concept)
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        None => statement
            .query_map([MAX_SCANNED_GROUPS as i64], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };
    drop(statement);
    drop(connection);

    let mut found = Vec::new();
    for canonical in canonicals {
        let Some(group) = documents_for(tools, &canonical)? else {
            continue;
        };
        found.extend(
            contradictions_in(tools, &group)?
                .into_iter()
                .filter(|item| {
                    compared_concept.is_none_or(|concept| {
                        crate::normalize::canonical_key(&item.concept)
                            == crate::normalize::canonical_key(concept)
                    })
                }),
        );
        if found.len() >= MAX_REPORTED_CONTRADICTIONS {
            break;
        }
    }
    found.truncate(MAX_REPORTED_CONTRADICTIONS);
    Ok(found)
}

/// Reúne la ficha de un identificador: campos con evidencia, conflictos y los
/// campos que faltan en unos documentos y existen en otros.
pub fn dossier(tools: &ToolEngine, canonical: &str) -> Result<Option<Dossier>> {
    let Some(group) = documents_for(tools, canonical)? else {
        return Ok(None);
    };
    let ids = group.document_ids();
    let values = field_values(tools, &ids)?;
    let mut fields = Vec::new();
    let mut missing = Vec::new();
    for (concept, entries) in values {
        let distinct = entries
            .iter()
            .map(|entry| entry.normalized.as_str())
            .collect::<BTreeSet<_>>();
        let documents_with = entries
            .iter()
            .map(|entry| entry.document_id)
            .collect::<BTreeSet<_>>();
        let conflicting = distinct.len() > 1
            && entries
                .iter()
                .map(|entry| entry.document_id)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
            && documents_with.iter().all(|document| {
                entries
                    .iter()
                    .filter(|entry| entry.document_id == *document)
                    .map(|entry| entry.normalized.as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 1
            });
        if ids.len() > 1 && documents_with.len() < ids.len() {
            let absent = group
                .documents
                .iter()
                .filter(|document| !documents_with.contains(&document.document_id))
                .map(|document| document.path.clone())
                .collect::<Vec<_>>();
            missing.push(DossierGap {
                concept: concept.clone(),
                absent_in: absent,
            });
        }
        fields.push(DossierField {
            concept,
            values: entries,
            conflicting,
        });
    }
    Ok(Some(Dossier {
        group,
        fields,
        missing,
    }))
}

/// Documentos que mencionan un texto sin clave estable. Se usa para explicar
/// por qué no se puede afirmar una relación: hay menciones, no vínculo.
pub fn mentions_without_key(tools: &ToolEngine, text: &str) -> Result<Vec<FieldValue>> {
    let connection = tools.database().connect()?;
    let mut statement = connection.prepare(
        "SELECT v.document_id, d.path, v.text_value, v.normalized_value, v.location,
                v.excerpt, v.evidence_id, d.origin, c.display_name
         FROM extracted_values v
         JOIN documents d ON d.id = v.document_id
         JOIN concepts c ON c.id = v.concept_id
         WHERE v.normalized_value = ?1
         ORDER BY v.document_id, v.id
         LIMIT 20",
    )?;
    let normalized = crate::normalize::normalize_spanish(text);
    let rows = statement
        .query_map(params![normalized], |row| {
            let document_id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let value: String = row.get(2)?;
            let concept: String = row.get(8)?;
            Ok(FieldValue {
                document_id,
                path: path.clone(),
                value: value.clone(),
                normalized: row.get(3)?,
                evidence: Evidence {
                    id: row.get(6)?,
                    document_id,
                    path,
                    origin: row.get(7)?,
                    location: row.get(4)?,
                    excerpt: row.get(5)?,
                    normalized_value: Some(normalize_exact(&value)),
                    value: Some(value.clone()),
                    matched: Some(value),
                    field: Some(concept),
                    match_kind: "campo".into(),
                    reliable: true,
                    confidence: None,
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // La consulta usa la normalización con raíces, que une singular y plural
    // («Álamo» y «Álamos» comparten raíz). Aquí se exige igualdad literal: dos
    // razones sociales que sólo se parecen no son el mismo valor.
    let literal = normalize_exact(text);
    Ok(rows
        .into_iter()
        .filter(|mention| normalize_exact(&mention.value) == literal)
        .collect())
}
