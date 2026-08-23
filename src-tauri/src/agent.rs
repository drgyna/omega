use crate::{error::Result, model::Answer, normalize::search_terms, tools::ToolEngine};

#[derive(Clone)]
pub struct Agent {
    tools: ToolEngine,
}

impl Agent {
    pub fn new(tools: ToolEngine) -> Self {
        Self { tools }
    }

    pub fn answer_local(&self, question: &str) -> Result<Answer> {
        if is_global_inventory_query(question) {
            let count = self.tools.indexed_document_count()?;
            return Ok(Answer {
                // Es un estado del índice, no una muestra de los primeros
                // resultados ni una interpretación del acervo.
                text: format!(
                    "Consulta global: se consultó el índice completo ({count} documentos indexados)."
                ),
                mode: "local".into(),
                verified: true,
                citations: vec![],
                warning: None,
            });
        }
        // La interfaz pagina las citas visualmente. No se usa ese tamaño de
        // página como sustituto del total de documentos que cumplen una
        // condición estructurada; de lo contrario el conteo mostrado podría
        // truncarse silenciosamente.
        let hits = self.tools.search(question, &[], usize::MAX)?;
        if hits.is_empty() {
            return Ok(Answer {
                text: "No encontré documentos que coincidan exactamente con ese criterio.".into(),
                mode: "local".into(),
                verified: true,
                citations: vec![],
                warning: None,
            });
        }
        Ok(Answer {
            text: format!("{} resultados con evidencia específica.", hits.len()),
            mode: "local".into(),
            verified: true,
            citations: hits.into_iter().map(|hit| hit.evidence).collect(),
            warning: None,
        })
    }

    pub fn answer(&self, question: &str) -> Result<Answer> {
        self.answer_local(question)
    }
}

fn is_global_inventory_query(question: &str) -> bool {
    let terms = search_terms(question);
    let asks_for_count = terms.iter().any(|term| {
        term.starts_with("cuant") || term.starts_with("numer") || term == "total" || term == "hay"
    });
    let mentions_index_scope = terms.iter().any(|term| {
        term.starts_with("indice") || term.starts_with("acervo") || term.starts_with("index")
    });
    asks_for_count && mentions_index_scope && terms.iter().any(|term| term.starts_with("document"))
}
