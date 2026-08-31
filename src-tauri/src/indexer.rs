use std::{
    collections::{HashMap, HashSet},
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
    model::{IndexPhases, IndexReport, OcrStatus, ParsedChunk, ParsedDocument, ParsedRecord},
    normalize::{canonical_identifier, canonical_key, normalize_exact, normalize_spanish},
    parser::{DocumentParser, LabelVocabulary, SUPPORTED_EXTENSIONS, records_from_pdf_pages},
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
        let started_discover = Instant::now();
        let files = discover(&source);
        let discover_ms = started_discover.elapsed().as_millis();

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
        let started_purge = Instant::now();
        transaction.execute(
            "DELETE FROM chunks_fts
             WHERE document_id IN (SELECT id FROM documents WHERE source_id = ?1)",
            [source_id],
        )?;
        transaction.execute("DELETE FROM documents WHERE source_id = ?1", [source_id])?;
        transaction.execute(
            "DELETE FROM unindexed_documents WHERE source_id = ?1",
            [source_id],
        )?;
        let purge_ms = started_purge.elapsed().as_millis();
        let mut report = IndexReport {
            source_id,
            discovered: files.len(),
            indexed: 0,
            modified: 0,
            skipped: 0,
            ocr_pending: 0,
            ocr_low_confidence: 0,
            ocr_failed: 0,
            ocr_unavailable: 0,
            duplicate_groups: 0,
            duplicate_documents: 0,
            values: 0,
            warnings: vec![],
            phases: IndexPhases {
                discover_ms,
                purge_ms,
                ..IndexPhases::default()
            },
            elapsed_ms: 0,
        };
        let mut parse_ms_by_parser: HashMap<String, u128> = HashMap::new();
        let mut documents_by_parser: HashMap<String, usize> = HashMap::new();

        for path in files {
            let started_parse = Instant::now();
            let parsed = self.parser.parse(&path);
            let parse_ms = started_parse.elapsed().as_millis();
            report.phases.parse_ms += parse_ms;
            // El parser que se usó de verdad, no el que la extensión sugiere:
            // un PDF con capa de texto y uno escaneado cuestan órdenes de
            // magnitud distintos y no pueden compartir cubeta.
            let bucket = match &parsed {
                Ok(document) => document.parser.clone(),
                Err(_) => "sin parser (error)".to_owned(),
            };
            *parse_ms_by_parser.entry(bucket.clone()).or_default() += parse_ms;
            *documents_by_parser.entry(bucket).or_default() += 1;
            match parsed {
                Ok(document) => {
                    // El estado OCR se cuenta aunque el archivo no llegue a
                    // indexarse: un archivo ilegible tiene que quedar visible
                    // en el reporte, no desaparecer dentro de «omitidos».
                    count_ocr_state(&mut report, document.ocr_status);
                    // Lo que el parser no pudo dar por bueno se informa, aunque
                    // el resto del archivo sí se indexe.
                    report.warnings.extend(document.warnings.iter().cloned());
                    // El SHA-256 se calcula una sola vez por archivo y sirve
                    // para las tres cosas que lo necesitan: guardar la lectura
                    // del OCR, detectar cambios e insertar el documento.
                    let started_hash = Instant::now();
                    let hashed = file_hash(&path);
                    report.phases.hash_ms += started_hash.elapsed().as_millis();
                    let current_hash = match hashed {
                        Ok(hash) => hash,
                        Err(error) => {
                            report.skipped += 1;
                            record_unindexed(
                                &transaction,
                                source_id,
                                &source,
                                &path,
                                "archivo ilegible",
                            )?;
                            report.warnings.push(format!("{}: {error}", path.display()));
                            continue;
                        }
                    };
                    // Guardar la lectura del OCR es trabajo del indexador, no
                    // del parser: aquí ya existe la transacción de escritura,
                    // así que no hay dos conexiones peleando por el mismo
                    // archivo. Se guarda aunque el documento acabe omitido —un
                    // escaneo del que no salió nada costó exactamente lo mismo
                    // de reconocer— y nunca cuando no había motor o el OCR no
                    // llegó a correr: eso es un hecho del equipo, no del
                    // archivo, y congelarlo impediría reintentarlo el día que
                    // el motor exista.
                    if matches!(
                        document.ocr_status,
                        OcrStatus::Complete | OcrStatus::LowConfidence | OcrStatus::Failed
                    ) {
                        remember_ocr(&transaction, &current_hash, &document)?;
                    }
                    if !has_indexable_evidence(&document) {
                        report.skipped += 1;
                        record_unindexed(
                            &transaction,
                            source_id,
                            &source,
                            &path,
                            "sin contenido extraíble",
                        )?;
                        report.warnings.push(format!(
                            "{}: sin contenido extraíble; no se creó un documento indexado (parser={}, {} [{}])",
                            path.display(),
                            document.parser,
                            document.ocr_status.description(),
                            document.ocr_status.as_str()
                        ));
                        continue;
                    }
                    let canonical_path = match path.canonicalize() {
                        Ok(path) => path.to_string_lossy().to_string(),
                        Err(error) => {
                            report.skipped += 1;
                            record_unindexed(&transaction, source_id, &source, &path, "ruta ilegible")?;
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
                    let started_insert = Instant::now();
                    report.values += insert_document(
                        &transaction,
                        source_id,
                        &path,
                        origin_folder(&source, &path),
                        document,
                        &current_hash,
                    )?;
                    report.phases.insert_ms += started_insert.elapsed().as_millis();
                    report.indexed += 1;
                }
                Err(OmegaError::Unsupported(message)) => {
                    report.skipped += 1;
                    record_unindexed(&transaction, source_id, &source, &path, "formato no soportado")?;
                    report.warnings.push(message);
                }
                Err(error) => {
                    // Un archivo ilegible o corrupto no invalida el resto de
                    // una carpeta autorizada. Se conserva un aviso explícito
                    // y nunca se crea evidencia a partir de contenido ausente.
                    report.skipped += 1;
                    record_unindexed(&transaction, source_id, &source, &path, "error al analizar")?;
                    report.warnings.push(format!("{}: {error}", path.display()));
                }
            }
        }
        // Segunda pasada: la carátula de dos columnas de los PDF con capa de
        // texto, ya con el vocabulario completo. Va aquí, después del recorrido
        // entero, y no dentro del bucle, porque el vocabulario de un acervo no
        // depende del orden en que se lean sus archivos: si el PDF se indexara
        // antes que el documento que escribe ese rótulo con dos puntos, dentro
        // del bucle no habría nada contra lo que contrastar y el resultado
        // dependería del orden alfabético de la carpeta.
        let started_cover = Instant::now();
        let cover_values = cover_pass(&transaction, source_id)?;
        report.values += cover_values;
        report.phases.cover_pass_values = cover_values;
        report.phases.cover_pass_ms = started_cover.elapsed().as_millis();

        let started_finalize = Instant::now();
        transaction.execute(
            "DELETE FROM concepts
             WHERE NOT EXISTS (SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id)",
            [],
        )?;
        crate::db::retype_concepts(&transaction)?;
        report_duplicates(&transaction, &mut report)?;
        transaction.execute(
            "UPDATE source_folders SET indexed_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [source_id],
        )?;
        transaction.commit()?;
        report.phases.finalize_ms = started_finalize.elapsed().as_millis();
        report.phases.parse_ms_by_parser = sorted_by_time(parse_ms_by_parser);
        report.phases.documents_by_parser = sorted_by_count(documents_by_parser);
        report.elapsed_ms = started.elapsed().as_millis();
        Ok(report)
    }
}

/// Guarda el resultado del OCR de un archivo, indexado por su contenido.
///
/// `ParsedDocument` ya trae exactamente lo que hace falta para reconstruir la
/// lectura —estado, confianza y los fragmentos con su ubicación—, así que no
/// hay que volver a consultar nada ni cambiar el contrato del parser.
fn remember_ocr(connection: &Connection, hash: &str, document: &ParsedDocument) -> Result<()> {
    let chunks = document
        .chunks
        .iter()
        .map(|chunk| (chunk.location.clone(), chunk.content.clone()))
        .collect::<Vec<_>>();
    let chunks = serde_json::to_string(&chunks).unwrap_or_else(|_| "[]".to_owned());
    connection.execute(
        "INSERT OR REPLACE INTO ocr_cache(content_hash, status, confidence, chunks)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            hash,
            document.ocr_status.as_str(),
            document.ocr_confidence,
            chunks
        ],
    )?;
    Ok(())
}

