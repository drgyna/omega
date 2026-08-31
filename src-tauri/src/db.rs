use std::{fs, path::{Path, PathBuf}};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    error::Result,
    model::{AppStatus, SourceSummary},
    normalize::normalize_literal,
    recovery::{
        BackupPolicy, RecoveryReport, backup_policy, create_atomic_backup, integrity_is_valid,
        persist_recovery_notice, recover_database,
    },
};

#[derive(Debug, Clone)]
pub struct Database {
    path: PathBuf,
    recovery: Option<RecoveryReport>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_internal(path.as_ref(), false)
    }

    /// Apertura usada por la aplicación instalada. A diferencia de `open`,
    /// que conserva el contrato estricto de las evaluaciones, ésta recupera
    /// una SQLite dañada y devuelve un reporte explícito del resultado.
    pub fn open_recovering(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_internal(path.as_ref(), true)
    }

    fn open_internal(path: &Path, recovery_enabled: bool) -> Result<Self> {
        let mut database = Self {
            path: path.to_path_buf(),
            recovery: None,
        };
        let existed_with_bytes = fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0);
        let initial = database.connect();
        let mut connection = match initial {
            Ok(connection) => match integrity_is_valid(&connection) {
                Ok(true) => connection,
                Ok(false) | Err(_) if existed_with_bytes && recovery_enabled => {
                    drop(connection);
                    database.recovery = Some(recover_database(path)?);
                    database.connect()?
                }
                Ok(false) => {
                    return Err(crate::error::OmegaError::InvalidArguments(format!(
                        "{} no supera PRAGMA integrity_check; la apertura estricta no modificó el archivo",
                        path.display()
                    )));
                }
                Err(error) => return Err(error),
            },
            Err(_) if existed_with_bytes && recovery_enabled => {
                database.recovery = Some(recover_database(path)?);
                database.connect()?
            }
            Err(error) => return Err(error),
        };
        if !integrity_is_valid(&connection)? {
            return Err(crate::error::OmegaError::InvalidArguments(format!(
                "{} sigue sin integridad después de la recuperación",
                path.display()
            )));
        }
        if migration_requires_backup(&connection)? {
            create_atomic_backup(&connection, path)?;
        }
        migrate(&mut connection)?;
        if !integrity_is_valid(&connection)? {
            return Err(crate::error::OmegaError::InvalidArguments(
                "la migración terminó sin superar PRAGMA integrity_check".into(),
            ));
        }
        if let Some(report) = &database.recovery {
            persist_recovery_notice(report)?;
            eprintln!("Recuperación SQLite: {}", report.message);
        }
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

    pub fn recovery_report(&self) -> Option<&RecoveryReport> {
        self.recovery.as_ref()
    }

    pub fn backup_policy(path: impl AsRef<Path>) -> BackupPolicy {
        backup_policy(path.as_ref())
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
        retype_concepts(&transaction)?;
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

    pub fn status(&self) -> Result<AppStatus> {
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

fn migration_requires_backup(connection: &Connection) -> Result<bool> {
    let user_tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if user_tables == 0 {
        return Ok(false);
    }
    let has_column = |table: &str, column: &str| -> Result<bool> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        Ok(statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|name| name == column))
    };
    let document_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'documents'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(!has_column("documents", "origin")?
        || !has_column("documents", "ocr_confidence")?
        || !has_column("extracted_values", "literal_value")?
        || !has_column("extracted_values", "identifier_canonical")?
        || !document_sql.is_some_and(|sql| sql.contains("'unavailable'")))
}

