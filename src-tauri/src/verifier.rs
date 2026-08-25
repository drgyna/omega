use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::Deserialize;

use crate::{
    error::{OmegaError, Result},
    model::{Answer, Evidence},
    normalize::normalize_spanish,
};

#[derive(Debug, Deserialize)]
pub struct ModelAnswer {
    pub claims: Vec<ModelClaim>,
}

#[derive(Debug, Deserialize)]
pub struct ModelClaim {
    pub statement: String,
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub verbatim_values: Vec<String>,
}

/// Candado único de respaldo literal: un valor sólo puede aparecer en una
/// respuesta si figura en la evidencia citada. La comparación usa la misma
/// normalización que el resto del motor y mira exactamente lo que la interfaz
/// muestra de cada cita: su extracto y su valor.
///
/// Lo comparten la verificación de una respuesta del modelo y la síntesis
/// local, para que ninguna de las dos invente un dato por su cuenta.
pub fn value_is_supported(evidence: &[&Evidence], value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let evidence_text = evidence
        .iter()
        .map(|item| {
            format!(
                "{} {}",
                item.excerpt,
                item.value.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    normalize_spanish(&evidence_text).contains(&normalize_spanish(value))
}

pub fn verify_model_answer(raw: &str, available: &[Evidence]) -> Result<Answer> {
    let parsed: ModelAnswer = serde_json::from_str(raw)
        .map_err(|error| OmegaError::Verification(format!("JSON final inválido: {error}")))?;
    if parsed.claims.is_empty() {
        return Err(OmegaError::Verification(
            "la respuesta no contiene afirmaciones".into(),
        ));
    }

    let evidence = available
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let numeric =
        Regex::new(r"(?i)(?:\$\s*)?\d[\d,.]*(?:\s*[A-Z]{3}|\s*%)?|[A-Z]{2,}-\d{2,}[\w-]*")
            .expect("valid regex");
    let proper_name = Regex::new(r"\b(?:Lic\.\s*)?[\p{Lu}ÁÉÍÓÚÑ][\p{L}áéíóúñ]+(?:\s+(?:de|del|la|las|los|y)\s+|\s+)[\p{Lu}ÁÉÍÓÚÑ][\p{L}áéíóúñ]+(?:\s+[\p{Lu}ÁÉÍÓÚÑ][\p{L}áéíóúñ]+)*")
        .expect("valid regex");
    let mut used = Vec::new();
    let mut used_ids = HashSet::new();
    let mut statements = Vec::new();

    for claim in parsed.claims {
        if claim.statement.trim().is_empty() || claim.evidence_ids.is_empty() {
            return Err(OmegaError::Verification(
                "cada afirmación debe tener texto y evidencia".into(),
            ));
        }
        let cited = claim
            .evidence_ids
            .iter()
            .map(|id| {
                evidence
                    .get(id.as_str())
                    .copied()
                    .ok_or_else(|| OmegaError::Verification(format!("evidencia inexistente: {id}")))
            })
            .collect::<Result<Vec<_>>>()?;
        if claim.verbatim_values.is_empty() {
            return Err(OmegaError::Verification(
                "cada afirmación debe declarar al menos un valor literal respaldado".into(),
            ));
        }
        for value in &claim.verbatim_values {
            if !value_is_supported(&cited, value) {
                return Err(OmegaError::Verification(format!(
                    "el valor '{}' no aparece en la evidencia citada",
                    value
                )));
            }
        }
        let declared = claim
            .verbatim_values
            .iter()
            .map(|value| normalize_spanish(value))
            .collect::<Vec<_>>()
            .join(" ");
        for protected in numeric
            .find_iter(&claim.statement)
            .chain(proper_name.find_iter(&claim.statement))
        {
            if !declared.contains(&normalize_spanish(protected.as_str())) {
                return Err(OmegaError::Verification(format!(
                    "'{}' no fue declarado como valor respaldado",
                    protected.as_str()
                )));
            }
        }

        let citation_numbers = claim
            .evidence_ids
            .iter()
            .map(|id| {
                if used_ids.insert(id.clone()) {
                    used.push((*evidence.get(id.as_str()).expect("checked evidence")).clone());
                }
                used.iter()
                    .position(|item| item.id == *id)
                    .expect("inserted citation")
                    + 1
            })
            .collect::<Vec<_>>();
        let marks = citation_numbers
            .iter()
            .map(|number| format!("[{number}]"))
            .collect::<Vec<_>>()
            .join("");
        statements.push(format!("{} {marks}", claim.statement.trim()));
    }

    Ok(Answer {
        text: statements.join(" "),
        mode: "ai".into(),
        verified: true,
        citations: used,
        warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> Evidence {
        Evidence {
            id: "v-1".into(),
            document_id: 1,
            path: "/tmp/a.txt".into(),
            origin: "fixture".into(),
            location: "línea 2".into(),
            excerpt: "Cliente: Ana Pérez. Total: $10.00 MXN".into(),
            normalized_value: None,
            value: Some("$10.00 MXN".into()),
            matched: Some("$10.00 MXN".into()),
            field: Some("Cliente".into()),
            match_kind: "campo".into(),
            reliable: true,
            confidence: None,
        }
    }

    #[test]
    fn rejects_an_unsupported_number() {
        let raw = r#"{"claims":[{"statement":"Ana Pérez debe $20.00 MXN.","evidence_ids":["v-1"],"verbatim_values":["Ana Pérez"]}]}"#;
        assert!(verify_model_answer(raw, &[evidence()]).is_err());
    }

    #[test]
    fn accepts_values_copied_from_evidence() {
        let raw = r#"{"claims":[{"statement":"Ana Pérez debe $10.00 MXN.","evidence_ids":["v-1"],"verbatim_values":["Ana Pérez","$10.00 MXN"]}]}"#;
        assert!(verify_model_answer(raw, &[evidence()]).unwrap().verified);
    }
}
