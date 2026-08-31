//! Ronda 7 — censo del acervo: cuántos ARCHIVOS hay, no cuántos valores se
//! pudieron leer.
//!
//! El defecto que motiva estas pruebas tenía dos caras, y las dos eran de
//! honestidad, no de cobertura:
//!
//!  1. «¿Cuántos documentos cumplen el filtro area=rh, kind=vacaciones?» se
//!     contestaba «984 documentos cumplen simultáneamente los criterios
//!     (carpeta = rh)»: el filtro que el motor no supo leer (`kind=`) se caía
//!     en silencio y la respuesta seguía diciendo «simultáneamente los
//!     criterios», en plural, como si los hubiera aplicado los dos.
//!  2. «¿Cuántos documentos hay en el área X?» daba el número de documentos
//!     INDEXADOS sin decirlo, cuando el índice sí sabe cuántos archivos
//!     descubrió y no pudo leer (`unindexed_documents`).
//!
//! La ronda 1 había decidido no dar cifras exactas en conteos amplios (F4
//! opción (a)) porque «en tiempo de consulta Omega no tiene registro de qué
//! documentos del alcance no logró indexar». Esa tabla existe desde entonces,
//! así que el motivo dejó de aplicar **para los conteos de archivo** — y sigue
//! aplicando, intacto, para los conteos por campo extraído, que es justo lo que
//! comprueba la última prueba de este archivo.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// Un filtro que el motor no sabe aplicar no puede caerse en silencio.
#[test]
fn a_filter_the_engine_cannot_read_stops_the_count_instead_of_disappearing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    for index in 0..4 {
        fs::write(
            root.join(format!("rh/{index:05}_vacaciones.md")),
            format!("Empleado: E-{index}\nEstado: Aprobado\n"),
        )
        .unwrap();
    }

    let engine = index(root, "censo-1");
    let answer = engine
        .ask("¿Cuántos documentos cumplen el filtro area=rh, color=azul?")
        .unwrap();

    assert!(
        !answer.text.contains("cumplen simultáneamente los criterios (carpeta = rh)"),
        "un filtro no entendido no puede desaparecer dejando la respuesta en plural: {}",
        answer.text
    );
}

/// El tipo de documento sale del nombre del archivo, y el conteo lo dice.
#[test]
fn counting_by_document_type_says_it_read_the_file_name() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    for index in 0..4 {
        fs::write(
            root.join(format!("rh/{index:05}_vacaciones.md")),
            format!("Empleado: E-{index}\nEstado: Aprobado\n"),
        )
        .unwrap();
    }
    for index in 10..13 {
        fs::write(
            root.join(format!("rh/{index:05}_nomina.md")),
            format!("Empleado: E-{index}\nEstado: Pagado\n"),
        )
        .unwrap();
    }

    let engine = index(root, "censo-2");
    let answer = engine
        .ask("¿Cuántos documentos cumplen el filtro area=rh, kind=vacaciones?")
        .unwrap();

    assert!(
        answer.text.starts_with("4 documentos"),
        "el conteo por tipo es exacto: {}",
        answer.text
    );
    assert!(
        answer.text.contains("nombre del archivo"),
        "contar por el nombre del archivo se declara, no se presenta como contenido leído: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "contar archivos no es haber leído lo que dicen"
    );
}

/// Los archivos que no se pudieron leer entran en el total y se declaran.
#[test]
fn the_total_counts_the_files_that_could_not_be_read_and_says_how_many() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    for index in 0..5 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.md")),
            format!("Folio: P-{index}\nEstado: Abierto\n"),
        )
        .unwrap();
    }
    // Descubierto y no indexable: sin contenido extraíble.
    fs::write(root.join("ventas/00009_pedido.md"), "   \n\n").unwrap();

    let engine = index(root, "censo-3");
    let answer = engine
        .ask("¿Cuántos documentos totales conforman el corpus completo?")
        .unwrap();

    assert!(
        answer.text.starts_with("6 documentos"),
        "el total incluye el archivo que no se pudo leer: {}",
        answer.text
    );
    assert!(
        answer.text.contains("5 se pudieron indexar y 1 no"),
        "la partición se declara, no se deja deducir: {}",
        answer.text
    );
}

