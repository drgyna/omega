//! Omega lee los documentos que citó y redacta un resumen.
//!
//! Lo que estas pruebas protegen, en orden de importancia:
//!
//!  1. La lectura es un **añadido**. `Answer::text` y `Answer::citations`
//!     salen idénticos con resumen y sin él.
//!  2. Es **universal**. Los mismos documentos con los nombres de campo
//!     cambiados producen el mismo resumen, palabra por palabra, salvo esos
//!     nombres; y dos acervos de rubros distintos se leen con el mismo
//!     binario y la misma calidad.
//!  3. Es **literal**. Ningún valor aparece en el texto si no está escrito en
//!     el documento del que se dice.
//!
//! Todas las fixtures viven en directorios temporales y no describen ningún
//! giro de negocio: sus campos se llaman «Alfa», «Beta» o «Uno», que es la
//! única forma de comprobar que el motor no reconoce ninguna palabra.

use std::{fs, path::Path};

use omega_core::{Answer, AnswerReading, Clock, Database, Evidence, OmegaEngine, ToolEngine};
use regex::Regex;

const TODAY: &str = "2026-08-26";

/// Nombres de campo de la fixture base y sus equivalentes renombrados. Ni un
/// solo término se comparte entre las dos listas, y ninguna de las dos
/// significa nada: si el motor leyera los rótulos, una de las dos se leería
/// peor que la otra.
const FIELDS: [&str; 5] = ["Alfa", "Beta", "Gamma", "Delta", "Épsilon"];
const RENAMED: [&str; 5] = ["Uno", "Dos", "Tres", "Cuatro", "Cinco"];

const CLOSING: &str =
    "El responsable conserva la evidencia del movimiento hasta que el área correspondiente autorice su cierre formal.";

