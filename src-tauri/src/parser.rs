use std::{fs::File, io::Read, path::Path, sync::Arc, sync::LazyLock};

use calamine::{Data, Reader, open_workbook_auto};
use regex::Regex;
use zip::ZipArchive;

use crate::{
    error::{OmegaError, Result},
    model::{OcrStatus, ParsedChunk, ParsedDocument, ParsedRecord},
    ocr::{OcrEngine, SystemOcr},
    workbook::WorkbookSemantics,
};

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "xlsx", "xls", "docx", "doc", "pdf", "png", "jpg", "jpeg", "tiff", "tif",
    "bmp", "webp", "heic",
];

pub trait DocumentParser: Send + Sync {
    fn parse(&self, path: &Path) -> Result<ParsedDocument>;
}

/// Parser local de todos los formatos admitidos. El motor OCR es un
/// parámetro explícito: en producción es el del sistema, y sustituirlo es el
/// punto de extensión documentado en `docs/OCR.md`. Ningún proveedor puede
/// ser de red.
pub struct LocalDocumentParser {
    ocr: Arc<dyn OcrEngine>,
}

impl Default for LocalDocumentParser {
    fn default() -> Self {
        Self {
            ocr: Arc::new(SystemOcr),
        }
    }
}

impl LocalDocumentParser {
    pub fn with_ocr(ocr: Arc<dyn OcrEngine>) -> Self {
        Self { ocr }
    }
}

impl DocumentParser for LocalDocumentParser {
    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let declared = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        // La extensión es una afirmación del nombre del archivo, no un hecho
        // sobre su contenido. Antes de despachar se contrasta con la firma
        // real de los primeros bytes: un `.pdf` que por dentro es texto llano
        // se leía con el parser de PDF, fallaba, y acababa como «documento sin
        // contenido» sin que nadie supiera por qué.
        let disguise = detect_disguised_extension(path, &declared);
        let extension = match &disguise {
            // Cuando el contenido real se puede leer, se lee: negarse a
            // extraerlo sería castigar al usuario por el nombre del archivo.
            // Lo que no se hace es callarlo: la discrepancia viaja con el
            // documento y toda respuesta que lo cite la declara.
            Some(disguise) => disguise.parse_as.clone(),
            None => declared.clone(),
        };
        let parsed = self.parse_as(path, &extension);
        return match (parsed, disguise) {
            (Ok(mut document), Some(disguise)) => {
                document.declared_format_mismatch = Some(disguise.detected.clone());
                document.warnings.push(format!(
                    "{}: la extensión declarada (.{declared}) no corresponde al contenido real del archivo, que es {}. Se leyó como {} y toda respuesta que lo cite lo declara.",
                    path.display(),
                    disguise.detected,
                    disguise.parse_as
                ));
                Ok(document)
            }
            (parsed, _) => parsed,
        };
    }
}

/// Extensión declarada que no corresponde al contenido real.
struct Disguise {
    /// Nombre legible de lo que el contenido resultó ser.
    detected: String,
    /// Extensión con la que conviene leerlo de verdad.
    parse_as: String,
}

/// Firma de bytes esperada para una extensión, cuando la tiene.
///
/// Sólo se declara para formatos cuyo comienzo es obligatorio por
/// especificación: un PDF real siempre lleva `%PDF-` en su cabecera y un
/// DOCX/XLSX real es un ZIP, que siempre empieza por `PK`. Los formatos de
/// texto (`txt`, `md`, `csv`) no tienen firma y por eso nunca se marcan por
/// ausencia: sólo se marcan cuando el contenido lleva la firma **de otro**
/// formato, que sí es una prueba positiva.
fn expected_signature(extension: &str) -> Option<&'static str> {
    match extension {
        "pdf" => Some("pdf"),
        "docx" | "xlsx" => Some("zip"),
        _ => None,
    }
}

/// Formato reconocido por los primeros bytes del archivo, si alguno lo es.
fn signature_of(head: &[u8]) -> Option<&'static str> {
    // Un PDF admite basura antes de la cabecera; los lectores la toleran
    // dentro del primer kilobyte, así que se busca ahí y no sólo en el byte 0.
    if head
        .windows(5)
        .take(1024)
        .any(|window| window == b"%PDF-")
    {
        return Some("pdf");
    }
    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08")
    {
        return Some("zip");
    }
    if head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        return Some("ole");
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("png");
    }
    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    None
}

/// ¿Miente la extensión sobre el contenido?
///
/// Deliberadamente conservador: sólo devuelve algo ante una prueba positiva,
/// nunca ante una duda. Un archivo vacío no es un disfraz —es un archivo
/// vacío, y esa ruta ya existe—, y una extensión sin firma obligatoria nunca
/// se marca por no tenerla.
fn detect_disguised_extension(path: &Path, declared: &str) -> Option<Disguise> {
    let mut file = File::open(path).ok()?;
    let mut head = vec![0u8; 4096];
    let read = file.read(&mut head).ok()?;
    head.truncate(read);
    if head.is_empty() {
        return None;
    }
    let found = signature_of(&head);
    if let Some(expected) = expected_signature(declared) {
        if found == Some(expected) {
            return None;
        }
        // El contenido lleva la firma de otro formato conocido: se dice cuál.
        if let Some(found) = found {
            return Some(Disguise {
                detected: describe_signature(found).to_owned(),
                parse_as: extension_for_signature(found).to_owned(),
            });
        }
        // Sin firma alguna y con texto legible dentro: es texto plano
        // disfrazado. Si no es legible como texto tampoco, no se afirma nada
        // sobre qué es: sólo que no es lo que dice ser.
        return Some(if std::str::from_utf8(&head).is_ok() {
            Disguise {
                detected: "texto plano".to_owned(),
                parse_as: "txt".to_owned(),
            }
        } else {
            Disguise {
                detected: "contenido binario de formato desconocido".to_owned(),
                parse_as: declared.to_owned(),
            }
        });
    }
    // Extensión de texto con la firma de un formato binario dentro.
    if matches!(declared, "txt" | "md" | "csv") {
        if let Some(found) = found {
            return Some(Disguise {
                detected: describe_signature(found).to_owned(),
                parse_as: extension_for_signature(found).to_owned(),
            });
        }
    }
    None
}

