use std::sync::LazyLock;

use regex::Regex;

use crate::{
    model::{TypedValue, ValueKind},
    normalize::normalize_spanish,
};

pub fn classify_value(label: &str, raw: &str) -> TypedValue {
    let value = raw.trim();
    let normalized = normalize_spanish(value);

    if let Some((number, currency)) = parse_money(value) {
        return TypedValue {
            kind: ValueKind::Money,
            text_value: value.to_owned(),
            normalized_value: normalized,
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
        numeric_value: None,
        currency: None,
        date_value: None,
    }
}

pub fn parse_money(raw: &str) -> Option<(f64, Option<String>)> {
    static MONEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*\$?\s*([0-9][0-9,]*(?:\.[0-9]{1,2})?)\s*([A-Z]{3})?\s*$")
            .expect("valid money regex")
    });
    let captures = MONEY.captures(raw)?;
    if !raw.contains('$') && captures.get(2).is_none() {
        return None;
    }
    let number = captures.get(1)?.as_str().replace(',', "").parse().ok()?;
    let currency = captures
        .get(2)
        .map(|part| part.as_str().to_uppercase())
        .or_else(|| Some("MXN".into()));
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

fn valid_iso(year: i32, month: u32, day: u32) -> Option<String> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 || !(1900..=2200).contains(&year) {
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
}
