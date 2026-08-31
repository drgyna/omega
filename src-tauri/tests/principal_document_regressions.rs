//! Ronda 8 — desempate del documento principal cuando dos documentos
//! mencionan el mismo folio.
//!
//! `answer.rs::principal_document` prefería el documento donde el
//! identificador aparece antes (`ordinal`) y, a igualdad, el que menciona
//! menos identificadores distintos. Contra el acervo real esas dos señales
//! empataban en las 18 preguntas afectadas: los dos candidatos eran carátulas
//! que registran el folio en la MISMA posición, así que el motor no elegía
//! ninguno y contestaba «ninguno se distingue como el registro principal».
//!
//! Esta ronda añade dos señales más, ambas derivadas del índice en tiempo de
//! consulta y ninguna de una lista escrita en el código:
//!
//!   * el tipo de documento que la pregunta nombra, leído del nombre del
//!     archivo con `census::kind_of_path`;
//!   * la coincidencia unánime: si TODOS los candidatos registran el mismo
//!     valor del campo pedido, elegir deja de hacer falta.
//!
//! Y deja intacto lo que ya funcionaba: si ninguna de las señales desempata,
//! la respuesta sigue siendo «sin concluir». No se adivina para contestar.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// La señal que ya existía: el folio aparece en la carátula de un documento
/// (posición baja) y dentro de una fila de tabla del otro (posición alta).
/// Gana la carátula, sin que las señales nuevas intervengan.
#[test]
fn the_earlier_position_still_decides_the_principal_document() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("area")).unwrap();
    // Carátula: el folio es el segundo campo del documento.
    fs::write(
        root.join("area/00101_ficha_registro.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00001\nResponsable: Amparo Nieves Olmedo\n",
    )
    .unwrap();
    // Listado: el folio aparece mucho más abajo, dentro de una fila.
    fs::write(
        root.join("area/00102_listado_mensual.md"),
        "Empresa: Grupo Ejemplo\nÁrea: Operaciones\nPeriodo: 2024\nTotal: 12\n\
         Concepto: consolidado\nNota: sin incidencias\nOrigen: interno\n\
         Revisión: anual\nEstado: cerrado\nREF: REF-2024-00001\n\
         Responsable: Bernardo Quiroz Salas\n",
    )
    .unwrap();

    let engine = index(root, "posicion");
    let answer = engine
        .ask("En el documento con folio REF-2024-00001 (ficha_registro, área Operaciones), ¿cuál es el valor del campo \"Responsable\"?")
        .unwrap();

    assert!(
        answer.text.contains("Amparo Nieves Olmedo"),
        "gana el documento que registra el folio antes: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Bernardo Quiroz Salas"),
        "no gana el documento que sólo lo menciona de pasada: {}",
        answer.text
    );
}

/// La señal nueva: dos carátulas con el folio en la MISMA posición, de tipos
/// distintos. La pregunta nombra uno de los dos tipos y ése es el que manda.
#[test]
fn the_kind_named_by_the_question_breaks_a_tie_in_position() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("area")).unwrap();
    fs::write(
        root.join("area/00201_comprobante_pago.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00002\nResponsable: Casilda Vera Montes\n",
    )
    .unwrap();
    fs::write(
        root.join("area/00202_nota_entrega.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00002\nResponsable: Damián Cortés Alba\n",
    )
    .unwrap();

    let engine = index(root, "tipo");
    let answer = engine
        .ask("En el documento con folio REF-2024-00002 (nota_entrega, área Operaciones), ¿cuál es el valor del campo \"Responsable\"?")
        .unwrap();

    assert!(
        answer.text.contains("Damián Cortés Alba"),
        "manda el tipo que la pregunta nombra: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("Casilda Vera Montes"),
        "no gana el documento del otro tipo: {}",
        answer.text
    );
}

/// Empate en posición y en tipo, pero los dos documentos registran el MISMO
/// valor: elegir deja de hacer falta, y la respuesta lo dice.
#[test]
fn a_value_every_document_agrees_on_is_answered_without_choosing() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("area")).unwrap();
    fs::write(
        root.join("area/00301_acta_revision.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00003\nResponsable: Eulalia Prado Ochoa\n",
    )
    .unwrap();
    fs::write(
        root.join("area/00302_acta_revision.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00003\nResponsable: Eulalia Prado Ochoa\n",
    )
    .unwrap();

    let engine = index(root, "unanime");
    let answer = engine
        .ask("En el documento con folio REF-2024-00003 (acta_revision, área Operaciones), ¿cuál es el valor del campo \"Responsable\"?")
        .unwrap();

    assert!(
        answer.text.contains("Eulalia Prado Ochoa"),
        "si todos coinciden, el valor no depende de cuál se elija: {}",
        answer.text
    );
    assert!(
        answer.text.contains("coinciden"),
        "la respuesta declara por qué no hizo falta elegir: {}",
        answer.text
    );
}

/// Empate genuino: misma posición, mismo tipo y valores DISTINTOS. Ninguna
/// señal desempata y la respuesta sigue siendo «sin concluir». Es el límite
/// que esta ronda deja declarado en vez de adivinar.
#[test]
fn a_genuine_tie_is_still_left_unresolved() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("area")).unwrap();
    fs::write(
        root.join("area/00401_acta_revision.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00004\nResponsable: Fabiola Rentería Cruz\n",
    )
    .unwrap();
    fs::write(
        root.join("area/00402_acta_revision.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00004\nResponsable: Gonzalo Tirado Peña\n",
    )
    .unwrap();

    let engine = index(root, "empate");
    let answer = engine
        .ask("En el documento con folio REF-2024-00004 (acta_revision, área Operaciones), ¿cuál es el valor del campo \"Responsable\"?")
        .unwrap();

    assert!(
        !answer.text.contains("Fabiola Rentería Cruz")
            && !answer.text.contains("Gonzalo Tirado Peña"),
        "un empate genuino no se resuelve adivinando: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "y desde luego no se sella como verificada: {}",
        answer.text
    );
}

/// El tipo sólo desempata si la pregunta nombra el de EXACTAMENTE uno de los
/// candidatos. Si nombra un tipo que ninguno tiene, el empate sigue siendo un
/// empate: la señal no puede inventarse un ganador.
#[test]
fn a_kind_no_candidate_has_does_not_break_the_tie() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("area")).unwrap();
    fs::write(
        root.join("area/00501_comprobante_pago.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00005\nResponsable: Herminia Lozano Ruiz\n",
    )
    .unwrap();
    fs::write(
        root.join("area/00502_nota_entrega.md"),
        "Empresa: Grupo Ejemplo\nREF: REF-2024-00005\nResponsable: Ismael Bravo Cañas\n",
    )
    .unwrap();

    let engine = index(root, "tipo-ausente");
    let answer = engine
        .ask("En el documento con folio REF-2024-00005 (acta_revision, área Operaciones), ¿cuál es el valor del campo \"Responsable\"?")
        .unwrap();

    assert!(
        !answer.text.contains("Herminia Lozano Ruiz")
            && !answer.text.contains("Ismael Bravo Cañas"),
        "un tipo que ningún candidato tiene no elige por nadie: {}",
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