fn engine_over(root: &Path) -> (tempfile::TempDir, OmegaEngine) {
    let home = tempfile::tempdir().unwrap();
    let engine = OmegaEngine::open_with_clock(
        home.path().join("omega.db"),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    engine.index_source(source).unwrap();
    (home, engine)
}

/// Un registro con los cinco campos, en el mismo orden y con los mismos
/// valores; sólo cambian los rótulos.
fn write_record(path: &Path, fields: &[&str; 5], number: usize, state: &str) {
    fs::write(
        path,
        format!(
            "{}: REG-77-{number:04}\n{}: {state}\n{}: $1,200.00 MXN\n{}: 2026-04-02\n{}: Delegación Norte\n\n{CLOSING}\n",
            fields[0], fields[1], fields[2], fields[3], fields[4]
        ),
    )
    .unwrap();
}

/// Acervo de un solo registro, con los rótulos que se le pidan.
fn single_record_corpus(fields: &[&str; 5]) -> (tempfile::TempDir, tempfile::TempDir, OmegaEngine) {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("acervo");
    fs::create_dir_all(&corpus).unwrap();
    write_record(&corpus.join("registro.md"), fields, 1, "Activo");
    let (home, engine) = engine_over(&corpus);
    (fixture, home, engine)
}

fn reading(answer: &Answer) -> &AnswerReading {
    answer
        .reading
        .as_ref()
        .unwrap_or_else(|| panic!("la respuesta debe traer lectura: {}", answer.text))
}

/// Primera frase del resumen. Corta por el punto y aparte, no por cualquier
/// punto: un importe o una fecha traen puntos propios.
fn first_sentence(text: &str) -> &str {
    let paragraph = text.split("\n\n").next().unwrap_or(text);
    match paragraph.find(". ") {
        Some(end) => &paragraph[..end + 1],
        None => paragraph,
    }
}

/// Toda la evidencia de un documento: sus campos y el contenido íntegro de
/// cada pasaje. Es contra esto —y sólo contra esto— que se puede afirmar algo
/// de ese documento.
fn document_evidence(tools: &ToolEngine, path: &str) -> Vec<Evidence> {
    let document_id = document_id(tools, path);
    let mut evidence = tools
        .document_text(document_id)
        .unwrap()
        .into_iter()
        .map(|passage| {
            let mut item = passage.evidence;
            item.excerpt = passage.content;
            item
        })
        .collect::<Vec<_>>();
    evidence.extend(
        tools
            .document_values(document_id)
            .unwrap()
            .into_iter()
            .map(|value| value.evidence),
    );
    evidence
}

fn document_id(tools: &ToolEngine, path: &str) -> i64 {
    tools
        .search(&file_name(path), &[], 50)
        .unwrap()
        .iter()
        .find(|hit| hit.evidence.path == path)
        .map(|hit| hit.evidence.document_id)
        .unwrap_or_else(|| panic!("el documento {path} debe existir en el índice"))
}

/// Las ubicaciones que el índice le puso a un documento —«línea 5», «párrafo
/// 2», «tabla 1, fila 12»—. Son coordenadas del índice, igual que el número
/// de una cita: no son contenido del documento y no pueden verificarse contra
/// él, así que se excluyen del recuento de cifras.
fn index_coordinates(tools: &ToolEngine, path: &str) -> Vec<String> {
    let document_id = document_id(tools, path);
    tools
        .document_values(document_id)
        .unwrap()
        .into_iter()
        .map(|value| value.evidence.location)
        .chain(
            tools
                .document_text(document_id)
                .unwrap()
                .into_iter()
                .map(|passage| passage.location),
        )
        .collect()
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

/// Cifras, importes, porcentajes e identificadores que aparecen en un texto:
/// exactamente la clase de dato que no se puede inventar. Es la misma forma
/// que protege `verifier::verify_model_answer` en la ruta del modelo.
fn quantities(text: &str) -> Vec<String> {
    // Sin `(?i)`: en una respuesta redactada, una cifra al final de una frase
    // seguida de una palabra en minúsculas («Leí 2 documentos») no es un
    // importe con moneda, y tratarla como tal sólo produce ruido.
    let pattern =
        Regex::new(r"(?:\$\s*)?\d[\d,.]*(?:\s*[A-Z]{3}|\s*%)?|[A-Z]{2,}-\d{2,}[\w-]*").unwrap();
    pattern
        .find_iter(text)
        .map(|found| found.as_str().trim().to_owned())
        .collect()
}

/// 1. La lectura no toca la respuesta. Fuera del crate no se puede tener el
///    mismo `Answer` antes y después de `attach` —el motor ya lo entrega
///    compuesto—, así que aquí se comprueba lo observable: el texto y las
///    citas son idénticos pregunta a pregunta, y nada del resumen se filtra
///    al cuerpo de la respuesta. La invariante estricta vive en la prueba
///    unitaria de `lectura.rs`.
#[test]
fn the_reading_never_changes_the_answer_or_its_citations() {
    let (_fixture, _home, engine) = single_record_corpus(&FIELDS);

    for question in [
        "¿Qué Alfa aparece en el registro REG-77-0001?",
        "¿Qué Épsilon aparece en el registro REG-77-0001?",
        "Muéstrame los documentos con Beta Activo.",
    ] {
        let direct = engine.ask(question).unwrap();
        let conversational = engine.ask_in_conversation(question, question).unwrap();

        assert_eq!(direct.text, conversational.text, "«{question}»");
        assert_eq!(
            direct.citations.len(),
            conversational.citations.len(),
            "«{question}»"
        );
        for (a, b) in direct.citations.iter().zip(&conversational.citations) {
            assert_eq!(a.id, b.id, "«{question}»");
            assert_eq!(a.excerpt, b.excerpt, "«{question}»");
        }

        let reading = reading(&direct);
        for sentence in reading
            .text
            .split(['\n', '.'])
            .map(str::trim)
            .filter(|part| part.chars().count() > 30)
        {
            assert!(
                !direct.text.contains(sentence),
                "el resumen no puede filtrarse a la respuesta: «{sentence}» en «{}»",
                direct.text
            );
        }
    }
}

/// 2. Un documento citado varias veces se lee una sola vez y conserva todos
///    sus números de cita: son los mismos con los que la interfaz numera la
///    evidencia.
#[test]
fn a_document_cited_twice_is_read_once_and_keeps_every_citation_number() {
    let (_fixture, _home, engine) = single_record_corpus(&FIELDS);
    let answer = engine
        .ask("¿Qué Gamma aparece en el registro REG-77-0001?")
        .unwrap();
    assert!(
        answer.citations.len() > 1,
        "la fixture debe citar el documento más de una vez: {:?}",
        answer.citations
    );
    let expected = (1..=answer.citations.len()).collect::<Vec<_>>();

    let reading = reading(&answer);
    assert_eq!(reading.documents.len(), 1, "{:?}", reading.documents);
    assert_eq!(reading.documents[0].citation_numbers, expected);
    assert!(
        reading.documents[0].passages_read > 0,
        "leer un documento es leer sus pasajes: {:?}",
        reading.documents[0]
    );
}

/// 3. El campo que respondió la pregunta abre el resumen. No se entierra al
///    final ni se deja al lector buscarlo.
#[test]
fn the_answering_field_opens_the_reading() {
    let (_fixture, _home, engine) = single_record_corpus(&FIELDS);

    for (field, value) in [
        (FIELDS[4], "Delegación Norte"),
        (FIELDS[2], "$1,200.00 MXN"),
        (FIELDS[1], "Activo"),
    ] {
        let question = format!("¿Qué {field} aparece en el registro REG-77-0001?");
        let answer = engine.ask(&question).unwrap();
        let opening = first_sentence(&reading(&answer).text);
        assert!(
            opening.contains(field) && opening.contains(value),
            "«{question}» debe abrir por {field}: {opening}"
        );
    }
}

/// 4. Un campo repetido con el mismo valor no se publica dos veces.
#[test]
fn a_field_repeated_with_the_same_value_is_published_once() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("acervo");
    fs::create_dir_all(&corpus).unwrap();
    // El mismo par campo/valor escrito dos veces, como ocurre cuando un
    // formato repite su encabezado en una carátula y en el cuerpo.
    fs::write(
        corpus.join("registro.md"),
        format!(
            "Alfa: REG-77-0001\nBeta: Activo\nGamma: $1,200.00 MXN\nBeta: Activo\nDelta: 2026-04-02\n\n{CLOSING}\n"
        ),
    )
    .unwrap();
    let (_home, engine) = engine_over(&corpus);

    let answer = engine
        .ask("¿Qué Gamma aparece en el registro REG-77-0001?")
        .unwrap();
    let text = &reading(&answer).text;
    assert_eq!(
        text.matches("Beta").count(),
        1,
        "el campo repetido se publica una sola vez: {text}"
    );
    assert_eq!(
        text.matches("Activo").count(),
        1,
        "el valor repetido se publica una sola vez: {text}"
    );
}

/// 5. Con veinte documentos citados se leen los veinte. El detalle de cada
///    uno se recorta —y se declara—, pero no se descarta ninguno.
#[test]
fn twenty_cited_documents_are_all_read() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("acervo");
    fs::create_dir_all(&corpus).unwrap();
    for number in 1..=20 {
        write_record(
            &corpus.join(format!("registro-{number:02}.md")),
            &FIELDS,
            number,
            "Activo",
        );
    }
    let (_home, engine) = engine_over(&corpus);

    // Un valor a secas cita cada documento que lo declara; un listado por
    // filtro recorta la muestra antes de llegar aquí, y lo que se quiere
    // comprobar es justamente que veinte citas se leen enteras.
    let answer = engine.ask("Delegación Norte").unwrap();
    assert_eq!(answer.citations.len(), 20, "{}", answer.text);

    let reading = reading(&answer);
    assert_eq!(reading.documents.len(), 20, "{}", reading.text);
    assert!(reading.text.contains("20"), "{}", reading.text);
    assert!(
        reading.truncated,
        "con veinte documentos el detalle se recorta y se declara: {}",
        reading.text
    );
    for number in 1..=20 {
        let name = format!("registro-{number:02}.md");
        assert!(
            reading.documents.iter().any(|document| document.path.ends_with(&name)),
            "falta {name} entre los documentos leídos"
        );
        assert!(reading.text.contains(&name), "falta {name} en el resumen");
    }
}

