//! Ronda 5 · punto B — la carátula de dos columnas de un PDF con capa de texto.
//!
//! Al extraer el texto de un PDF, la fila «Área | Dirección, reportes
//! ejecutivos» pierde la rejilla que la separaba y llega como «Área Dirección,
//! reportes ejecutivos»: un solo espacio y ningún separador. Antes se resolvía
//! con una lista de dieciocho nombres de campo escrita en `parser.rs` —la única
//! parte del motor con vocabulario de un corpus concreto—, y por eso un
//! `pdf_text` aportaba 1,4 campos por documento frente a los 25-34 de los demás
//! formatos.
//!
//! Ahora el corte se contrasta contra el vocabulario de rótulos que el propio
//! acervo ya conoce. Estas pruebas fijan las dos mitades: que la carátula se
//! extrae, y —sobre todo— que el texto libre NO se convierte en campos.

use omega_core::{ParsedChunk, parser::{LabelVocabulary, NoLabelVocabulary, records_from_pdf_pages}};

/// Vocabulario de prueba: exactamente los rótulos que se le den, comparados
/// sin distinguir mayúsculas ni acentos, igual que el del índice.
struct Vocabulary(Vec<String>);

impl Vocabulary {
    fn of(names: &[&str]) -> Self {
        Self(names.iter().map(|name| normalize(name)).collect())
    }
}

impl LabelVocabulary for Vocabulary {
    fn knows(&self, candidate: &str) -> bool {
        self.0.contains(&normalize(candidate))
    }
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|character| match character {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' => 'u',
            other => other,
        })
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn page(content: &str) -> Vec<ParsedChunk> {
    vec![ParsedChunk {
        location: "página 1".into(),
        content: content.to_owned(),
    }]
}

fn field<'a>(records: &'a [omega_core::ParsedRecord], label: &str) -> Option<&'a str> {
    records
        .iter()
        .find(|record| record.label == label)
        .map(|record| record.value.as_str())
}

const COVER: &str = "\
Empresa Grupo Nexo Industrial, S.A. de C.V.

Área Dirección, reportes ejecutivos, planeación

Planta/Sucursal PLT-09 — Planta Saltillo Norte

Responsable Jimena Quintero Beltrán (EMP-2018-0460)

Moneda USD
";

#[test]
fn the_two_column_cover_is_extracted_against_the_vocabulary_of_the_acervo() {
    let vocabulary = Vocabulary::of(&["Empresa", "Área", "Planta/Sucursal", "Responsable", "Moneda"]);
    let records = records_from_pdf_pages(&page(COVER), &vocabulary);

    assert_eq!(
        field(&records, "Empresa"),
        Some("Grupo Nexo Industrial, S.A. de C.V.")
    );
    assert_eq!(
        field(&records, "Área"),
        Some("Dirección, reportes ejecutivos, planeación")
    );
    assert_eq!(
        field(&records, "Planta/Sucursal"),
        Some("PLT-09 — Planta Saltillo Norte")
    );
    assert_eq!(field(&records, "Moneda"), Some("USD"));
    assert_eq!(records.len(), 5, "{records:?}");
    // La ubicación señala la línea real de la página que el usuario puede abrir.
    assert_eq!(records[0].location, "página 1, línea 1");
    assert_eq!(records[1].location, "página 1, línea 3");
}

/// Con el vocabulario vacío el parser se comporta exactamente como antes de
/// que existiera: sólo reconoce el par que el propio documento escribió con
/// dos puntos. Es la garantía de que su independencia respecto del índice no
/// se rompe para ningún caso.
#[test]
fn without_a_vocabulary_only_the_written_separator_counts() {
    let records = records_from_pdf_pages(&page(COVER), &NoLabelVocabulary);
    assert!(records.is_empty(), "{records:?}");

    let with_colon = "Folio: AC-1\nEstado: Abierto\nResponsable: Ada\n";
    let records = records_from_pdf_pages(&page(with_colon), &NoLabelVocabulary);
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(field(&records, "Folio"), Some("AC-1"));
}

/// Una línea de texto libre que por casualidad empieza por el nombre de un
/// campo no es un par: está sola, no forma bloque, y no se indexa.
#[test]
fn an_isolated_prose_line_that_starts_with_a_field_name_is_not_a_field() {
    let vocabulary = Vocabulary::of(&["Empresa", "Documento", "Área", "Moneda"]);
    let prose = "\
Grupo Nexo Industrial, S.A. de C.V. — Documento interno

Por medio del presente documento se deja constancia de lo siguiente:

Documento generado como parte del control documental de la compañía.

La información aquí contenida es confidencial.
";
    let records = records_from_pdf_pages(&page(prose), &vocabulary);
    assert!(records.is_empty(), "{records:?}");
}

/// Una línea que es, entera, un rótulo conocido —un encabezado de tabla que
/// quedó en su propia línea— no se parte: hacerlo inventaría un valor donde
/// sólo hay un nombre de campo.
#[test]
fn a_line_that_is_only_a_known_label_is_not_split() {
    let vocabulary = Vocabulary::of(&[
        "Importe estimado del contrato",
        "Importe estimado",
        "Folio",
        "Estado",
        "Responsable",
    ]);
    // El encabezado suelto va al final: una línea que no es par corta el
    // bloque, así que las tres filas reales van seguidas.
    let lines = "\
Folio AC-1

Estado Abierto

Responsable Ada Serrano

Importe estimado del contrato
";
    let records = records_from_pdf_pages(&page(lines), &vocabulary);
    assert_eq!(field(&records, "Folio"), Some("AC-1"));
    assert_eq!(field(&records, "Estado"), Some("Abierto"));
    assert!(
        field(&records, "Importe estimado").is_none(),
        "un encabezado suelto no puede convertirse en «Importe estimado = del contrato»: {records:?}"
    );
}

/// Gana el rótulo más largo, y el corte cae siempre en un espacio: una grafía
/// corrupta más larga del mismo rótulo no puede partir el valor por la mitad.
#[test]
fn the_longest_label_wins_and_the_cut_falls_on_a_space() {
    let vocabulary = Vocabulary::of(&[
        "Cantidad",
        "Cantidad recibida",
        "Planta/Sucursal",
        "Planta/Sucursal PLT",
        "Moneda",
    ]);
    let lines = "\
Cantidad recibida 252.24 piezas

Planta/Sucursal PLT-09 — Planta Saltillo Norte

Moneda USD
";
    let records = records_from_pdf_pages(&page(lines), &vocabulary);
    assert_eq!(field(&records, "Cantidad recibida"), Some("252.24 piezas"));
    assert_eq!(
        field(&records, "Planta/Sucursal"),
        Some("PLT-09 — Planta Saltillo Norte"),
        "la grafía corrupta «Planta/Sucursal PLT» no termina en espacio y no puede cortar aquí: {records:?}"
    );
}

/// Dos líneas seguidas no bastan: una carátula de dos columnas siempre tiene
/// varias filas, y el mínimo es lo que separa un bloque de una coincidencia.
#[test]
fn two_lines_are_not_a_cover_block() {
    let vocabulary = Vocabulary::of(&["Empresa", "Moneda"]);
    let two = "Empresa Grupo Nexo Industrial\n\nMoneda USD\n";
    assert!(records_from_pdf_pages(&page(two), &vocabulary).is_empty());

    let three = "Empresa Grupo Nexo Industrial\n\nMoneda USD\n\nEmpresa Otra Compañía\n";
    assert_eq!(records_from_pdf_pages(&page(three), &vocabulary).len(), 3);
}
