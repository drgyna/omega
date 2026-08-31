//! Redacción de los resultados del razonamiento local.
//!
//! Son funciones puras: reciben datos ya calculados y devuelven texto. No
//! consultan la base ni deciden nada. Todo número que aparece aquí llegó de un
//! cálculo con evidencia, y toda cifra derivada se declara como cálculo local.

use crate::{
    calc::{Bucket, Decimal, Operation, RowComputation, RowOperation, render_amount},
    census::CensusCount,
    model::{Answer, Evidence, OcrStatus},
    relations::{Contradiction, Dossier, FieldValue, RelationGroup},
};

/// Recuento de archivos del acervo, con su cobertura declarada.
///
/// El texto separa siempre tres cosas que un solo número mezclaría:
///
///  1. **Cuántos archivos hay** — el dato que la pregunta pidió. Es completo
///     porque el indexador anota también lo que no pudo leer.
///  2. **Cuántos se pudieron leer** — la parte sobre la que Omega puede
///     afirmar algo del contenido.
///  3. **De dónde salió el recorte** — la carpeta, el tipo leído del nombre
///     del archivo, o el campo con el que la pregunta nombró la carpeta.
///
/// El punto 3 no es cortesía: contar por el nombre del archivo es una lectura
/// del índice, no del documento, y una respuesta que no lo dijera estaría
/// presentando una convención de nombres como si fuera contenido leído.
pub fn census(
    total: CensusCount,
    groups: Option<&[(String, CensusCount)]>,
    origin: Option<&str>,
    kind: Option<&str>,
    origin_from_value: Option<&(String, i64)>,
) -> String {
    let mut scope = Vec::new();
    if let Some(origin) = origin {
        scope.push(format!("carpeta {origin}"));
    }
    if let Some(kind) = kind {
        scope.push(format!("tipo «{kind}» según el nombre del archivo"));
    }
    let where_ = if scope.is_empty() {
        "el acervo".to_owned()
    } else {
        scope.join(", ")
    };
    let mut parts = vec![format!(
        "{} {} en {where_}.",
        total.discovered,
        plural(total.discovered, "documento", "documentos")
    )];
    if total.unindexed > 0 {
        parts.push(format!(
            "De ésos, {} {} indexar y {} no: quedan contados en el total —el indexador los descubrió— pero de su contenido no puedo afirmar nada.",
            total.indexed,
            plural(total.indexed, "se pudo", "se pudieron"),
            total.unindexed
        ));
    } else if total.discovered > 0 {
        parts.push("Todos se pudieron indexar.".to_owned());
    }
    if kind.is_some() || groups.is_some() {
        parts.push(
            "El tipo se lee del nombre del archivo, no de su contenido: es un metadato del índice."
                .to_owned(),
        );
    }
    if let Some((value, documents)) = origin_from_value {
        parts.push(format!(
            "La pregunta nombró el ámbito por el valor «{value}», que no es el nombre de la carpeta. Los {documents} documentos indexados que registran ese valor exacto están todos en la carpeta {}, y por eso se cuenta esa carpeta; no puedo afirmar que los {} archivos lo registren, porque de los que no se indexaron no leí nada.",
            origin.unwrap_or(""),
            total.discovered
        ));
    }
    if let Some(groups) = groups {
        if groups.is_empty() {
            parts.push("Ningún nombre de archivo del alcance deja ver un tipo.".to_owned());
        } else {
            let rows = groups
                .iter()
                .map(|(kind, count)| {
                    vec![
                        kind.clone(),
                        count.discovered.to_string(),
                        count.indexed.to_string(),
                        count.unindexed.to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            parts.push(table(
                &["Tipo", "Documentos", "Indexados", "Sin indexar"],
                &rows,
            ));
        }
    }
    parts.join("\n\n")
}

pub fn operation_title(operation: Operation) -> &'static str {
    match operation {
        Operation::Sum => "Suma",
        Operation::Average => "Promedio",
        Operation::Minimum => "Mínimo",
        Operation::Maximum => "Máximo",
        Operation::Count => "Conteo",
    }
}

pub fn bucket_amount(bucket: &Bucket) -> String {
    render_amount(bucket.value, bucket.currency.as_deref())
}

/// Cuántos valores y cuántos documentos entraron en un resultado. El requisito
/// es explícito: una cifra sin su número de operandos no es verificable.
pub fn evidence_size(bucket: &Bucket) -> String {
    format!(
        "{} {} en {} {}",
        bucket.value_count,
        plural(bucket.value_count, "valor", "valores"),
        bucket.document_ids.len(),
        plural(bucket.document_ids.len(), "documento", "documentos")
    )
}

pub fn plural<'a>(count: usize, singular: &'a str, many: &'a str) -> &'a str {
    if count == 1 { singular } else { many }
}

pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let separator = header
        .iter()
        .map(|_| "---")
        .collect::<Vec<_>>()
        .join(" | ");
    let mut lines = vec![
        format!("| {} |", header.join(" | ")),
        format!("| {separator} |"),
    ];
    for row in rows {
        lines.push(format!("| {} |", row.join(" | ")));
    }
    lines.join("\n")
}

/// Documentos del alcance que no aportaron un valor al cálculo, con el
/// motivo. Un cálculo nunca declara haber usado el alcance completo si algún
/// documento se quedó fuera: aquí se cuenta cuántos y por qué.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScopeExclusions {
    /// El documento no tiene ningún valor para el campo pedido.
    pub missing_field: usize,
    /// El documento tiene el campo, pero su valor no es un número utilizable.
    pub invalid_value: usize,
    /// El documento tiene un valor numérico, pero en una moneda distinta de
    /// la que la pregunta pidió.
    pub currency_mismatch: usize,
}