/// Recalcula el tipo de cada concepto a partir de los valores que el acervo
/// contiene en este momento.
///
/// Antes el tipo lo fijaba el primer registro que llegara al índice y no se
/// volvía a tocar, así que `Importe: N/D` en el archivo alfabéticamente
/// primero convertía el campo entero en texto —y con él dejaba de poder
/// sumarse— aunque el resto del acervo trajera importes reales. El tipo tiene
/// que describir el acervo, no el orden en que se leyeron sus archivos.
///
/// La regla es la mayoría, y sólo se rompe el empate con una precedencia fija
/// que prefiere el tipo más específico. Así un marcador de ausencia suelto no
/// destipifica un campo de dinero, y un único valor que parece número tampoco
/// convierte en numérico un campo que de verdad es texto. El resultado no
/// depende del orden de inserción: es una función del contenido.
pub(crate) fn retype_concepts(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE concepts SET value_type = (
             SELECT v.value_type
             FROM extracted_values v
             WHERE v.concept_id = concepts.id
             GROUP BY v.value_type
             ORDER BY COUNT(*) DESC,
                      CASE v.value_type
                          WHEN 'money' THEN 0
                          WHEN 'date' THEN 1
                          WHEN 'percentage' THEN 2
                          WHEN 'number' THEN 3
                          WHEN 'state' THEN 4
                          ELSE 5
                      END
             LIMIT 1
         )
         WHERE EXISTS (
             SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id
         )",
        [],
    )?;
    Ok(())
}

/// Todo el esquema y las migraciones de datos (columnas nuevas, índices,
/// backfill de `literal_value`) corren dentro de una única transacción SQLite:
/// o se aplican por completo, o no se aplica ninguna. Si algún paso falla —
/// disco lleno, proceso interrumpido — la transacción nunca se confirma y
/// `Connection::transaction` la deshace por completo al salir de alcance
/// (`Transaction::drop` hace ROLLBACK si no se llamó a `commit`), así que la
/// base queda exactamente como estaba antes de intentar abrirla. Sigue siendo
/// idempotente: cada paso conserva sus guardas (`IF NOT EXISTS`, comprobación
/// de columna) y puede volver a ejecutarse sin duplicar nada.
fn migrate(connection: &mut Connection) -> Result<()> {
    run_migration(connection, None)?;
    // Va después: reconstruir `documents` necesita que las columnas que la
    // migración anterior añade (`origin`, `ocr_confidence`) ya existan.
    widen_ocr_status_check(connection)
}

/// Estados OCR admitidos por el índice. Es la misma lista que conoce
/// `OcrStatus`: si el motor puede producir un estado, el esquema tiene que
/// poder guardarlo, o el estado acabaría degradándose a otro que miente.
const OCR_STATUS_VALUES: &str =
    "'not_required', 'pending', 'complete', 'low_confidence', 'failed', 'unavailable'";

/// Ensancha el `CHECK` de `documents.ocr_status` en bases creadas antes de que
/// existiera el estado «no hay motor OCR».
///
/// SQLite no permite alterar una restricción en su sitio, así que se aplica el
/// procedimiento que la propia documentación de SQLite recomienda: crear la
/// tabla nueva, copiar, borrar la vieja y renombrar. Se hace con
/// `foreign_keys = OFF` —fuera de cualquier transacción, que es donde ese
/// PRAGMA surte efecto— para que borrar la tabla antigua no arrastre en
/// cascada los fragmentos, valores y entidades que la referencian. La
/// comprobación de integridad corre antes de confirmar: si algo quedara
/// colgando, la transacción no se confirma y la base sigue como estaba.
///
/// Idempotente: si el `CHECK` ya admite el estado, no toca nada.
fn widen_ocr_status_check(connection: &mut Connection) -> Result<()> {
    let definition: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'documents'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(definition) = definition else {
        return Ok(());
    };
    if definition.contains("'unavailable'") {
        return Ok(());
    }
    connection.execute_batch("PRAGMA foreign_keys = OFF")?;
    let rebuilt = rebuild_documents_table(connection);
    // El PRAGMA se restaura pase lo que pase: la conexión sigue viva y no
    // puede quedarse sin integridad referencial por un fallo de migración.
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    rebuilt
}

