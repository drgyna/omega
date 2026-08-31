//! Prueba de migración real: una base indexada con el esquema anterior a
//! `literal_value` (la columna que ahora decide si un filtro «Campo: 50»
//! encuentra «Campo: 50%») tiene que seguir funcionando en cuanto se abre con
//! el binario nuevo, sin que nadie tenga que reindexar a mano.
//!
//! Vive dentro del crate (no en `tests/`) porque construye el acervo llamando
//! a `canonical_key` y `classify_value` — las mismas funciones que usaba el
//! indexador antiguo — para que los datos de la base «vieja» sean fieles a lo
//! que un indexador real habría escrito, en vez de un valor tecleado a mano
//! que podría no coincidir con las reglas reales de normalización.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::{
    db::Database,
    extract::classify_value,
    model::ToolFilter,
    normalize::canonical_key,
    tools::ToolEngine,
};

/// Esquema tal como estaba justo antes de que existiera `literal_value`.
/// Copiado a mano, sin pasar por `db::migrate`, para reproducir exactamente
/// lo que un usuario real tiene en disco desde antes de este cambio.
const OLD_SCHEMA: &str = r#"
CREATE TABLE source_folders (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    indexed_at TEXT,
    revoked_at TEXT
);

CREATE TABLE documents (
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
CREATE INDEX idx_documents_source ON documents(source_id);

CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    location TEXT NOT NULL,
    content TEXT NOT NULL,
    UNIQUE(document_id, ordinal)
);
CREATE INDEX idx_chunks_document ON chunks(document_id);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    chunk_id UNINDEXED,
    document_id UNINDEXED,
    content,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE concepts (
    id INTEGER PRIMARY KEY,
    canonical_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    value_type TEXT NOT NULL CHECK (
        value_type IN ('money', 'date', 'percentage', 'number', 'state', 'text')
    ),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE concept_aliases (
    id INTEGER PRIMARY KEY,
    concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
    alias TEXT NOT NULL,
    normalized_alias TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('local_rule', 'ai', 'user')),
    status TEXT NOT NULL CHECK (status IN ('system', 'suggested', 'confirmed', 'rejected')),
    UNIQUE(concept_id, normalized_alias, origin)
);
CREATE INDEX idx_alias_normalized ON concept_aliases(normalized_alias, status);

