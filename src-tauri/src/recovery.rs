use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::error::{OmegaError, Result};

pub const BACKUP_RETENTION: usize = 3;
pub const MAX_BACKUP_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_BACKUP_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupPolicy {
    pub directory: PathBuf,
    pub retention: usize,
    pub max_backup_bytes: u64,
    pub max_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoverySource {
    Backup { path: PathBuf },
    CleanDatabase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryReport {
    pub damaged_database: PathBuf,
    pub quarantined_copy: PathBuf,
    pub source: RecoverySource,
    pub reindex_required: bool,
    pub notice_path: PathBuf,
    pub message: String,
}

pub(crate) fn backup_policy(database: &Path) -> BackupPolicy {
    BackupPolicy {
        directory: sibling_with_suffix(database, ".backups"),
        retention: BACKUP_RETENTION,
        max_backup_bytes: MAX_BACKUP_BYTES,
        max_total_bytes: MAX_BACKUP_TOTAL_BYTES,
    }
}

pub(crate) fn integrity_is_valid(connection: &Connection) -> Result<bool> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(!rows.is_empty() && rows.iter().all(|row| row.eq_ignore_ascii_case("ok")))
}

pub(crate) fn create_atomic_backup(connection: &Connection, database: &Path) -> Result<PathBuf> {
    create_atomic_backup_inner(connection, database, None)
}

fn create_atomic_backup_inner(
    connection: &Connection,
    database: &Path,
    fault: Option<RecoveryFault>,
) -> Result<PathBuf> {
    let policy = backup_policy(database);
    secure_directory(&policy.directory)?;
    let source_bytes = fs::metadata(database)?.len();
    if source_bytes > policy.max_backup_bytes {
        return Err(OmegaError::InvalidArguments(format!(
            "la base ocupa {source_bytes} bytes y supera el máximo de backup local de {} bytes; la migración no se ejecutó",
            policy.max_backup_bytes
        )));
    }
    let stamp = unique_stamp();
    let prefix = database
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("omega.db");
    let final_path = policy
        .directory
        .join(format!("{prefix}.backup-{stamp}.sqlite3"));
    let temporary = policy
        .directory
        .join(format!(".{prefix}.backup-{stamp}.tmp"));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }

    connection.execute("VACUUM INTO ?1", [temporary.to_string_lossy().as_ref()])?;
    secure_file(&temporary)?;
    let temporary_connection = open_read_only(&temporary)?;
    if !integrity_is_valid(&temporary_connection)? || !looks_like_omega(&temporary_connection)? {
        drop(temporary_connection);
        let _ = fs::remove_file(&temporary);
        return Err(OmegaError::InvalidArguments(
            "el backup temporal no superó la validación SQLite/Omega; la migración no se ejecutó"
                .into(),
        ));
    }
    drop(temporary_connection);
    File::open(&temporary)?.sync_all()?;
    if matches!(fault, Some(RecoveryFault::AfterBackupSynced)) {
        return Err(forced_recovery_failure("backup antes del rename atómico"));
    }
    fs::rename(&temporary, &final_path)?;
    sync_parent(&final_path)?;
    rotate_backups(&policy)?;
    Ok(final_path)
}

pub(crate) fn recover_database(database: &Path) -> Result<RecoveryReport> {
    recover_database_inner(database, None)
}

fn recover_database_inner(database: &Path, fault: Option<RecoveryFault>) -> Result<RecoveryReport> {
    let stamp = unique_stamp();
    let stage = sibling_with_suffix(database, &format!(".restore-{stamp}.tmp"));
    let mut restored_from = None;
    for backup in backup_candidates(database)? {
        let _ = fs::remove_file(&stage);
        if fs::copy(&backup, &stage).is_err() {
            continue;
        }
        secure_file(&stage)?;
        if prepare_restored_backup(&stage).is_ok() {
            restored_from = Some(backup);
            break;
        }
    }

    if restored_from.is_none() {
        let _ = fs::remove_file(&stage);
        let connection = Connection::open(&stage)?;
        if !integrity_is_valid(&connection)? {
            return Err(OmegaError::InvalidArguments(
                "no se pudo preparar una base SQLite limpia para la recuperación".into(),
            ));
        }
        drop(connection);
        secure_file(&stage)?;
    }
    File::open(&stage)?.sync_all()?;

    // La copia dañada se conserva antes de reemplazar su ruta. Si el proceso
    // se interrumpe antes del rename, la base original sigue intacta; si se
    // interrumpe después, la copia de cuarentena ya está sincronizada.
    let quarantine = preserve_damaged_set(database, stamp)?;
    if matches!(fault, Some(RecoveryFault::BeforeRestoreRename)) {
        return Err(forced_recovery_failure(
            "restauración antes del rename atómico",
        ));
    }
    remove_sqlite_sidecars(database)?;
    fs::rename(&stage, database)?;
    sync_parent(database)?;

    let source = match restored_from {
        Some(path) => RecoverySource::Backup { path },
        None => RecoverySource::CleanDatabase,
    };
    let notice_path = sibling_with_suffix(database, ".recovery.json");
    let message = match &source {
        RecoverySource::Backup { path } => format!(
            "SQLite dañada preservada en {}. Se restauró el backup válido {} y se invalidó toda evidencia derivada; es obligatorio reindexar las fuentes autorizadas.",
            quarantine.display(),
            path.display()
        ),
        RecoverySource::CleanDatabase => format!(
            "SQLite dañada preservada en {}. No había un backup válido: se creó una base limpia; es obligatorio volver a autorizar e indexar las fuentes.",
            quarantine.display()
        ),
    };
    Ok(RecoveryReport {
        damaged_database: database.to_path_buf(),
        quarantined_copy: quarantine,
        source,
        reindex_required: true,
        notice_path,
        message,
    })
}