fn rebuild_documents_table(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction()?;
    tx.execute_batch(&format!(
        r#"
        CREATE TABLE documents_migrated (
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
                CHECK (ocr_status IN ({OCR_STATUS_VALUES})),
            ocr_confidence REAL,
            -- Qué resultó ser el contenido cuando no coincide con la
            -- extensión declarada («texto plano», «un PDF»). NULL es el caso
            -- normal. Se guarda para que cualquier respuesta que cite el
            -- documento pueda declarar la discrepancia, no sólo el reporte de
            -- indexación del día en que se indexó.
            declared_format_mismatch TEXT,
            indexed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO documents_migrated (
            id, source_id, path, title, origin, extension, parser, content_hash,
            size_bytes, modified_unix, ocr_status, ocr_confidence, indexed_at
        )
        SELECT id, source_id, path, title, origin, extension, parser, content_hash,
               size_bytes, modified_unix, ocr_status, ocr_confidence, indexed_at
        FROM documents;
        DROP TABLE documents;
        ALTER TABLE documents_migrated RENAME TO documents;
        CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source_id);
        CREATE INDEX IF NOT EXISTS idx_documents_origin ON documents(origin);
        "#
    ))?;
    let dangling: i64 = tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
        row.get(0)
    })?;
    if dangling > 0 {
        return Err(crate::error::OmegaError::InvalidArguments(format!(
            "la migración de ocr_status dejaría {dangling} referencias colgando"
        )));
    }
    tx.commit()?;
    Ok(())
}

/// Puntos en los que las pruebas de este módulo pueden forzar un error
/// *dentro* de la transacción de migración, para comprobar que un fallo a
/// mitad de camino —no sólo uno justo antes de `COMMIT`— deshace todo lo que
/// ya corrió. El tipo existe siempre (lo referencia `run_migration`, que es
/// código de producción); sólo el constructor que lo expone a las pruebas
/// (`migrate_with_fault`, más abajo) está detrás de `#[cfg(test)]`, así que
/// fuera de pruebas el valor siempre es `None` y ningún branch de fallo se
/// activa nunca.
// Fuera de pruebas nada construye estas variantes (sólo lo hace
// `migrate_with_fault`, que es `#[cfg(test)]`), así que el build normal las
// ve como código muerto aunque `run_migration` sí las compara.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum MigrationFault {
    /// Falla justo después de `ALTER TABLE ... ADD COLUMN literal_value`,
    /// antes de crear su índice y antes de que el backfill escriba nada.
    AfterLiteralValueColumnAdded,
    /// Falla a mitad del backfill, después de actualizar exactamente
    /// `rows_updated` de las filas pendientes, dejando el resto sin tocar
    /// dentro de la misma transacción todavía sin confirmar.
    DuringBackfill { rows_updated: usize },
}

/// Variante usada sólo por las pruebas de este módulo para inyectar uno de
/// los fallos de `MigrationFault` dentro de la transacción real.
#[cfg(test)]
pub(crate) fn migrate_with_fault(connection: &mut Connection, fault: MigrationFault) -> Result<()> {
    run_migration(connection, Some(fault))
}

fn forced_test_failure() -> crate::error::OmegaError {
    crate::error::OmegaError::InvalidArguments(
        "fallo forzado de prueba dentro de la migración".into(),
    )
}

