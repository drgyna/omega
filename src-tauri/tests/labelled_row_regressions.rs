//! Ronda 8 — nombrar una fila de tabla por su etiqueta.
//!
//! Una tabla con encabezado se indexa por sus encabezados, que es la lectura
//! correcta: la fila `Tornillo hexagonal 3/8 | 250 | piezas` bajo
//! `Artículo | Existencia | Unidad` queda como Artículo=«Tornillo hexagonal
//! 3/8», Existencia=250, Unidad=piezas.
//!
//! Pero nadie pregunta por el encabezado. Se pregunta «¿cuál es el valor de
//! "Tornillo hexagonal 3/8"?», nombrando la fila por su etiqueta. Las dos
//! formas señalan la misma casilla del mismo papel, y Omega sólo entendía una:
//! el dato estaba indexado y citado, y aun así la respuesta era «no encontré
//! evidencia» o una muestra de búsqueda.
//!
//! Estas pruebas usan a propósito vocabularios de giros DISTINTOS —una
//! ferretería, una notaría, un despacho— y ninguno de ellos aparece en la
//! lógica: la regla se apoya sólo en el orden de extracción y en que una
//! columna etiqueta se repite una vez por fila.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// Ferretería: tabla de dos columnas. La fila tiene una sola celda, así que
/// la respuesta es exactamente el valor pedido.
#[test]
fn a_two_column_table_answers_with_the_single_cell() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("almacen")).unwrap();
    fs::write(
        root.join("almacen/00110_inventario.csv"),
        "Artículo,Costo\n\
         Tornillo hexagonal 3/8,18.40\n\
         Taquete de expansión 1/4,6.25\n\
         Broca para concreto reforzada,74.90\n\
         REF,REF-2024-00010\n",
    )
    .unwrap();

    let engine = index(root, "ferreteria");
    let answer = engine
        .ask("En el documento con folio REF-2024-00010 (inventario, área Almacén), ¿cuál es el valor del campo \"Broca para concreto reforzada\"?")
        .unwrap();

    assert!(
        answer.text.contains("74.90"),
        "la etiqueta de la fila lleva a su celda: {}",
        answer.text
    );
    assert!(!answer.citations.is_empty(), "y va citada");
}

/// Notaría: tabla más ancha. No se elige columna por el usuario —la pregunta
/// no dice cuál— así que se devuelve la fila entera, cada celda con el nombre
/// de su columna.
#[test]
fn a_wider_table_answers_with_the_whole_row() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("protocolo")).unwrap();
    fs::write(
        root.join("protocolo/00220_arancel.csv"),
        "Concepto,Importe,Base,Observaciones\n\
         Derechos de registro,3200.00,Arancel A,Sujeto a avalúo\n\
         Cotejo de documentos,480.00,Arancel B,Por foja\n\
         ESC,ESC-2024-00031,-,-\n",
    )
    .unwrap();

    let engine = index(root, "notaria");
    let answer = engine
        .ask("En el documento con folio ESC-2024-00031 (arancel, área Protocolo), ¿cuál es el valor del campo \"Cotejo de documentos\"?")
        .unwrap();

    assert!(
        answer.text.contains("480.00"),
        "la fila trae su importe: {}",
        answer.text
    );
    assert!(
        answer.text.contains("Por foja"),
        "y el resto de su fila, sin elegir por el usuario: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("3200.00"),
        "pero no se cuela la fila de al lado: {}",
        answer.text
    );
}

/// La misma etiqueta encabezando varias filas no tiene una respuesta: no hay
/// forma de saber a cuál de ellas se refiere la pregunta, y elegir sería
/// adivinar.
#[test]
fn a_label_repeated_in_several_rows_is_not_answered() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("obra")).unwrap();
    fs::write(
        root.join("obra/00330_bitacora.csv"),
        "Jornada,Turno,Avance\n\
         12 de marzo de 2025,Matutino,Cimentación\n\
         12 de marzo de 2025,Vespertino,Armado\n\
         12 de marzo de 2025,Nocturno,Colado\n\
         OBRA,OBRA-2025-00007,-\n",
    )
    .unwrap();

    let engine = index(root, "repetida");
    let answer = engine
        .ask("En el documento con folio OBRA-2025-00007 (bitacora, área Obra), ¿cuál es el valor del campo \"12 de marzo de 2025\"?")
        .unwrap();

    assert!(
        !answer.text.contains("Matutino")
            && !answer.text.contains("Vespertino")
            && !answer.text.contains("Nocturno"),
        "tres filas comparten la etiqueta: no se elige ninguna: {}",
        answer.text
    );
}

/// Un valor de carátula no es la etiqueta de una fila. Su columna aparece una
/// sola vez en el documento, así que no encabeza ninguna tabla y la vía no se
/// abre.
#[test]
fn a_cover_value_is_not_treated_as_a_row_label() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("despacho")).unwrap();
    fs::write(
        root.join("despacho/00440_expediente.md"),
        "# Expediente\n\n\
         - **Empresa:** Bufete Ejemplo\n\
         - **Materia:** Mercantil\n\
         - **EXP:** EXP-2025-00044\n\
         - **Responsable:** Práxedes Alcántara Vidal\n",
    )
    .unwrap();

    let engine = index(root, "caratula");
    let answer = engine
        .ask("En el documento con folio EXP-2025-00044 (expediente, área Despacho), ¿cuál es el valor del campo \"Mercantil\"?")
        .unwrap();

    assert!(
        !answer.text.contains("Práxedes Alcántara Vidal"),
        "un valor de carátula no arrastra el resto del documento como si fuera su fila: {}",
        answer.text
    );
}