/// 6. Nada de lo que el resumen afirma de un documento está ausente de la
///    evidencia de ese documento. El candado es el mismo del resto del motor.
#[test]
fn every_quantity_in_the_reading_is_written_in_its_own_document() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("acervo");
    fs::create_dir_all(&corpus).unwrap();
    for number in 1..=3 {
        write_record(
            &corpus.join(format!("registro-{number:02}.md")),
            &FIELDS,
            number,
            if number == 1 { "Activo" } else { "Cerrado" },
        );
    }
    let (_home, engine) = engine_over(&corpus);
    let tools = ToolEngine::new(Database::open(engine.database_path()).unwrap());

    // Un documento solo y varios a la vez: son dos redacciones distintas y
    // las dos tienen que sostenerse.
    for question in [
        "¿Qué Épsilon aparece en el registro REG-77-0001?",
        "Muéstrame los documentos con Beta Cerrado.",
    ] {
        let answer = engine.ask(question).unwrap();
        let reading = reading(&answer);
        let union = reading
            .documents
            .iter()
            .flat_map(|document| document_evidence(&tools, &document.path))
            .collect::<Vec<_>>();

        let coordinates = reading
            .documents
            .iter()
            .flat_map(|document| index_coordinates(&tools, &document.path))
            .collect::<Vec<_>>();

        for line in reading.text.lines() {
            // Una línea que nombra un documento sólo puede hablar de él; el
            // resto del texto habla del conjunto.
            let owner = reading
                .documents
                .iter()
                .find(|document| line.contains(&file_name(&document.path)));
            let evidence = match owner {
                Some(document) => document_evidence(&tools, &document.path),
                None => union.clone(),
            };
            let refs = evidence.iter().collect::<Vec<_>>();
            // Frase a frase: una cifra al final de una frase y una palabra en
            // mayúsculas al principio de la siguiente no forman un valor.
            for quantity in line.split(". ").flat_map(quantities) {
                // Ni el número de una cita ni la ubicación de un valor son
                // contenido del documento: los pone el índice.
                let coordinate = coordinates
                    .iter()
                    .any(|location| location.contains(quantity.trim_end_matches('.')));
                if coordinate
                    || line.contains(&format!("cita {quantity}"))
                    || line.contains(&format!("citas {quantity}"))
                    || line.contains(&format!(", {quantity},"))
                {
                    continue;
                }
                assert!(
                    omega_core::value_is_supported(&refs, &quantity),
                    "«{quantity}» no está en la evidencia del documento del que se dice.\nLínea: {line}"
                );
            }
        }
    }
}