impl ScopeExclusions {
    pub fn is_empty(&self) -> bool {
        self.missing_field == 0 && self.invalid_value == 0 && self.currency_mismatch == 0
    }

    pub fn total(&self) -> usize {
        self.missing_field + self.invalid_value + self.currency_mismatch
    }
}

/// Línea que declara qué quedó fuera del cálculo y por qué. Se omite sólo
/// cuando no hubo ninguna exclusión.
pub fn exclusion_note(exclusions: ScopeExclusions) -> Option<String> {
    if exclusions.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if exclusions.missing_field > 0 {
        parts.push(format!(
            "{} {} sin ese campo",
            exclusions.missing_field,
            plural(exclusions.missing_field, "documento", "documentos")
        ));
    }
    if exclusions.invalid_value > 0 {
        parts.push(format!(
            "{} {} con un valor que no es un número",
            exclusions.invalid_value,
            plural(exclusions.invalid_value, "documento", "documentos")
        ));
    }
    if exclusions.currency_mismatch > 0 {
        parts.push(format!(
            "{} {} en otra moneda",
            exclusions.currency_mismatch,
            plural(exclusions.currency_mismatch, "documento", "documentos")
        ));
    }
    Some(format!(
        "{} {} del alcance no se {}: {}.",
        exclusions.total(),
        plural(exclusions.total(), "documento", "documentos"),
        plural(exclusions.total(), "usó", "usaron"),
        parts.join("; ")
    ))
}

/// Resultado de una operación sobre un campo, con separación por moneda.
///
/// `exclusions` declara qué documentos del alcance se quedaron fuera del
/// cálculo y por qué: nunca se calla que algo se excluyó.
/// Cobertura declarada de un cálculo por categoría. Los cinco motivos se
/// llevan por separado a propósito: «no tiene campo monetario», «tiene más de
/// uno», «lo tiene pero no es un número» y «está en otra moneda» son hechos
/// distintos, y colapsarlos en «faltan 718» esconde justo lo que hace
/// interpretable la cifra.
#[derive(Clone, Copy, Debug, Default)]
pub struct CategoryCoverage {
    pub scope_documents: usize,
    pub used_documents: usize,
    pub without_category: usize,
    pub ambiguous_category: usize,
    pub invalid_value: usize,
    pub currency_mismatch: usize,
}

impl CategoryCoverage {
    pub fn excluded(&self) -> usize {
        self.without_category + self.ambiguous_category + self.invalid_value
            + self.currency_mismatch
    }

    pub fn is_complete(&self) -> bool {
        self.excluded() == 0
    }
}

/// Adjetivo con el que nombrar una categoría de valor en prosa.
pub fn category_adjective(value_type: &str) -> &'static str {
    match value_type {
        "money" => "monetario",
        "percentage" => "porcentual",
        _ => "numérico",
    }
}

