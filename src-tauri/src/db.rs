use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::{
    error::Result,
    model::{AppStatus, SourceSummary},
};

#[derive(Debug, Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = Self {
            path: path.as_ref().to_path_buf(),
        };
        let connection = database.connect()?;
        migrate(&connection)?;
        Ok(database)
    }

    pub fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(connection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn add_source(&self, canonical_path: &Path) -> Result<i64> {
        let connection = self.connect()?;
        let path = canonical_path.to_string_lossy();
        connection.execute(
            "INSERT INTO source_folders(path) VALUES (?1)
             ON CONFLICT(path) DO UPDATE SET revoked_at = NULL",
            [path.as_ref()],
        )?;
        Ok(connection.query_row(
            "SELECT id FROM source_folders WHERE path = ?1",
            [path.as_ref()],
            |row| row.get(0),
        )?)
    }

    pub fn source_path(&self, source_id: i64) -> Result<Option<PathBuf>> {
        let connection = self.connect()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT path FROM source_folders WHERE id = ?1 AND revoked_at IS NULL",
                [source_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.map(PathBuf::from))
    }

    pub fn list_sources(&self) -> Result<Vec<SourceSummary>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT s.id, s.path, COUNT(d.id), s.indexed_at
             FROM source_folders s
             LEFT JOIN documents d ON d.source_id = s.id
             WHERE s.revoked_at IS NULL
             GROUP BY s.id
             ORDER BY s.created_at, s.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SourceSummary {
                id: row.get(0)?,
                path: row.get(1)?,
                document_count: row.get(2)?,
                indexed_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Purga todos los artefactos derivados con operaciones de conjunto. No
    /// ejecuta una sentencia por documento y no deja filas fantasma en FTS.
    pub fn purge_source(&self, source_id: i64, remove_authorization: bool) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM chunks_fts
             WHERE document_id IN (SELECT id FROM documents WHERE source_id = ?1)",
            [source_id],
        )?;
        transaction.execute("DELETE FROM documents WHERE source_id = ?1", [source_id])?;
        transaction.execute(
            "DELETE FROM concepts
             WHERE NOT EXISTS (
                SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id
             )",
            [],
        )?;
        if remove_authorization {
            transaction.execute("DELETE FROM source_folders WHERE id = ?1", [source_id])?;
        } else {
            transaction.execute(
                "UPDATE source_folders SET indexed_at = NULL WHERE id = ?1",
                [source_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_ai_enabled(&self, enabled: bool) -> Result<()> {
        let connection = self.connect()?;
        connection.execute(
            "INSERT INTO settings(key, value) VALUES ('ai_enabled', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [if enabled { "true" } else { "false" }],
        )?;
        Ok(())
    }

    pub fn ai_enabled(&self) -> Result<bool> {
        let connection = self.connect()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'ai_enabled'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.as_deref() == Some("true"))
    }

    pub fn status(&self, api_key_stored: bool) -> Result<AppStatus> {
        let connection = self.connect()?;
        let count = |table: &str| -> Result<i64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            Ok(connection.query_row(&sql, [], |row| row.get(0))?)
        };
        Ok(AppStatus {
            sources: connection.query_row(
                "SELECT COUNT(*) FROM source_folders WHERE revoked_at IS NULL",
                [],
                |row| row.get(0),
            )?,
            documents: count("documents")?,
            concepts: count("concepts")?,
            values: count("extracted_values")?,
            ai_enabled: self.ai_enabled()?,
            api_key_stored,
        })
    }

    pub fn is_authorized_document(&self, path: &Path) -> Result<bool> {
        let canonical = path.canonicalize()?;
        let connection = self.connect()?;
        let found: i64 = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM documents d
                JOIN source_folders s ON s.id = d.source_id
                WHERE d.path = ?1 AND s.revoked_at IS NULL
             )",
            [canonical.to_string_lossy().as_ref()],
            |row| row.get(0),
        )?;
        Ok(found == 1)
    }
}

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS source_folders (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            indexed_at TEXT,
            revoked_at TEXT
        );

        CREATE TABLE IF NOT EXISTS documents (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL REFERENCES source_folders(id) ON DELETE CASCADE,
            path TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            origin TEXT NOT NULL DEFAULT '',
            extension TEXT NOT NULL,
            parser TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            modified_unix INTEGER NOT NULL,
            ocr_status TEXT NOT NULL DEFAULT 'not_required'
                CHECK (ocr_status IN ('not_required', 'pending', 'complete', 'low_confidence', 'failed')),
            ocr_confidence REAL,
            indexed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source_id);

        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY,
            document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL,
            location TEXT NOT NULL,
            content TEXT NOT NULL,
            UNIQUE(document_id, ordinal)
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_document ON chunks(document_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            chunk_id UNINDEXED,
            document_id UNINDEXED,
            content,
            tokenize = 'unicode61 remove_diacritics 2'
        );

        CREATE TABLE IF NOT EXISTS concepts (
            id INTEGER PRIMARY KEY,
            canonical_key TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            value_type TEXT NOT NULL CHECK (
                value_type IN ('money', 'date', 'percentage', 'number', 'state', 'text')
            ),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS concept_aliases (
            id INTEGER PRIMARY KEY,
            concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            alias TEXT NOT NULL,
            normalized_alias TEXT NOT NULL,
            origin TEXT NOT NULL CHECK (origin IN ('local_rule', 'ai', 'user')),
            status TEXT NOT NULL CHECK (status IN ('system', 'suggested', 'confirmed', 'rejected')),
            UNIQUE(concept_id, normalized_alias, origin)
        );
        CREATE INDEX IF NOT EXISTS idx_alias_normalized ON concept_aliases(normalized_alias, status);

        CREATE TABLE IF NOT EXISTS extracted_values (
            id INTEGER PRIMARY KEY,
            evidence_id TEXT NOT NULL UNIQUE,
            document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE RESTRICT,
            value_type TEXT NOT NULL,
            text_value TEXT NOT NULL,
            normalized_value TEXT NOT NULL,
            identifier_canonical TEXT,
            numeric_value REAL,
            currency TEXT,
            date_value TEXT,
            location TEXT NOT NULL,
            excerpt TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_values_concept_numeric
            ON extracted_values(concept_id, numeric_value);
        CREATE INDEX IF NOT EXISTS idx_values_document ON extracted_values(document_id);
        CREATE INDEX IF NOT EXISTS idx_values_normalized
            ON extracted_values(concept_id, normalized_value);
        CREATE INDEX IF NOT EXISTS idx_values_date ON extracted_values(date_value);

        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY,
            document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE RESTRICT,
            name TEXT NOT NULL,
            normalized_name TEXT NOT NULL,
            role TEXT NOT NULL,
            relationship_kind TEXT NOT NULL CHECK (relationship_kind IN ('owner', 'mention')),
            evidence_id TEXT NOT NULL REFERENCES extracted_values(evidence_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(normalized_name);
        CREATE INDEX IF NOT EXISTS idx_entities_document ON entities(document_id);

        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO settings(key, value) VALUES ('ai_enabled', 'false');
        "#,
    )?;
    // SQLite no admite ADD COLUMN IF NOT EXISTS. Este esquema se distribuyó
    // antes de que `origin` existiera, por lo que la migración debe conservar
    // los índices ya creados por usuarios existentes.
    let has_origin = connection
        .prepare("PRAGMA table_info(documents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "origin");
    if !has_origin {
        connection.execute(
            "ALTER TABLE documents ADD COLUMN origin TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_documents_origin ON documents(origin)",
        [],
    )?;
    let columns = connection
        .prepare("PRAGMA table_info(documents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == "ocr_confidence") {
        connection.execute("ALTER TABLE documents ADD COLUMN ocr_confidence REAL", [])?;
    }
    let value_columns = connection
        .prepare("PRAGMA table_info(extracted_values)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !value_columns
        .iter()
        .any(|name| name == "identifier_canonical")
    {
        connection.execute(
            "ALTER TABLE extracted_values ADD COLUMN identifier_canonical TEXT",
            [],
        )?;
    }
    connection.execute(
        "CREATE INDEX IF NOT EXISTS idx_values_identifier_canonical
         ON extracted_values(identifier_canonical)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_starts_disabled() {
        let temporary = tempfile::NamedTempFile::new().unwrap();
        let database = Database::open(temporary.path()).unwrap();
        assert!(!database.ai_enabled().unwrap());
    }
}
