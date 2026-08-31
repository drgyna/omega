use std::sync::LazyLock;

use regex::Regex;

use crate::{
    model::{TypedValue, ValueKind},
    normalize::{normalize_literal, normalize_spanish},
};

pub fn classify_value(label: &str, raw: &str) -> TypedValue {
    let value = raw.trim();
    let normalized = normalize_spanish(value);
    // A diferencia de `normalized_value` (que borra puntuación y aplica
    // raíces para búsquedas difusas), `literal_value` sólo pliega mayúsculas
    // y acentos. Es la clave que decide si un filtro «es el mismo valor»:
    // conserva el «%», las comillas y los dos puntos, así que nunca junta
    // «50» con «50%» ni «Pendiente» con ««Pendiente»» sólo porque ambos
    // pierdan su puntuación al normalizarse.
    let literal = normalize_literal(value);

    if let Some((number, currency)) = parse_money(value) {
        return TypedValue {
            kind: ValueKind::Money,
            text_value: value.to_owned(),
            normalized_value: normalized,
            literal_value: literal,
            numeric_value: Some(number),
            currency,
            date_value: None,
        };
    }

    if let Some(date) = parse_date(value) {
        return TypedValue {
            kind: ValueKind::Date,
            text_value: value.to_owned(),
            normalized_value: normalized,
            literal_value: literal,
            numeric_value: None,
            currency: None,
            date_value: Some(date),
        };
    }

    if let Some(number) = parse_percentage(value) {
        return TypedValue {
            kind: ValueKind::Percentage,
            text_value: value.to_owned(),
            normalized_value: normalized,
            literal_value: literal,
            numeric_value: Some(number),
            currency: None,
            date_value: None,
        };
    }

    if let Some(number) = parse_number(value) {
        return TypedValue {
            kind: ValueKind::Number,
            text_value: value.to_owned(),
            normalized_value: normalized,
            literal_value: literal,
            numeric_value: Some(number),
            currency: None,
            date_value: None,
        };
    }

    let label_root = normalize_spanish(label);
    let kind = if label_root.split_whitespace().any(|part| part == "estad") {
        ValueKind::State
    } else {
        ValueKind::Text
    };
    TypedValue {
        kind,
        text_value: value.to_owned(),
        normalized_value: normalized,
        literal_value: literal,
        numeric_value: None,
        currency: None,
        date_value: None,
    }
}

pub fn parse_money(raw: &str) -> Option<(f64, Option<String>)> {
    static MONEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*([0-9][0-9,]*(?:\.[0-9]{1,2})?)\s*([A-Z]{3})?\s*$")
            .expect("valid money regex")
    });
    let mut value = raw.trim();
    let mut negative = false;
    if let Some(rest) = value.strip_prefix('-') {
        negative = true;
        value = rest.trim_start();
    }
    let symbol_currency = value.chars().next().and_then(|symbol| match symbol {
        '$' | '¥' => Some(None),
        '€' => Some(Some("EUR")),
        '£' => Some(Some("GBP")),
        '₹' => Some(Some("INR")),
        '₩' => Some(Some("KRW")),
        _ => None,
    });
    if symbol_currency.is_some() {
        let symbol = value.chars().next().expect("checked symbol");
        value = value.strip_prefix(symbol).expect("checked symbol").trim_start();
    }
    if let Some(rest) = value.strip_prefix('$') {
        value = rest.trim_start();
    }
    // Los documentos reales escriben negativos tanto como «-$50» como
    // «$-50». Se acepta un único signo en cualquiera de esas dos posiciones,
    // pero nunca se elimina: se conserva en el valor indexado y calculado.
    if let Some(rest) = value.strip_prefix('-') {
        if negative {
            return None;
        }
        negative = true;
        value = rest.trim_start();
    }
    let captures = MONEY.captures(value)?;
    if symbol_currency.is_none() && captures.get(2).is_none() {
        return None;
    }
    let mut number: f64 = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
    if negative {
        number = -number;
    }
    // Un «$» sin código de moneda es dinero de moneda desconocida, nunca MXN
    // por defecto: inventar una moneda que el documento no escribió falsearía
    // cualquier suma que mezcle acervos de países distintos.
    let written_currency = captures.get(2).map(|part| part.as_str().to_uppercase());
    let currency = match (symbol_currency.flatten(), written_currency) {
        (Some(symbol), Some(written)) if !symbol.eq_ignore_ascii_case(&written) => return None,
        (_, Some(written)) => Some(written),
        (Some(symbol), None) => Some(symbol.to_owned()),
        (None, None) => None,
    };
    Some((number, currency))
}

fn parse_percentage(raw: &str) -> Option<f64> {
    let stripped = raw.trim().strip_suffix('%')?.trim().replace(',', "");
    stripped.parse().ok()
}

fn parse_number(raw: &str) -> Option<f64> {
    let stripped = raw.trim().replace(',', "");
    if stripped
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        stripped.parse().ok()
    } else {
        None
    }
}

