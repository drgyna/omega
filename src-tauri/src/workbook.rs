//! Semántica de celda de un libro OOXML (`.xlsx`).
//!
//! Una hoja de cálculo no guarda «15%» ni «$1,250.00 MXN»: guarda `0.15` y
//! `1250`, y aparte un formato de celda que dice cómo hay que leerlos. Leer
//! sólo el número perdía esa mitad del hecho y convertía un porcentaje en un
//! número suelto y un importe en una cifra sin moneda.
//!
//! Y una fórmula tampoco es un valor: el resultado que el archivo trae en
//! caché puede faltar, o el propio libro puede declarar que ya no corresponde
//! a sus fórmulas. En ninguno de los dos casos se puede publicar una cifra
//! como si alguien la hubiera escrito.
//!
//! Este módulo sólo lee el archivo. No conoce ningún campo ni giro de negocio.

use std::{collections::HashMap, fs::File, io::Read, path::Path, sync::LazyLock};

use regex::Regex;
use zip::ZipArchive;

/// Lo que el formato de celda dice sobre cómo leer el número que la celda
/// guarda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberSemantics {
    /// El número es una proporción y se muestra multiplicado por cien.
    Percentage,
    /// El número es dinero. Sólo hay moneda cuando el formato escribe un
    /// código ISO: un símbolo suelto no autoriza a inventar cuál es.
    Money(Option<String>),
    /// Sin semántica añadida: el número es el número.
    Plain,
}

/// Fórmula de una celda y si su resultado en caché es utilizable.
#[derive(Debug, Clone)]
pub struct CellFormula {
    pub expression: String,
}

#[derive(Debug, Default)]
pub struct SheetSemantics {
    formats: HashMap<String, NumberSemantics>,
    formulas: HashMap<String, CellFormula>,
}

#[derive(Debug, Default)]
pub struct WorkbookSemantics {
    sheets: HashMap<String, SheetSemantics>,
    /// El libro pide recálculo completo al abrirse: es la marca con la que
    /// Excel declara que los resultados en caché de sus fórmulas ya no
    /// corresponden a sus fórmulas.
    stale_cache: bool,
}

impl WorkbookSemantics {
    /// Lee la semántica de un `.xlsx`. Devuelve `None` cuando el archivo no es
    /// un paquete OOXML (por ejemplo un `.xls` binario): en ese caso no hay
    /// formato de celda que conservar y el resto del parser sigue igual.
    pub fn read(path: &Path) -> Option<Self> {
        let mut archive = ZipArchive::new(File::open(path).ok()?).ok()?;
        let workbook = part(&mut archive, "xl/workbook.xml")?;
        let relations = part(&mut archive, "xl/_rels/workbook.xml.rels").unwrap_or_default();
        let styles = part(&mut archive, "xl/styles.xml").unwrap_or_default();
        let cell_formats = style_formats(&styles);

        let targets = relationship_targets(&relations);
        let mut sheets = HashMap::new();
        for (name, relation_id) in workbook_sheets(&workbook) {
            let Some(target) = targets.get(&relation_id) else {
                continue;
            };
            let part_name = format!("xl/{}", target.trim_start_matches('/'));
            let Some(xml) = part(&mut archive, &part_name) else {
                continue;
            };
            sheets.insert(name, sheet_semantics(&xml, &cell_formats));
        }
        Some(Self {
            sheets,
            stale_cache: needs_full_recalculation(&workbook),
        })
    }

    pub fn stale_cache(&self) -> bool {
        self.stale_cache
    }

    pub fn semantics(&self, sheet: &str, reference: &str) -> NumberSemantics {
        self.sheets
            .get(sheet)
            .and_then(|sheet| sheet.formats.get(reference))
            .cloned()
            .unwrap_or(NumberSemantics::Plain)
    }

    pub fn formula(&self, sheet: &str, reference: &str) -> Option<&CellFormula> {
        self.sheets
            .get(sheet)
            .and_then(|sheet| sheet.formulas.get(reference))
    }
}

fn part(archive: &mut ZipArchive<File>, name: &str) -> Option<String> {
    let mut contents = String::new();
    archive
        .by_name(name)
        .ok()?
        .read_to_string(&mut contents)
        .ok()?;
    Some(contents)
}

fn workbook_sheets(xml: &str) -> Vec<(String, String)> {
    static SHEET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<sheet\b[^>]*/?>"#).expect("valid sheet regex"));
    SHEET
        .find_iter(xml)
        .filter_map(|tag| {
            let name = attribute(tag.as_str(), "name")?;
            let relation = attribute(tag.as_str(), "r:id")
                .or_else(|| attribute(tag.as_str(), "id"))?;
            Some((decode(&name), relation))
        })
        .collect()
}