/// La composición por tipo reparte el total sin perder lo ilegible.
#[test]
fn the_breakdown_by_type_adds_up_to_the_declared_total() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("servicio")).unwrap();
    for index in 0..3 {
        fs::write(
            root.join(format!("servicio/{index:05}_devolucion.md")),
            format!("Folio: DEV-{index}\nEstado: Abierto\n"),
        )
        .unwrap();
    }
    for index in 10..12 {
        fs::write(
            root.join(format!("servicio/{index:05}_ticket_servicio.md")),
            format!("Folio: TIC-{index}\nEstado: Cerrado\n"),
        )
        .unwrap();
    }
    fs::write(root.join("servicio/00020_devolucion.md"), "   \n\n").unwrap();

    let engine = index(root, "censo-4");
    let answer = engine
        .ask("Resume la composición documental del área servicio: ¿cuántos documentos de cada tipo existen?")
        .unwrap();

    assert!(
        answer.text.starts_with("6 documentos"),
        "el total del área incluye lo ilegible: {}",
        answer.text
    );
    assert!(
        answer.text.contains("| devolucion | 4 | 3 | 1 |"),
        "cada tipo declara descubiertos, indexados y sin indexar: {}",
        answer.text
    );
    assert!(
        answer.text.contains("| ticket_servicio | 2 | 2 | 0 |"),
        "{}",
        answer.text
    );
}

/// Un valor de campo puede nombrar la carpeta, pero sólo si el índice lo
/// demuestra: los documentos que lo escriben tienen que estar todos en ella.
#[test]
fn a_field_value_names_a_folder_only_when_the_index_proves_it_does() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("rh")).unwrap();
    for index in 0..4 {
        fs::write(
            root.join(format!("rh/{index:05}_vacaciones.md")),
            format!("Área: Recursos humanos y capacitación\nEmpleado: E-{index}\n"),
        )
        .unwrap();
    }
    fs::write(root.join("rh/00009_vacaciones.md"), "   \n\n").unwrap();

    let engine = index(root, "censo-5");
    let answer = engine
        .ask("¿Cuántos documentos totales pertenecen al área de Recursos humanos y capacitación en todo el corpus?")
        .unwrap();

    assert!(
        answer.text.starts_with("5 documentos"),
        "la carpeta se identificó por el valor y el total la incluye entera: {}",
        answer.text
    );
    assert!(
        answer.text.contains("no es el nombre de la carpeta"),
        "de dónde salió la correspondencia se dice: {}",
        answer.text
    );
    assert!(
        answer.text.contains("no puedo afirmar que los 5 archivos lo registren"),
        "no se afirma que lo ilegible registre el valor: {}",
        answer.text
    );
}

/// El censo NO se queda con los conteos por contenido: ahí la cifra sigue
/// siendo la de lo que se logró leer, y ésa es la parte de F4 que no cambió.
#[test]
fn a_count_over_an_extracted_field_is_still_a_content_count() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("ventas")).unwrap();
    for index in 0..3 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.md")),
            format!("Moneda: EUR\nFolio: P-{index}\n"),
        )
        .unwrap();
    }
    for index in 10..12 {
        fs::write(
            root.join(format!("ventas/{index:05}_pedido.md")),
            format!("Moneda: MXN\nFolio: P-{index}\n"),
        )
        .unwrap();
    }

    let engine = index(root, "censo-6");
    let answer = engine
        .ask("¿Cuántos documentos en total tienen Moneda: EUR?")
        .unwrap();

    assert!(
        !answer.text.contains("nombre del archivo"),
        "un filtro de contenido no se contesta contando archivos: {}",
        answer.text
    );
    assert!(
        answer.text.contains('3'),
        "el conteo por campo sigue siendo el de siempre: {}",
        answer.text
    );
}

// ─────────────────────────────────────────────────────────────────────────

fn index(root: &Path, name: &str) -> OmegaEngine {
    let engine = OmegaEngine::open_with_clock(
        root.join(format!("omega-{name}.db")),
        Clock::fixed(TODAY).unwrap(),
    )
    .unwrap();
    let source = engine.authorize_source(root).unwrap();
    let report = engine.index_source(source).unwrap();
    assert!(report.indexed > 0, "la fixture debe indexarse");
    engine
}