-- Sin `literal_value`: exactamente la forma anterior a este cambio.
CREATE TABLE extracted_values (
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
CREATE INDEX idx_values_concept_numeric ON extracted_values(concept_id, numeric_value);
CREATE INDEX idx_values_document ON extracted_values(document_id);
CREATE INDEX idx_values_normalized ON extracted_values(concept_id, normalized_value);
CREATE INDEX idx_values_date ON extracted_values(date_value);
CREATE INDEX idx_values_identifier_canonical ON extracted_values(identifier_canonical);

CREATE TABLE entities (
    id INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL,
    role TEXT NOT NULL,
    relationship_kind TEXT NOT NULL CHECK (relationship_kind IN ('owner', 'mention')),
    evidence_id TEXT NOT NULL REFERENCES extracted_values(evidence_id) ON DELETE CASCADE
);
CREATE INDEX idx_entities_name ON entities(normalized_name);
CREATE INDEX idx_entities_document ON entities(document_id);
"#;

struct LegacyRecord {
    document_id: i64,
    concept_label: &'static str,
    raw_value: &'static str,
}

/// Construye una base con el esquema viejo e inserta cada valor pasándolo por
/// `classify_value`/`canonical_key` — las mismas funciones que usaba el
/// indexador de la versión anterior — para que la base "vieja" sea fiel a lo
/// que ese indexador habría escrito de verdad, incluida cualquier rareza de
/// su normalización.
fn build_legacy_database(path: &std::path::Path, records: &[LegacyRecord]) {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(OLD_SCHEMA).unwrap();
    connection
        .execute("INSERT INTO source_folders(id, path) VALUES (1, '/fixture')", [])
        .unwrap();

    let document_ids = records
        .iter()
        .map(|record| record.document_id)
        .collect::<std::collections::BTreeSet<_>>();
    for document_id in document_ids {
        connection
            .execute(
                "INSERT INTO documents(
                    id, source_id, path, title, extension, parser, content_hash, size_bytes, modified_unix
                 ) VALUES (?1, 1, ?2, ?2, 'md', 'texto plano', ?3, 10, 0)",
                params![
                    document_id,
                    format!("/fixture/{document_id}.md"),
                    format!("hash-{document_id}")
                ],
            )
            .unwrap();
    }

    let mut concept_ids: HashMap<String, i64> = HashMap::new();
    let mut next_concept_id = 1i64;
    for (index, record) in records.iter().enumerate() {
        let key = canonical_key(record.concept_label);
        let concept_id = *concept_ids.entry(key.clone()).or_insert_with(|| {
            let id = next_concept_id;
            next_concept_id += 1;
            let typed = classify_value(record.concept_label, record.raw_value);
            connection
                .execute(
                    "INSERT INTO concepts(id, canonical_key, display_name, value_type)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, key, record.concept_label, typed.kind.as_str()],
                )
                .unwrap();
            id
        });
        let typed = classify_value(record.concept_label, record.raw_value);
        connection
            .execute(
                "INSERT INTO extracted_values(
                    evidence_id, document_id, concept_id, value_type, text_value,
                    normalized_value, numeric_value, currency, location, excerpt
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'línea 1', ?5)",
                params![
                    format!("v-{index}"),
                    record.document_id,
                    concept_id,
                    typed.kind.as_str(),
                    typed.text_value,
                    typed.normalized_value,
                    typed.numeric_value,
                    typed.currency,
                ],
            )
            .unwrap();
    }
    connection.close().unwrap();
}

const RECORDS: &[LegacyRecord] = &[
    LegacyRecord { document_id: 1, concept_label: "Descuento", raw_value: "50" },
    LegacyRecord { document_id: 2, concept_label: "Descuento", raw_value: "50%" },
    // Valor real con «, » y : a la vez.
    LegacyRecord { document_id: 3, concept_label: "Nota", raw_value: "«Revisión: 10:30»" },
    // Sólo comillas, para comprobar que el valor completo (con acento) se conserva.
    LegacyRecord { document_id: 4, concept_label: "Estado", raw_value: "«Pendiente»" },
    // Campo y valor con Unicode fuera de ASCII más allá de acentos comunes.
    LegacyRecord { document_id: 5, concept_label: "Ubicación", raw_value: "Müller & Söhne — Zürich" },
];

fn resolved_value(records: &[LegacyRecord], concept_label: &str, raw_value: &str) -> String {
    classify_value(
        records
            .iter()
            .find(|r| r.concept_label == concept_label)
            .unwrap()
            .concept_label,
        raw_value,
    )
    .text_value
}