fn describe_signature(signature: &str) -> &'static str {
    match signature {
        "pdf" => "un PDF",
        "zip" => "un archivo ZIP (formato OOXML: DOCX/XLSX)",
        "ole" => "un documento binario de Office antiguo (OLE)",
        "png" => "una imagen PNG",
        "jpg" => "una imagen JPEG",
        _ => "otro formato",
    }
}

fn extension_for_signature(signature: &str) -> &'static str {
    match signature {
        "pdf" => "pdf",
        "zip" => "docx",
        "png" => "png",
        "jpg" => "jpg",
        // Un OLE antiguo sigue sin poder leerse localmente: se despacha a la
        // rama que ya lo declara sin inventar contenido.
        _ => "doc",
    }
}

impl LocalDocumentParser {
    fn parse_as(&self, path: &Path, extension: &str) -> Result<ParsedDocument> {
        match extension {
            "txt" | "md" => parse_plain_text(path, &extension),
            "csv" => parse_csv(path),
            "xlsx" | "xls" => parse_workbook(path, &extension),
            "docx" => parse_docx(path),
            "doc" => Err(OmegaError::Unsupported(format!(
                "{} es DOC binario y requiere conversión local a DOCX o PDF para indexarse con evidencia verificable",
                path.display()
            ))),
            "pdf" => parse_pdf(path, self.ocr.as_ref()),
            "png" | "jpg" | "jpeg" | "tiff" | "tif" | "bmp" | "webp" | "heic" => {
                parse_ocr(path, &format!("image_{extension}"), self.ocr.as_ref())
            }
            _ => Err(OmegaError::Unsupported(path.display().to_string())),
        }
    }
}

fn plain_document(text: String, parser: String, records: Vec<ParsedRecord>) -> ParsedDocument {
    ParsedDocument {
        text,
        chunks: vec![],
        records,
        parser,
        ocr_status: OcrStatus::NotRequired,
        ocr_confidence: None,
        warnings: vec![],
        declared_format_mismatch: None,
    }
}

fn parse_plain_text(path: &Path, parser: &str) -> Result<ParsedDocument> {
    let text = std::fs::read_to_string(path)?;
    let records = records_from_text(&text, "línea");
    Ok(plain_document(text, parser.to_owned(), records))
}

fn parse_csv(path: &Path) -> Result<ParsedDocument> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        // La primera fila no siempre es el encabezado: estos archivos suelen
        // abrir con una carátula «campo,valor» y poner la tabla más abajo. Se
        // leen todas las filas como datos y el encabezado se elige después.
        .has_headers(false)
        .from_path(path)
        .map_err(|error| OmegaError::Parse(error.to_string()))?;
    let rows = reader
        .records()
        .map(|row| {
            row.map(|row| row.iter().map(ToOwned::to_owned).collect::<Vec<String>>())
                .map_err(|error| OmegaError::Parse(error.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    // Mismo criterio que `parse_workbook`: el encabezado es la fila con más
    // celdas con contenido entre las primeras 25. Tomar rígidamente la fila 1
    // convertía el valor de la carátula en nombre de campo, y con él quedaban
    // mal etiquetados todos los valores del archivo.
    //
    // Y antes de elegir encabezado hay que preguntarse si el archivo tiene
    // uno: un archivo que es sólo carátula no tiene tabla que encabezar, y
    // ahí cualquier fila «gana» el máximo por empate. Ver `is_cover_only`.
    let cover_only = is_cover_only(&rows);
    let mut header_index = 0usize;
    let mut header_score = 0usize;
    for (index, row) in rows.iter().take(25).enumerate() {
        let score = row.iter().filter(|value| !value.trim().is_empty()).count();
        if score > header_score {
            header_index = index;
            header_score = score;
        }
    }
    let headers = rows.get(header_index).cloned().unwrap_or_default();

    let mut text = String::new();
    let mut records = Vec::new();
    let mut chunks = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&row.join(" | "));
        if cover_only || row_index < header_index {
            // Carátula: «campo,valor» en dos columnas, igual que en una hoja.
            if let Some(record) = header_pair_record(row, "", row_index, None, false) {
                let location = format!("fila {}, celda B{} ({})", row_index + 1, row_index + 1, record.label);
                chunks.push(ParsedChunk {
                    location: location.clone(),
                    content: record.excerpt.clone(),
                });
                records.push(ParsedRecord { location, ..record });
            }
            continue;
        }
        if row_index == header_index {
            continue;
        }
        for (column, value) in row.iter().enumerate() {
            if value.trim().is_empty() {
                continue;
            }
            let label = headers
                .get(column)
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| format!("columna {}", column + 1));
            let location = format!(
                "fila {}, celda {}{} ({label})",
                row_index + 1,
                column_name(column),
                row_index + 1
            );
            let excerpt = format!("{label}: {}", value.trim());
            records.push(ParsedRecord {
                label,
                value: value.trim().to_owned(),
                location: location.clone(),
                excerpt: excerpt.clone(),
            });
            chunks.push(ParsedChunk {
                location,
                content: excerpt,
            });
        }
    }
    let mut document = plain_document(text, "csv".into(), records);
    document.chunks = chunks;
    Ok(document)
}