fn run_migration(connection: &mut Connection, fault: Option<MigrationFault>) -> Result<()> {
    let tx = connection.transaction()?;
    tx.execute_batch(
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
                CHECK (ocr_status IN (
                    'not_required', 'pending', 'complete',
                    'low_confidence', 'failed', 'unavailable'
                )),
            ocr_confidence REAL,
            -- Qué resultó ser el contenido cuando no coincide con la
            -- extensión declarada («texto plano», «un PDF»). NULL es el caso
            -- normal. Se guarda para que cualquier respuesta que cite el
            -- documento pueda declarar la discrepancia, no sólo el reporte de
            -- indexación del día en que se indexó.
            declared_format_mismatch TEXT,
            indexed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_documents_source ON documents(source_id);
        CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash);

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
            -- Un texto de OCR reconoce por zonas, no por línea lógica completa:
            -- un rótulo de OCR puede ser un fragmento («y (EMP») en vez del
            -- campo real. Mientras esta bandera siga en 1, un documento de
            -- texto plano puede todavía reemplazar el nombre; una vez que un
            -- rótulo fiable lo hace, queda fijo.
            display_name_from_ocr INTEGER NOT NULL DEFAULT 0,
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
            literal_value TEXT NOT NULL DEFAULT '',
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

        -- Archivos que el descubrimiento sí encontró pero que el índice no
        -- pudo aceptar (sin contenido extraíble, formato no soportado, error
        -- de lectura). Sin esta tabla, un conteo por carpeta o por formato
        -- excluía en silencio lo que no se logró indexar y se presentaba como
        -- si fuera el universo completo. No guarda contenido: sólo la ruta, de
        -- dónde venía, su extensión y por qué quedó fuera.
        CREATE TABLE IF NOT EXISTS unindexed_documents (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL REFERENCES source_folders(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            origin TEXT NOT NULL DEFAULT '',
            extension TEXT NOT NULL DEFAULT '',
            reason TEXT NOT NULL,
            recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(source_id, path)
        );
        CREATE INDEX IF NOT EXISTS idx_unindexed_origin
            ON unindexed_documents(origin, extension);
        -- Misma razón que arriba: es la columna hija de la clave foránea
        -- hacia `source_folders`.
        CREATE INDEX IF NOT EXISTS idx_unindexed_source
            ON unindexed_documents(source_id);

        -- Resultado de OCR por hash del contenido del archivo.
        --
        -- El perfilado de la ronda 4 midió que el OCR es el 94,3 % del tiempo
        -- de una indexación completa (703.760 ms de 746.043 sobre 10.000
        -- documentos). Reindexar volvía a correr Vision sobre imágenes
        -- idénticas. La clave es el SHA-256 del archivo, así que la
        -- invalidación es correcta por construcción: si el archivo cambia,
        -- cambia el hash y no hay acierto de caché.
        CREATE TABLE IF NOT EXISTS ocr_cache (
            content_hash TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            confidence REAL,
            chunks TEXT NOT NULL,
            recognized_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

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
        -- `entities.evidence_id` es la columna hija de una clave foránea hacia
        -- `extracted_values(evidence_id)`. Sin índice, borrar una fila padre
        -- obliga a SQLite a recorrer `entities` entera para comprobar la
        -- cascada: reindexar 9.747 documentos borraba 158.049 valores y
        -- recorría 44.880 entidades por cada uno — del orden de 7·10⁹
        -- comparaciones. Medido: la purga del índice anterior pasa de
        -- 740 s a 4,0 s con este índice, y es la causa —nunca aislada hasta
        -- ahora— de que la segunda indexación fuera siempre más lenta que la
        -- primera.
        CREATE INDEX IF NOT EXISTS idx_entities_evidence ON entities(evidence_id);

        "#,
    )?;
    // SQLite no admite ADD COLUMN IF NOT EXISTS. Este esquema se distribuyó
    // antes de que `origin` existiera, por lo que la migración debe conservar
    // los índices ya creados por usuarios existentes.
    let has_origin = tx
        .prepare("PRAGMA table_info(documents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "origin");
    if !has_origin {
        tx.execute(
            "ALTER TABLE documents ADD COLUMN origin TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_documents_origin ON documents(origin)",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash)",
        [],
    )?;
    let columns = tx
        .prepare("PRAGMA table_info(documents)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|name| name == "ocr_confidence") {
        tx.execute("ALTER TABLE documents ADD COLUMN ocr_confidence REAL", [])?;
    }
    if !columns
        .iter()
        .any(|name| name == "declared_format_mismatch")
    {
        tx.execute(
            "ALTER TABLE documents ADD COLUMN declared_format_mismatch TEXT",
            [],
        )?;
    }
    let value_columns = tx
        .prepare("PRAGMA table_info(extracted_values)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !value_columns
        .iter()
        .any(|name| name == "identifier_canonical")
    {
        tx.execute(
            "ALTER TABLE extracted_values ADD COLUMN identifier_canonical TEXT",
            [],
        )?;
    }
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_values_identifier_canonical
         ON extracted_values(identifier_canonical)",
        [],
    )?;
    if !value_columns.iter().any(|name| name == "literal_value") {
        tx.execute(
            "ALTER TABLE extracted_values ADD COLUMN literal_value TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if matches!(fault, Some(MigrationFault::AfterLiteralValueColumnAdded)) {
        return Err(forced_test_failure());
    }
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_values_literal
         ON extracted_values(concept_id, literal_value)",
        [],
    )?;
    let concept_columns = tx
        .prepare("PRAGMA table_info(concepts)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !concept_columns
        .iter()
        .any(|name| name == "display_name_from_ocr")
    {
        tx.execute(
            "ALTER TABLE concepts ADD COLUMN display_name_from_ocr INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    // Los filtros se resuelven desde los valores que coinciden. Incluir el
    // documento en el índice evita recorrer el acervo documento por documento
    // al construir conteos y muestras de evidencia a escala.
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_values_filter_literal_document
         ON extracted_values(concept_id, literal_value, document_id)",
        [],
    )?;
    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_values_filter_numeric_document
         ON extracted_values(concept_id, value_type, numeric_value, document_id)",
        [],
    )?;
    // Rellena `literal_value` para cualquier fila que no la tenga todavía:
    // una base creada antes de que la columna existiera, o una fila que se
    // haya quedado vacía por cualquier otra razón. Se ejecuta en cada
    // apertura y es barata cuando no hay nada pendiente (la consulta no
    // devuelve filas), así que abrir una base antigua ya deja los filtros
    // funcionando sin que nadie tenga que reindexar a mano.
    let fail_backfill_after = match fault {
        Some(MigrationFault::DuringBackfill { rows_updated }) => Some(rows_updated),
        _ => None,
    };
    backfill_literal_value(&tx, fail_backfill_after)?;
    tx.commit()?;
    Ok(())
}

/// Calcula `literal_value` a partir de `text_value` para las filas que
/// todavía no la tienen. Un `text_value` real nunca normaliza a una cadena
/// vacía (el parser ya descarta valores vacíos), así que `literal_value = ''`
/// es una marca inequívoca de «pendiente de rellenar», sin necesitar una
/// columna de versión de esquema aparte. Idempotente: repetirlo sobre una
/// base ya rellenada no hace nada.
///
/// `fail_after_rows` sólo lo usan las pruebas de este módulo: si tiene un
/// valor, se fuerza un error justo después de actualizar esa cantidad de
/// filas, con el resto de las pendientes todavía sin tocar. Sirve para
/// comprobar que un fallo a mitad del backfill —no sólo uno después de que
/// termine— deshace también las filas que sí llegó a actualizar, porque
/// vive dentro de la misma transacción que nunca se confirma.
fn backfill_literal_value(connection: &Connection, fail_after_rows: Option<usize>) -> Result<()> {
    let pending = {
        let mut statement =
            connection.prepare("SELECT id, text_value FROM extracted_values WHERE literal_value = ''")?;
        statement
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if pending.is_empty() {
        return Ok(());
    }
    let mut update = connection.prepare("UPDATE extracted_values SET literal_value = ?1 WHERE id = ?2")?;
    for (rows_done, (id, text_value)) in pending.into_iter().enumerate() {
        update.execute(params![normalize_literal(&text_value), id])?;
        if fail_after_rows == Some(rows_done + 1) {
            return Err(forced_test_failure());
        }
    }
    Ok(())
}