#[test]
fn opening_a_legacy_database_repairs_its_filters_without_manual_intervention() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy.db");
    build_legacy_database(&path, RECORDS);

    // Sólo `Database::open`: nada de reindexar, nada de un paso manual.
    let database = Database::open(&path).unwrap();
    let tools = ToolEngine::new(database.clone());

    let find = |concept: &str, equals: &str| {
        tools
            .query_documents(
                &[ToolFilter { concept: concept.into(), equals: equals.into() }],
                None,
                10,
            )
            .unwrap()
    };

    // 1) «50» y «50%» comparten dígitos, pero siguen siendo valores distintos
    // aunque los datos vinieran de la base antigua.
    let number = find("Descuento", "50");
    assert_eq!(number.document_count, 1, "«50» no debe encontrar el documento de «50%»");
    assert!(number.evidence.iter().any(|item| item.value.as_deref() == Some("50")));

    let percent = find("Descuento", "50%");
    assert_eq!(percent.document_count, 1, "«50%» no debe encontrar el documento de «50»");
    assert!(percent.evidence.iter().any(|item| item.value.as_deref() == Some("50%")));

    // 2) Un valor real con «, » y : a la vez se conserva completo.
    let quoted_colon = find("Nota", "«Revisión: 10:30»");
    assert_eq!(quoted_colon.document_count, 1, "{:?}", quoted_colon.evidence);
    assert!(
        quoted_colon
            .evidence
            .iter()
            .any(|item| item.value.as_deref() == Some("«Revisión: 10:30»")),
        "el valor con dos puntos no debe truncarse: {:?}",
        quoted_colon.evidence
    );

    // 3) «Pendiente» entre comillas se conserva completo, no recortado.
    let quoted = find("Estado", "«Pendiente»");
    assert_eq!(quoted.document_count, 1, "{:?}", quoted.evidence);
    assert!(quoted.evidence.iter().any(|item| item.value.as_deref() == Some("«Pendiente»")));

    // 4) Campos y valores con Unicode (diéresis, ampersand, guion largo)
    // siguen funcionando tras la migración.
    let unicode = find("Ubicación", "Müller & Söhne — Zürich");
    assert_eq!(unicode.document_count, 1, "{:?}", unicode.evidence);
    assert!(
        unicode
            .evidence
            .iter()
            .any(|item| item.value.as_deref() == Some(&resolved_value(RECORDS, "Ubicación", "Müller & Söhne — Zürich")))
    );

    // 5) El backfill realmente ocurrió: la columna ya no está en blanco para
    // ninguna de las filas antiguas.
    let connection = database.connect().unwrap();
    let empty_literal_values: i64 = connection
        .query_row("SELECT COUNT(*) FROM extracted_values WHERE literal_value = ''", [], |row| row.get(0))
        .unwrap();
    assert_eq!(empty_literal_values, 0, "abrir la base debe rellenar literal_value para todas las filas antiguas");

    // 6) Idempotencia: reabrir la misma base no rompe nada ni duplica datos.
    drop(tools);
    drop(database);
    let reopened = Database::open(&path).unwrap();
    let tools_again = ToolEngine::new(reopened);
    let number_again = tools_again
        .query_documents(&[ToolFilter { concept: "Descuento".into(), equals: "50".into() }], None, 10)
        .unwrap();
    assert_eq!(number_again.document_count, 1);
}

/// Migrar una base vieja sin filas en `extracted_values` no debe fallar: el
/// backfill se salta sin ejecutar ninguna actualización.
#[test]
fn migrating_a_legacy_database_with_no_rows_does_not_fail() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy-empty.db");
    {
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(OLD_SCHEMA).unwrap();
        connection.close().unwrap();
    }
    Database::open(&path).unwrap();
}

/// El alcance heredado entre turnos de conversación es un `ToolFilter` más:
/// si viene de una base migrada, debe conservar el mismo tipo (número vs.
/// porcentaje) que un filtro construido en una base nueva.
#[test]
fn a_filter_inherited_across_turns_keeps_its_type_after_migration() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy-context.db");
    build_legacy_database(&path, RECORDS);
    let database = Database::open(&path).unwrap();
    let tools = ToolEngine::new(database);

    // Simula lo que hace un turno posterior de la conversación: reutilizar un
    // `ToolFilter` ya resuelto (como lo guarda `ConversationState`) en una
    // consulta nueva, sin volver a pasar por `resolved_filters`.
    let inherited = ToolFilter { concept: "Descuento".into(), equals: "50%".into() };
    let result = tools.query_documents(&[inherited], None, 10).unwrap();
    assert_eq!(result.document_count, 1);
    assert!(result.evidence.iter().any(|item| item.value.as_deref() == Some("50%")));
    assert!(
        !result.evidence.iter().any(|item| item.value.as_deref() == Some("50")),
        "un filtro heredado de «50%» no puede colarse al documento de «50»: {:?}",
        result.evidence
    );
}