pub(crate) fn persist_recovery_notice(report: &RecoveryReport) -> Result<()> {
    let temporary = sibling_with_suffix(&report.notice_path, ".tmp");
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| OmegaError::InvalidArguments(error.to_string()))?;
    fs::write(&temporary, bytes)?;
    secure_file(&temporary)?;
    File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, &report.notice_path)?;
    sync_parent(&report.notice_path)
}

fn prepare_restored_backup(path: &Path) -> Result<()> {
    let mut connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = DELETE;")?;
    if !integrity_is_valid(&connection)? || !looks_like_omega(&connection)? {
        return Err(OmegaError::InvalidArguments(
            "backup SQLite inválido o ajeno a Omega".into(),
        ));
    }
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM chunks_fts", [])?;
    transaction.execute("DELETE FROM documents", [])?;
    transaction.execute("DELETE FROM concepts", [])?;
    transaction.execute("UPDATE source_folders SET indexed_at = NULL", [])?;
    transaction.commit()?;
    if !integrity_is_valid(&connection)? {
        return Err(OmegaError::InvalidArguments(
            "el backup restaurado perdió integridad al invalidar evidencia".into(),
        ));
    }
    drop(connection);
    File::open(path)?.sync_all()?;
    Ok(())
}

fn backup_candidates(database: &Path) -> Result<Vec<PathBuf>> {
    let policy = backup_policy(database);
    let Ok(entries) = fs::read_dir(&policy.directory) else {
        return Ok(vec![]);
    };
    let mut candidates = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_finished_backup(path, database))
        .filter(|path| {
            fs::metadata(path)
                .is_ok_and(|metadata| metadata.len() > 0 && metadata.len() <= MAX_BACKUP_BYTES)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.reverse();
    Ok(candidates)
}

fn rotate_backups(policy: &BackupPolicy) -> Result<()> {
    let mut backups = fs::read_dir(&policy.directory)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("sqlite3"))
        .filter_map(|path| {
            fs::metadata(&path)
                .ok()
                .map(|metadata| (path, metadata.len()))
        })
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| left.0.cmp(&right.0));
    let mut total = backups.iter().map(|(_, bytes)| bytes).sum::<u64>();
    while backups.len() > policy.retention || total > policy.max_total_bytes {
        let (oldest, bytes) = backups.remove(0);
        fs::remove_file(oldest)?;
        total = total.saturating_sub(bytes);
    }
    sync_parent(&policy.directory)
}

fn preserve_damaged_set(database: &Path, stamp: u128) -> Result<PathBuf> {
    let quarantine = sibling_with_suffix(database, &format!(".corrupt-{stamp}"));
    atomic_file_copy(database, &quarantine)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", database.display(), suffix));
        if sidecar.exists() {
            let preserved = PathBuf::from(format!("{}{}", quarantine.display(), suffix));
            atomic_file_copy(&sidecar, &preserved)?;
        }
    }
    Ok(quarantine)
}

fn atomic_file_copy(source: &Path, destination: &Path) -> Result<()> {
    let temporary = sibling_with_suffix(destination, ".tmp");
    fs::copy(source, &temporary)?;
    secure_file(&temporary)?;
    File::open(&temporary)?.sync_all()?;
    fs::rename(&temporary, destination)?;
    sync_parent(destination)
}

fn remove_sqlite_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{}", database.display(), suffix));
        if sidecar.exists() {
            fs::remove_file(sidecar)?;
        }
    }
    Ok(())
}

fn looks_like_omega(connection: &Connection) -> Result<bool> {
    let exists: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'source_folders')",
        [],
        |row| row.get(0),
    )?;
    Ok(exists == 1)
}

fn open_read_only(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn is_finished_backup(path: &Path, database: &Path) -> bool {
    let prefix = database
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("omega.db");
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(&format!("{prefix}.backup-")) && name.ends_with(".sqlite3")
        })
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("omega.db");
    path.with_file_name(format!("{name}{suffix}"))
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn secure_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn secure_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum RecoveryFault {
    AfterBackupSynced,
    BeforeRestoreRename,
}

fn forced_recovery_failure(stage: &str) -> OmegaError {
    OmegaError::InvalidArguments(format!("interrupción simulada durante {stage}"))
}

#[cfg(test)]
pub(crate) fn create_backup_with_interruption(
    connection: &Connection,
    database: &Path,
) -> Result<PathBuf> {
    create_atomic_backup_inner(connection, database, Some(RecoveryFault::AfterBackupSynced))
}

#[cfg(test)]
pub(crate) fn create_backup_for_test(connection: &Connection, database: &Path) -> Result<PathBuf> {
    create_atomic_backup_inner(connection, database, None)
}

#[cfg(test)]
pub(crate) fn recover_with_interruption(database: &Path) -> Result<RecoveryReport> {
    recover_database_inner(database, Some(RecoveryFault::BeforeRestoreRename))
}
