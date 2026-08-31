//! Ronda 9 — un importe negativo donde el campo nunca lo es.
//!
//! Un importe negativo en una orden de compra, una factura o un vale de
//! almacén casi nunca es un dato: es un signo que se coló en la captura, una
//! exportación que invirtió la columna o un OCR que leyó un guion de más.
//! Omega lo devolvía como valor y, además, **declaraba la respuesta
//! verificada**.
//!
//! Pero «negativo» a secas no puede ser el criterio: hay campos donde el signo
//! es parte del oficio —un ajuste de saldo, una nota de crédito, una
//! devolución, una desviación contra presupuesto— y ahí el valor negativo es
//! exactamente el dato correcto. Por eso la regla no mira el signo suelto:
//! mira si **ese campo, en este acervo**, se usa alguna vez en negativo.
//!
//! Las pruebas usan a propósito vocabularios de giros distintos —una
//! panadería, una cooperativa de ahorro, un taller— y ninguno de ellos aparece
//! en la lógica: lo único que la regla consulta es el índice, al responder.

use std::{fs, path::Path};

use omega_core::{Clock, OmegaEngine};

const TODAY: &str = "2026-08-30";

/// Panadería: 24 vales de insumo, y en uno solo el costo salió negativo.
/// Ningún otro valor del campo lo es, así que el signo es una rareza: se
/// reporta el valor —para que se pueda ir a corregirlo— pero **no se da por
/// bueno** y la respuesta no queda verificada.
#[test]
fn a_negative_where_the_field_never_is_gets_flagged_and_is_not_verified() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("almacen")).unwrap();
    for index in 0..24 {
        let amount = if index == 7 {
            "-$4,310.75 MXN".to_owned()
        } else {
            format!("${}.50 MXN", 1_200 + index * 37)
        };
        fs::write(
            root.join(format!("almacen/{:05}_vale.md", 100 + index)),
            format!(
                "# Vale de insumo\n\n\
                 - **Panificadora:** Horno de Piedra\n\
                 - **VALE:** VALE-2025-{:05}\n\
                 - **Costo de insumo:** {amount}\n\
                 - **Encargado:** Rosalba Ceniceros Pinto\n",
                index + 1
            ),
        )
        .unwrap();
    }

    let engine = index(root, "panaderia");
    let answer = engine
        .ask("En el documento con folio VALE-2025-00008 (vale, área Almacén), ¿cuál es el valor del campo \"Costo de insumo\"?")
        .unwrap();

    assert!(
        answer.text.contains("-$4,310.75"),
        "el valor se muestra, para que se pueda ir a corregirlo: {}",
        answer.text
    );
    assert!(
        answer.text.contains("negativo"),
        "y se dice que el signo es el problema: {}",
        answer.text
    );
    assert!(
        !answer.verified,
        "nunca se declara verificado un valor que el propio motor señala como dudoso: {}",
        answer.text
    );
}

/// Cooperativa de ahorro: el ajuste de saldo es negativo **por oficio**, y una
/// cuarta parte de los registrados lo son. Ahí un negativo no es una rareza y
/// llamarlo sospechoso sería acusar de inválido a un dato correcto: se
/// responde como cualquier otro valor.
#[test]
fn a_negative_in_a_field_that_is_routinely_negative_is_answered_normally() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("socios")).unwrap();
    for index in 0..24 {
        let amount = if index % 4 == 0 {
            format!("-${}.00 MXN", 500 + index * 11)
        } else {
            format!("${}.00 MXN", 700 + index * 13)
        };
        fs::write(
            root.join(format!("socios/{:05}_movimiento.md", 300 + index)),
            format!(
                "# Movimiento de cuenta\n\n\
                 - **Cooperativa:** Caja Solidaria del Bajío\n\
                 - **MOV:** MOV-2025-{:05}\n\
                 - **Ajuste de saldo:** {amount}\n\
                 - **Cajero:** Fidencio Aparicio Rentería\n",
                index + 1
            ),
        )
        .unwrap();
    }

    let engine = index(root, "cooperativa");
    let answer = engine
        .ask("En el documento con folio MOV-2025-00001 (movimiento, área Socios), ¿cuál es el valor del campo \"Ajuste de saldo\"?")
        .unwrap();

    assert!(
        answer.text.contains("-$500.00"),
        "el valor negativo es el dato: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("sospechoso"),
        "y no se le acusa de nada: en este campo el signo es normal: {}",
        answer.text
    );
    assert!(
        answer.verified,
        "así que la respuesta conserva su sello: {}",
        answer.text
    );
}

/// Taller: sólo cinco documentos registran el campo. Con tan pocos valores no
/// hay costumbre que contradecir —un negativo no es una rareza, es uno de los
/// primeros datos— y Omega no inventa una norma que el acervo no tiene.
#[test]
fn a_field_with_too_few_values_does_not_support_calling_a_sign_anomalous() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("servicio")).unwrap();
    for index in 0..5 {
        let amount = if index == 2 {
            "-$980.00 MXN".to_owned()
        } else {
            format!("${}.00 MXN", 1_500 + index * 40)
        };
        fs::write(
            root.join(format!("servicio/{:05}_hojalateria.md", 700 + index)),
            format!(
                "# Presupuesto de hojalatería\n\n\
                 - **Taller:** Carrocerías del Norte\n\
                 - **HOJ:** HOJ-2025-{:05}\n\
                 - **Margen del presupuesto:** {amount}\n\
                 - **Valuador:** Ambrosio Nájera Quintanilla\n",
                index + 1
            ),
        )
        .unwrap();
    }

    let engine = index(root, "taller");
    let answer = engine
        .ask("En el documento con folio HOJ-2025-00003 (hojalateria, área Servicio), ¿cuál es el valor del campo \"Margen del presupuesto\"?")
        .unwrap();

    assert!(
        answer.text.contains("-$980.00"),
        "el valor se responde: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("sospechoso"),
        "sin declararlo anómalo contra un historial que no existe: {}",
        answer.text
    );
}

/// La misma rareza, preguntada por la **clave interna de indexación** en vez
/// de por el folio. Las dos rutas tienen que decir lo mismo: el candado no
/// puede depender de cómo se nombró el documento.
#[test]
fn the_same_flag_applies_when_the_document_is_named_by_its_internal_key() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("almacen")).unwrap();
    for index in 0..24 {
        let amount = if index == 7 {
            "-$4,310.75 MXN".to_owned()
        } else {
            format!("${}.50 MXN", 1_200 + index * 37)
        };
        fs::write(
            root.join(format!("almacen/{:05}_vale.md", 100 + index)),
            format!(
                "# Vale de insumo\n\n\
                 - **Panificadora:** Horno de Piedra\n\
                 - **VALE:** VALE-2025-{:05}\n\
                 - **Costo de insumo:** {amount}\n\
                 - **Encargado:** Rosalba Ceniceros Pinto\n",
                index + 1
            ),
        )
        .unwrap();
    }

    let engine = index(root, "panaderia-clave");
    let answer = engine
        .ask("¿Cuál es el valor del campo \"Costo de insumo\" en el documento D00107?")
        .unwrap();

    assert!(
        answer.text.contains("-$4,310.75") && answer.text.contains("negativo"),
        "la ruta de clave interna señala el mismo problema: {}",
        answer.text
    );
    assert!(!answer.verified, "y tampoco lo da por verificado: {}", answer.text);
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