/// Fuerza el fallo justo después del `ALTER TABLE ... ADD COLUMN
/// literal_value`, antes de que el backfill escriba nada. Sobre una base
/// legada donde esa columna todavía no existe, comprueba que al reabrir:
/// la columna no quedó agregada a medias (no existe en absoluto, no es que
/// exista vacía), el índice de esquema que corrió antes en la misma
/// transacción (`idx_documents_origin`) tampoco sobrevivió, y que la base
/// puede volver a migrarse por el camino normal sin que nadie intervenga.
#[test]
fn a_failure_right_after_alter_table_leaves_no_dangling_column() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy-fault-alter.db");
    build_legacy_database(&path, RECORDS);
    assert!(
        !column_exists(&Connection::open(&path).unwrap(), "extracted_values", "literal_value"),
        "OLD_SCHEMA no debe traer todavía esta columna"
    );
    assert!(
        !index_exists(&Connection::open(&path).unwrap(), "idx_documents_origin"),
        "OLD_SCHEMA no debe traer todavía este índice"
    );

    {
        let mut connection = Connection::open(&path).unwrap();
        let outcome = crate::db::migrate_with_fault(
            &mut connection,
            crate::db::MigrationFault::AfterLiteralValueColumnAdded,
        );
        assert!(outcome.is_err(), "el fallo forzado debe propagarse");
    }

    // Ni la columna que el ALTER TABLE sí llegó a añadir, ni el índice de un
    // paso anterior de la misma transacción, deben haber sobrevivido: el
    // rollback deshace TODO lo que corrió, no sólo el último paso.
    let connection = Connection::open(&path).unwrap();
    assert!(
        !column_exists(&connection, "extracted_values", "literal_value"),
        "un fallo justo después del ALTER TABLE no puede dejar la columna a medio agregar"
    );
    assert!(
        !index_exists(&connection, "idx_documents_origin"),
        "el rollback debe deshacer también los pasos anteriores al fallo, no sólo el del ALTER TABLE"
    );
    drop(connection);

    // La base no quedó inservible: reabrirla por el camino normal completa
    // la migración igual que si el intento roto nunca hubiera pasado.
    let database = Database::open(&path).unwrap();
    let tools = ToolEngine::new(database);
    let result = tools
        .query_documents(&[ToolFilter { concept: "Descuento".into(), equals: "50%".into() }], None, 10)
        .unwrap();
    assert_eq!(result.document_count, 1);
    let connection = Connection::open(&path).unwrap();
    assert!(column_exists(&connection, "extracted_values", "literal_value"));
    assert!(index_exists(&connection, "idx_documents_origin"));
    assert_eq!(blank_literal_value_rows(&connection), 0);
}

/// Fuerza el fallo a mitad del backfill —después de actualizar sólo 2 de las
/// 5 filas pendientes—, con la columna `literal_value` ya presente pero en
/// blanco (como quedaría tras un `ALTER TABLE` real). Comprueba que ninguna
/// fila quedó parcialmente actualizada (ni las 2 que sí llegó a tocar) y que
/// la base puede volver a migrarse correctamente después.
#[test]
fn a_failure_mid_backfill_leaves_no_partially_updated_rows() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy-fault-backfill.db");
    build_legacy_database(&path, RECORDS);
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "ALTER TABLE extracted_values ADD COLUMN literal_value TEXT NOT NULL DEFAULT ''",
                [],
            )
            .unwrap();
    }
    let pending_before: i64 = Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM extracted_values", [], |row| row.get(0))
        .unwrap();
    assert_eq!(pending_before, 5, "RECORDS debe dejar 5 filas pendientes de backfill");

    {
        let mut connection = Connection::open(&path).unwrap();
        let outcome = crate::db::migrate_with_fault(
            &mut connection,
            crate::db::MigrationFault::DuringBackfill { rows_updated: 2 },
        );
        assert!(outcome.is_err(), "el fallo forzado debe propagarse");
    }

    // Las 2 filas que el backfill sí llegó a actualizar antes del fallo
    // deben volver a quedar en blanco: la transacción nunca se confirmó, así
    // que no puede haber un resultado a medias entre "actualizada" y "no".
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        blank_literal_value_rows(&connection),
        5,
        "un fallo a mitad del backfill no puede dejar ninguna fila parcialmente actualizada"
    );
    drop(connection);

    // Reabrir por el camino normal debe completar el backfill de las 5
    // filas, sin que el intento interrumpido haya dejado nada corrupto.
    let database = Database::open(&path).unwrap();
    let tools = ToolEngine::new(database);
    let result = tools
        .query_documents(&[ToolFilter { concept: "Descuento".into(), equals: "50%".into() }], None, 10)
        .unwrap();
    assert_eq!(result.document_count, 1);
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        blank_literal_value_rows(&connection),
        0,
        "la reapertura normal debe terminar el backfill que el intento fallido deshizo"
    );
}