/// Ordena un reparto de tiempo de mayor a menor: el primer renglón es el que
/// hay que mirar antes de optimizar nada.
fn sorted_by_time(measured: HashMap<String, u128>) -> Vec<(String, u128)> {
    let mut rows = measured.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows
}

fn sorted_by_count(measured: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut rows = measured.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows
}

/// Deja constancia de un archivo descubierto que el índice no pudo aceptar.
///
/// Un conteo por carpeta o por formato que ignore estos archivos no es un
/// conteo del acervo: es un conteo de lo que se logró leer, presentado como si
/// fuera lo que hay. Guardar la ruta, su carpeta y su extensión —nunca su
/// contenido— permite que la respuesta declare cuántos documentos del alcance
/// quedaron fuera, en vez de excluirlos en silencio.
fn record_unindexed(
    connection: &Connection,
    source_id: i64,
    source: &Path,
    path: &Path,
    reason: &str,
) -> Result<()> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_lowercase();
    connection.execute(
        "INSERT OR REPLACE INTO unindexed_documents(source_id, path, origin, extension, reason)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            source_id,
            path.to_string_lossy().as_ref(),
            origin_folder(source, path),
            extension,
            reason,
        ],
    )?;
    Ok(())
}

/// Detecta y reporta los grupos de documentos con contenido idéntico.
///
/// No borra ni fusiona nada: la política es conservar cada copia y decir que
/// lo es. Cambiar un conteo por su cuenta sería alterar el hecho que el acervo
/// contiene, y ningún índice puede saber si dos copias son un error de archivo
/// o dos entregas reales.
///
/// El alcance es todo el índice, no sólo la fuente recién indexada: un archivo
/// duplicado entre dos carpetas autorizadas distintas sigue siendo el mismo
/// contenido.
fn report_duplicates(connection: &Connection, report: &mut IndexReport) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT content_hash, COUNT(*), GROUP_CONCAT(path, ' | ')
         FROM documents
         GROUP BY content_hash
         HAVING COUNT(*) > 1
         ORDER BY content_hash",
    )?;
    let groups = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(1)? as usize,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (copies, paths) in &groups {
        report.duplicate_groups += 1;
        report.duplicate_documents += copies;
        report.warnings.push(format!(
            "{copies} documentos con contenido idéntico byte a byte: {paths}. Se conservan todos y ninguna suma ni conteo cambia por ello; una respuesta que se apoye en ellos lo advertirá."
        ));
    }
    Ok(())
}