pub fn parse_date(raw: &str) -> Option<String> {
    static ISO_DATE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").expect("valid ISO date regex"));
    static SPANISH_DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*(\d{1,2})\s+de\s+([a-záéíóúñ]+)\s+de\s+(\d{4})\s*$")
            .expect("valid Spanish date regex")
    });
    if let Some(parts) = ISO_DATE.captures(raw.trim()) {
        return valid_iso(
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
            parts[3].parse().ok()?,
        );
    }
    let parts = SPANISH_DATE.captures(raw)?;
    let day: u32 = parts[1].parse().ok()?;
    let month = match normalize_spanish(&parts[2]).as_str() {
        "enero" => 1,
        "febrer" => 2,
        "marzo" => 3,
        "abril" => 4,
        "mayo" => 5,
        "junio" => 6,
        "julio" => 7,
        "agost" => 8,
        "septiembre" | "setiembre" => 9,
        "octubre" => 10,
        "noviembre" => 11,
        "diciembre" => 12,
        _ => return None,
    };
    valid_iso(parts[3].parse().ok()?, month, day)
}

/// Una fecha con forma correcta pero día inexistente no es una fecha.
///
/// Aceptar `2024-02-31` porque «31 ≤ 31» la convertía en un valor de tipo
/// fecha con su propio `date_value`, y desde ahí actuaba como fecha válida en
/// cualquier rango que la contuviera por orden lexicográfico. El calendario es
/// el mismo que usa el resto del motor (`dates::days_in_month`), incluida la
/// regla de los siglos para los bisiestos, para que la validación y el
/// aritmética de fechas nunca discrepen.
fn valid_iso(year: i32, month: u32, day: u32) -> Option<String> {
    if !(1900..=2200).contains(&year) {
        return None;
    }
    // `days_in_month` devuelve 0 para un mes fuera de 1..=12, así que este
    // mismo predicado cubre también los meses imposibles.
    if day == 0 || day > crate::dates::days_in_month(year, month) {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

pub fn resembles_entity(value: &str, kind: &ValueKind) -> bool {
    if !matches!(kind, ValueKind::Text) {
        return false;
    }
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() || words.len() > 14 {
        return false;
    }
    let starts_uppercase = words
        .iter()
        .filter(|word| word.chars().next().map(char::is_uppercase).unwrap_or(false))
        .count();
    starts_uppercase >= 2 || value.contains("S.A.") || value.contains("S.C.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_values_without_a_business_dictionary() {
        let money = classify_value("Importe", "$612,500.00 MXN");
        assert_eq!(money.numeric_value, Some(612_500.0));
        assert_eq!(money.currency.as_deref(), Some("MXN"));
        let date = classify_value("Fecha", "23 de marzo de 2024");
        assert_eq!(date.date_value.as_deref(), Some("2024-03-23"));
        assert!(matches!(
            classify_value("Estado", "Cerrada").kind,
            ValueKind::State
        ));
    }

    #[test]
    fn a_dollar_sign_without_a_currency_code_never_becomes_mxn() {
        let money = classify_value("Importe", "$500.00");
        assert_eq!(money.numeric_value, Some(500.0));
        assert_eq!(
            money.currency, None,
            "un «$» sin código no debe inventar una moneda concreta"
        );
        assert!(matches!(money.kind, ValueKind::Money));
    }

    #[test]
    fn an_impossible_day_is_not_a_date() {
        assert_eq!(parse_date("2024-02-31"), None);
        assert_eq!(parse_date("2024-04-31"), None);
        assert_eq!(parse_date("2023-02-29"), None);
        assert_eq!(parse_date("31 de febrero de 2024"), None);
        assert_eq!(parse_date("2024-02-29").as_deref(), Some("2024-02-29"));
        assert_eq!(parse_date("2000-02-29").as_deref(), Some("2000-02-29"));
        assert_eq!(parse_date("1900-02-29"), None, "1900 no es bisiesto");
        // Sin tipo de fecha, el valor sigue existiendo como texto: no se
        // borra del acervo, deja de poder filtrar por periodo.
        let typed = classify_value("Fecha de emisión", "2024-02-31");
        assert!(matches!(typed.kind, ValueKind::Text));
        assert_eq!(typed.date_value, None);
        assert_eq!(typed.text_value, "2024-02-31");
    }

    #[test]
    fn a_currency_code_other_than_mxn_is_kept_as_written() {
        let usd = classify_value("Importe", "$500.00 USD");
        assert_eq!(usd.currency.as_deref(), Some("USD"));
        let eur = classify_value("Importe", "$500.00 EUR");
        assert_eq!(eur.currency.as_deref(), Some("EUR"));
    }

    #[test]
    fn negative_money_keeps_its_sign_before_or_after_the_symbol() {
        for literal in ["-$1,200.50 MXN", "$-1,200.50 MXN", "-1,200.50 MXN"] {
            let money = classify_value("Importe", literal);
            assert!(matches!(money.kind, ValueKind::Money), "{literal}");
            assert_eq!(money.numeric_value, Some(-1_200.5), "{literal}");
            assert_eq!(money.currency.as_deref(), Some("MXN"), "{literal}");
        }
    }

    #[test]
    fn explicit_non_dollar_symbols_are_money_without_relabeling_them() {
        let euro = classify_value("Importe", "€500.00 EUR");
        assert!(matches!(euro.kind, ValueKind::Money));
        assert_eq!(euro.currency.as_deref(), Some("EUR"));
        assert_eq!(euro.numeric_value, Some(500.0));

        let pound = classify_value("Importe", "£10.00");
        assert!(matches!(pound.kind, ValueKind::Money));
        assert_eq!(pound.currency.as_deref(), Some("GBP"));

        assert!(
            parse_money("€500.00 USD").is_none(),
            "un símbolo y un código contradictorios no son evidencia monetaria fiable"
        );
    }
}
