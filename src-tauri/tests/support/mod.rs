//! Fixtures genéricos para las pruebas de integración: facturas, contratos,
//! inventario, proyectos y ventas. Ningún fixture describe un giro concreto
//! ni depende de un corpus real del repositorio.
#![allow(dead_code)]

use std::{fs, io::Write, path::Path};

use zip::{ZipWriter, write::SimpleFileOptions};

/// DOCX con párrafos sueltos.
pub fn write_docx(path: &Path, paragraphs: &[&str]) {
    let body = paragraphs
        .iter()
        .map(|text| format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", escape(text)))
        .collect::<String>();
    write_docx_body(path, &body);
}

/// DOCX con una tabla de tantas columnas como traiga cada fila. La primera
/// fila es el encabezado.
pub fn write_docx_table(path: &Path, rows: &[Vec<&str>]) {
    write_docx_tables(path, std::slice::from_ref(&rows.to_vec()));
}

/// DOCX con varias tablas seguidas.
pub fn write_docx_tables(path: &Path, tables: &[Vec<Vec<&str>>]) {
    let body = tables.iter().map(|rows| render_table(rows)).collect::<String>();
    write_docx_body(path, &body);
}

fn render_table(rows: &[Vec<&str>]) -> String {
    let table = rows
        .iter()
        .map(|row| {
            let cells = row
                .iter()
                .map(|cell| {
                    format!(
                        "<w:tc><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc>",
                        escape(cell)
                    )
                })
                .collect::<String>();
            format!("<w:tr>{cells}</w:tr>")
        })
        .collect::<String>();
    format!("<w:tbl>{table}</w:tbl>")
}

fn write_docx_body(path: &Path, body: &str) {
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (name, contents) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_owned()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_owned()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#)),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// Celda de una hoja de cálculo escrita a mano: valor, formato de número
/// opcional y fórmula opcional con o sin resultado en caché.
pub struct SheetCell {
    pub value: Option<String>,
    pub inline_text: Option<String>,
    pub number_format: Option<String>,
    pub formula: Option<String>,
}

impl SheetCell {
    pub fn text(value: &str) -> Self {
        Self {
            value: None,
            inline_text: Some(value.to_owned()),
            number_format: None,
            formula: None,
        }
    }

    pub fn number(value: &str) -> Self {
        Self {
            value: Some(value.to_owned()),
            inline_text: None,
            number_format: None,
            formula: None,
        }
    }

    /// Número con el formato de celda con el que lo escribiría una hoja real
    /// (`0.00%`, `"$"#,##0.00`, …).
    pub fn formatted(value: &str, number_format: &str) -> Self {
        Self {
            value: Some(value.to_owned()),
            inline_text: None,
            number_format: Some(number_format.to_owned()),
            formula: None,
        }
    }

    /// Fórmula con resultado en caché.
    pub fn formula(expression: &str, cached: &str) -> Self {
        Self {
            value: Some(cached.to_owned()),
            inline_text: None,
            number_format: None,
            formula: Some(expression.to_owned()),
        }
    }

    /// Fórmula sin resultado en caché: la hoja nunca se recalculó.
    pub fn formula_without_value(expression: &str) -> Self {
        Self {
            value: None,
            inline_text: None,
            number_format: None,
            formula: Some(expression.to_owned()),
        }
    }

    pub fn empty() -> Self {
        Self {
            value: None,
            inline_text: None,
            number_format: None,
            formula: None,
        }
    }
}

pub fn column_name(mut index: usize) -> String {
    let mut name = String::new();
    loop {
        name.insert(0, (b'A' + (index % 26) as u8) as char);
        if index < 26 {
            return name;
        }
        index = index / 26 - 1;
    }
}

/// XLSX de texto plano: filas de celdas literales.
pub fn write_xlsx(path: &Path, rows: &[Vec<&str>]) {
    let grid = rows
        .iter()
        .map(|row| row.iter().map(|cell| SheetCell::text(cell)).collect())
        .collect::<Vec<Vec<_>>>();
    write_xlsx_cells(path, "Data", &grid);
}

/// XLSX completo con `styles.xml`: conserva formatos de celda y fórmulas.
pub fn write_xlsx_cells(path: &Path, sheet: &str, rows: &[Vec<SheetCell>]) {
    write_workbook(path, sheet, rows, false);
}

/// Igual, pero el libro pide recálculo completo al abrirse: es la marca con
/// la que Excel declara que los valores en caché de sus fórmulas ya no son de
/// fiar.
pub fn write_xlsx_needing_recalculation(path: &Path, sheet: &str, rows: &[Vec<SheetCell>]) {
    write_workbook(path, sheet, rows, true);
}

fn write_workbook(path: &Path, sheet: &str, rows: &[Vec<SheetCell>], full_calc_on_load: bool) {
    let mut formats: Vec<String> = Vec::new();
    for row in rows {
        for cell in row {
            if let Some(format) = &cell.number_format
                && !formats.contains(format)
            {
                formats.push(format.clone());
            }
        }
    }
    let num_fmts = formats
        .iter()
        .enumerate()
        .map(|(index, code)| {
            format!(
                r#"<numFmt numFmtId="{}" formatCode="{}"/>"#,
                164 + index,
                escape(code)
            )
        })
        .collect::<String>();
    // xf 0 es el estilo por defecto; el resto mapea 1:1 con `formats`.
    let cell_xfs = formats
        .iter()
        .enumerate()
        .map(|(index, _)| {
            format!(
                r#"<xf numFmtId="{}" fontId="0" fillId="0" borderId="0" applyNumberFormat="1"/>"#,
                164 + index
            )
        })
        .collect::<String>();
    let styles = format!(
        r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="{}">{num_fmts}</numFmts><fonts count="1"><font/></fonts><fills count="1"><fill/></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="{}"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/>{cell_xfs}</cellXfs></styleSheet>"#,
        formats.len(),
        formats.len() + 1
    );

    let mut sheet_data = String::new();
    for (row_index, row) in rows.iter().enumerate() {
        sheet_data.push_str(&format!(r#"<row r="{}">"#, row_index + 1));
        for (column, cell) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(column), row_index + 1);
            let style = cell
                .number_format
                .as_ref()
                .and_then(|format| formats.iter().position(|item| item == format))
                .map(|index| format!(r#" s="{}""#, index + 1))
                .unwrap_or_default();
            let formula = cell
                .formula
                .as_ref()
                .map(|expression| format!("<f>{}</f>", escape(expression)))
                .unwrap_or_default();
            if let Some(text) = &cell.inline_text {
                sheet_data.push_str(&format!(
                    r#"<c r="{reference}"{style} t="inlineStr">{formula}<is><t>{}</t></is></c>"#,
                    escape(text)
                ));
            } else if let Some(value) = &cell.value {
                sheet_data
                    .push_str(&format!(r#"<c r="{reference}"{style}>{formula}<v>{value}</v></c>"#));
            } else if !formula.is_empty() {
                sheet_data.push_str(&format!(r#"<c r="{reference}"{style}>{formula}</c>"#));
            }
        }
        sheet_data.push_str("</row>");
    }

    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (name, contents) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_owned()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_owned()),
        ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#.to_owned()),
        ("xl/workbook.xml", format!(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="{}" sheetId="1" r:id="rId1"/></sheets>{}</workbook>"#, escape(sheet), if full_calc_on_load { r#"<calcPr calcId="0" fullCalcOnLoad="1"/>"# } else { r#"<calcPr calcId="191029"/>"# })),
        ("xl/styles.xml", styles),
        ("xl/worksheets/sheet1.xml", format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{sheet_data}</sheetData></worksheet>"#)),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// PDF con capa de texto nativa: no necesita OCR.
pub fn write_pdf(path: &Path, lines: &[&str]) {
    let mut stream = String::from("BT /F1 12 Tf 72 720 Td 16 TL");
    for line in lines {
        stream.push_str(&format!(" ({}) Tj T*", line.replace('(', "").replace(')', "")));
    }
    stream.push_str(" ET");
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    let mut output = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = output.len();
    output.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets.iter().skip(1) {
        output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    output.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    fs::write(path, output).unwrap();
}

/// PDF sin capa de texto: obliga a pasar por OCR.
pub fn write_scanned_pdf(path: &Path) {
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_owned(),
        "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
    ];
    let mut output = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = output.len();
    output.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets.iter().skip(1) {
        output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    output.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    fs::write(path, output).unwrap();
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
