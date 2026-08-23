use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Instant, UNIX_EPOCH},
};

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{
    db::Database,
    error::{OmegaError, Result},
    extract::{classify_value, resembles_entity},
    model::{IndexReport, OcrStatus, ParsedDocument},
    normalize::{canonical_identifier, canonical_key, normalize_spanish},
    parser::{DocumentParser, SUPPORTED_EXTENSIONS},
};

pub struct Indexer<'a> {
    database: &'a Database,
    parser: &'a dyn DocumentParser,
}

impl<'a> Indexer<'a> {
    pub fn new(database: &'a Database, parser: &'a dyn DocumentParser) -> Self {
        Self { database, parser }
    }

    pub fn authorize(&self, path: &Path) -> Result<i64> {
        let canonical = path.canonicalize()?;
        if !canonical.is_dir() {
            return Err(OmegaError::InvalidArguments(format!(
                "{} no es una carpeta",
                canonical.display()
            )));
        }
        self.database.add_source(&canonical)
    }

    pub fn index_source(&self, source_id: i64) -> Result<IndexReport> {
        let started = Instant::now();
        let source = self
            .database
            .source_path(source_id)?
            .ok_or_else(|| OmegaError::UnauthorizedPath(source_id.to_string()))?;
        let files = discover(&source);

        let mut connection = self.database.connect()?;
        let transaction = connection.transaction()?;
        let existing_hashes = transaction
            .prepare("SELECT path, content_hash FROM documents WHERE source_id = ?1")?
            .query_map([source_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        // La purga y el nuevo índice viven en la misma transacción: sigue
        // siendo una operación de conjunto y un fallo conserva el índice sano anterior.
        transaction.execute(
            "DELETE FROM chunks_fts
             WHERE document_id IN (SELECT id FROM documents WHERE source_id = ?1)",
            [source_id],
        )?;
        transaction.execute("DELETE FROM documents WHERE source_id = ?1", [source_id])?;
        let mut report = IndexReport {
            source_id,
            discovered: files.len(),
            indexed: 0,
            modified: 0,
            skipped: 0,
            ocr_pending: 0,
            values: 0,
            warnings: vec![],
            elapsed_ms: 0,
        };

        for path in files {
            match self.parser.parse(&path) {
                Ok(document) => {
                    let canonical_path = match path.canonicalize() {
                        Ok(path) => path.to_string_lossy().to_string(),
                        Err(error) => {
                            report.skipped += 1;
                            report.warnings.push(format!("{}: {error}", path.display()));
                            continue;
                        }
                    };
                    let current_hash = match file_hash(&path) {
                        Ok(hash) => hash,
                        Err(error) => {
                            report.skipped += 1;
                            report.warnings.push(format!("{}: {error}", path.display()));
                            continue;
                        }
                    };
                    if existing_hashes
                        .get(&canonical_path)
                        .is_some_and(|previous| previous != &current_hash)
                    {
                        report.modified += 1;
                    }
                    if document.ocr_status == OcrStatus::Pending {
                        report.ocr_pending += 1;
                    }
                    report.values += insert_document(
                        &transaction,
                        source_id,
                        &path,
                        origin_folder(&source, &path),
                        document,
                    )?;
                    report.indexed += 1;
                }
                Err(OmegaError::Unsupported(message)) => {
                    report.skipped += 1;
                    report.warnings.push(message);
                }
                Err(error) => {
                    // Un archivo ilegible o corrupto no invalida el resto de
                    // una carpeta autorizada. Se conserva un aviso explícito
                    // y nunca se crea evidencia a partir de contenido ausente.
                    report.skipped += 1;
                    report.warnings.push(format!("{}: {error}", path.display()));
                }
            }
        }
        transaction.execute(
            "DELETE FROM concepts
             WHERE NOT EXISTS (SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id)",
            [],
        )?;
        transaction.execute(
            "UPDATE source_folders SET indexed_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [source_id],
        )?;
        transaction.commit()?;
        report.elapsed_ms = started.elapsed().as_millis();
        Ok(report)
    }
}

fn file_hash(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

fn discover(source: &Path) -> Vec<PathBuf> {
    let mut paths = WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|extension| SUPPORTED_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn insert_document(
    connection: &Connection,
    source_id: i64,
    path: &Path,
    origin: String,
    parsed: ParsedDocument,
) -> Result<usize> {
    let bytes = fs::read(path)?;
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let canonical = path.canonicalize()?;
    let title = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("Documento");
    let extension = canonical
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    connection.execute(
        "INSERT INTO documents(
            source_id, path, title, origin, extension, parser, content_hash,
            size_bytes, modified_unix, ocr_status, ocr_confidence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            source_id,
            canonical.to_string_lossy().as_ref(),
            title,
            origin,
            extension,
            parsed.parser,
            hash,
            metadata.len() as i64,
            modified,
            parsed.ocr_status.as_str(),
            parsed.ocr_confidence,
        ],
    )?;
    let document_id = connection.last_insert_rowid();

    let source_chunks = if parsed.chunks.is_empty() {
        chunks(&parsed.text)
    } else {
        parsed
            .chunks
            .into_iter()
            .map(|chunk| (chunk.location, chunk.content))
            .collect()
    };
    for (ordinal, (location, content)) in source_chunks.into_iter().enumerate() {
        connection.execute(
            "INSERT INTO chunks(document_id, ordinal, location, content) VALUES (?1, ?2, ?3, ?4)",
            params![document_id, ordinal as i64, location, content],
        )?;
        let chunk_id = connection.last_insert_rowid();
        connection.execute(
            "INSERT INTO chunks_fts(chunk_id, document_id, content) VALUES (?1, ?2, ?3)",
            params![chunk_id, document_id, content],
        )?;
    }

    let mut inserted = 0;
    for (ordinal, record) in parsed.records.into_iter().enumerate() {
        let typed = classify_value(&record.label, &record.value);
        let key = canonical_key(&record.label);
        connection.execute(
            "INSERT INTO concepts(canonical_key, display_name, value_type)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(canonical_key) DO NOTHING",
            params![key, record.label, typed.kind.as_str()],
        )?;
        let concept_id: i64 = connection.query_row(
            "SELECT id FROM concepts WHERE canonical_key = ?1",
            [&key],
            |row| row.get(0),
        )?;
        let alias = normalize_spanish(&record.label);
        connection.execute(
            "INSERT OR IGNORE INTO concept_aliases(
                concept_id, alias, normalized_alias, origin, status
             ) VALUES (?1, ?2, ?3, 'local_rule', 'system')",
            params![concept_id, record.label, alias],
        )?;
        let evidence_id = format!("v-{document_id}-{ordinal}");
        connection.execute(
            "INSERT INTO extracted_values(
                evidence_id, document_id, concept_id, value_type, text_value,
                normalized_value, identifier_canonical, numeric_value, currency, date_value, location, excerpt
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                evidence_id,
                document_id,
                concept_id,
                typed.kind.as_str(),
                typed.text_value,
                typed.normalized_value,
                canonical_identifier(&record.value),
                typed.numeric_value,
                typed.currency,
                typed.date_value,
                record.location,
                record.excerpt,
            ],
        )?;
        if resembles_entity(&record.value, &typed.kind) {
            let relationship = if normalize_spanish(&record.label).contains("propietari") {
                "owner"
            } else {
                "mention"
            };
            connection.execute(
                "INSERT INTO entities(
                    document_id, concept_id, name, normalized_name, role,
                    relationship_kind, evidence_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    document_id,
                    concept_id,
                    record.value,
                    normalize_spanish(&record.value),
                    key,
                    relationship,
                    evidence_id,
                ],
            )?;
        }
        inserted += 1;
    }
    Ok(inserted)
}

