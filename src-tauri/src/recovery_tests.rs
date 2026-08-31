use std::{
    fs,
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use crate::{Database, OmegaEngine, RecoverySource, recovery};

fn write_document(root: &Path, identifier: &str) -> PathBuf {
    let path = root.join("evidencia.md");
    fs::write(
        &path,
        format!("# Evidencia\n\nFolio: {identifier}\nEstado: Activo\nImporte: $10.00 MXN\n"),
    )
    .unwrap();
    path
}

fn indexed_database(root: &Path, database_path: &Path, identifier: &str) -> PathBuf {
    let document = write_document(root, identifier);
    let engine = OmegaEngine::open(database_path).unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert_eq!(report.indexed, 1);
    assert!(!engine.search(identifier).unwrap().is_empty());
    drop(engine);
    document
}

fn create_valid_backup(database_path: &Path) -> PathBuf {
    let connection = Connection::open(database_path).unwrap();
    let backup = recovery::create_backup_for_test(&connection, database_path).unwrap();
    drop(connection);
    backup
}

fn truncate_database(path: &Path) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(64)
        .unwrap();
}

#[test]
fn a_valid_database_opens_without_recovery_or_an_unnecessary_backup() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-VALID");

    let reopened = OmegaEngine::open_recovering(&database_path).unwrap();
    assert!(reopened.recovery_report().is_none());
    assert_eq!(reopened.status().unwrap().documents, 1);
    assert!(!reopened.search("RECOVERY-VALID").unwrap().is_empty());
    assert!(!Database::backup_policy(&database_path).directory.exists());
}

#[test]
fn a_truncated_database_restores_the_newest_valid_backup_without_stale_evidence() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    let document = indexed_database(&corpus, &database_path, "RECOVERY-OLD");
    let backup = create_valid_backup(&database_path);
    fs::write(
        &document,
        "# Evidencia actual\n\nFolio: RECOVERY-NEW\nEstado: Activo\nImporte: $20.00 MXN\n",
    )
    .unwrap();
    truncate_database(&database_path);

    let engine = OmegaEngine::open_recovering(&database_path).unwrap();
    let recovery = engine
        .recovery_report()
        .expect("debe publicar recuperación");
    assert_eq!(recovery.source, RecoverySource::Backup { path: backup });
    assert!(recovery.reindex_required);
    assert!(recovery.quarantined_copy.exists());
    assert!(recovery.notice_path.exists());
    assert_eq!(engine.status().unwrap().documents, 0);
    assert!(engine.search("RECOVERY-OLD").unwrap().is_empty());
    assert!(engine.search("RECOVERY-NEW").unwrap().is_empty());

    let sources = engine.sources().unwrap();
    assert_eq!(sources.len(), 1, "el backup conserva la autorización local");
    let report = engine.index_source(sources[0].id).unwrap();
    assert_eq!(report.indexed, 1);
    assert!(engine.search("RECOVERY-OLD").unwrap().is_empty());
    let hits = engine.search("RECOVERY-NEW").unwrap();
    assert!(!hits.is_empty());
    let paths = hits
        .iter()
        .map(|hit| hit.evidence.path.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        paths.len(),
        1,
        "la recuperación no duplica documentos ni citas"
    );
    assert_eq!(
        Path::new(paths.first().unwrap()),
        document.canonicalize().unwrap()
    );
}

#[test]
fn a_corrupt_newest_backup_is_skipped_for_the_last_valid_one() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-BACKUP");
    let valid = create_valid_backup(&database_path);
    let policy = Database::backup_policy(&database_path);
    let corrupt = policy
        .directory
        .join("omega.db.backup-999999999999999999999999999.sqlite3");
    fs::write(&corrupt, b"backup corrupto").unwrap();
    fs::write(&database_path, b"base corrupta").unwrap();

    let recovered = OmegaEngine::open_recovering(&database_path).unwrap();
    assert_eq!(
        recovered.recovery_report().unwrap().source,
        RecoverySource::Backup { path: valid }
    );
    assert_eq!(recovered.status().unwrap().documents, 0);
    assert!(
        corrupt.exists(),
        "un backup rechazado no se reescribe ni se borra"
    );
}

#[test]
fn no_valid_backup_creates_a_clean_database_and_preserves_the_damage() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-CLEAN");
    let damaged = b"SQLite irrecuperable sin backup";
    fs::write(&database_path, damaged).unwrap();

    let recovered = OmegaEngine::open_recovering(&database_path).unwrap();
    let report = recovered.recovery_report().unwrap();
    assert_eq!(report.source, RecoverySource::CleanDatabase);
    assert_eq!(fs::read(&report.quarantined_copy).unwrap(), damaged);
    assert_eq!(recovered.status().unwrap().sources, 0);
    assert_eq!(recovered.status().unwrap().documents, 0);
    assert!(recovered.search("RECOVERY-CLEAN").unwrap().is_empty());
}

#[test]
fn an_interrupted_backup_never_becomes_a_restore_candidate() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-INTERRUPTED-BACKUP");
    let connection = Connection::open(&database_path).unwrap();

    let outcome = recovery::create_backup_with_interruption(&connection, &database_path);
    assert!(outcome.is_err());
    let policy = Database::backup_policy(&database_path);
    let names = fs::read_dir(&policy.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name.ends_with(".tmp")));
    assert!(!names.iter().any(|name| name.ends_with(".sqlite3")));
    assert!(recovery::integrity_is_valid(&connection).unwrap());
    drop(connection);

    let valid = create_valid_backup(&database_path);
    assert!(valid.exists());
}

#[test]
fn an_interrupted_restore_keeps_the_corrupt_original_until_a_retry_succeeds() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-INTERRUPTED-RESTORE");
    create_valid_backup(&database_path);
    let damaged = b"base danada que debe sobrevivir a la interrupcion";
    fs::write(&database_path, damaged).unwrap();

    let outcome = recovery::recover_with_interruption(&database_path);
    assert!(outcome.is_err());
    assert_eq!(fs::read(&database_path).unwrap(), damaged);
    assert!(
        fixture
            .path()
            .read_dir()
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
    );

    let recovered = OmegaEngine::open_recovering(&database_path).unwrap();
    assert!(matches!(
        recovered.recovery_report().unwrap().source,
        RecoverySource::Backup { .. }
    ));
    assert_eq!(recovered.status().unwrap().documents, 0);
}

#[test]
fn backup_rotation_keeps_only_the_three_newest_complete_files() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    fs::create_dir(&corpus).unwrap();
    let database_path = fixture.path().join("omega.db");
    indexed_database(&corpus, &database_path, "RECOVERY-ROTATION");
    for _ in 0..5 {
        create_valid_backup(&database_path);
    }
    let policy = Database::backup_policy(&database_path);
    let complete = fs::read_dir(&policy.directory)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry.path().extension().and_then(|value| value.to_str()) == Some("sqlite3")
        })
        .count();
    assert_eq!(complete, policy.retention);
}