fn index_exists(connection: &Connection, name: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

fn column_exists(connection: &Connection, table: &str, column: &str) -> bool {
    connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .iter()
        .any(|name| name == column)
}

fn blank_literal_value_rows(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT COUNT(*) FROM extracted_values WHERE literal_value = ''",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// P1-A. El esquema anterior valida `ocr_status` con una lista que no incluye
/// `unavailable`: una base ya existente no podría registrar «no hay motor
/// OCR» y el estado tendría que degradarse a otro que miente. Abrir la base
/// con el binario nuevo tiene que ensanchar esa restricción sin perder ni una
/// fila ni una referencia de las tablas hijas.
#[test]
fn opening_a_legacy_database_accepts_the_unavailable_ocr_state() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("legacy-ocr.db");
    build_legacy_database(&path, RECORDS);

    // La base vieja rechaza el estado nuevo: ése es el punto de partida.
    {
        let legacy = Connection::open(&path).unwrap();
        let rejected = legacy.execute(
            "UPDATE documents SET ocr_status = 'unavailable' WHERE id = 1",
            [],
        );
        assert!(
            rejected.is_err(),
            "el esquema anterior no admite el estado nuevo"
        );
    }

    let documents_before: i64 = {
        let connection = Connection::open(&path).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
            .unwrap()
    };
    let values_before: i64 = {
        let connection = Connection::open(&path).unwrap();
        connection
            .query_row("SELECT COUNT(*) FROM extracted_values", [], |row| row.get(0))
            .unwrap()
    };

    // Sólo `Database::open`: sin reindexar y sin ningún paso manual.
    let database = Database::open(&path).unwrap();
    let backup_policy = Database::backup_policy(&path);
    let backups = std::fs::read_dir(&backup_policy.directory)
        .expect("una migración destructiva debe crear antes su directorio de backup")
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite3")
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1, "la migración debe tener un backup atómico previo");
    let connection = database.connect().unwrap();

    connection
        .execute(
            "UPDATE documents SET ocr_status = 'unavailable' WHERE id = 1",
            [],
        )
        .expect("el índice migrado debe poder registrar «no hay motor OCR»");

    // Un estado inventado sigue estando prohibido: ensanchar no es abrir.
    assert!(
        connection
            .execute("UPDATE documents SET ocr_status = 'perfecto' WHERE id = 2", [])
            .is_err(),
        "la restricción sigue acotada a los estados reales"
    );

    // Ni las filas ni las referencias de las tablas hijas se pierden al
    // reconstruir la tabla.
    let documents_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))
        .unwrap();
    let values_after: i64 = connection
        .query_row("SELECT COUNT(*) FROM extracted_values", [], |row| row.get(0))
        .unwrap();
    assert_eq!(documents_after, documents_before);
    assert_eq!(values_after, values_before);
    let orphans: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM extracted_values v
             WHERE NOT EXISTS (SELECT 1 FROM documents d WHERE d.id = v.document_id)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "ningún valor puede quedar sin su documento");

    // Y sigue siendo idempotente: reabrir no vuelve a reconstruir ni rompe.
    drop(connection);
    let reopened = Database::open(&path).unwrap();
    let connection = reopened.connect().unwrap();
    let status: String = connection
        .query_row("SELECT ocr_status FROM documents WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "unavailable", "el estado sobrevive a reabrir");
}