/// Cálculo sobre la categoría de valor, con la cobertura declarada al lado.
///
/// La primera línea dice siempre que el campo pedido no está: la cifra que
/// viene después no es la de ese campo y la respuesta no puede dejar que se
/// lea como si lo fuera.
pub fn category_computation(
    operation: Operation,
    requested: &str,
    value_type: &str,
    buckets: &[Bucket],
    fields: &[(String, usize)],
    coverage: CategoryCoverage,
) -> String {
    let adjective = category_adjective(value_type);
    let heading = format!(
        "No encontré ningún valor de «{requested}» en este alcance. Lo que sí puedo calcular sin elegir por ti: cada documento que tiene **exactamente un** campo {adjective} determina él mismo cuál es, así que sumé ésos."
    );
    let figure = if buckets.len() == 1 {
        format!(
            "{} del campo {adjective} de cada documento: {} — {}.",
            operation_title(operation),
            computation_amount(operation, &buckets[0]),
            evidence_size(&buckets[0])
        )
    } else {
        let rows = buckets
            .iter()
            .map(|bucket| {
                vec![
                    bucket.currency.clone().unwrap_or_else(|| "Sin moneda".into()),
                    computation_amount(operation, bucket),
                    bucket.value_count.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        format!(
            "{} del campo {adjective} de cada documento — {} monedas, calculadas por separado porque no pueden combinarse:\n\n{}",
            operation_title(operation),
            buckets.len(),
            table(&["Moneda", operation_title(operation), "Valores"], &rows)
        )
    };
    let mut parts = vec![heading, figure, coverage_note(coverage)];
    if !fields.is_empty() {
        let named = fields
            .iter()
            .map(|(field, count)| {
                format!(
                    "«{field}» ({count} {})",
                    plural(*count, "documento", "documentos")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("Campos usados: {named}."));
    }
    parts.join("\n\n")
}

/// Línea de cobertura: cuántos de cuántos, y el motivo de cada exclusión.
pub fn coverage_note(coverage: CategoryCoverage) -> String {
    let head = format!(
        "Cobertura: {} de {} {} del alcance.",
        coverage.used_documents,
        coverage.scope_documents,
        plural(coverage.scope_documents, "documento", "documentos")
    );
    if coverage.is_complete() {
        return head;
    }
    let mut reasons = Vec::new();
    if coverage.without_category > 0 {
        reasons.push(format!(
            "{} sin ningún campo de esa clase",
            coverage.without_category
        ));
    }
    if coverage.ambiguous_category > 0 {
        reasons.push(format!(
            "{} con más de uno, sin que el documento diga cuál es el principal",
            coverage.ambiguous_category
        ));
    }
    if coverage.invalid_value > 0 {
        reasons.push(format!(
            "{} con un valor que no es un número",
            coverage.invalid_value
        ));
    }
    if coverage.currency_mismatch > 0 {
        reasons.push(format!("{} en otra moneda", coverage.currency_mismatch));
    }
    format!(
        "{head} {} {} fuera: {}.",
        coverage.excluded(),
        plural(coverage.excluded(), "quedó", "quedaron"),
        reasons.join("; ")
    )
}

/// Respuesta a «¿con qué confianza leíste este documento y puedo citar su
/// texto?».
///
/// Los cuatro estados se contestan por separado porque son hechos distintos:
/// «no hizo falta OCR», «se leyó bien», «se leyó por debajo del umbral» y «no
/// salió nada». La cita es un metadato **con valor** —el estado de
/// reconocimiento—, así que sostiene la respuesta; y su `reliable` sale del
/// mismo criterio que usa todo el motor, de modo que una lectura débil deja la
/// respuesta sin verificar sin ninguna regla nueva.
pub fn reading_reliability(reading: &crate::tools::DocumentReading) -> Answer {
    let percentage = reading
        .confidence
        .map(|value| format!(" (confianza media del reconocimiento: {:.0} %)", value * 100.0))
        .unwrap_or_default();
    let (text, citable) = match reading.status {
        OcrStatus::NotRequired => (
            format!(
                "Ese documento no necesitó reconocimiento óptico: su texto viene del propio archivo (.{}), así que la fiabilidad de la lectura no depende de un OCR. Su texto se puede citar.",
                reading.extension
            ),
            true,
        ),
        OcrStatus::Complete => (
            format!(
                "Confianza alta: el reconocimiento óptico de ese documento quedó por encima del umbral de {:.0} % que usa Omega{percentage}. Su texto se puede citar y una respuesta apoyada en él se declara verificada.",
                crate::ocr::RELIABLE_CONFIDENCE * 100.0
            ),
            true,
        ),
        OcrStatus::LowConfidence => (
            format!(
                "Confianza baja: el reconocimiento óptico de ese documento quedó por debajo del umbral de {:.0} % que usa Omega{percentage}. Puedo mostrar su texto, pero **no** se puede citar de forma confiable: ninguna respuesta apoyada en él se declara verificada.",
                crate::ocr::RELIABLE_CONFIDENCE * 100.0
            ),
            false,
        ),
        OcrStatus::Failed | OcrStatus::Pending => (
            "Sin texto recuperable: el reconocimiento óptico corrió sobre ese documento y no entregó texto utilizable, así que no hay evidencia textual que citar.".to_owned(),
            false,
        ),
        OcrStatus::Unavailable => (
            "No hay motor de reconocimiento óptico en este equipo, así que ese documento nunca se leyó. No es que su texto sea malo: es que no se intentó.".to_owned(),
            false,
        ),
    };
    if !citable && reading.values == 0 {
        // Sin un solo valor extraído no hay nada que citar: la respuesta lo
        // dice y no lleva evidencia, que es justo lo que la distingue de una
        // lectura débil pero aprovechable.
        return Answer::unverified(text);
    }
    let label = format!(
        "{}{}",
        match reading.status {
            OcrStatus::NotRequired => "no requirió OCR".to_owned(),
            OcrStatus::Complete => "confianza alta".to_owned(),
            OcrStatus::LowConfidence => "confianza baja".to_owned(),
            other => other.as_str().to_owned(),
        },
        reading
            .confidence
            .map(|value| format!(" ({value:.2})"))
            .unwrap_or_default()
    );
    Answer::verified(
        text,
        vec![Evidence {
            id: format!("m-{}-estado de reconocimiento", reading.document_id),
            document_id: reading.document_id,
            path: reading.path.clone(),
            origin: reading.origin.clone(),
            location: "metadato: estado de reconocimiento (OCR)".into(),
            excerpt: label.clone(),
            normalized_value: Some(label.to_lowercase()),
            value: Some(label.clone()),
            matched: Some(label),
            field: Some("estado de reconocimiento (OCR)".into()),
            match_kind: "campo".into(),
            reliable: reading.status.is_reliable()
                && reading
                    .confidence
                    .is_none_or(|value| value >= crate::ocr::RELIABLE_CONFIDENCE),
            ocr_status: Some(reading.stored_status.clone()),
            ocr_confidence: reading.confidence,
            confidence: reading.confidence,
        }],
    )
}

pub fn computation(
    operation: Operation,
    concept: &str,
    buckets: &[Bucket],
    exclusions: ScopeExclusions,
    group_by: Option<&str>,
) -> String {
    let note = exclusion_note(exclusions)
        .map(|note| format!("\n\n{note}"))
        .unwrap_or_default();
    if let Some(group_by) = group_by.filter(|_| buckets.iter().any(|bucket| bucket.group.is_some())) {
        let rows = buckets
            .iter()
            .map(|bucket| {
                vec![
                    bucket.group.clone().unwrap_or_else(|| "Sin valor".into()),
                    computation_amount(operation, bucket),
                    bucket.value_count.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        let total_values = buckets.iter().map(|bucket| bucket.value_count).sum::<usize>();
        return format!(
            "{} de «{concept}» agrupada por «{group_by}» ({total_values} {}):\n\n{}{note}",
            operation_title(operation),
            plural(total_values, "valor", "valores"),
            table(&["Grupo", operation_title(operation), "Valores"], &rows),
        );
    }
    if buckets.len() == 1 {
        let bucket = &buckets[0];
        return format!(
            "{} de «{concept}»: {} — {}.{note}",
            operation_title(operation),
            computation_amount(operation, bucket),
            evidence_size(bucket)
        );
    }
    let rows = buckets
        .iter()
        .map(|bucket| {
            vec![
                bucket.currency.clone().unwrap_or_else(|| "Sin moneda".into()),
                computation_amount(operation, bucket),
                bucket.value_count.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    format!(
        "{} de «{concept}» — {} monedas, calculadas por separado porque no pueden combinarse:\n\n{}{note}",
        operation_title(operation),
        buckets.len(),
        table(&["Moneda", operation_title(operation), "Valores"], &rows)
    )
}

/// La ruta histórica de sumas mostraba dos decimales incluso cuando un campo
/// numérico sin código de moneda acababa exactamente en entero. Conservamos
/// ese contrato de presentación sin convertir la aritmética a `f64`.
fn computation_amount(operation: Operation, bucket: &Bucket) -> String {
    let amount = bucket_amount(bucket);
    if operation == Operation::Sum && bucket.currency.is_none() && !amount.contains('.') {
        format!("{amount}.00")
    } else {
        amount
    }
}

/// Misma operación repetida sobre varios campos de un mismo conjunto, cuando
/// el usuario pidió «todos» en vez de elegir uno solo en una aclaración.
///
/// Un campo sin valores en el alcance se declara «Sin datos»: nunca se
/// inventa un cero ni se omite la fila.
pub fn computation_many(operation: Operation, results: &[(String, Vec<Bucket>)]) -> String {
    let mut rows = Vec::new();
    let mut total_values = 0usize;
    let mut total_documents = std::collections::BTreeSet::new();
    for (concept, buckets) in results {
        if buckets.is_empty() {
            rows.push(vec![
                concept.clone(),
                "Sin datos".into(),
                "0".into(),
                "0".into(),
            ]);
            continue;
        }
        for bucket in buckets {
            rows.push(vec![
                concept.clone(),
                bucket_amount(bucket),
                bucket.value_count.to_string(),
                bucket.document_ids.len().to_string(),
            ]);
            total_values += bucket.value_count;
            total_documents.extend(bucket.document_ids.iter().copied());
        }
    }
    format!(
        "{} sobre {} campos del mismo conjunto — {total_values} {} en {} {}:\n\n{}",
        operation_title(operation),
        results.len(),
        plural(total_values, "valor", "valores"),
        total_documents.len(),
        plural(total_documents.len(), "documento", "documentos"),
        table(
            &["Campo", operation_title(operation), "Valores", "Documentos"],
            &rows
        )
    )
}

/// Operación entre dos campos numéricos del mismo documento («Cantidad ×
/// Precio unitario»). Siempre muestra la fórmula de un caso real —nunca sólo
/// la cifra final— y declara qué documentos quedaron fuera y por qué: no
/// hay manera de verificar la cifra sin ver de dónde salió.
pub fn row_computation(
    operation: RowOperation,
    left_name: &str,
    right_name: &str,
    computation: &RowComputation,
) -> String {
    // Los conteos salen del reparto del alcance completo, no de la longitud
    // de `skipped`: sólo así entran también los documentos que no tenían
    // ninguno de los dos campos, que no aparecen en ninguna lista de
    // operandos y antes desaparecían de la explicación.
    let breakdown = computation.breakdown;
    let mut warnings = Vec::new();
    let zero_divisions = breakdown.division_by_zero;
    let incompatible = breakdown.incompatible_units;
    let invalid_values = breakdown.invalid_value;
    if zero_divisions > 0 {
        warnings.push(format!(
            "{zero_divisions} {} {} porque {} entre cero",
            plural(zero_divisions, "documento", "documentos"),
            plural(zero_divisions, "no se calculó", "no se calcularon"),
            plural(zero_divisions, "dividía", "dividían")
        ));
    }
    if incompatible > 0 {
        warnings.push(format!(
            "{incompatible} {} {} por unidades incompatibles",
            plural(incompatible, "documento", "documentos"),
            plural(incompatible, "no se calculó", "no se calcularon")
        ));
    }
    if invalid_values > 0 {
        warnings.push(format!(
            "{invalid_values} {} {} los dos campos, pero con un valor que no es un número",
            plural(invalid_values, "documento", "documentos"),
            plural(invalid_values, "tenía", "tenían")
        ));
    }
    if breakdown.one_field_only > 0 {
        warnings.push(format!(
            "{} {} sólo {} uno de los dos campos",
            breakdown.one_field_only,
            plural(breakdown.one_field_only, "documento", "documentos"),
            plural(breakdown.one_field_only, "tenía", "tenían")
        ));
    }
    if breakdown.neither_field > 0 {
        warnings.push(format!(
            "{} {} no {} ninguno de los dos campos",
            breakdown.neither_field,
            plural(breakdown.neither_field, "documento", "documentos"),
            plural(breakdown.neither_field, "tenía", "tenían")
        ));
    }
    let warning_line = if warnings.is_empty() {
        String::new()
    } else {
        format!("\n\n{}.", warnings.join("; "))
    };
    // Sin ningún resultado la respuesta sigue debiendo el desglose: decir
    // sólo «no encontré documentos» esconde cuántos había en el alcance y
    // por qué ninguno sirvió.
    if computation.outcomes.is_empty() {
        return format!(
            "No encontré documentos con «{left_name}» y «{right_name}» a la vez, con unidades compatibles y sin dividir entre cero.{warning_line}"
        );
    }
    let sample = &computation.outcomes[0];
    let formula = format!(
        "{} {} {} = {}",
        sample.left_rendered,
        operation.symbol(),
        sample.right_rendered,
        render_amount(sample.value, sample.currency.as_deref())
    );
    if computation.outcomes.len() == 1 && warnings.is_empty() {
        return format!("«{left_name}» {} «{right_name}»: {formula}.", operation.verb());
    }
    let mut totals: std::collections::BTreeMap<Option<String>, (Decimal, usize)> =
        std::collections::BTreeMap::new();
    for outcome in &computation.outcomes {
        let entry = totals
            .entry(outcome.currency.clone())
            .or_insert((Decimal::ZERO, 0));
        entry.0 = entry.0.add(outcome.value);
        entry.1 += 1;
    }
    let total_line = totals
        .iter()
        .map(|(currency, (value, count))| {
            format!(
                "{} ({count} {})",
                render_amount(*value, currency.as_deref()),
                plural(*count, "documento", "documentos")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{} de «{left_name}» {} «{right_name}» calculada documento por documento — ejemplo: {formula}. Total: {total_line}.{warning_line}",
        operation.title(),
        operation.verb()
    )
}

/// Conteo de documentos del alcance.
pub fn document_count(count: usize) -> String {
    format!(
        "{count} {} en el alcance.",
        plural(count, "documento", "documentos")
    )
}

/// Ranking por grupo. Nombra al primero y muestra la tabla completa acotada.
pub fn ranking(
    operation: Operation,
    concept: &str,
    group_concept: &str,
    buckets: &[Bucket],
    descending: bool,
) -> String {
    let leader = &buckets[0];
    let rows = buckets
        .iter()
        .take(10)
        .map(|bucket| {
            vec![
                bucket.group.clone().unwrap_or_else(|| "Sin valor".into()),
                bucket_amount(bucket),
                bucket.value_count.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    let currencies = buckets
        .iter()
        .map(|bucket| bucket.currency.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let warning = if currencies.len() > 1 {
        "\n\nHay más de una moneda en el alcance: cada fila conserva la suya y las filas no son comparables entre sí."
    } else {
        ""
    };
    let total_values = buckets
        .iter()
        .map(|bucket| bucket.value_count)
        .sum::<usize>();
    format!(
        "«{group_concept}» con {} {} de «{concept}»: {} — {}.\n\nSe usaron {total_values} {} con evidencia en {} {}.\n\n{}{warning}",
        if descending { "mayor" } else { "menor" },
        operation_title(operation).to_lowercase(),
        leader.group.as_deref().unwrap_or("Sin valor"),
        bucket_amount(leader),
        plural(total_values, "valor", "valores"),
        buckets.len(),
        plural(buckets.len(), "grupo", "grupos"),
        table(
            &[group_concept, operation_title(operation), "Valores"],
            &rows
        )
    )
}

/// Comparación entre dos conjuntos ya calculados.
pub fn comparison(
    operation: Operation,
    concept: &str,
    dimension: &str,
    left: (&str, Option<&Bucket>),
    right: (&str, Option<&Bucket>),
) -> String {
    let mut rows = Vec::new();
    for (label, bucket) in [left, right] {
        rows.push(match bucket {
            Some(bucket) => vec![
                label.to_owned(),
                bucket_amount(bucket),
                bucket.value_count.to_string(),
            ],
            None => vec![label.to_owned(), "Sin datos".into(), "0".into()],
        });
    }
    let head = format!(
        "{} de «{concept}» por «{dimension}»:\n\n{}",
        operation_title(operation),
        table(&[dimension, operation_title(operation), "Valores"], &rows)
    );
    format!("{head}\n\n{}", difference_note(left, right))
}

/// Diferencia y variación porcentual entre dos resultados, o la explicación de
/// por qué no pueden calcularse.
pub fn difference_note(left: (&str, Option<&Bucket>), right: (&str, Option<&Bucket>)) -> String {
    let (left_label, left_bucket) = left;
    let (right_label, right_bucket) = right;
    let (Some(first), Some(second)) = (left_bucket, right_bucket) else {
        let missing = if left_bucket.is_none() {
            left_label
        } else {
            right_label
        };
        return format!(
            "No puedo calcular la diferencia: no hay valores con evidencia para «{missing}» en este alcance."
        );
    };
    if first.currency != second.currency {
        return format!(
            "No puedo restar los dos resultados: «{left_label}» está en {} y «{right_label}» en {}. Cantidades de monedas distintas no se combinan.",
            currency_label(first),
            currency_label(second)
        );
    }
    let difference = second.value.sub(first.value);
    let change = Decimal::percent_change(first.value, second.value);
    let greater = if second.value > first.value {
        format!("«{right_label}» es el mayor de los dos.")
    } else if second.value < first.value {
        format!("«{left_label}» es el mayor de los dos.")
    } else {
        "Los dos lados tienen el mismo valor.".to_owned()
    };
    let variation = match change {
        Some(value) => format!("Variación respecto a «{left_label}»: {} %.", value.render_signed()),
        None => format!(
            "La variación porcentual no está definida: «{left_label}» vale {} y no se puede dividir entre cero.",
            bucket_amount(first)
        ),
    };
    format!(
        "{greater} Diferencia («{right_label}» menos «{left_label}»): {}; en valor absoluto, {}. {variation} Cálculo local de Omega sobre {} y {} valores con evidencia.",
        render_amount(difference, first.currency.as_deref()),
        render_amount(difference.abs(), first.currency.as_deref()),
        first.value_count,
        second.value_count
    )
}

fn currency_label(bucket: &Bucket) -> String {
    bucket
        .currency
        .clone()
        .unwrap_or_else(|| "una magnitud sin moneda".into())
}

/// Comparación entre dos periodos del mismo alcance.
pub fn periods(
    operation: Operation,
    concept: &str,
    date_field: &str,
    previous: (&str, Option<&Bucket>),
    current: (&str, Option<&Bucket>),
) -> String {
    let mut rows = Vec::new();
    for (label, bucket) in [previous, current] {
        rows.push(match bucket {
            Some(bucket) => vec![
                label.to_owned(),
                bucket_amount(bucket),
                bucket.value_count.to_string(),
            ],
            None => vec![label.to_owned(), "Sin datos".into(), "0".into()],
        });
    }
    format!(
        "{} de «{concept}» por periodo, con el campo de fecha «{date_field}»:\n\n{}\n\n{}",
        operation_title(operation),
        table(&["Periodo", operation_title(operation), "Valores"], &rows),
        difference_note(previous, current)
    )
}

/// Documentos que respaldan un cálculo anterior.
pub fn supporting_documents(
    operation: &str,
    concept: &str,
    rendered: &str,
    value_count: usize,
    evidence: &[Evidence],
) -> String {
    let mut documents = evidence
        .iter()
        .map(|item| (item.path.clone(), item.location.clone()))
        .collect::<Vec<_>>();
    documents.dedup();
    let list = documents
        .iter()
        .take(20)
        .enumerate()
        .map(|(index, (path, location))| {
            format!("{}. {} — {}", index + 1, file_name(path), location)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "El resultado anterior ({operation} de «{concept}» = {rendered}) se calculó con {value_count} {} con evidencia:\n\n{list}",
        plural(value_count, "valor", "valores")
    )
}

/// Contradicciones entre documentos vinculados por una clave.
pub fn contradictions(items: &[Contradiction]) -> String {
    let head = format!(
        "{} {} entre documentos que comparten una clave estable. Omega no decide cuál valor es correcto: muestra ambos con su evidencia.",
        items.len(),
        plural(items.len(), "contradicción", "contradicciones")
    );
    let blocks = items
        .iter()
        .map(|item| {
            let values = item
                .entries
                .iter()
                .map(|entry| format!("- {} — {}", entry.value, file_name(&entry.path)))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "**{}** — vínculo por «{}»; el campo «{}» no coincide:\n\n{values}",
                item.display,
                item.linking_fields.join(", "),
                item.concept
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{head}\n\n{blocks}")
}

/// Ficha extractiva de un identificador.
pub fn dossier(dossier: &Dossier) -> String {
    let group = &dossier.group;
    let rows = dossier
        .fields
        .iter()
        .flat_map(|field| {
            field.values.iter().map(move |value| {
                vec![
                    field.concept.clone(),
                    value.value.clone(),
                    file_name(&value.path),
                ]
            })
        })
        .take(60)
        .collect::<Vec<_>>();
    let mut sections = vec![
        format!(
            "{} — {} {} vinculados por «{}».",
            group.display,
            group.documents.len(),
            plural(group.documents.len(), "documento", "documentos"),
            group.linking_fields().join(", ")
        ),
        table(&["Campo", "Valor", "Documento"], &rows),
    ];
    let conflicts = dossier
        .fields
        .iter()
        .filter(|field| field.conflicting)
        .map(|field| format!("«{}»", field.concept))
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        sections.push(format!(
            "Campos con valores en conflicto entre los documentos vinculados: {}.",
            conflicts.join(", ")
        ));
    }
    if !dossier.missing.is_empty() {
        let gaps = dossier
            .missing
            .iter()
            .take(10)
            .map(|gap| {
                format!(
                    "«{}» no aparece en {}",
                    gap.concept,
                    gap.absent_in
                        .iter()
                        .map(|path| file_name(path))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        sections.push(format!("Campos ausentes en algún documento: {gaps}."));
    }
    sections.push(relation_list(group));
    sections.join("\n\n")
}

pub fn relation_list(group: &RelationGroup) -> String {
    let items = group
        .documents
        .iter()
        .take(20)
        .enumerate()
        .map(|(index, document)| {
            format!(
                "{}. {} — {}",
                index + 1,
                file_name(&document.path),
                document.field
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Documentos vinculados:\n\n{items}")
}

/// Respuesta cuando alguien pide relacionar algo que no tiene clave estable.
pub fn relation_without_key(subject: &str, mentions: &[FieldValue]) -> String {
    let head = format!(
        "No puedo vincular documentos por «{subject}»: ese valor no produce una clave estable. Unir documentos por parecido de nombres, por una ciudad o por una cifra repetida no es evidencia de relación."
    );
    if mentions.is_empty() {
        return format!("{head} Tampoco encontré documentos que escriban ese valor exacto.");
    }
    let list = mentions
        .iter()
        .take(20)
        .enumerate()
        .map(|(index, mention)| {
            format!(
                "{}. {} — {}",
                index + 1,
                file_name(&mention.path),
                mention.evidence.field.as_deref().unwrap_or("campo")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{head}\n\n{} {} escriben ese valor exacto; son menciones, no un vínculo comprobado:\n\n{list}",
        mentions.len(),
        plural(mentions.len(), "documento", "documentos")
    )
}

/// Línea de alcance. Es la misma información que la interfaz muestra como
/// etiquetas, escrita en la respuesta para que el texto se sostenga solo.
pub fn scope_line(parts: &[String]) -> Option<String> {
    (!parts.is_empty()).then(|| format!("Alcance: {}.", parts.join(" · ")))
}

pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_owned()
}

/// Evidencia sintética que declara un cálculo local. Nunca se presenta como si
/// la cifra estuviera escrita en un documento.
pub fn calculation_evidence(
    operation: &str,
    concept: &str,
    rendered: &str,
    value_count: usize,
    sample: Option<&Evidence>,
) -> Evidence {
    Evidence {
        // Mantiene el prefijo estable de las notas de agregación históricas:
        // consumidores y regresiones lo usan para distinguir el cálculo local
        // de un valor literal del documento.
        id: format!("calc-{operation}-{concept}-{rendered}"),
        document_id: sample.map(|item| item.document_id).unwrap_or(0),
        path: sample.map(|item| item.path.clone()).unwrap_or_default(),
        origin: sample.map(|item| item.origin.clone()).unwrap_or_default(),
        location: format!("cálculo local sobre {value_count} valores"),
        excerpt: format!(
            "{operation} de «{concept}» = {rendered}. Cálculo local de Omega sobre {value_count} valores con evidencia; la cifra no aparece escrita en ningún documento."
        ),
        normalized_value: None,
        value: Some(rendered.to_owned()),
        matched: None,
        field: Some(concept.to_owned()),
        match_kind: "cálculo".into(),
        reliable: sample.is_none_or(|item| item.reliable),
        ocr_status: sample.and_then(|item| item.ocr_status.clone()),
        ocr_confidence: sample.and_then(|item| item.ocr_confidence),
        confidence: sample.and_then(|item| item.ocr_confidence),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_exclusions_produce_no_note() {
        assert_eq!(exclusion_note(ScopeExclusions::default()), None);
    }

    #[test]
    fn a_computation_never_claims_the_full_scope_when_something_was_excluded() {
        let exclusions = ScopeExclusions {
            missing_field: 3,
            invalid_value: 2,
            currency_mismatch: 1,
        };
        let note = exclusion_note(exclusions).expect("con exclusiones no puede ser None");
        assert!(note.starts_with("6 documentos del alcance no se usaron:"));
        assert!(note.contains("3 documentos sin ese campo"));
        assert!(note.contains("2 documentos con un valor que no es un número"));
        assert!(note.contains("1 documento en otra moneda"));
    }

    #[test]
    fn computation_appends_the_exclusion_note_to_a_single_bucket_result() {
        let bucket = Bucket {
            group: None,
            currency: Some("MXN".into()),
            value: Decimal::from_f64(100.0).unwrap(),
            value_count: 1,
            document_ids: std::collections::BTreeSet::from([1]),
            evidence: vec![],
            has_unreliable_evidence: false,
        };
        let exclusions = ScopeExclusions {
            missing_field: 1,
            invalid_value: 0,
            currency_mismatch: 0,
        };
        let text = computation(Operation::Sum, "Importe", &[bucket], exclusions, None);
        assert!(
            text.contains("1 documento del alcance no se usó: 1 documento sin ese campo."),
            "{text}"
        );
    }
}
