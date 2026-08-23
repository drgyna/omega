use std::{fs::File, io::Read, path::Path};

use calamine::{Data, Reader, open_workbook_auto};
use regex::Regex;
use zip::ZipArchive;

use crate::{
    error::{OmegaError, Result},
    model::{OcrStatus, ParsedChunk, ParsedDocument, ParsedRecord},
    ocr,
};

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "txt", "md", "csv", "xlsx", "xls", "docx", "doc", "pdf", "png", "jpg", "jpeg", "tiff", "tif",
    "bmp", "webp", "heic",
];

pub trait DocumentParser: Send + Sync {
    fn parse(&self, path: &Path) -> Result<ParsedDocument>;
}

#[derive(Default)]
pub struct LocalDocumentParser;

impl DocumentParser for LocalDocumentParser {
    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_lowercase();
        match extension.as_str() {
            "txt" | "md" => parse_plain_text(path, &extension),
            "csv" => parse_csv(path),
            "xlsx" | "xls" => parse_workbook(path, &extension),
            "docx" => parse_docx(path),
            "doc" => Err(OmegaError::Unsupported(format!(
                "{} es DOC binario y requiere conversión local a DOCX o PDF para indexarse con evidencia verificable",
                path.display()
            ))),
            "pdf" => parse_pdf(path),
            "png" | "jpg" | "jpeg" | "tiff" | "tif" | "bmp" | "webp" | "heic" => {
                parse_ocr(path, &format!("image_{extension}"))
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
        .from_path(path)
        .map_err(|error| OmegaError::Parse(error.to_string()))?;
    let headers = reader
        .headers()
        .map_err(|error| OmegaError::Parse(error.to_string()))?
        .iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut text = headers.join(" | ");
    let mut records = Vec::new();
    let mut chunks = Vec::new();
    for (row_index, row) in reader.records().enumerate() {
        let row = row.map_err(|error| OmegaError::Parse(error.to_string()))?;
        text.push('\n');
        text.push_str(&row.iter().collect::<Vec<_>>().join(" | "));
        for (column, value) in row.iter().enumerate() {
            if value.trim().is_empty() {
                continue;
            }
            let label = headers
                .get(column)
                .cloned()
                .unwrap_or_else(|| format!("columna {}", column + 1));
            let location = format!(
                "fila {}, celda {}{} ({label})",
                row_index + 2,
                column_name(column),
                row_index + 2
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
    let mut text = String::new();
    let mut records = Vec::new();
    let mut chunks = Vec::new();
    for sheet_name in workbook.sheet_names().to_owned() {
        let range = workbook
            .worksheet_range(&sheet_name)
            .map_err(|error| OmegaError::Parse(error.to_string()))?;
        let headers = range
            .rows()
            .next()
            .unwrap_or_default()
            .iter()
            .map(cell_text)
            .collect::<Vec<_>>();
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("Hoja: {sheet_name}\n"));
        for (row_index, row) in range.rows().enumerate() {
            let cells = row.iter().map(cell_text).collect::<Vec<_>>();
            text.push_str(&cells.join(" | "));
            text.push('\n');
            if row_index == 0 {
                continue;
            }
            for (column, value) in cells.iter().enumerate() {
                if value.trim().is_empty() {
                    continue;
                }
                let label = headers
                    .get(column)
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("columna {}", column + 1));
                let cell = format!("{}{}", column_name(column), row_index + 1);
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
    Ok(document)
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

    let paragraph = Regex::new(r"</w:p>").expect("valid regex");
    let tabs = Regex::new(r"<w:tab[^>]*/>").expect("valid regex");
    let tags = Regex::new(r"<[^>]+>").expect("valid regex");
    let xml = paragraph.replace_all(&xml, "\n");
    let xml = tabs.replace_all(&xml, "\t");
    let text = decode_xml_entities(&tags.replace_all(&xml, ""));
    let records = records_from_text(&text, "párrafo");
    let chunks = text
        .lines()
        .enumerate()
        .filter_map(|(index, paragraph)| {
            let content = paragraph.trim();
            (!content.is_empty()).then(|| ParsedChunk {
                location: format!("párrafo {}", index + 1),
                content: content.to_owned(),
            })
        })
        .collect();
    let mut document = plain_document(text, "docx".into(), records);
    document.chunks = chunks;
    Ok(document)
}

fn parse_pdf(path: &Path) -> Result<ParsedDocument> {
    let text = pdf_extract::extract_text(path).unwrap_or_default();
    if text.trim().chars().count() >= 20 {
        let records = records_from_pdf_pages(&text);
        let mut document = plain_document(text.clone(), "pdf_text".into(), records);
        document.chunks = page_chunks(&text);
        return Ok(document);
    }
    parse_ocr(path, "pdf_ocr")
}

fn records_from_pdf_pages(text: &str) -> Vec<ParsedRecord> {
    text.split('\u{c}')
        .enumerate()
        .flat_map(|(page, content)| {
            content
                .lines()
                .enumerate()
                .filter_map(move |(line, value)| {
                    let trimmed = value.trim();
                    let (label, raw) = trimmed.split_once(':')?;
                    let label = label.trim();
                    let raw = raw.trim();
                    (!label.is_empty() && !raw.is_empty() && label.len() <= 120).then(|| {
                        ParsedRecord {
                            label: label.to_owned(),
                            value: raw.to_owned(),
                            location: format!("página {}, línea {}", page + 1, line + 1),
                            excerpt: trimmed.to_owned(),
                        }
                    })
                })
        })
        .collect()
}

fn parse_ocr(path: &Path, parser: &str) -> Result<ParsedDocument> {
    let outcome = ocr::recognize(path);
    let text = outcome
        .chunks
        .iter()
        .map(|chunk| chunk.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ParsedDocument {
        records: records_from_text(&text, "texto OCR"),
        text,
        chunks: outcome.chunks,
        parser: parser.into(),
        ocr_status: outcome.status,
        ocr_confidence: outcome.confidence,
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
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('-') {
                return None;
            }
            let (label, value) = trimmed.split_once(':')?;
            let label = label.trim();
            let value = value.trim();
            if label.is_empty() || value.is_empty() || label.len() > 120 {
                return None;
            }
            Some(ParsedRecord {
                label: label.to_owned(),
                value: value.to_owned(),
                location: format!("{location_prefix} {}", index + 1),
                excerpt: trimmed.to_owned(),
            })
        })
        .collect()
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
}