/// Si la columna etiqueta no es la PRIMERA de su tabla, cortar «hasta la
/// siguiente aparición» recogería el final de esta fila y el principio de la
/// siguiente. Ese caso se detecta y no se contesta.
#[test]
fn a_label_column_that_is_not_the_first_is_refused() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("taller")).unwrap();
    // «Refacción» es la segunda columna: su fila real no se puede delimitar
    // con la regla de corte, así que la vía debe cerrarse.
    fs::write(
        root.join("taller/00550_ordenes.csv"),
        "Orden,Refacción,Costo\n\
         A-1,Balero delantero,910.00\n\
         A-2,Banda de distribución,1450.00\n\
         SRV,SRV-2025-00012,-\n",
    )
    .unwrap();

    let engine = index(root, "columna-interior");
    let answer = engine
        .ask("En el documento con folio SRV-2025-00012 (ordenes, área Taller), ¿cuál es el valor del campo \"Balero delantero\"?")
        .unwrap();

    assert!(
        !answer.text.contains("A-2"),
        "no se mezcla el final de una fila con el principio de la siguiente: {}",
        answer.text
    );
}

// ── Ronda 9: la misma capacidad, por clave interna de indexación ─────────
//
// La ronda 8 conectó la vía de fila etiquetada sólo en la ruta que resuelve el
// documento por un folio ESCRITO en la pregunta. La ruta que lo localiza por
// su clave interna de indexación (`D#####`) no la tenía, aunque es la misma
// pregunta sobre la misma casilla del mismo papel. Estas pruebas fijan que las
// dos rutas contesten igual — y que la de clave interna herede las mismas
// cuatro exigencias, no una versión relajada de ellas.

/// Imprenta, tabla de dos columnas, preguntada por `D#####`.
#[test]
fn the_internal_key_route_also_resolves_a_labelled_row() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("produccion")).unwrap();
    fs::write(
        root.join("produccion/00660_tiraje.csv"),
        "Trabajo,Millares\n\
         Volante media carta selección,12.5\n\
         Engargolado con espiral doble,3.75\n\
         Lona tensada para exhibidor,0.5\n",
    )
    .unwrap();

    let engine = index(root, "imprenta");
    let answer = engine
        .ask("¿Cuál es el valor del campo \"Engargolado con espiral doble\" en el documento D00660?")
        .unwrap();

    assert!(
        answer.text.contains("3.75"),
        "la etiqueta de la fila lleva a su celda también por la clave interna: {}",
        answer.text
    );
    assert!(!answer.citations.is_empty(), "y va citada");
}

/// Clínica veterinaria, tabla más ancha: se devuelve la fila entera, sin
/// elegir columna por el usuario y sin arrastrar la fila de al lado.
#[test]
fn the_internal_key_route_answers_with_the_whole_row() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("clinica")).unwrap();
    fs::write(
        root.join("clinica/00770_tarifario.csv"),
        "Servicio,Costo,Duración,Notas\n\
         Desparasitación interna,340.00,20 minutos,Repetir a los 15 días\n\
         Profilaxis dental canina,1890.00,90 minutos,Requiere ayuno previo\n",
    )
    .unwrap();

    let engine = index(root, "veterinaria");
    let answer = engine
        .ask("¿Cuál es el valor del campo \"Profilaxis dental canina\" en el documento D00770?")
        .unwrap();

    assert!(
        answer.text.contains("1890.00") && answer.text.contains("Requiere ayuno previo"),
        "la fila entera, cada celda con el nombre de su columna: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Repetir a los 15 días"),
        "y no se cuela la fila de al lado: {}",
        answer.text
    );
}

/// Exigencia 1, intacta en la ruta nueva: una etiqueta que encabeza varias
/// filas no tiene una respuesta, y elegir una sería adivinar.
#[test]
fn the_internal_key_route_refuses_a_label_repeated_in_several_rows() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("vivero")).unwrap();
    fs::write(
        root.join("vivero/00880_riego.csv"),
        "Jornada,Sector,Litros\n\
         04 de abril de 2025,Invernadero A,820\n\
         04 de abril de 2025,Invernadero B,610\n\
         04 de abril de 2025,Vivero exterior,1450\n",
    )
    .unwrap();

    let engine = index(root, "vivero");
    let answer = engine
        .ask("¿Cuál es el valor del campo \"04 de abril de 2025\" en el documento D00880?")
        .unwrap();

    assert!(
        !answer.text.contains("Invernadero A")
            && !answer.text.contains("Invernadero B")
            && !answer.text.contains("Vivero exterior"),
        "tres filas comparten la etiqueta: no se elige ninguna: {}",
        answer.text
    );
}

/// Exigencia 4, intacta en la ruta nueva: si la columna etiqueta no es la
/// primera de su tabla, lo que se recogería serían dos medias filas.
#[test]
fn the_internal_key_route_refuses_a_label_column_that_is_not_the_first() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("panaderia")).unwrap();
    fs::write(
        root.join("panaderia/00990_produccion.csv"),
        "Turno,Producto,Piezas\n\
         Matutino,Concha de vainilla,240\n\
         Vespertino,Oreja de hojaldre,180\n",
    )
    .unwrap();

    let engine = index(root, "columna-interior-clave");
    let answer = engine
        .ask("¿Cuál es el valor del campo \"Concha de vainilla\" en el documento D00990?")
        .unwrap();

    assert!(
        !answer.text.contains("Vespertino"),
        "no se mezcla el final de una fila con el principio de la siguiente: {}",
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