/// 7. La prueba de universalidad: el mismo acervo con los campos renombrados
///    produce el mismo resumen, palabra por palabra, salvo los nombres. Si
///    alguna frase dependiera de lo que un rótulo significa, las dos
///    redacciones se separarían aquí.
#[test]
fn renaming_every_field_does_not_degrade_the_reading() {
    let (_fixture, _home, original) = single_record_corpus(&FIELDS);
    let (_other_fixture, _other_home, renamed) = single_record_corpus(&RENAMED);

    let first = original
        .ask(&format!("¿Qué {} aparece en el registro REG-77-0001?", FIELDS[4]))
        .unwrap();
    let second = renamed
        .ask(&format!("¿Qué {} aparece en el registro REG-77-0001?", RENAMED[4]))
        .unwrap();

    let mut translated = reading(&first).text.clone();
    for (before, after) in FIELDS.iter().zip(RENAMED) {
        translated = translated.replace(before, after);
    }
    assert_eq!(
        translated,
        reading(&second).text,
        "el resumen no puede depender de cómo se llamen los campos"
    );
    assert_eq!(
        reading(&first).documents.len(),
        reading(&second).documents.len()
    );
}

/// 8. Dos acervos de rubros distintos, el mismo binario y la misma calidad.
///    Se comprueba la estructura, que es lo único comparable entre rubros: el
///    campo que responde, lo que el documento declara y su cierre en prosa.
#[test]
fn two_corpora_of_different_domains_are_read_with_the_same_quality() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();

    for (corpus, question, field) in [
        (
            "corpus-prueba-ferreteria",
            "¿Qué Proveedor aparece en la orden de compra FER-26-0030?",
            "Proveedor",
        ),
        (
            "corpus-prueba-despacho-legal",
            "¿Qué Cliente aparece en el registro de tiempo profesional DLG-26-0084?",
            "Cliente",
        ),
    ] {
        let (_home, engine) = engine_over(&root.join(corpus));
        let answer = engine.ask(question).unwrap();
        let reading = reading(&answer);

        assert!(
            first_sentence(&reading.text).contains(field),
            "{corpus}: el campo que responde debe abrir el resumen: {}",
            reading.text
        );
        assert!(
            reading.text.contains("Cierra con: «"),
            "{corpus}: falta el cierre en prosa: {}",
            reading.text
        );
        assert!(
            reading.text.split("\n\n").count() >= 3,
            "{corpus}: el resumen debe tener identidad, cuerpo y cierre: {}",
            reading.text
        );
        assert_eq!(reading.documents.len(), 1, "{corpus}: {:?}", reading.documents);
        assert!(
            reading.documents[0].passages_read > 0 && reading.documents[0].reliable,
            "{corpus}: {:?}",
            reading.documents[0]
        );
    }
}
