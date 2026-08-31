//! Fechas civiles y reloj inyectable.
//!
//! El motor nunca lee la fecha del sistema de forma implícita: quien construye
//! el motor decide qué día es «hoy», y cualquier rango derivado de esa fecha se
//! muestra resuelto en la respuesta. Así una prueba es reproducible y el
//! usuario ve el periodo exacto que se aplicó.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::normalize::normalize_exact;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CivilDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl CivilDate {
    pub fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        let valid = (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month);
        valid.then_some(Self { year, month, day })
    }

    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        let mut parts = trimmed.split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Self::new(year, month, day)
    }

    pub fn to_iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// Días desde 1970-01-01 en el calendario gregoriano proléptico. Permite
    /// desplazar rangos sin sumar una dependencia de fechas al proyecto.
    pub fn to_days(self) -> i64 {
        let year = self.year as i64 - i64::from(self.month <= 2);
        let era = if year >= 0 { year } else { year - 399 } / 400;
        let year_of_era = year - era * 400;
        let month = self.month as i64;
        let day_of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + self.day as i64 - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    pub fn from_days(days: i64) -> Self {
        let shifted = days + 719_468;
        let era = if shifted >= 0 {
            shifted
        } else {
            shifted - 146_096
        } / 146_097;
        let day_of_era = shifted - era * 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
        let month = (month_index + if month_index < 10 { 3 } else { -9 }) as u32;
        Self {
            year: (year + i64::from(month <= 2)) as i32,
            month,
            day,
        }
    }
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Fuente de la fecha actual. `Fixed` existe para que las pruebas y la fábrica
/// de evaluación no dependan del día en que se ejecutan.
#[derive(Clone, Copy, Debug)]
pub enum Clock {
    System,
    Fixed(CivilDate),
}

impl Clock {
    pub fn fixed(iso: &str) -> Option<Self> {
        CivilDate::parse(iso).map(Self::Fixed)
    }

    pub fn today(&self) -> CivilDate {
        match self {
            Self::Fixed(date) => *date,
            Self::System => {
                let seconds = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|value| value.as_secs() as i64)
                    .unwrap_or(0);
                CivilDate::from_days(seconds.div_euclid(86_400))
            }
        }
    }
}

/// Rango cerrado de fechas con la unidad de la que salió.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateRange {
    pub from: CivilDate,
    pub to: CivilDate,
    /// Determina cómo se calcula el periodo anterior sin volver a leer la
    /// pregunta: un mes retrocede a un mes, un año a un año.
    pub unit: RangeUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeUnit {
    Month,
    Year,
    Custom,
}

impl DateRange {
    pub fn month(year: i32, month: u32) -> Option<Self> {
        Some(Self {
            from: CivilDate::new(year, month, 1)?,
            to: CivilDate::new(year, month, days_in_month(year, month))?,
            unit: RangeUnit::Month,
        })
    }

    pub fn year(year: i32) -> Option<Self> {
        Some(Self {
            from: CivilDate::new(year, 1, 1)?,
            to: CivilDate::new(year, 12, 31)?,
            unit: RangeUnit::Year,
        })
    }

    pub fn custom(from: CivilDate, to: CivilDate) -> Self {
        let (from, to) = if from <= to { (from, to) } else { (to, from) };
        Self {
            from,
            to,
            unit: RangeUnit::Custom,
        }
    }

    pub fn label(&self) -> String {
        format!("{} a {}", self.from.to_iso(), self.to.to_iso())
    }

    /// Periodo inmediatamente anterior del mismo tamaño. Un mes retrocede a un
    /// mes natural completo; un rango arbitrario retrocede tantos días como
    /// abarca, sin inventar una unidad de calendario que la pregunta no dio.
    pub fn previous_period(&self) -> Option<Self> {
        match self.unit {
            RangeUnit::Month => {
                let (year, month) = if self.from.month == 1 {
                    (self.from.year - 1, 12)
                } else {
                    (self.from.year, self.from.month - 1)
                };
                Self::month(year, month)
            }
            RangeUnit::Year => Self::year(self.from.year - 1),
            RangeUnit::Custom => {
                let span = self.to.to_days() - self.from.to_days() + 1;
                Some(Self {
                    from: CivilDate::from_days(self.from.to_days() - span),
                    to: CivilDate::from_days(self.to.to_days() - span),
                    unit: RangeUnit::Custom,
                })
            }
        }
    }
}

