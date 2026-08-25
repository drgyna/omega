use std::{fs, io::Write, path::Path};

use omega_core::{OmegaEngine, ToolEngine};
use zip::{ZipWriter, write::SimpleFileOptions};

#[test]
fn retrieves_precise_short_evidence_across_local_text_formats() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let group = root.join("group-alpha");
    fs::create_dir(&group).unwrap();
    fs::write(
        group.join("notes.txt"),
        "Banner text\nReference: ITEM-42\nDetail: target words appear here\n",
    )
    .unwrap();
    fs::write(group.join("suffix.txt"), "Reference: ITEM-420\n").unwrap();
    fs::write(
        group.join("table.csv"),
        "Code,Description\nCSV-31,compact cell evidence\n",
    )
    .unwrap();
    write_docx(&group.join("memo.docx"), "Code: DOCX-66");
    write_xlsx(&group.join("sheet.xlsx"), "Code", "XLSX-88");
    write_pdf(
        &group.join("native.pdf"),
        "Code: PDF-37 native text fixture",
    );

    let database = tempfile::NamedTempFile::new().unwrap();
    let engine = OmegaEngine::open(database.path()).unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert_eq!(report.indexed, 6);

    let tools = ToolEngine::new(omega_core::Database::open(database.path()).unwrap());
    let exact = tools.exact_lookup("ITEM-42", 20).unwrap();
    assert_eq!(exact.len(), 1, "un sufijo no es una coincidencia exacta");
    assert_evidence(&exact[0].evidence, "ITEM-42", "exacta");
    assert!(exact[0].evidence.path.ends_with("notes.txt"));

    // La ruta que usa la interfaz (OmegaEngine::ask) debe conservar el modo
    // exacto aunque el identificador esté dentro de una frase natural.
    let natural = engine
        .ask("Encuentra el documento que contiene exactamente el identificador ITEM-42")
        .unwrap();
    assert_eq!(natural.citations.len(), 1);
    assert_evidence(&natural.citations[0], "ITEM-42", "canónica");
    assert!(natural.citations[0].path.ends_with("notes.txt"));

    let quoted_phrase = engine
        .ask("Encuentra el documento que contiene \"target words appear here\"")
        .unwrap();
    // La frase literal sigue procediendo de un solo documento. La segunda cita
    // no es otro resultado: es el identificador de ese mismo registro, que la
    // respuesta nombra y por tanto tiene que respaldar con evidencia.
    assert_eq!(
        quoted_phrase.text,
        "El campo «Detail» de ITEM-42 es target words appear here (notes.txt, línea 3)."
    );
    assert_eq!(quoted_phrase.citations.len(), 2);
    assert!(
        quoted_phrase
            .citations
            .iter()
            .all(|evidence| evidence.path.ends_with("notes.txt"))
    );
    assert_evidence(
        &quoted_phrase.citations[0],
        "target words appear here",
        "exacta",
    );

    let contains_phrase = engine.ask("menciona target words appear here").unwrap();
    assert_eq!(contains_phrase.citations.len(), 1);
    assert_evidence(
        &contains_phrase.citations[0],
        "target words appear here",
        "contiene",
    );

    let missing_natural = engine
        .ask("Encuentra el documento que contiene exactamente el identificador ITEM-999")
        .unwrap();
    assert!(missing_natural.citations.is_empty());

    // El flujo completo de OmegaEngine no puede ampliar un identificador
    // truncado a otro que lo tenga como prefijo o sufijo.
    let incomplete_natural = engine.ask("Encuentra exactamente ITEM-4").unwrap();
    assert!(incomplete_natural.citations.is_empty());

    for (query, expected, location) in [
        ("CSV-31", "CSV-31", "celda A2"),
        ("XLSX-88", "XLSX-88", "celda A2"),
        ("DOCX-66", "DOCX-66", "párrafo"),
        ("PDF-37 native text fixture", "PDF-37", "página 1"),
    ] {
        let hits = tools.search(query, &[], 20).unwrap();
        assert!(!hits.is_empty(), "faltó recuperar {expected}");
        assert_evidence(&hits[0].evidence, expected, "canónica");
        assert!(hits[0].evidence.location.contains(location));
    }

    let filename = tools.exact_lookup("notes.txt", 20).unwrap();
    assert_eq!(filename.len(), 1);
    assert_eq!(
        filename[0].evidence.field.as_deref(),
        Some("nombre de archivo")
    );
    let filename_answer = engine.ask("notes.txt").unwrap();
    assert_eq!(filename_answer.citations.len(), 1);
    assert_eq!(
        filename_answer.citations[0].field.as_deref(),
        Some("nombre de archivo")
    );

    let folder = tools.search("group-alpha", &[], 20).unwrap();
    assert_eq!(folder.len(), 6);
    assert!(
        folder
            .iter()
            .all(|hit| hit.evidence.origin == "group-alpha")
    );

    let no_hits = tools.exact_lookup("ITEM-999", 20).unwrap();
    assert!(no_hits.is_empty());
    let answer = engine.ask("ITEM-999").unwrap();
    assert!(answer.citations.is_empty());
}