/// Contabiliza el estado OCR de un archivo recién analizado. Cada estado
/// tiene su propio contador: «no hay motor», «el motor falló» y «el motor leyó
/// mal» son hechos distintos y colapsarlos oculta cuál de los tres pasó.
fn count_ocr_state(report: &mut IndexReport, status: OcrStatus) {
    match status {
        OcrStatus::Pending => report.ocr_pending += 1,
        OcrStatus::LowConfidence => report.ocr_low_confidence += 1,
        OcrStatus::Failed => report.ocr_failed += 1,
        OcrStatus::Unavailable => report.ocr_unavailable += 1,
        OcrStatus::NotRequired | OcrStatus::Complete => {}
    }
}

fn has_indexable_evidence(document: &ParsedDocument) -> bool {
    !document.text.trim().is_empty()
        || document
            .chunks
            .iter()
            .any(|chunk| !chunk.content.trim().is_empty())
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

/// El SHA-256 llega ya calculado desde el bucle. Antes se leía el archivo
/// entero y se resumía **dos veces** por documento —una para detectar cambios
/// y otra aquí— sobre exactamente los mismos bytes. Es una fracción pequeña
/// del total (≈1 s de 746 s en la medición de esta ronda), pero es trabajo
/// duplicado sin ninguna razón.
fn insert_document(
    connection: &Connection,
    source_id: i64,
    path: &Path,
    origin: String,
    parsed: ParsedDocument,
    hash: &str,
) -> Result<usize> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
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
            size_bytes, modified_unix, ocr_status, ocr_confidence,
            declared_format_mismatch
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            parsed.declared_format_mismatch,
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

    // El OCR reconoce por zona de página, no por línea lógica completa: un
    // rótulo de un documento escaneado puede ser un fragmento («y (EMP» en vez
    // de «EMP», visto en `operaciones/02797_orden_mantenimiento.pdf`) que por
    // canonicalización colisiona con el campo real de cientos de documentos de
    // texto plano. Si ese fragmento se indexa primero, `display_name` queda
    // fijado al fragmento para siempre y ningún documento limpio puede
    // corregirlo. Un rótulo de texto (no OCR) sí puede reemplazar uno de OCR;
    // lo inverso nunca ocurre, y un rótulo de texto ya asentado tampoco se
    // vuelve a tocar.
    let record_is_ocr = matches!(
        parsed.ocr_status,
        OcrStatus::Complete | OcrStatus::LowConfidence
    );
    insert_records(connection, document_id, parsed.records, record_is_ocr, 0)
}