const MONTH_ROOTS: [(&str, u32); 13] = [
    ("enero", 1),
    ("febrero", 2),
    ("marzo", 3),
    ("abril", 4),
    ("mayo", 5),
    ("junio", 6),
    ("julio", 7),
    ("agosto", 8),
    ("septiembre", 9),
    ("setiembre", 9),
    ("octubre", 10),
    ("noviembre", 11),
    ("diciembre", 12),
];

/// La fecha escrita como la escribe el acervo: «11 de octubre de 2024».
///
/// Reutiliza la misma tabla de meses con la que se leen las fechas, para que
/// escribir y leer no puedan discrepar. `MONTH_ROOTS` tiene dos entradas para
/// septiembre; `find` devuelve la primera, que es la forma plena.
pub fn spanish_long_date(date: CivilDate) -> String {
    let month = MONTH_ROOTS
        .iter()
        .find(|(_, number)| *number == date.month)
        .map(|(name, _)| *name)
        .unwrap_or("");
    format!("{:02} de {month} de {}", date.day, date.year)
}

/// Señales de que la pregunta habla de un periodo anterior al del contexto.
pub fn asks_for_previous_period(question: &str) -> bool {
    let normalized = normalize_exact(question);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    words.windows(2).any(|pair| {
        matches!(
            pair,
            ["mes", "pasado"]
                | ["mes", "anterior"]
                | ["mes", "previo"]
                | ["ano", "pasado"]
                | ["ano", "anterior"]
                | ["ano", "previo"]
                | ["periodo", "anterior"]
                | ["periodo", "previo"]
        )
    })
}

/// Lee un periodo explícito o relativo dentro de una pregunta.
///
/// Sólo reconoce formas de calendario —nunca nombres de campos ni de negocio—
/// y devuelve el rango ya resuelto contra el reloj recibido.
pub fn range_in_question(
    question: &str,
    clock: &Clock,
    anchor: Option<&DateRange>,
) -> Option<DateRange> {
    let normalized = normalize_exact(&compact_iso_dates(question));
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let today = clock.today();

    if words.windows(2).any(|pair| matches!(pair, ["este", "mes"])) {
        return DateRange::month(today.year, today.month);
    }
    if words.windows(2).any(|pair| matches!(pair, ["este", "ano"])) {
        return DateRange::year(today.year);
    }
    if words.windows(2).any(|pair| {
        matches!(
            pair,
            ["mes", "pasado"] | ["mes", "anterior"] | ["mes", "previo"]
        )
    }) {
        let current = anchor
            .filter(|range| range.unit == RangeUnit::Month)
            .cloned()
            .or_else(|| DateRange::month(today.year, today.month))?;
        return current.previous_period();
    }
    if words.windows(2).any(|pair| {
        matches!(
            pair,
            ["ano", "pasado"] | ["ano", "anterior"] | ["ano", "previo"]
        )
    }) {
        let current = anchor
            .filter(|range| range.unit == RangeUnit::Year)
            .cloned()
            .or_else(|| DateRange::year(today.year))?;
        return current.previous_period();
    }

    let iso_dates = words
        .iter()
        .filter_map(|word| iso_word(word))
        .collect::<Vec<_>>();
    if iso_dates.len() >= 2 {
        return Some(DateRange::custom(iso_dates[0], iso_dates[1]));
    }
    if let Some(single) = iso_dates.first() {
        if words
            .iter()
            .any(|word| matches!(*word, "desde" | "despues" | "posteriores"))
        {
            return Some(DateRange::custom(*single, CivilDate::new(2200, 12, 31)?));
        }
        if words
            .iter()
            .any(|word| matches!(*word, "hasta" | "antes" | "previas"))
        {
            return Some(DateRange::custom(CivilDate::new(1900, 1, 1)?, *single));
        }
        return Some(DateRange::custom(*single, *single));
    }

    let year = words
        .iter()
        .filter_map(|word| word.parse::<i32>().ok())
        .find(|value| (1900..=2200).contains(value));
    if let Some(month) = month_in(&words) {
        let resolved = year
            .or_else(|| anchor.map(|range| range.from.year))
            .unwrap_or(today.year);
        return DateRange::month(resolved, month);
    }
    year.and_then(DateRange::year)
}

