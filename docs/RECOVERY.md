# Backup y recuperación local de SQLite

Omega trata `omega.db` como un índice derivado de las fuentes autorizadas. La recuperación prioriza
no perder el archivo dañado y no publicar evidencia que pueda haber quedado desactualizada.

## Política

- **Ubicación:** directorio hermano `omega.db.backups/`, con permisos `0700`. Cada backup terminado
  se llama `omega.db.backup-<marca monotónica>.sqlite3` y tiene permisos `0600`.
- **Creación:** antes de cualquier migración de esquema que altere o reconstruya datos existentes,
  `VACUUM INTO` crea una copia consistente —incluido el estado confirmado en WAL— en un archivo
  temporal del mismo volumen. La copia debe superar `PRAGMA integrity_check` y contener el esquema
  de Omega; después se sincroniza y renombra atómicamente.
- **Límites:** máximo 512 MiB por backup, tres backups completos y 1 GiB total. Si una base supera
  el límite individual, la migración se detiene sin modificarla. La rotación elimina primero el
  backup completo más antiguo; los `.tmp` interrumpidos nunca son candidatos de restauración.
- **Retención:** los tres backups completos más recientes que quepan en el límite total. Los
  backups corruptos se ignoran, pero no se borran ni sobrescriben automáticamente.

## Recuperación al arrancar la aplicación

La aplicación instalada usa `OmegaEngine::open_recovering`; las herramientas de evaluación pueden
seguir usando la apertura estricta, que devuelve un error sin tocar una base corrupta.

1. Se abre SQLite y se exige `PRAGMA integrity_check = ok`.
2. Si falla, se prepara en un archivo temporal el backup válido más reciente. Si no existe, se
   prepara una SQLite limpia.
3. Antes de sustituir la ruta activa se copia byte por byte la base dañada —y, si existen, sus
   sidecars `-wal`/`-shm`— a `omega.db.corrupt-<marca>...`, se sincroniza y nunca se sobrescribe.
4. El reemplazo preparado se renombra atómicamente sobre `omega.db`. Una interrupción deja o la
   original dañada o la restaurada completa, nunca media SQLite.
5. Al restaurar un backup se conservan las rutas de fuentes autorizadas, pero se eliminan todos los
   documentos, FTS, valores, entidades, conceptos y citas derivados. Reindexar es obligatorio: un
   backup nunca puede resucitar evidencia vieja.
6. Se escribe `omega.db.recovery.json` con la copia preservada, el backup usado o el fallback limpio
   y `reindex_required: true`. El mismo aviso se emite al log local del proceso.

La cuarentena no tiene rotación automática: contiene el incidente original y sólo debe eliminarse
manualmente después de verificar la recuperación y conservarla conforme a la política local de la
organización.