#[test]
fn canonical_identifier_search_is_format_tolerant_without_prefix_matches() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, value) in [
        ("dash.txt", "ABC-123"),
        ("underscore.txt", "ABC_123"),
        ("dot_slash.txt", "AB.C/ 123"),
        ("longer.txt", "ABC-1234"),
        ("leading.txt", "XABC-123"),
        ("item.txt", "ITEM-42"),
        ("item_longer.txt", "ITEM-420"),
    ] {
        fs::write(root.join(name), format!("Reference: {value}\n")).unwrap();
    }
    let database = tempfile::NamedTempFile::new().unwrap();
    let engine = OmegaEngine::open(database.path()).unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();

    let answer = engine.ask("Encuentra abc123").unwrap();
    let paths = answer
        .citations
        .iter()
        .map(|evidence| Path::new(&evidence.path).file_name().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 3);
    assert!(paths.iter().any(|name| name == "dash.txt"));
    assert!(paths.iter().any(|name| name == "underscore.txt"));
    assert!(paths.iter().any(|name| name == "dot_slash.txt"));
    assert!(answer.citations.iter().all(|evidence| {
        evidence.match_kind == "canónica"
            && evidence.normalized_value.as_deref() == Some("abc123")
            && evidence.value.is_some()
    }));

    let no_prefix = engine.ask("Encuentra exactamente ABC-12").unwrap();
    assert!(no_prefix.citations.is_empty());
    let item = engine.ask("Encuentra ITEM42").unwrap();
    assert_eq!(item.citations.len(), 1);
    assert!(item.citations[0].path.ends_with("item.txt"));

    let prefix = engine.ask("empieza con ABC12").unwrap();
    assert_eq!(prefix.citations.len(), 4);
    assert!(
        prefix
            .citations
            .iter()
            .all(|evidence| evidence.match_kind == "prefijo")
    );
}

#[test]
fn reports_the_real_structured_total_independently_from_visual_pagination() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    // El nombre del campo y el valor son datos neutros del fixture, no una
    // categoría de producción. Superar 100 comprueba que ask no confunde su
    // antiguo límite de resultados con el total recuperado.
    for number in 0..125 {
        fs::write(
            root.join(format!("record-{number:03}.txt")),
            "Lifecycle: Ready\n",
        )
        .unwrap();
    }
    let database = tempfile::NamedTempFile::new().unwrap();
    let engine = OmegaEngine::open(database.path()).unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();

    let answer = engine.ask("Lifecycle Ready").unwrap();
    assert_eq!(answer.citations.len(), 125);
    // El texto sintetizado sigue reportando el total real recuperado, no el
    // tamaño de página con el que la interfaz muestra las citas.
    assert_eq!(
        answer.text,
        "Encontré 125 valores de «Lifecycle», todos «Ready»."
    );
    assert!(answer.citations.iter().all(|evidence| {
        evidence.field.as_deref() == Some("Lifecycle")
            && evidence.value.as_deref() == Some("Ready")
            && evidence.match_kind == "campo"
    }));

    let absent_value = engine.ask("Lifecycle Missing").unwrap();
    assert!(
        absent_value.citations.is_empty(),
        "un campo explícito con otro valor no puede caer a búsqueda parcial"
    );
}