fn relationship_targets(xml: &str) -> HashMap<String, String> {
    static RELATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<Relationship\b[^>]*/?>"#).expect("valid rels regex"));
    RELATION
        .find_iter(xml)
        .filter_map(|tag| {
            Some((
                attribute(tag.as_str(), "Id")?,
                attribute(tag.as_str(), "Target")?,
            ))
        })
        .collect()
}

fn needs_full_recalculation(workbook_xml: &str) -> bool {
    static CALC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<calcPr\b[^>]*/?>"#).expect("valid calcPr regex"));
    CALC.find_iter(workbook_xml).any(|tag| {
        let flag = |name: &str| attribute(tag.as_str(), name).unwrap_or_default();
        matches!(flag("fullCalcOnLoad").as_str(), "1" | "true")
            || matches!(flag("calcCompleted").as_str(), "0" | "false")
    })
}

/// `cellXfs` en orden: el índice del estilo de una celda apunta a esta lista.
fn style_formats(styles_xml: &str) -> Vec<NumberSemantics> {
    static NUM_FMT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<numFmt\b[^>]*/?>"#).expect("valid numFmt regex"));
    static CELL_XFS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<cellXfs\b.*?</cellXfs>").expect("valid cellXfs regex")
    });
    static XF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<xf\b[^>]*/?>"#).expect("valid xf regex"));

    let custom = NUM_FMT
        .find_iter(styles_xml)
        .filter_map(|tag| {
            Some((
                attribute(tag.as_str(), "numFmtId")?,
                decode(&attribute(tag.as_str(), "formatCode")?),
            ))
        })
        .collect::<HashMap<_, _>>();

    let Some(block) = CELL_XFS.find(styles_xml) else {
        return Vec::new();
    };
    XF.find_iter(block.as_str())
        .map(|tag| {
            let id = attribute(tag.as_str(), "numFmtId").unwrap_or_else(|| "0".into());
            match custom.get(&id) {
                Some(code) => semantics_of_format(code),
                None => builtin_semantics(&id),
            }
        })
        .collect()
}

/// Formatos numéricos que OOXML define sin escribirlos en `styles.xml`.
fn builtin_semantics(id: &str) -> NumberSemantics {
    match id {
        // 0%, 0.00%
        "9" | "10" => NumberSemantics::Percentage,
        // Moneda y contabilidad: el símbolo depende de la configuración
        // regional de quien creó el archivo, así que no hay código ISO que
        // conservar y no se puede inventar uno.
        "5" | "6" | "7" | "8" | "37" | "38" | "39" | "40" | "41" | "42" | "43" | "44" => {
            NumberSemantics::Money(None)
        }
        _ => NumberSemantics::Plain,
    }
}

/// Traduce un código de formato de celda a lo que significa.
pub fn semantics_of_format(code: &str) -> NumberSemantics {
    static ISO_IN_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#""[^"]*?\b([A-Z]{3})\b[^"]*?""#).expect("valid ISO literal regex")
    });
    static ISO_IN_BRACKET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[\$([A-Za-z]{3})[^\]]*\]").expect("valid bracket currency regex")
    });

    // Un `%` escapado o entrecomillado es texto, no un porcentaje.
    if has_unquoted(code, '%') {
        return NumberSemantics::Percentage;
    }
    if let Some(found) = ISO_IN_BRACKET.captures(code) {
        return NumberSemantics::Money(Some(found[1].to_uppercase()));
    }
    if let Some(found) = ISO_IN_LITERAL.captures(code) {
        return NumberSemantics::Money(Some(found[1].to_owned()));
    }
    // Un símbolo de moneda sin código: es dinero, pero de moneda desconocida.
    if code.contains('$')
        || code.contains('€')
        || code.contains('£')
        || code.contains('¥')
        || code.contains("[$")
    {
        return NumberSemantics::Money(None);
    }
    NumberSemantics::Plain
}

/// Busca un carácter fuera de comillas y fuera de un escape `\`.
fn has_unquoted(code: &str, needle: char) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for character in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' | '_' => escaped = true,
            '"' => quoted = !quoted,
            other if other == needle && !quoted => return true,
            _ => {}
        }
    }
    false
}