fn parse_workbook(path: &Path, extension: &str) -> Result<ParsedDocument> {
    let mut workbook =
        open_workbook_auto(path).map_err(|error| OmegaError::Parse(error.to_string()))?;
    // Formatos de celda y fórmulas: la mitad del hecho que el valor solo no
    // contiene. `None` para un `.xls` binario, que no es un paquete OOXML.
    let semantics = WorkbookSemantics::read(path);
    let stale_cache = semantics.as_ref().is_some_and(WorkbookSemantics::stale_cache);
    let mut text = String::new();
    let mut records = Vec::new();
    let mut chunks = Vec::new();
    let mut warnings = Vec::new();
    if stale_cache {
        warnings.push(format!(
            "{}: el libro pide recálculo completo al abrirse, así que los resultados en caché de sus fórmulas no se indexan como valores",
            path.display()
        ));
    }
    for sheet_name in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&sheet_name)
            .map_err(|error| OmegaError::Parse(error.to_string()))?;
        // Las hojas reales suelen tener un título o logotipo antes de la
        // tabla. Se elige como encabezado la fila de texto con más celdas no
        // vacías, en vez de asumir rígidamente la primera fila.
        // Antes hay que preguntarse si la hoja tiene tabla: una hoja que es
        // sólo carátula no tiene encabezado que elegir (ver `is_cover_only`).
        let mut header_index = 0usize;
        let mut header_score = 0usize;
        for (index, row) in range.rows().take(25).enumerate() {
            let score = row
                .iter()
                .map(cell_text)
                .filter(|value| !value.trim().is_empty())
                .count();
            if score > header_score {
                header_index = index;
                header_score = score;
            }
        }
        let headers = range
            .rows()
            .nth(header_index)
            .unwrap_or_default()
            .iter()
            .map(cell_text)
            .collect::<Vec<_>>();
        // Las celdas se resuelven una sola vez: el formato de celda es parte
        // del valor (`0.15` con formato de porcentaje es `15%`, y `1250` con
        // formato de moneda es un importe, no un número suelto), y la forma
        // completa de la hoja es lo que decide si hay tabla o sólo carátula.
        let sheet_rows = range
            .rows()
            .enumerate()
            .map(|(row_index, row)| {
                row.iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        let raw = cell_text(cell);
                        let reference = format!("{}{}", column_name(column), row_index + 1);
                        match semantics.as_ref() {
                            Some(semantics) if !raw.trim().is_empty() => crate::workbook::render(
                                &raw,
                                &semantics.semantics(&sheet_name, &reference),
                            ),
                            _ => raw,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let cover_only = is_cover_only(&sheet_rows);
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("Hoja: {sheet_name}\n"));
        for (row_index, cells) in sheet_rows.iter().enumerate() {
            text.push_str(&cells.join(" | "));
            text.push('\n');
            if cover_only || row_index <= header_index {
                // La zona anterior al encabezado no es tabular, pero suele
                // llevar la carátula de la hoja escrita como pares
                // «campo | valor» en dos columnas — la misma forma que las
                // tablas de dos columnas de un DOCX, que sí se indexan.
                // Descartarla entera perdía justo donde muchas plantillas
                // escriben el importe, el folio y el responsable.
                if let Some(record) = header_pair_record(
                    cells,
                    &sheet_name,
                    row_index,
                    semantics.as_ref(),
                    stale_cache,
                ) {
                    chunks.push(ParsedChunk {
                        location: record.location.clone(),
                        content: record.excerpt.clone(),
                    });
                    records.push(record);
                }
                continue;
            }
            // Filas parciales de totales/notas no se mezclan con registros
            // tabulares: un registro necesita una clave en la primera columna.
            if cells.first().is_none_or(|value| value.trim().is_empty()) {
                continue;
            }
            for (column, value) in cells.iter().enumerate() {
                let cell = format!("{}{}", column_name(column), row_index + 1);
                // Una fórmula no es un valor. Si su resultado falta, o si el
                // libro declara que el que trae en caché ya no corresponde,
                // la celda no puede convertirse en una cifra que nadie
                // escribió. Se omite como valor y se dice por qué; la fila
                // sigue en el texto del documento, que es lo que el archivo
                // sí contiene.
                if let Some(formula) =
                    semantics.as_ref().and_then(|s| s.formula(&sheet_name, &cell))
                {
                    if value.trim().is_empty() {
                        warnings.push(format!(
                            "{}: hoja {sheet_name}, celda {cell} tiene la fórmula {} sin resultado en caché; no se indexa un valor que la hoja no escribió",
                            path.display(),
                            formula.expression
                        ));
                        continue;
                    }
                    if stale_cache {
                        warnings.push(format!(
                            "{}: hoja {sheet_name}, celda {cell} trae en caché un resultado de {} que el libro marca para recálculo; no se indexa como valor",
                            path.display(),
                            formula.expression
                        ));
                        continue;
                    }
                }
                if value.trim().is_empty() {
                    continue;
                }
                let label = headers
                    .get(column)
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("columna {}", column + 1));
                let location = format!("hoja {sheet_name}, celda {cell} ({label})");
                let excerpt = format!("{label}: {}", value.trim());
                records.push(ParsedRecord {
                    label,
                    value: value.trim().to_owned(),
                    location: location.clone(),
                    excerpt: excerpt.clone(),
                });
                chunks.push(ParsedChunk {
                    location,
                    content: excerpt,
                });
            }
        }
    }
    let mut document = plain_document(text, extension.to_owned(), records);
    document.chunks = chunks;
    document.warnings = warnings;
    Ok(document)
}

