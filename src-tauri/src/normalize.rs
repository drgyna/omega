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

/// Compara dos términos ya reducidos por `root_token`, tolerando la única
/// ambigüedad que esa función no puede resolver sin un diccionario: un
/// sustantivo que ya termina en "e" sólo agrega "s" en plural ("cliente" ->
/// "clientes"), mientras que uno que termina en consonante agrega "es"
/// completo ("papel" -> "papeles") — y ambos plurales terminan exactamente
/// igual ("consonante" + "e" + "s"), sin ninguna señal local que distinga los
/// dos casos dentro de la sola palabra plural. Consecuencia medible: cuando el
/// singular termina en "e", su raíz y la de su plural quedan separadas por
/// exactamente un carácter de más en el singular (la "e" que sí es parte de la
/// raíz), y una es siempre prefijo de la otra — nunca al revés ni con una
/// diferencia mayor, porque la única regla que puede fallar así es la de "es"
/// en `root_token`, que trunca una cantidad fija.
///
/// El umbral de longitud evita que esta tolerancia una palabras cortas no
/// relacionadas que por casualidad comparten prefijo ("mes"/"mesa"), igual que
/// el resto de las reglas de `root_token` ya exigen una longitud mínima antes
/// de actuar.
pub fn stems_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    shorter.len() >= 5 && longer.len() - shorter.len() == 1 && longer.starts_with(shorter)
}

/// Reduce hasta que ninguna regla vuelva a aplicar, en vez de una sola pasada.
///
/// Un sustantivo español que YA termina en "-és"/"-es" en singular (interés,
/// francés) sólo se distingue de un plural real formado con ese mismo sufijo
/// (papeles, de papel) por cuántas veces hace falta quitarlo para llegar a una
/// raíz estable. Con una sola pasada, "interes" (el propio campo, sin acento)
/// activa la regla de plural una vez y llega a "inter"; pero "intereses" (la
/// pregunta) sólo la activa una vez ella también y se queda en "interes" — un
/// paso más corto, porque nunca vuelve a intentarlo sobre su propio resultado.
/// Singular y plural quedan en tokens distintos y no convergen.
///
/// Repetir la reducción hasta el punto fijo no es un parche por palabra: es la
/// misma regla de siempre, aplicada las veces que hagan falta para que
/// cualquier encadenamiento de sufijos (accidental o no) termine en la misma
/// raíz sin importar de qué forma partió. Siempre termina porque cada regla
/// que se activa acorta la palabra, y ya hay un piso por debajo del cual
/// ninguna regla vuelve a activarse.
fn root_token(token: &str) -> String {
    let mut word = token.to_owned();
    loop {
        let reduced = strip_one_suffix(&word);
        if reduced == word {
            return word;
        }
        word = reduced;
    }
}

fn strip_one_suffix(token: &str) -> String {
    let mut word = token.to_owned();

    if word.len() > 6 && word.ends_with("ces") {
        word.truncate(word.len() - 3);
        word.push('z');
    } else if word.len() > 6 && word.ends_with("es") {
        word.truncate(word.len() - 2);
    } else if word.len() >= 5 && word.ends_with('s') {
        // "tipos" (5 letras) debe reducirse a "tipo" igual que cualquier
        // plural más largo: el umbral estricto (> 5) dejaba fuera justo las
        // palabras de 5 letras, así que "tipos" nunca coincidía con el campo
        // "Tipo de X" en preguntas naturales.
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
    fn a_five_letter_plural_shares_its_root_with_the_singular() {
        // "tipos" (exactamente 5 letras) es el caso que se quedaba fuera del
        // umbral anterior y nunca emparejaba con el campo "Tipo de X".
        assert_eq!(normalize_spanish("tipos"), normalize_spanish("tipo"));
        assert_eq!(normalize_spanish("Notas"), normalize_spanish("nota"));
        assert_eq!(normalize_spanish("datos"), normalize_spanish("dato"));
    }

    #[test]
    fn a_singular_that_already_ends_in_es_still_converges_with_its_plural() {
        // "Interés" pierde el acento al normalizar y queda en "interes", que
        // por sí solo ya termina en "es" — la misma forma que produce la
        // regla de plural. Una sola pasada trataba esa terminación nativa
        // como si fuera el sufijo de un plural y la reducía de más, dejando
        // el campo (singular) y la pregunta (plural) en raíces distintas.
        assert_eq!(normalize_spanish("Interés"), normalize_spanish("intereses"));
        assert_eq!(normalize_spanish("francés"), normalize_spanish("franceses"));
        // Un campo cuyo singular no comparte esa terminación no debe verse
        // afectado por permitir varias pasadas: sigue conviniendo en el mismo
        // punto que antes.
        assert_eq!(
            normalize_spanish("prioridad"),
            normalize_spanish("prioridades")
        );
    }

    #[test]
    fn stems_match_bridges_the_e_final_plural_ambiguity_root_token_cannot_resolve() {
        // "cliente"/"clientes" y afines: la raíz del singular es la del
        // plural más una "e" — root_token por sí solo no puede saber si esa
        // "e" es parte de la raíz o del sufijo sin un diccionario.
        assert!(stems_match(&root_token("cliente"), &root_token("clientes")));
        assert!(stems_match(
            &root_token("expediente"),
            &root_token("expedientes")
        ));
        // No debe unir palabras cortas no relacionadas que por casualidad
        // comparten prefijo.
        assert!(!stems_match("mes", "mesa"));
        // Un caso que ya converge exactamente sigue funcionando igual (el
        // camino corto de `a == b`, sin depender de la tolerancia).
        assert!(stems_match(&root_token("papel"), &root_token("papeles")));
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