/// Escribe los campos de un documento: concepto, alias, valor y, si el valor
/// nombra una entidad, su mención.
///
/// Está separado de `insert_document` porque la segunda pasada de carátula
/// añade campos a un documento **ya insertado** y tiene que escribirlos por
/// exactamente el mismo camino. `first_ordinal` continúa la numeración de los
/// identificadores de evidencia (`v-{documento}-{ordinal}`), que son únicos por
/// documento.
fn insert_records(
    connection: &Connection,
    document_id: i64,
    records: Vec<ParsedRecord>,
    record_is_ocr: bool,
    first_ordinal: usize,
) -> Result<usize> {
    let mut inserted = 0;
    for (offset, record) in records.into_iter().enumerate() {
        let ordinal = first_ordinal + offset;
        let typed = classify_value(&record.label, &record.value);
        let key = canonical_key(&record.label);
        connection.execute(
            "INSERT INTO concepts(canonical_key, display_name, value_type, display_name_from_ocr)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(canonical_key) DO UPDATE SET
                display_name = excluded.display_name,
                value_type = excluded.value_type,
                display_name_from_ocr = excluded.display_name_from_ocr
             WHERE concepts.display_name_from_ocr = 1 AND excluded.display_name_from_ocr = 0",
            params![key, record.label, typed.kind.as_str(), record_is_ocr as i64],
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
                normalized_value, literal_value, identifier_canonical, numeric_value, currency, date_value, location, excerpt
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                evidence_id,
                document_id,
                concept_id,
                typed.kind.as_str(),
                typed.text_value,
                typed.normalized_value,
                typed.literal_value,
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

/// Vocabulario de rótulos leído del índice.
///
/// Un rótulo entra aquí porque **algún documento del acervo ya lo escribió**
/// como nombre de campo —con dos puntos, o como encabezado de una tabla o de
/// una hoja—. No hay ninguna lista en el código: si el acervo no conoce un
/// nombre, el motor tampoco.
///
/// Tres filtros, y los tres los decide el índice, no un juicio escrito aquí:
///
///  1. **Nada que sólo haya visto el OCR.** El índice ya distingue el rótulo
///     leído de texto del leído por reconocimiento óptico —y ya se niega a que
///     el segundo pise al primero— porque el OCR parte frases en sitios
///     arbitrarios: «A continuacion se detalla» y «Empresa Grupo Nexo
///     Industral» son conceptos reales de un acervo con documentos escaneados.
///     Un rótulo que ningún documento de texto escribió no es vocabulario.
///  2. **Nada que haya escrito un solo documento.** Dos es el mínimo que hace
///     que un nombre sea compartido y no el accidente de un archivo.
///  3. **Forma de rótulo**: la primera palabra empieza por letra y ninguna de
///     las siguientes empieza por mayúscula. Es la convención tipográfica de un
///     nombre de campo en español («Cantidad recibida», «Importe estimado del
///     contrato») y lo que separa un rótulo de un rótulo pegado a su propio
///     valor («Empresa: Grupo Nexo Industrial») o de un título con folio
///     («Acta de Junta Directiva Junio 2023»), que son la misma carátula de dos
///     columnas leída mal en otro formato.
struct IndexedLabels {
    labels: HashSet<String>,
}

impl IndexedLabels {
    fn load(connection: &Connection) -> Result<Self> {
        let mut statement = connection.prepare(
            "SELECT c.display_name
             FROM concepts c
             WHERE c.display_name_from_ocr = 0
               AND (
                    SELECT COUNT(DISTINCT v.document_id)
                    FROM extracted_values v
                    WHERE v.concept_id = c.id
               ) >= 2",
        )?;
        let labels = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|name| looks_like_a_label(name))
            .map(|name| normalize_exact(&name))
            .filter(|name| !name.is_empty())
            .collect::<HashSet<_>>();
        Ok(Self { labels })
    }

    fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }
}

impl LabelVocabulary for IndexedLabels {
    fn knows(&self, candidate: &str) -> bool {
        self.labels.contains(&normalize_exact(candidate))
    }
}

/// ¿Tiene esta cadena la forma tipográfica de un nombre de campo?
fn looks_like_a_label(name: &str) -> bool {
    let mut words = name.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    first.chars().next().is_some_and(char::is_alphabetic)
        && words.all(|word| !word.chars().next().is_some_and(char::is_uppercase))
}

/// Segunda pasada sobre la carátula de los PDF con capa de texto.
///
/// Relee los fragmentos que la primera pasada ya guardó —no vuelve a abrir
/// ningún archivo— y les aplica el vocabulario completo del acervo. Los campos
/// que la primera pasada ya extrajo por sus dos puntos se reconocen por su
/// ubicación y no se repiten.
fn cover_pass(connection: &Connection, source_id: i64) -> Result<usize> {
    let vocabulary = IndexedLabels::load(connection)?;
    if vocabulary.is_empty() {
        return Ok(0);
    }
    let documents = connection
        .prepare("SELECT id FROM documents WHERE source_id = ?1 AND parser = 'pdf_text'")?
        .query_map([source_id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    let mut inserted = 0;
    for document_id in documents {
        let pages = connection
            .prepare(
                "SELECT location, content FROM chunks WHERE document_id = ?1 ORDER BY ordinal",
            )?
            .query_map([document_id], |row| {
                Ok(ParsedChunk {
                    location: row.get(0)?,
                    content: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if pages.is_empty() {
            continue;
        }
        let already: HashSet<String> = connection
            .prepare("SELECT location FROM extracted_values WHERE document_id = ?1")?
            .query_map([document_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        let records = records_from_pdf_pages(&pages, &vocabulary)
            .into_iter()
            .filter(|record| !already.contains(&record.location))
            .collect::<Vec<_>>();
        if records.is_empty() {
            continue;
        }
        // Un PDF con capa de texto no pasó por OCR: sus rótulos son de texto y
        // pueden fijar el nombre visible de un concepto igual que los de la
        // primera pasada.
        inserted += insert_records(connection, document_id, records, false, already.len())?;
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

    /// Qué entra en el vocabulario de rótulos y qué no, por su sola forma.
    /// Los otros dos filtros —nada de OCR, nada que escriba un solo
    /// documento— los aplica la consulta y se comprueban sobre el índice real.
    #[test]
    fn only_a_label_shaped_name_counts_as_vocabulary() {
        // Nombres de campo: primera palabra en mayúscula, el resto no.
        assert!(looks_like_a_label("Responsable"));
        assert!(looks_like_a_label("Cantidad recibida"));
        assert!(looks_like_a_label("Importe estimado del contrato"));
        assert!(looks_like_a_label("Planta/Sucursal"));
        assert!(looks_like_a_label("Cumplimiento de meta (%)"));
        assert!(looks_like_a_label("SKU relacionado"));

        // Un rótulo pegado a su propio valor, que es la misma carátula de dos
        // columnas leída mal en otro formato.
        assert!(!looks_like_a_label("Empresa: Grupo Nexo Industrial"));
        assert!(!looks_like_a_label("Grupo Nexo Industrial, S.A. de C.V."));
        // Un título con folio no es un campo.
        assert!(!looks_like_a_label("Acta de Junta Directiva Junio 2023"));
        assert!(!looks_like_a_label("Expediente Jurídico EXP-2025-00042"));
        // Ni un número suelto que quedó como nombre.
        assert!(!looks_like_a_label("17"));
        assert!(!looks_like_a_label(""));
    }

    #[test]
    fn chunk_locations_remain_navigable() {
        let result = chunks("uno\ndos\ntres");
        assert_eq!(result[0].0, "líneas 1-3");
    }

    #[test]
    fn empty_parses_are_not_indexable_evidence() {
        assert!(!has_indexable_evidence(&ParsedDocument {
            text: String::new(),
            chunks: vec![],
            records: vec![],
            parser: "empty".into(),
            ocr_status: OcrStatus::Failed,
            ocr_confidence: None,
            warnings: vec![],
            declared_format_mismatch: None,
        }));
    }
}