fn sheet_semantics(xml: &str, cell_formats: &[NumberSemantics]) -> SheetSemantics {
    static CELL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?s)<c\b[^>]*?(?:/>|>.*?</c>)").expect("valid cell regex")
    });
    static FORMULA: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<f\b[^>]*>(.*?)</f>").expect("valid formula regex"));

    let mut semantics = SheetSemantics::default();
    for cell in CELL.find_iter(xml) {
        let text = cell.as_str();
        let Some(reference) = attribute(text, "r") else {
            continue;
        };
        if let Some(style) = attribute(text, "s").and_then(|value| value.parse::<usize>().ok())
            && let Some(format) = cell_formats.get(style)
            && *format != NumberSemantics::Plain
        {
            semantics.formats.insert(reference.clone(), format.clone());
        }
        if let Some(found) = FORMULA.captures(text) {
            semantics.formulas.insert(
                reference,
                CellFormula {
                    expression: decode(&found[1]),
                },
            );
        }
    }
    semantics
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let pattern = format!(r#"{}\s*=\s*"([^"]*)""#, regex::escape(name));
    Regex::new(&pattern)
        .ok()?
        .captures(tag)
        .map(|found| found[1].to_owned())
}

fn decode(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Escribe el valor de una celda tal como la hoja lo muestra, conservando la
/// semántica que aporta su formato.
///
/// Todo se hace sobre el texto y con decimal exacto: `0.15` como porcentaje es
/// `15%`, nunca `15.000000000000002%`.
pub fn render(raw: &str, semantics: &NumberSemantics) -> String {
    match semantics {
        NumberSemantics::Plain => raw.to_owned(),
        NumberSemantics::Percentage => match shift_decimal_right(raw, 2) {
            Some(shifted) => format!("{shifted}%"),
            None => raw.to_owned(),
        },
        NumberSemantics::Money(currency) => {
            let amount = crate::calc::Decimal::from_text(raw)
                .map(|value| value.render_money())
                .unwrap_or_else(|| raw.to_owned());
            match currency {
                Some(code) => format!("${amount} {code}"),
                // Sin código no se inventa uno: es la misma regla que rige el
                // texto plano, donde un «$» suelto nunca se convierte en MXN.
                None => format!("${amount}"),
            }
        }
    }
}

/// Corre el punto decimal a la derecha sin pasar por coma flotante.
fn shift_decimal_right(value: &str, places: usize) -> Option<String> {
    let trimmed = value.trim();
    let negative = trimmed.starts_with('-');
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let mut fraction = fraction.to_owned();
    while fraction.len() < places {
        fraction.push('0');
    }
    let mut shifted = format!("{whole}{}", &fraction[..places]);
    let remainder = fraction[places..].trim_end_matches('0');
    while shifted.len() > 1 && shifted.starts_with('0') {
        shifted.remove(0);
    }
    if !remainder.is_empty() {
        shifted.push('.');
        shifted.push_str(remainder);
    }
    Some(if negative && shifted != "0" {
        format!("-{shifted}")
    } else {
        shifted
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_code_says_whether_a_number_is_a_percentage_or_money() {
        assert_eq!(semantics_of_format("0.00%"), NumberSemantics::Percentage);
        assert_eq!(semantics_of_format("0%"), NumberSemantics::Percentage);
        assert_eq!(
            semantics_of_format(r##""$"#,##0.00"##),
            NumberSemantics::Money(None)
        );
        assert_eq!(
            semantics_of_format(r#"#,##0.00" MXN""#),
            NumberSemantics::Money(Some("MXN".into()))
        );
        assert_eq!(
            semantics_of_format(r#"[$USD-409]#,##0.00"#),
            NumberSemantics::Money(Some("USD".into()))
        );
        assert_eq!(semantics_of_format("#,##0.00"), NumberSemantics::Plain);
        assert_eq!(semantics_of_format("yyyy-mm-dd"), NumberSemantics::Plain);
        // Un «%» entrecomillado es texto, no un porcentaje.
        assert_eq!(semantics_of_format(r#"0.00" %""#), NumberSemantics::Plain);
    }

    #[test]
    fn the_decimal_point_moves_exactly_never_through_a_float() {
        assert_eq!(shift_decimal_right("0.15", 2).as_deref(), Some("15"));
        assert_eq!(shift_decimal_right("0.075", 2).as_deref(), Some("7.5"));
        assert_eq!(shift_decimal_right("0.12345", 2).as_deref(), Some("12.345"));
        assert_eq!(shift_decimal_right("1", 2).as_deref(), Some("100"));
        assert_eq!(shift_decimal_right("0", 2).as_deref(), Some("0"));
        assert_eq!(shift_decimal_right("-0.05", 2).as_deref(), Some("-5"));
        assert_eq!(shift_decimal_right("texto", 2), None);
    }

    #[test]
    fn rendering_keeps_the_currency_the_sheet_wrote_and_no_other() {
        assert_eq!(render("0.15", &NumberSemantics::Percentage), "15%");
        assert_eq!(
            render("1250", &NumberSemantics::Money(Some("MXN".into()))),
            "$1,250.00 MXN"
        );
        assert_eq!(render("750.5", &NumberSemantics::Money(None)), "$750.50");
        assert_eq!(render("12", &NumberSemantics::Plain), "12");
    }
}