#[test]
fn reindexing_replaces_changed_files_and_reports_unreadable_ones() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let text = root.join("mutable.txt");
    fs::write(&text, "Reference: RECORD-1\n").unwrap();
    fs::write(root.join("legacy.doc"), b"not a supported local document").unwrap();
    fs::write(root.join("broken.xlsx"), b"not a workbook").unwrap();

    let database = tempfile::NamedTempFile::new().unwrap();
    let engine = OmegaEngine::open(database.path()).unwrap();
    let source = engine.authorize_source(root).unwrap();
    let first = engine.index_source(source).unwrap();
    assert_eq!(first.indexed, 1);
    assert_eq!(first.skipped, 2);
    assert_eq!(first.warnings.len(), 2);

    fs::write(&text, "Reference: RECORD-2\n").unwrap();
    let second = engine.index_source(source).unwrap();
    assert_eq!(second.indexed, 1);
    assert_eq!(second.modified, 1);
    assert!(engine.ask("RECORD-1").unwrap().citations.is_empty());
    assert_eq!(engine.ask("RECORD2").unwrap().citations.len(), 1);

    fs::remove_file(&text).unwrap();
    let third = engine.index_source(source).unwrap();
    assert_eq!(third.indexed, 0);
    assert!(engine.ask("RECORD2").unwrap().citations.is_empty());
}

/// La verificación OCR usa archivos externos: OMEGA_OCR_FIXTURE apunta a una
/// imagen o PDF escaneado y OMEGA_OCR_QUERY al literal que debe recuperarse.
/// Se omite si el entorno no aporta ese fixture, sin fingir que el OCR pasó.
#[test]
fn retrieves_ocr_fixture_when_explicitly_configured() {
    let Some(path) = std::env::var_os("OMEGA_OCR_FIXTURE") else {
        eprintln!("OCR no verificado: define OMEGA_OCR_FIXTURE y OMEGA_OCR_QUERY");
        return;
    };
    let query = std::env::var("OMEGA_OCR_QUERY").expect("define OMEGA_OCR_QUERY");
    let fixture_path = Path::new(&path);
    let root = fixture_path.parent().expect("fixture sin carpeta");
    let database = tempfile::NamedTempFile::new().unwrap();
    let engine = OmegaEngine::open(database.path()).unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();
    let hits = engine.search(&query).unwrap();
    assert!(!hits.is_empty(), "el OCR no produjo evidencia recuperable");
    assert!(
        hits.iter()
            .any(|hit| Path::new(&hit.evidence.path) == fixture_path)
    );
    assert!(
        hits.iter()
            .any(|hit| hit.evidence.location.contains("página"))
    );
}

fn assert_evidence(evidence: &omega_core::Evidence, value: &str, kind: &str) {
    assert_eq!(evidence.match_kind, kind);
    assert!(evidence.excerpt.contains(value));
    assert!(evidence.excerpt.chars().count() <= 220);
    assert!(!evidence.location.is_empty());
}

fn write_docx(path: &Path, text: &str) {
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    zip.start_file("[Content_Types].xml", options).unwrap();
    write!(zip, r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
    zip.start_file("_rels/.rels", options).unwrap();
    write!(zip, r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#).unwrap();
    zip.start_file("word/document.xml", options).unwrap();
    write!(zip, r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#).unwrap();
    zip.finish().unwrap();
}

fn write_xlsx(path: &Path, header: &str, value: &str) {
    let file = fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (name, contents) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_owned()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_owned()),
        ("xl/_rels/workbook.xml.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_owned()),
        ("xl/workbook.xml", r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_owned()),
        ("xl/worksheets/sheet1.xml", format!(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>{header}</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>{value}</t></is></c></row></sheetData></worksheet>"#)),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn write_pdf(path: &Path, text: &str) {
    let stream = format!("BT /F1 18 Tf 72 720 Td ({text}) Tj ET");
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