fn month_in(words: &[&str]) -> Option<u32> {
    words.iter().find_map(|word| {
        MONTH_ROOTS
            .iter()
            .find(|(name, _)| *name == *word)
            .map(|(_, month)| *month)
    })
}

fn iso_word(word: &str) -> Option<CivilDate> {
    (word.len() == 8 && word.chars().all(|character| character.is_ascii_digit()))
        .then(|| {
            CivilDate::new(
                word[0..4].parse().ok()?,
                word[4..6].parse().ok()?,
                word[6..8].parse().ok()?,
            )
        })
        .flatten()
}

/// Compacta las fechas ISO antes de normalizar, para que `2024-03-31` no se
/// pierda al separar el texto por guiones.
pub fn compact_iso_dates(question: &str) -> String {
    let characters = question.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(question.len());
    let mut index = 0;
    while index < characters.len() {
        let window = characters
            .get(index..index + 10)
            .map(|chunk| chunk.iter().collect::<String>());
        match window.as_deref().and_then(parse_iso_window) {
            Some(compact) => {
                output.push_str(&compact);
                index += 10;
            }
            None => {
                output.push(characters[index]);
                index += 1;
            }
        }
    }
    output
}

fn parse_iso_window(window: &str) -> Option<String> {
    let bytes = window.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    CivilDate::parse(window).map(|date| format!("{:04}{:02}{:02}", date.year, date.month, date.day))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        for iso in ["1970-01-01", "2024-02-29", "2026-08-24", "2000-03-01"] {
            let date = CivilDate::parse(iso).unwrap();
            assert_eq!(CivilDate::from_days(date.to_days()), date);
        }
        assert_eq!(CivilDate::parse("2024-02-30"), None);
    }

    #[test]
    fn relative_periods_resolve_against_the_injected_clock() {
        let clock = Clock::fixed("2026-03-15").unwrap();
        let range = range_in_question("¿cuánto suman el mes pasado?", &clock, None).unwrap();
        assert_eq!(range.from.to_iso(), "2026-02-01");
        assert_eq!(range.to.to_iso(), "2026-02-28");
        let year = range_in_question("¿cuántos hay el año pasado?", &clock, None).unwrap();
        assert_eq!(year.label(), "2025-01-01 a 2025-12-31");
    }

    #[test]
    fn a_month_anchor_moves_one_calendar_month_back() {
        let clock = Clock::fixed("2026-08-24").unwrap();
        let anchor = DateRange::month(2026, 1).unwrap();
        let previous =
            range_in_question("compáralo con el mes anterior", &clock, Some(&anchor)).unwrap();
        assert_eq!(previous.label(), "2025-12-01 a 2025-12-31");
    }

    #[test]
    fn explicit_calendar_expressions_do_not_need_the_clock() {
        let clock = Clock::fixed("2026-08-24").unwrap();
        let month = range_in_question("registros de marzo de 2024", &clock, None).unwrap();
        assert_eq!(month.label(), "2024-03-01 a 2024-03-31");
        let year = range_in_question("documentos de 2024", &clock, None).unwrap();
        assert_eq!(year.label(), "2024-01-01 a 2024-12-31");
        let explicit = range_in_question("entre 2024-01-10 y 2024-02-20", &clock, None).unwrap();
        assert_eq!(explicit.label(), "2024-01-10 a 2024-02-20");
    }

    #[test]
    fn a_custom_range_shifts_back_by_its_own_span() {
        let range = DateRange::custom(
            CivilDate::parse("2024-01-10").unwrap(),
            CivilDate::parse("2024-01-19").unwrap(),
        );
        let previous = range.previous_period().unwrap();
        assert_eq!(previous.label(), "2023-12-31 a 2024-01-09");
    }
}
