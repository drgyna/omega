//! Redacción de los resultados del razonamiento local.
//!
//! Son funciones puras: reciben datos ya calculados y devuelven texto. No
//! consultan la base ni deciden nada. Todo número que aparece aquí llegó de un
//! cálculo con evidencia, y toda cifra derivada se declara como cálculo local.

use crate::{
    calc::{Bucket, Decimal, Operation, RowComputation, RowIssue, RowOperation, render_amount},
    model::Evidence,
    relations::{Contradiction, Dossier, FieldValue, RelationGroup},
};

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

/// Resultado de una operación sobre un campo, con separación por moneda.
pub fn computation(operation: Operation, concept: &str, buckets: &[Bucket]) -> String {
    if buckets.len() == 1 {
        let bucket = &buckets[0];
        return format!(
            "{} de «{concept}»: {} — {}.",
            operation_title(operation),
            bucket_amount(bucket),
            evidence_size(bucket)
        );
    }
    let rows = buckets
        .iter()
        .map(|bucket| {
            vec![
                bucket.currency.clone().unwrap_or_else(|| "Sin moneda".into()),
                bucket_amount(bucket),
                bucket.value_count.to_string(),
            ]
        })
        .collect::<Vec<_>>();
    format!(
        "{} de «{concept}» — {} monedas, calculadas por separado porque no pueden combinarse:\n\n{}",
        operation_title(operation),
        buckets.len(),
        table(&["Moneda", operation_title(operation), "Valores"], &rows)
    )
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
    if computation.outcomes.is_empty() {
        return format!(
            "No encontré documentos con «{left_name}» y «{right_name}» a la vez, con unidades compatibles y sin dividir entre cero."
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
    let mut warnings = Vec::new();
    let zero_divisions = computation
        .skipped
        .iter()
        .filter(|skip| skip.issue == RowIssue::DivisionByZero)
        .count();
    let incompatible = computation.skipped.len() - zero_divisions;
    if zero_divisions > 0 {
        warnings.push(format!(
            "{zero_divisions} {} no se calcularon porque dividían entre cero",
            plural(zero_divisions, "documento", "documentos")
        ));
    }
    if incompatible > 0 {
        warnings.push(format!(
            "{incompatible} {} no se calcularon por unidades incompatibles",
            plural(incompatible, "documento", "documentos")
        ));
    }
    if computation.unmatched_documents > 0 {
        warnings.push(format!(
            "{} {} sólo tenían uno de los dos campos",
            computation.unmatched_documents,
            plural(computation.unmatched_documents, "documento", "documentos")
        ));
    }
    let warning_line = if warnings.is_empty() {
        String::new()
    } else {
        format!("\n\n{}.", warnings.join("; "))
    };
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
        "Suma de «{left_name}» {} «{right_name}» calculada documento por documento — ejemplo: {formula}. Total: {total_line}.{warning_line}",
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
        id: format!("calculo-{operation}-{concept}-{rendered}"),
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
        reliable: true,
        confidence: None,
    }
}
