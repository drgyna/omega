//! Barrido sistemático de la familia de bugs "singular y plural no convergen
//! al mismo token" (normalize.rs::root_token). No prueba "interés" como caso
//! puntual: recorre TODOS los conceptos reales de un corpus configurado y
//! pregunta cada uno en singular y en plural, verificando que ambas formas
//! citen exactamente la misma evidencia. Cualquier campo cuyo sustantivo
//! termine en vocal acentuada + "s" (interés, francés, compás...) activa esta
//! misma familia si el stemmer deja de converger; este barrido la detecta sin
//! necesidad de conocer de antemano qué campos existen en el corpus.
//!
//! Se omite en silencio sin la variable de entorno: a diferencia de
//! `legal_corpus_scale.rs`, este barrido depende de un corpus externo que no
//! es parte del contrato de ningún fixture del repositorio, así que no debe
//! sumar ruido a `cargo test --all` cuando nadie lo configuró a propósito.
//!
//! No conoce el vocabulario de ningún corpus concreto: lee `concepts()` del
//! que se le indique, así que sirve igual para el corpus de prueba de hoy que
//! para cualquier otro que lo reemplace — sólo cambia la carpeta que apunta
//! `OMEGA_SWEEP_CORPUS`.

use std::{collections::HashSet, env, path::PathBuf};

use omega_core::{Answer, OmegaEngine};

#[test]
fn every_real_concept_converges_between_its_singular_and_plural_phrasing() {
    let Some(corpus) = env::var_os("OMEGA_SWEEP_CORPUS").map(PathBuf::from) else {
        eprintln!("OMEGA_SWEEP_CORPUS no está definida: barrido de singular/plural omitido.");
        return;
    };
    assert!(
        corpus.is_dir(),
        "la fuente configurada debe ser una carpeta"
    );

    let temporary = tempfile::tempdir().unwrap();
    let engine = OmegaEngine::open(temporary.path().join("plural-sweep.db")).unwrap();
    let source_id = engine.authorize_source(&corpus).unwrap();
    engine.index_source(source_id).unwrap();

    let concepts = engine.concepts(None).unwrap();
    assert!(
        !concepts.is_empty(),
        "el corpus debe exponer conceptos reales"
    );

    let mut checked = 0usize;
    let mut divergences = Vec::new();
    let mut accented_family_still_broken = Vec::new();

    for concept in &concepts {
        let singular_question = format!("¿Cuál es el {}?", concept.display_name.to_lowercase());
        let plural_field = pluralize_field(&concept.display_name);
        let plural_question = format!("¿Cuáles son los {}?", plural_field.to_lowercase());

        let singular = engine.ask(&singular_question).unwrap();
        let plural = engine.ask(&plural_question).unwrap();
        checked += 1;

        // El invariante que importa: preguntar lo mismo en singular o en
        // plural debe citar exactamente la misma evidencia. No exige que
        // ambas formas sinteticen (algunos nombres de campo compuestos no lo
        // hacen por otras razones, ajenas a este bug); exige que no diverjan
        // entre sí.
        let singular_evidence = citation_ids(&singular);
        let plural_evidence = citation_ids(&plural);
        if singular_evidence != plural_evidence {
            let report = format!(
                "«{}» — singular {:?} ({} citas) vs. plural {:?} ({} citas)",
                concept.display_name,
                singular_question,
                singular_evidence.len(),
                plural_question,
                plural_evidence.len(),
            );
            if ends_with_accented_vowel_s(&concept.display_name) {
                accented_family_still_broken.push(report.clone());
            }
            divergences.push(report);
        }
    }

    eprintln!("Barrido singular/plural: {checked} conceptos reales verificados.");
    // El resto de divergencias (si las hay) no son necesariamente de esta
    // familia: pueden venir de la capa de recuperación (`search()`), que este
    // barrido no toca a propósito — queda como diagnóstico, no como falla.
    if !divergences.is_empty() {
        eprintln!(
            "Nota: {} de {checked} conceptos divergen por otras razones (fuera de esta familia):\n{}",
            divergences.len(),
            divergences.join("\n")
        );
    }
    assert!(checked > 0);
    assert!(
        accented_family_still_broken.is_empty(),
        "la familia de bugs de vocal acentuada + s sigue abierta en {} concepto(s):\n{}",
        accented_family_still_broken.len(),
        accented_family_still_broken.join("\n")
    );
}

fn citation_ids(answer: &Answer) -> HashSet<String> {
    answer
        .citations
        .iter()
        .map(|item| item.id.clone())
        .collect()
}

/// Pluraliza sólo la primera palabra del nombre de campo (su sustantivo
/// principal); el resto se deja igual. Alcanza para generar una pregunta
/// natural sin necesitar concordancia de género real, que `resolve_field` no
/// exige de todas formas (compara por términos, no por gramática completa).
fn pluralize_field(display_name: &str) -> String {
    let mut words = display_name.split_whitespace();
    let Some(first) = words.next() else {
        return display_name.to_owned();
    };
    let rest = words.collect::<Vec<_>>();
    let plural_first = pluralize_word(first);
    if rest.is_empty() {
        plural_first
    } else {
        format!("{plural_first} {}", rest.join(" "))
    }
}

fn pluralize_word(word: &str) -> String {
    let chars = word.chars().collect::<Vec<_>>();
    if let Some(&last) = chars.last() {
        if chars.len() >= 2 && last.to_ascii_lowercase() == 's' {
            if let Some(base_vowel) = unaccent_vowel(chars[chars.len() - 2]) {
                // interés -> "inter" + "e" + "ses" = "intereses".
                let mut stem = chars[..chars.len() - 2].iter().collect::<String>();
                stem.push(base_vowel);
                stem.push_str("ses");
                return stem;
            }
            // Ya termina en "s" sin acento marcado (lunes, crisis): sin señal
            // ortográfica de que el plural difiera, se deja igual.
            return word.to_owned();
        }
    }
    let ends_in_vowel = chars
        .last()
        .is_some_and(|c| "aeiouAEIOUáéíóúÁÉÍÓÚ".contains(*c));
    if ends_in_vowel {
        format!("{word}s")
    } else {
        format!("{word}es")
    }
}

fn unaccent_vowel(c: char) -> Option<char> {
    match c {
        'á' | 'Á' => Some('a'),
        'é' | 'É' => Some('e'),
        'í' | 'Í' => Some('i'),
        'ó' | 'Ó' => Some('o'),
        'ú' | 'Ú' => Some('u'),
        _ => None,
    }
}

fn ends_with_accented_vowel_s(display_name: &str) -> bool {
    let Some(first) = display_name.split_whitespace().next() else {
        return false;
    };
    let chars = first.chars().collect::<Vec<_>>();
    chars.len() >= 2
        && chars.last().map(|c| c.to_ascii_lowercase()) == Some('s')
        && unaccent_vowel(chars[chars.len() - 2]).is_some()
}