/// ¿El archivo entero es carátula, sin ninguna tabla debajo?
///
/// El encabezado se elige como «la fila con más celdas con contenido». Ese
/// criterio funciona mientras haya una tabla real: al tener más columnas que
/// la carátula, la tabla gana el máximo. Cuando el archivo es sólo carátula
/// todas las filas miden lo mismo, gana la primera por ser la primera, y sus
/// dos celdas se convierten en los nombres de columna de todo lo demás: el
/// valor de la primera fila pasa a nombrar los campos, y los rótulos reales
/// de las filas de abajo pasan a ser valores suyos.
///
/// La forma «dos columnas» es ambigua por sí sola: una tabla real de dos
/// columnas (`Folio | Margen`, y debajo los folios y sus márgenes) tiene
/// exactamente la misma silueta que una carátula. Lo que las separa no es la
/// silueta sino de qué están hechas sus celdas, y eso se decide sólo por la
/// forma de la fila, nunca por un vocabulario fijo:
///
/// 1. **Toda fila con contenido es un par en A y B, o un título suelto en A.**
///    Una sola fila de tres columnas ya es una tabla y el archivo no es
///    carátula.
/// 2. **La primera columna entera se lee como una columna de rótulos.** En una
///    tabla real la primera columna lleva datos —folios, claves, fechas—, y un
///    dato no tiene forma de rótulo. Basta un `VTA-001` para descartar.
/// 3. **La primera fila no es un encabezado de verdad.** Si sus dos celdas
///    tienen forma de rótulo, hay que decidir si describe lo que viene abajo o
///    si es el primer campo de la carátula. Lo decide la columna de valores:
///    una columna de datos es homogénea —todo números, o todo texto—, mientras
///    que la de una carátula mezcla un nombre, una fecha, un importe y un
///    folio. Con valores homogéneos se respeta la lectura tabular.
///
/// Los tres criterios son conservadores: ante la duda se conserva la lectura
/// de tabla, que es la que el archivo tenía antes.
fn is_cover_only(rows: &[Vec<String>]) -> bool {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for row in rows {
        let filled = row
            .iter()
            .enumerate()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(column, _)| column)
            .collect::<Vec<_>>();
        match filled.as_slice() {
            // Fila vacía, o título suelto en A: ninguno de los dos es un par,
            // y ninguno de los dos desmiente que el archivo sea carátula.
            [] | [0] => continue,
            [0, 1] => pairs.push((row[0].trim(), row[1].trim())),
            // Cualquier otra forma —tres columnas, o una fila que empieza en
            // B— es tabular.
            _ => return false,
        }
    }
    // Con un solo par no hay archivo que juzgar: se conserva la lectura
    // anterior.
    let [(_, first_value), rest @ ..] = pairs.as_slice() else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if !pairs.iter().all(|(label, _)| looks_like_field_label(label)) {
        return false;
    }
    if looks_like_field_label(first_value) {
        let first_shape = value_shape(rest[0].1);
        if rest.iter().all(|(_, value)| value_shape(value) == first_shape) {
            return false;
        }
    }
    true
}

/// ¿La celda tiene forma de rótulo de campo, y no de dato?
///
/// Sólo se mira la forma, nunca el significado: un rótulo es corto, de pocas
/// palabras, sin dígitos —un folio, una fecha o un importe los llevan— y sin
/// la puntuación con que termina una frase. Ninguna lista de palabras
/// interviene aquí, para que el criterio valga igual en cualquier giro.
fn looks_like_field_label(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().count() <= 40
        && !value.chars().any(|character| character.is_ascii_digit())
        && value.split_whitespace().count() <= 4
        && !value.ends_with('.')
        && !value.ends_with(':')
}

/// Forma gruesa de un valor, para saber si una columna es homogénea. No
/// interpreta el dato: sólo distingue una cifra de un texto, y un texto que
/// lleva dígitos —un folio, una fecha escrita, una matrícula— de uno que no.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueShape {
    Number,
    TextWithDigits,
    Text,
}

fn value_shape(value: &str) -> ValueShape {
    static NUMBER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[-+]?[$€£]?\s?\d[\d.,\s]*\s?(%|[A-Z]{2,3})?$").expect("valid regex"));
    let value = value.trim();
    if NUMBER.is_match(value) {
        return ValueShape::Number;
    }
    if value.chars().any(|character| character.is_ascii_digit()) {
        return ValueShape::TextWithDigits;
    }
    ValueShape::Text
}

/// Par «campo | valor» de la carátula de una hoja: exactamente dos celdas con
/// contenido, la etiqueta en la primera columna y el valor en la segunda.
/// Cualquier otra forma —un título suelto, una fila de tres columnas— no es un
/// par y no se indexa, para no inventar campos donde sólo hay decoración.
fn header_pair_record(
    cells: &[String],
    sheet_name: &str,
    row_index: usize,
    semantics: Option<&WorkbookSemantics>,
    stale_cache: bool,
) -> Option<ParsedRecord> {
    let filled = cells
        .iter()
        .enumerate()
        .filter(|(_, value)| !value.trim().is_empty())
        .collect::<Vec<_>>();
    let [(0, label), (1, value)] = filled.as_slice() else {
        return None;
    };
    let label = label.trim();
    if label.is_empty() || label.len() > 120 {
        return None;
    }
    let cell = format!("{}{}", column_name(1), row_index + 1);
    // Una fórmula cuyo resultado no es fiable no se convierte en valor, con el
    // mismo criterio que ya se aplica en la zona tabular.
    if semantics
        .and_then(|item| item.formula(sheet_name, &cell))
        .is_some()
        && stale_cache
    {
        return None;
    }
    let value = value.trim();
    Some(ParsedRecord {
        label: label.to_owned(),
        value: value.to_owned(),
        location: format!("hoja {sheet_name}, celda {cell} ({label})"),
        excerpt: format!("{label}: {value}"),
    })
}

fn column_name(mut index: usize) -> String {
    let mut name = String::new();
    loop {
        name.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            return name;
        }
        index = index / 26 - 1;
    }
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        value => value.to_string(),
    }
}