/// La procedencia es metadato del índice, no una categoría de negocio. Para
/// documentos en la raíz se usa el nombre de la fuente; para el resto, la
/// carpeta relativa completa, de modo que funciona con cualquier jerarquía.
fn origin_folder(source: &Path, path: &Path) -> String {
    let parent = path.parent().unwrap_or(source);
    let relative = parent.strip_prefix(source).unwrap_or(parent);
    let rendered = relative.to_string_lossy().trim_matches('/').to_owned();
    if rendered.is_empty() || rendered == "." {
        source
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("fuente")
            .to_owned()
    } else {
        rendered
    }
}

fn chunks(text: &str) -> Vec<(String, String)> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut start_line = 1;
    let mut last_line = 1;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if current.is_empty() {
            start_line = line_number;
        }
        if !current.is_empty() && current.len() + line.len() > 1400 {
            chunks.push((
                format!("líneas {start_line}-{last_line}"),
                current.trim().to_owned(),
            ));
            current.clear();
            start_line = line_number;
        }
        current.push_str(line);
        current.push('\n');
        last_line = line_number;
    }
    if !current.trim().is_empty() {
        chunks.push((
            format!("líneas {start_line}-{last_line}"),
            current.trim().to_owned(),
        ));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_locations_remain_navigable() {
        let result = chunks("uno\ndos\ntres");
        assert_eq!(result[0].0, "líneas 1-3");
    }
}
