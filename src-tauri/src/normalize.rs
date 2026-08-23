use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const STOPWORDS: &[&str] = &[
    "de", "del", "la", "las", "el", "los", "un", "una", "y", "e", "en", "por", "para", "con",
    "sin", "al", "a", "que",
];

/// Normalización única para comparar tanto preguntas como valores del acervo.
/// Reduce acentos, puntuación, plurales y flexiones frecuentes de género sin
/// depender del vocabulario de un giro de negocio concreto.
pub fn normalize_spanish(input: &str) -> String {
    input
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(root_token)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalización literal para comparaciones que no admiten flexiones ni
/// aproximaciones. Conserva la separación de palabras y elimina sólo
/// diferencias de mayúsculas, acentos y puntuación; a diferencia de
/// `normalize_spanish`, no recorta plurales ni género.
pub fn normalize_exact(input: &str) -> String {
    input
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Conserva separadores y espacios para validar los límites de una búsqueda
/// literal dentro de texto libre. Sólo elimina diferencias de mayúsculas y
/// acentos, sin convertir guiones o barras en límites de palabra.
pub fn normalize_literal(input: &str) -> String {
    input
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase()
}

/// Clave de igualdad exclusivamente para identificadores alfanuméricos.
/// Los separadores habituales se ignoran sólo si el valor contiene letras y
/// números; códigos compuestos únicamente por dígitos conservan su formato
/// porque no es seguro asumir que sus separadores son equivalentes.
pub fn canonical_identifier(input: &str) -> Option<String> {
    let normalized = normalize_literal(input);
    let mut canonical = String::new();
    let mut has_letter = false;
    let mut has_number = false;
    for character in normalized.chars() {
        if character.is_alphabetic() {
            has_letter = true;
            canonical.push(character);
        } else if character.is_numeric() {
            has_number = true;
            canonical.push(character);
        } else if matches!(character, '-' | '_' | '/' | '.' | ' ' | '\t' | '\n' | '\r') {
            // Separador equivalente dentro de un identificador mixto.
        } else {
            return None;
        }
    }
    (has_letter && has_number && !canonical.is_empty()).then_some(canonical)
}

pub fn canonical_key(label: &str) -> String {
    let normalized = normalize_spanish(label);
    let kept = normalized
        .split_whitespace()
        .filter(|word| !STOPWORDS.contains(word))
        .collect::<Vec<_>>();
    if kept.is_empty() {
        normalized.replace(' ', "_")
    } else {
        kept.join("_")
    }
}

pub fn search_terms(input: &str) -> Vec<String> {
    normalize_spanish(input)
        .split_whitespace()
        .filter(|word| word.len() > 1 && !STOPWORDS.contains(word))
        .map(ToOwned::to_owned)
        .collect()
}

fn root_token(token: &str) -> String {
    let mut word = token.to_owned();

    if word.len() > 6 && word.ends_with("ces") {
        word.truncate(word.len() - 3);
        word.push('z');
    } else if word.len() > 6 && word.ends_with("es") {
        word.truncate(word.len() - 2);
    } else if word.len() > 5 && word.ends_with('s') {
        word.pop();
    }

    // pagado/pagada/pagados/pagadas -> pagad. Se limita a palabras largas
    // para no mutilar sustantivos breves como "día" o nombres propios.
    if word.len() > 5 && (word.ends_with('o') || word.ends_with('a')) {
        word.pop();
    }

    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gender_number_and_accents_share_one_root() {
        assert_eq!(normalize_spanish("Pagado"), normalize_spanish("pagadas"));
        assert_eq!(normalize_spanish("CONCLUSIÓN"), "conclusion");
    }

    #[test]
    fn literal_normalization_does_not_merge_distinct_values() {
        assert_eq!(normalize_exact("Records"), "records");
        assert_ne!(normalize_exact("Record"), normalize_exact("Records"));
    }

    #[test]
    fn literal_text_normalization_preserves_identifier_separators() {
        assert_eq!(normalize_literal("ALPHA-7"), "alpha-7");
    }

    #[test]
    fn canonical_identifier_ignores_only_safe_separators() {
        assert_eq!(canonical_identifier("Ab_Cd/12.3"), Some("abcd123".into()));
        assert_eq!(canonical_identifier("001-23"), None);
    }
}