fn parse_docx(path: &Path) -> Result<ParsedDocument> {
    let file = File::open(path)?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| OmegaError::Parse(error.to_string()))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|error| OmegaError::Parse(error.to_string()))?
        .read_to_string(&mut xml)?;

    let mut table_records = records_from_docx_tables(&xml);
    let paragraph = Regex::new(r"</w:p>").expect("valid regex");
    let tabs = Regex::new(r"<w:tab[^>]*/>").expect("valid regex");
    let tags = Regex::new(r"<[^>]+>").expect("valid regex");
    let xml = paragraph.replace_all(&xml, "\n");
    let xml = tabs.replace_all(&xml, "\t");
    let text = decode_xml_entities(&tags.replace_all(&xml, ""));
    let mut records = records_from_text(&text, "párrafo");
    records.append(&mut table_records);
    let mut chunks = text
        .lines()
        .enumerate()
        .filter_map(|(index, paragraph)| {
            let content = paragraph.trim();
            (!content.is_empty()).then(|| ParsedChunk {
                location: format!("párrafo {}", index + 1),
                content: content.to_owned(),
            })
        })
        .collect::<Vec<_>>();
    chunks.extend(
        records
            .iter()
            .filter(|record| record.location.starts_with("tabla "))
            .map(|record| ParsedChunk {
                location: record.location.clone(),
                content: record.excerpt.clone(),
            }),
    );
    let mut document = plain_document(text, "docx".into(), records);
    document.chunks = chunks;
    Ok(document)
}

fn parse_pdf(path: &Path, ocr: &dyn OcrEngine) -> Result<ParsedDocument> {
    let text = pdf_extract::extract_text(path).unwrap_or_default();
    if text.trim().chars().count() >= 20 {
        // Las páginas se calculan una sola vez y sirven para las dos cosas:
        // los fragmentos citables y los campos. Así la ubicación de un campo
        // («página 1, línea 6») señala exactamente la línea del fragmento que
        // el usuario puede abrir, y la segunda pasada del indexador —que sólo
        // tiene los fragmentos ya guardados— reproduce la misma ubicación sin
        // volver a abrir el PDF.
        let pages = page_chunks(&text);
        let records = records_from_pdf_pages(&pages, &NoLabelVocabulary);
        let mut document = plain_document(text.clone(), "pdf_text".into(), records);
        document.chunks = pages;
        return Ok(document);
    }
    parse_ocr(path, "pdf_ocr", ocr)
}

/// Vocabulario de rótulos que el propio acervo ya conoce.
///
/// Existe porque la carátula de dos columnas de un PDF pierde su separador al
/// extraer el texto: la fila «Área | Dirección, reportes ejecutivos» llega como
/// «Área Dirección, reportes ejecutivos», con un solo espacio y sin dos puntos,
/// y no hay forma de saber dónde acaba el rótulo mirando sólo esa línea. Antes
/// se resolvía con una lista de dieciocho nombres de campo escrita en este
/// mismo archivo —la única parte del motor con vocabulario de un corpus
/// concreto—, que extraía 1,4 campos por documento frente a los 25-34 de los
/// demás formatos.
///
/// La sustituye el vocabulario real: los rótulos que **otros documentos del
/// acervo ya escribieron** con dos puntos o como encabezado. El parser no
/// consulta la base: recibe el vocabulario como parámetro, y con el
/// vocabulario vacío se comporta exactamente igual que antes de existir.
pub trait LabelVocabulary {
    /// ¿Es esta cadena, exactamente, un rótulo que el acervo ya usa?
    ///
    /// La comparación es de quien implementa el rasgo; lo único que este
    /// archivo asume es que responde por la cadena completa y no por un
    /// parecido.
    fn knows(&self, candidate: &str) -> bool;
}

/// Vocabulario que no conoce ningún rótulo.
///
/// Es el que usa el parser por sí solo, y por eso `parse` sigue siendo una
/// función del archivo y de nada más: la independencia respecto de la base de
/// datos no se rompe para ningún caso: el contraste contra el vocabulario lo
/// pide explícitamente quien lo tiene, que es el indexador.
pub struct NoLabelVocabulary;

impl LabelVocabulary for NoLabelVocabulary {
    fn knows(&self, _candidate: &str) -> bool {
        false
    }
}

/// Cuántas líneas seguidas con forma de par hacen falta para creerse una
/// carátula.
///
/// Una tabla de dos columnas tiene siempre varias filas; una línea de texto
/// libre que por casualidad empieza por el nombre de un campo está sola. Tres
/// es el mínimo que distingue un bloque de una coincidencia: sobre el acervo de
/// auditoría descarta 4.172 líneas sueltas —entre ellas el encabezado corrido
/// que llevan los 2.178 PDF— y no pierde ninguna fila de carátula.
const COVER_BLOCK_MINIMUM: usize = 3;

/// Campos de un PDF con capa de texto, página por página.
///
/// Recibe las páginas ya troceadas —las mismas que se guardan como fragmentos
/// citables— para que la ubicación de cada campo apunte a una línea que existe
/// en el fragmento, y para que el indexador pueda repetir la extracción sobre
/// los fragmentos guardados sin volver a leer el archivo.
pub fn records_from_pdf_pages(
    pages: &[ParsedChunk],
    vocabulary: &dyn LabelVocabulary,
) -> Vec<ParsedRecord> {
    let mut records = Vec::new();
    for page in pages {
        let lines = page
            .content
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .collect::<Vec<_>>();
        let pairs = lines
            .iter()
            .map(|(_, line)| split_field_line(line.trim(), vocabulary))
            .collect::<Vec<_>>();
        for (index, pair) in pairs.iter().enumerate() {
            let Some(pair) = pair else { continue };
            // El corte por dos puntos lo escribió el documento y se cree tal
            // cual. El que sale del vocabulario es una lectura del motor, y
            // sólo se acepta dentro de un bloque de líneas que se cortan
            // igual: ahí es una carátula, no una frase que empieza por el
            // nombre de un campo.
            if pair.from_vocabulary && !belongs_to_a_cover_block(&pairs, index) {
                continue;
            }
            let (line, raw) = lines[index];
            if pair.label.chars().count() > 120 {
                continue;
            }
            records.push(ParsedRecord {
                label: pair.label.clone(),
                value: pair.value.clone(),
                location: format!("{}, línea {}", page.location, line + 1),
                excerpt: raw.trim().to_owned(),
            });
        }
    }
    records
}

/// ¿Está esta línea dentro de un bloque de al menos `COVER_BLOCK_MINIMUM`
/// líneas seguidas que se cortan en par etiqueta/valor?
fn belongs_to_a_cover_block(pairs: &[Option<FieldLine>], index: usize) -> bool {
    let mut run = 1;
    for pair in pairs[..index].iter().rev() {
        if pair.is_none() {
            break;
        }
        run += 1;
    }
    for pair in pairs.iter().skip(index + 1) {
        if pair.is_none() {
            break;
        }
        run += 1;
    }
    run >= COVER_BLOCK_MINIMUM
}

/// Par etiqueta/valor leído de una línea, con la procedencia del corte.
struct FieldLine {
    label: String,
    value: String,
    /// El corte salió del vocabulario del acervo, no de un separador escrito
    /// en el propio documento.
    from_vocabulary: bool,
}

fn parse_ocr(path: &Path, parser: &str, ocr: &dyn OcrEngine) -> Result<ParsedDocument> {
    let outcome = ocr.recognize(path);
    let text = outcome
        .chunks
        .iter()
        .map(|chunk| chunk.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // Cada campo hereda la ubicación del fragmento OCR del que salió —página
    // y zona—, no la línea del texto que se reconstruyó al concatenarlos: esa
    // línea no existe en el documento y no sirve para abrir la cita.
    let records = outcome
        .chunks
        .iter()
        .filter_map(|chunk| record_in_line(&chunk.content, chunk.location.clone()))
        .collect();
    Ok(ParsedDocument {
        records,
        text,
        chunks: outcome.chunks,
        parser: parser.into(),
        ocr_status: outcome.status,
        ocr_confidence: outcome.confidence,
        warnings: vec![],
        declared_format_mismatch: None,
    })
}

fn page_chunks(text: &str) -> Vec<ParsedChunk> {
    let pages = text.split('\u{c}').collect::<Vec<_>>();
    let multiple = pages.len() > 1;
    pages
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let content = page.trim();
            (!content.is_empty()).then(|| ParsedChunk {
                location: if multiple {
                    format!("página {}", index + 1)
                } else {
                    "página 1".into()
                },
                content: content.to_owned(),
            })
        })
        .collect()
}

pub fn records_from_text(text: &str, location_prefix: &str) -> Vec<ParsedRecord> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            record_in_line(line, format!("{location_prefix} {}", index + 1))
        })
        .collect()
}

/// Retira el viñetado y el énfasis de Markdown para dejar a la vista el par
/// «Etiqueta: valor» que hay debajo. Sólo quita decoración: si la línea no
/// llevaba ninguna, vuelve igual.
fn undecorate_list_item(line: &str) -> String {
    let without_bullet = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .unwrap_or(line);
    without_bullet.replace("**", "").replace("__", "")
}

/// Campo «Etiqueta: valor» dentro de una línea, ya anclado a la ubicación
/// que le corresponde. La ubicación la decide quien llama porque depende del
/// formato: línea, párrafo o zona de una página escaneada.
fn record_in_line(line: &str, location: String) -> Option<ParsedRecord> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Una lista de Markdown escribe los mismos pares «Etiqueta: valor» que un
    // texto plano, sólo que decorados: «- **Proveedor relacionado:** Indumex».
    // Descartar toda línea que empieza por «-» dejaba esos campos sin indexar
    // por completo. Se retira la decoración y se exige la misma forma de
    // siempre: una regla horizontal («---») no tiene dos puntos y se sigue
    // descartando sola, sin necesidad de una regla aparte.
    let undecorated = undecorate_list_item(trimmed);
    let (label, value) = undecorated.split_once(':')?;
    let label = label.trim().to_owned();
    let value = value.trim().to_owned();
    if label.is_empty() || value.is_empty() || label.len() > 120 {
        return None;
    }
    Some(ParsedRecord {
        label,
        value,
        location,
        excerpt: trimmed.to_owned(),
    })
}

/// Registros de las tablas de un DOCX.
///
/// Una tabla de dos columnas es una lista de pares «campo / valor»: así es
/// como Word representa de verdad un formulario o una carátula. Una tabla de
/// tres columnas o más es una tabla tabular: su primera fila son encabezados
/// y cada fila siguiente aporta un valor por columna.
///
/// Antes sólo se leían las dos primeras celdas de cada fila, con lo que la
/// tercera columna en adelante desaparecía del acervo, y todas las filas del
/// documento se numeraban como si pertenecieran a una única «tabla 1». Ahora
/// cada celda conserva su tabla, su fila, su columna y su encabezado.
///
/// Las tablas anidadas no se separan entre sí: `<w:tbl>` no se puede emparejar
/// con una expresión regular. Sus filas siguen indexándose, atribuidas a la
/// tabla exterior.
fn records_from_docx_tables(xml: &str) -> Vec<ParsedRecord> {
    let tables = Regex::new(r"(?s)<w:tbl\b.*?</w:tbl>").expect("valid DOCX table regex");
    tables
        .find_iter(xml)
        .enumerate()
        .flat_map(|(table_index, table)| {
            records_in_docx_table(table.as_str(), table_index + 1)
        })
        .collect()
}

fn records_in_docx_table(table_xml: &str, table_number: usize) -> Vec<ParsedRecord> {
    let rows = Regex::new(r"(?s)<w:tr\b.*?</w:tr>").expect("valid DOCX row regex");
    let cells = Regex::new(r"(?s)<w:tc\b.*?</w:tc>").expect("valid DOCX cell regex");
    let text_nodes =
        Regex::new(r"(?s)<w:t(?:\s[^>]*)?>(.*?)</w:t>").expect("valid DOCX text regex");

    let grid = rows
        .find_iter(table_xml)
        .map(|row| {
            cells
                .find_iter(row.as_str())
                .map(|cell| {
                    text_nodes
                        .captures_iter(cell.as_str())
                        .map(|capture| decode_xml_entities(&capture[1]))
                        .collect::<Vec<_>>()
                        .join("")
                        .trim()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    if width >= 3 {
        return tabular_docx_records(&grid, table_number);
    }
    grid.iter()
        .enumerate()
        .filter_map(|(row_index, values)| {
            let label = values.first()?;
            let value = values.get(1)?;
            if label.is_empty() || value.is_empty() {
                return None;
            }
            Some(ParsedRecord {
                label: label.clone(),
                value: value.clone(),
                location: format!(
                    "tabla {table_number}, fila {row}, celda B{row} ({label})",
                    row = row_index + 1
                ),
                excerpt: format!("{label}: {value}"),
            })
        })
        .collect()
}

/// Tabla con encabezados: la primera fila los nombra y cada fila siguiente
/// aporta un valor por columna, incluida la primera.
fn tabular_docx_records(grid: &[Vec<String>], table_number: usize) -> Vec<ParsedRecord> {
    let Some(headers) = grid.first() else {
        return Vec::new();
    };
    grid.iter()
        .enumerate()
        .skip(1)
        .flat_map(|(row_index, values)| {
            values
                .iter()
                .enumerate()
                .filter_map(move |(column, value)| {
                    if value.is_empty() {
                        return None;
                    }
                    let label = headers
                        .get(column)
                        .filter(|header| !header.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("columna {}", column + 1));
                    Some(ParsedRecord {
                        label: label.clone(),
                        value: value.clone(),
                        location: format!(
                            "tabla {table_number}, fila {row}, celda {}{row} ({label})",
                            column_name(column),
                            row = row_index + 1
                        ),
                        excerpt: format!("{label}: {value}"),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn split_field_line(line: &str, vocabulary: &dyn LabelVocabulary) -> Option<FieldLine> {
    if let Some((label, value)) = line.split_once(':') {
        let label = label.trim();
        let value = value.trim();
        return (!label.is_empty() && !value.is_empty()).then(|| FieldLine {
            label: label.to_owned(),
            value: value.to_owned(),
            from_vocabulary: false,
        });
    }
    // Al extraer el texto, la fila de dos columnas de una carátula pierde su
    // separador y queda como «Etiqueta Valor». Dónde acaba la etiqueta no se
    // puede saber mirando la línea; sí se puede preguntar al acervo, que ya
    // conoce ese rótulo por los documentos que lo escribieron con dos puntos.
    let (label, value) = split_by_known_label(line, vocabulary)?;
    Some(FieldLine {
        label,
        value,
        from_vocabulary: true,
    })
}

/// Corta la línea por el rótulo conocido más largo que la encabeza.
///
/// Tres condiciones, todas comprobables:
///
///  - El corte cae en un espacio. Sin esto, una grafía corrupta más larga del
///    mismo rótulo («Planta/Sucursal PLT», que el OCR dejó en el índice)
///    partiría el valor por la mitad: «Planta/Sucursal PLT» + «-09 — Planta
///    Saltillo Norte».
///  - Gana el rótulo **más largo**, para que «Cantidad recibida» no se lea como
///    «Cantidad» seguido de «recibida …».
///  - Si la línea entera es un rótulo conocido, no hay par: es un encabezado
///    suelto (una celda de tabla que quedó en su propia línea), y partirlo
///    inventaría un valor donde sólo había un nombre de campo.
fn split_by_known_label(line: &str, vocabulary: &dyn LabelVocabulary) -> Option<(String, String)> {
    if vocabulary.knows(line) {
        return None;
    }
    let mut boundaries = line
        .char_indices()
        .filter(|(_, character)| character.is_whitespace())
        .map(|(at, _)| at)
        .collect::<Vec<_>>();
    boundaries.reverse();
    boundaries.into_iter().find_map(|at| {
        let label = line[..at].trim();
        let value = line[at..].trim();
        (!label.is_empty() && !value.is_empty() && vocabulary.knows(label))
            .then(|| (label.to_owned(), value.to_owned()))
    })
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Contrato para otros proveedores puramente locales. La aplicación incorpora
/// Vision en macOS; no se llama a ningún servicio remoto de OCR.
pub trait OcrProvider: Send + Sync {
    fn recognize(&self, path: &Path) -> Result<ParsedDocument>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cells_have_precise_locations() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(26), "AA");
        let records = records_from_text("Clave: ITEM-7", "línea");
        assert_eq!(records[0].excerpt, "Clave: ITEM-7");
    }

    #[test]
    fn a_csv_that_opens_with_a_two_column_heading_keeps_its_field_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ficha.csv");
        std::fs::write(
            &path,
            "Documento,Inspección QC-1\n             Responsable,Teresa Ibarra\n             SKU relacionado,SKU-00066 — Sensores\n             \n             Fecha,Estatus,Severidad,Descripción breve\n             03 de mayo,Abierto,Alta,Falla reportada\n",
        )
        .unwrap();
        let document = parse_csv(&path).unwrap();
        let by_label = |label: &str| {
            document
                .records
                .iter()
                .find(|record| record.label == label)
                .map(|record| record.value.clone())
        };
        // La carátula conserva sus nombres de campo reales...
        assert_eq!(by_label("Responsable").as_deref(), Some("Teresa Ibarra"));
        assert_eq!(
            by_label("SKU relacionado").as_deref(),
            Some("SKU-00066 — Sensores")
        );
        // ...y el valor de la primera fila no se convierte en etiqueta.
        assert!(by_label("Inspección QC-1").is_none());
        // La tabla de abajo sigue usando su propio encabezado.
        assert_eq!(by_label("Severidad").as_deref(), Some("Alta"));
    }

    #[test]
    fn a_plain_two_column_table_still_uses_its_first_row_as_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tabla.csv");
        std::fs::write(&path, "Code,Description\nCSV-31,compact cell evidence\n").unwrap();
        let document = parse_csv(&path).unwrap();
        let labels = document
            .records
            .iter()
            .map(|record| record.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Code"), "labels: {labels:?}");
        assert!(labels.contains(&"Description"), "labels: {labels:?}");
    }

    #[test]
    fn a_two_column_sheet_heading_is_a_field_and_value_pair() {
        // La carátula de una hoja: etiqueta en A, valor en B.
        let cells = vec![
            "Costo de acción correctiva".to_owned(),
            "$199,911.56 MXN".to_owned(),
        ];
        let record = header_pair_record(&cells, "Datos", 9, None, false).expect("par");
        assert_eq!(record.label, "Costo de acción correctiva");
        assert_eq!(record.value, "$199,911.56 MXN");
        assert_eq!(
            record.location,
            "hoja Datos, celda B10 (Costo de acción correctiva)"
        );

        // Un título suelto no es un par.
        assert!(header_pair_record(&["Acción Correctiva AC-1".to_owned()], "Datos", 0, None, false).is_none());
        // Una fila de tres columnas es tabular, no una carátula.
        let wide = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert!(header_pair_record(&wide, "Datos", 0, None, false).is_none());
    }

    #[test]
    fn the_cover_only_criterion_separates_a_cover_from_a_two_column_table() {
        fn rows(raw: &[&[&str]]) -> Vec<Vec<String>> {
            raw.iter()
                .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
                .collect()
        }

        // Carátula: la columna A son rótulos y la de valores mezcla un
        // nombre, un texto con folio y una fecha.
        assert!(is_cover_only(&rows(&[
            &["Expediente Notarial EN-2024-0311"],
            &["Notaría", "Notaría 14 del Estado de Jalisco"],
            &["Acto", "Compraventa de inmueble"],
            &["Otorgante", "Rosalba Cifuentes Mena (OTG-0044)"],
            &["Fecha", "19 de agosto de 2024"],
        ])));

        // Tabla real: basta un dato en la primera columna para descartar.
        assert!(!is_cover_only(&rows(&[
            &["Folio", "Margen"],
            &["VTA-001", "15%"],
            &["VTA-002", "7.5%"],
        ])));

        // Cuadro vertical con encabezado: la columna A sí son rótulos, pero
        // la columna de valores es homogénea y describe una sola magnitud.
        assert!(!is_cover_only(&rows(&[
            &["Insumo", "Kilos"],
            &["Harina", "250"],
            &["Azúcar", "80"],
            &["Levadura", "12"],
        ])));

        // Una sola fila de tres columnas ya es tabla.
        assert!(!is_cover_only(&rows(&[
            &["Notaría", "Notaría 14 del Estado de Jalisco"],
            &["Acto", "Cantidad", "Importe"],
            &["Compraventa", "2", "18000"],
        ])));

        // Una sola columna no forma pares: no hay carátula que declarar.
        assert!(!is_cover_only(&rows(&[
            &["Guardia en turno"],
            &["Remedios Salgado Ibarra"],
            &["Faustino Lira Bermúdez"],
        ])));
    }

    #[test]
    fn markdown_list_fields_are_indexed_like_plain_text() {
        // Un campo escrito como viñeta de Markdown aporta el mismo par
        // «Etiqueta: valor» que su equivalente en texto plano.
        let records = records_from_text(
            "- **Proveedor relacionado:** Indumex (PROV-2017-0116)",
            "línea",
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].label, "Proveedor relacionado");
        assert_eq!(records[0].value, "Indumex (PROV-2017-0116)");
        // El extracto conserva la línea tal como está escrita en el documento.
        assert_eq!(
            records[0].excerpt,
            "- **Proveedor relacionado:** Indumex (PROV-2017-0116)"
        );

        // Una regla horizontal no es un campo y se sigue descartando.
        assert!(records_from_text("---", "línea").is_empty());
        assert!(records_from_text("- sin dos puntos", "línea").is_empty());
    }

    #[test]
    fn fixture_docx_exposes_table_fields() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../corpus-prueba-formatos-extremos/03_word_docx/001-minuta-de-comité.docx");
        if !path.is_file() {
            return;
        }
        let document = parse_docx(&path).unwrap();
        assert!(document.records.iter().any(|record| {
            record.label == "Folio"
                && record.value == "FMT-26-0051"
                && record.location.starts_with("tabla 1")
        }));
    }

    #[test]
    fn fixture_xlsx_finds_the_table_header_after_its_title() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../corpus-prueba-formatos-extremos/04_excel_xlsx/001-reporte-operativo.xlsx");
        if !path.is_file() {
            return;
        }
        let document = parse_workbook(&path, "xlsx").unwrap();
        assert!(document.records.iter().any(|record| {
            record.label == "Folio"
                && record.value == "XLS-26-0200"
                && record.location.contains("celda A4")
        }));
    }
}
