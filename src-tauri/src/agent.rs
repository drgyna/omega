use std::collections::HashSet;

use crate::{
    answer,
    calc::{self, Bucket, Decimal, Operation, RowOperation},
    census,
    conversation::{
        ComparisonMemory, ComparisonSide, ComputationMemory, ConversationState, OrdinalPosition,
    },
    dates::{self, CivilDate, Clock},
    error::Result,
    model::{
        AggregateRequest, AggregateResult, Answer, AnswerScope, Clarification, DateConstraint,
        Evidence, SearchHit, ToolFilter,
    },
    normalize::{canonical_key, normalize_exact},
    planner::{
        self, CensusRequest, Command, ComparisonDimension, DocumentSelection,
        DuplicateComparisonKind, PlannedScope, QueryIntent, QueryPlan, RowOperandSpec,
    },
    relations,
    report,
    tools::{
        self, DocumentQueryResult, FieldRole, LocatedDocument, OriginSummary, QuestionFieldRoles,
        TextQueryResult, ToolEngine, ValueQuery,
    },
};

const MAX_DOCUMENT_SAMPLE: usize = 12;
const MAX_DOCUMENT_CITATIONS: usize = 24;
const MAX_TEXT_CITATIONS: usize = 6;
const MAX_AGGREGATE_CITATIONS: usize = 18;

#[derive(Clone)]
pub struct Agent {
    tools: ToolEngine,
    /// Fuente de «hoy». Nunca se lee del sistema dentro del razonamiento: llega
    /// desde fuera para que una prueba pueda fijarla.
    clock: Clock,
}

impl Agent {
    pub fn new(tools: ToolEngine, clock: Clock) -> Self {
        Self { tools, clock }
    }

    /// Responde dentro de una conversación. El estado entra y sale por
    /// referencia: el agente no guarda memoria propia ni la comparte entre
    /// conversaciones.
    pub fn answer_in(&self, question: &str, state: &mut ConversationState) -> Result<Answer> {
        let plan = planner::plan_structured(&self.tools, question, state, &self.clock)?;
        crate::trace!("== PREGUNTA: {question}");
        crate::trace!("b) plan_structured -> Command::{:?}", plan.command);
        crate::trace!("b) plan_structured -> scope {:?}", plan.scope);
        // Ninguna respuesta deja un documento señalado por omisión: las rutas
        // que hablan de uno solo lo fijan ellas mismas. El plan ya leyó el
        // anterior, así que borrarlo aquí no pierde nada y evita que «ese
        // documento» siga apuntando a un archivo del que ya nadie habló. La
        // recuperación sí necesita saber cuál era, para poder comprobar si la
        // pregunta lo sigue describiendo; se le pasa aparte, no por el estado.
        let inherited = state.document.clone();
        state.document = None;
        let answer = match plan.command.clone() {
            Command::Retrieval => self.retrieval(question, state, inherited.as_deref())?,
            Command::Clarify(clarification) => clarify(clarification),
            Command::NoEvidence { message } => Answer::unverified(message),
            Command::Compute(operation) => self.compute(operation, &plan.scope, state)?,
            Command::Rank {
                operation,
                descending,
            } => self.rank(operation, descending, &plan.scope, state)?,
            Command::CompareGroups {
                operation,
                dimension,
                left,
                right,
            } => self.compare_groups(operation, &dimension, &left, &right, &plan.scope, state)?,
            Command::ComparePeriods {
                operation,
                previous,
            } => self.compare_periods(operation, &previous, &plan.scope, state)?,
            Command::ComparisonFollowUp { percentage } => {
                self.comparison_follow_up(percentage, state)
            }
            Command::EvidenceForLast => self.evidence_for_last(state),
            Command::DocumentInContext(selection) => {
                self.document_in_context(question, &selection, &plan.scope, state)?
            }
            Command::RelationWithoutKey => self.relation_without_key(question)?,
            Command::Contradictions {
                key,
                compared,
                identifier,
            } => self.contradictions(
                key.as_deref(),
                compared.as_deref(),
                identifier.as_deref(),
            )?,
            Command::Dossier { canonical } => self.dossier(&canonical, state)?,
            Command::ComputeMany {
                operation,
                concepts,
            } => self.compute_many(operation, &concepts, &plan.scope, state)?,
            Command::ComputeRow {
                operation,
                left,
                right,
            } => self.compute_row(operation, &left, &right, &plan.scope, state)?,
            Command::CompareFieldBetweenDocuments { field } => {
                self.compare_field_between_documents(&field, &plan.scope)?
            }
            Command::ComputeRowInDocument {
                operation,
                left,
                right,
            } => self.compute_row_in_document(operation, &left, &right, &plan.scope)?,
            Command::Census(request) => self.census(&request)?,
            Command::DuplicateComparison(kind) => self.duplicate_comparison(question, kind)?,
            Command::ComputeCategory {
                operation,
                requested,
                value_type,
            } => self.compute_category(
                operation,
                requested.as_deref(),
                &value_type,
                &plan.scope,
                state,
            )?,
        };
        // La aclaración pendiente vive exactamente un turno: o el usuario elige
        // una opción, o la siguiente pregunta la sustituye.
        state.pending = plan.pending.clone();
        state.last_question = Some(question.to_owned());
        let answer = self.with_duplicate_notice(answer, &plan.scope)?;
        self.with_format_mismatch_notice(answer)
    }

    /// Declara que un documento citado no es del formato que su extensión
    /// dice.
    ///
    /// Va en el mismo embudo que la advertencia de duplicados, y por la misma
    /// razón: ninguna ruta de respuesta puede quedarse sin ella. Se limita a
    /// los documentos que la respuesta **cita** —no a todo el alcance—: un
    /// archivo disfrazado que existe en el acervo pero no sostiene esta
    /// respuesta no la contamina.
    fn with_format_mismatch_notice(&self, mut answer: Answer) -> Result<Answer> {
        let documents = answer
            .citations
            .iter()
            .map(|evidence| evidence.document_id)
            .collect::<Vec<_>>();
        let mismatches = self.tools.declared_format_mismatches(&documents)?;
        if mismatches.is_empty() {
            return Ok(answer);
        }
        let named = mismatches
            .iter()
            .map(|(path, detected)| format!("{path}: es {detected}"))
            .collect::<Vec<_>>()
            .join("; ");
        let notice = format!(
            "Atención: {} de los documentos de esta respuesta no tienen el formato que declara su extensión ({named}). Se leyó su contenido real; el nombre del archivo no es una descripción fiable de lo que contiene.",
            mismatches.len()
        );
        answer.warning = Some(match answer.warning.take() {
            Some(existing) => format!("{existing} {notice}"),
            None => notice,
        });
        Ok(answer)
    }

    /// Advierte cuando la respuesta se apoya en documentos de contenido
    /// idéntico byte a byte.
    ///
    /// La política de duplicados no cambia ninguna cifra: dos copias de un
    /// archivo se cuentan como dos documentos porque el índice no puede saber
    /// si son un error de archivo o dos entregas reales. Lo que sí puede hacer
    /// —y hace aquí— es no dejar que esa suma pase por evidente.
    ///
    /// Va en el único embudo por el que salen todas las respuestas, para que
    /// ninguna ruta de cálculo, ranking, comparación o recuperación se quede
    /// sin la advertencia. El conjunto examinado son los documentos del
    /// alcance más los que la respuesta cita: un duplicado que vive en el
    /// acervo pero no participó en esta respuesta no la contamina.
    fn with_duplicate_notice(
        &self,
        mut answer: Answer,
        scope: &PlannedScope,
    ) -> Result<Answer> {
        // Cuando la respuesta se calculó sobre un campo concreto, los
        // documentos del alcance que no tienen ese campo no participaron: un
        // duplicado que sólo está ahí no puede haber inflado nada.
        let mut documents = match scope.concept.as_deref() {
            Some(concept) => self.tools.documents_with_values(&scope.documents, concept)?,
            None => scope.documents.clone(),
        };
        documents.extend(answer.citations.iter().map(|evidence| evidence.document_id));
        let groups = self.tools.duplicate_groups(&documents)?;
        if groups.is_empty() {
            return Ok(answer);
        }
        let named = groups
            .iter()
            .map(|group| group.paths.join(" | "))
            .collect::<Vec<_>>()
            .join("; ");
        let notice = format!(
            "Atención: {} de los documentos de esta respuesta tienen contenido idéntico byte a byte ({named}). Se cuentan como documentos distintos —el acervo los contiene por separado— y ninguna cifra se ajustó por ello.",
            groups.iter().map(|group| group.paths.len()).sum::<usize>()
        );
        answer.warning = Some(match answer.warning.take() {
            Some(existing) => format!("{existing} {notice}"),
            None => notice,
        });
        Ok(answer)
    }

    /// Ruta de recuperación clásica. Además de responder, deja en el contexto
    /// el predicado del conjunto que acaba de producir, para que el siguiente
    /// turno pueda referirse a él.
    fn retrieval(
        &self,
        question: &str,
        state: &mut ConversationState,
        inherited: Option<&str>,
    ) -> Result<Answer> {
        let plan = planner::plan(&self.tools, question)?;
        crate::trace!("c) planner::plan (clasico) -> intent={:?}", plan.intent);
        crate::trace!("c) planner::plan -> origin={:?} filters={:?}", plan.origin, plan.filters);
        let (answer, subject) = self.execute(question, &plan, inherited)?;
        self.remember_retrieval(question, &plan, state)?;
        // `remember_retrieval` reinicia el estado; el documento del que habló
        // esta respuesta se fija después, para que no se lo lleve por delante.
        state.document = subject;
        Ok(answer)
    }

    fn remember_retrieval(
        &self,
        question: &str,
        plan: &QueryPlan,
        state: &mut ConversationState,
    ) -> Result<()> {
        // Cada turno de recuperación reemplaza el conjunto recordado. Conservar
        // el anterior haría que «esos» apuntara a algo que el usuario ya no
        // tiene delante.
        *state = ConversationState {
            last_question: state.last_question.clone(),
            ..ConversationState::default()
        };
        let (filters, origin, concept, group_by) = match &plan.intent {
            QueryIntent::CountDocuments | QueryIntent::ListDocuments => (
                plan.filters.clone(),
                plan.origin.clone(),
                None,
                None,
            ),
            QueryIntent::Aggregate(request) => (
                request.filters.clone(),
                request.origin.clone(),
                Some(request.concept.clone()),
                request.group_by.clone(),
            ),
            // El resto de las rutas de recuperación —búsqueda literal, texto
            // libre— no llevan filtros en el plan porque no los aplican al
            // buscar. El conjunto que el usuario cree tener delante, en
            // cambio, sí está acotado por lo que él escribió: «¿qué documentos
            // hay en la carpeta ventas con Moneda: EUR?». Recordar esos pares
            // —sólo los que escribió con «Campo: valor», nunca uno inferido—
            // es lo que permite que el turno siguiente («de esos, ¿cuántos
            // …?») herede ese conjunto en vez de empezar del acervo entero.
            _ => (
                self.tools.written_filters(question)?.filters,
                plan.origin.clone(),
                None,
                None,
            ),
        };
        state.concept = concept;
        state.group_by = group_by;
        if filters.is_empty() && origin.is_none() {
            // Sin predicado no hay conjunto que recordar. Un identificador sí
            // se recuerda: define un conjunto reevaluable por clave estable.
            let candidates = relations::identifier_candidates(&self.tools, question)?;
            if candidates.len() == 1 {
                state.identifier = Some(candidates[0].clone());
                state.set = Some(crate::conversation::DocumentSet {
                    identifier: Some(candidates[0].clone()),
                    ..Default::default()
                });
            }
            return Ok(());
        }
        let documents = self
            .tools
            .documents_matching(&filters, origin.as_deref(), None)?;
        state.set = Some(
            crate::conversation::DocumentSet {
                filters,
                origin,
                document_count: documents.len() as i64,
                ..Default::default()
            }
            .with_paths(documents.iter().map(|document| document.path.clone())),
        );
        Ok(())
    }


    /// Responde por la fiabilidad de la **propia lectura** de un documento.
    ///
    /// Omega ya usa `ocr_status`/`ocr_confidence` para decidir si una respuesta
    /// puede declararse verificada, pero no sabía contestar la pregunta
    /// directa: «¿con qué confianza leíste este documento y puedo citar su
    /// texto?». Es un hecho mecánico del índice y no exige adivinar nada.
    ///
    /// Sólo se activa cuando la pregunta habla a la vez de reconocimiento
    /// óptico y de confianza o fiabilidad, y localiza exactamente un
    /// documento: dos condiciones estrechas para que no capture ninguna
    /// pregunta de contenido.
    fn reading_reliability_answer(&self, question: &str) -> Result<Option<Answer>> {
        if !asks_about_reading_reliability(question) {
            return Ok(None);
        }
        let located = self.tools.locate_documents_by_key(question)?;
        let [document] = located.as_slice() else {
            return Ok(None);
        };
        let reading = self.tools.document_reading(document.id)?;
        Ok(Some(report::reading_reliability(&reading)))
    }

    /// La pregunta pide qué se puede sacar de un archivo cuya extensión miente
    /// sobre su contenido.
    ///
    /// El indexador ya lo detecta y lo guarda (`declared_format_mismatch`,
    /// ronda 4), pero esa detección sólo salía como aviso pegado al final de
    /// una respuesta que, por delante, presentaba el contenido como si el
    /// archivo fuera lo que su nombre promete. Para esta pregunta concreta la
    /// discrepancia no es una nota al pie: es la respuesta.
    ///
    /// Sale como aclaración, no como dato, porque la pregunta trae una premisa
    /// que resultó falsa —que el archivo es un documento válido de ese
    /// formato— y qué hacer con el contenido que sí se leyó depende de una
    /// decisión que no es de Omega.
    fn disguised_file_answer(&self, question: &str) -> Result<Option<Answer>> {
        if !asks_what_can_be_extracted(question) {
            return Ok(None);
        }
        let located = self.tools.locate_documents_by_key(question)?;
        let [document] = located.as_slice() else {
            return Ok(None);
        };
        let mismatches = self.tools.declared_format_mismatches(&[document.id])?;
        let Some((_, detected)) = mismatches.first() else {
            return Ok(None);
        };
        // La extensión se lee del índice, no del rótulo que arma
        // `declared_format_mismatches` para los avisos: ese rótulo ya viene con
        // la extensión entre paréntesis y recortarlo a mano producía «(pdf))».
        let reading = self.tools.document_reading(document.id)?;
        let extension = reading.extension.clone();
        let values = self.tools.document_values(document.id)?;
        let file = reading
            .path
            .rsplit('/')
            .next()
            .unwrap_or(reading.path.as_str())
            .to_owned();
        let mut answer = clarify(Clarification {
            question: format!(
                "La extensión del archivo ({extension}) es engañosa: {file} no es un documento válido de ese formato, su contenido real es {detected}. Lo leí como tal y extraje {} campos, pero no doy por buena la premisa de la pregunta sin decirlo: ¿quieres que te muestre ese contenido, sabiendo de dónde sale?",
                values.len()
            ),
            options: vec![],
            reason: "extension_enganosa".to_owned(),
        });
        answer.citations = values
            .iter()
            .take(MAX_DOCUMENT_SAMPLE)
            .map(|value| value.evidence.clone())
            .collect();
        Ok(Some(answer))
    }

    /// Responde una pregunta que señala el documento por una clave de
    /// localización (ID interno de indexación o ruta de archivo).
    ///
    /// La clave sólo sirve para llegar al documento: la respuesta se arma con
    /// los valores realmente extraídos de él, y por tanto todas sus citas son
    /// evidencia textual. `Answer::verified` sigue decidiendo la confiabilidad
    /// a partir de esas citas, sin excepción — localizar por una clave no
    /// vuelve verificada a ninguna respuesta.
    fn located_answer(&self, question: &str) -> Result<Option<(Answer, String)>> {
        let located = self.tools.locate_documents_by_key(question)?;
        let [document] = located.as_slice() else {
            // Ninguno o varios: no se adivina. Con varios candidatos se deja
            // que la ruta normal enumere, igual que ante un folio ambiguo.
            return Ok(None);
        };
        Ok(self
            .answer_about_document(question, document)?
            .map(|answer| (answer, document.path.clone())))
    }

    /// Responde una pregunta sobre un documento ya identificado.
    ///
    /// Se separó de `located_answer` para que la resolución de referencias
    /// ordinales pueda reutilizarla: el documento llega ahí por su posición en
    /// el conjunto anterior en vez de por una clave escrita en la pregunta,
    /// pero todo lo demás —la guarda de que la pregunta nombre un campo que
    /// este documento sí registra, la síntesis con cita y el candado de
    /// evidencia— tiene que ser exactamente el mismo. Duplicarlo habría dejado
    /// una segunda puerta por la que responder sin las mismas garantías.
    fn answer_about_document(
        &self,
        question: &str,
        document: &LocatedDocument,
    ) -> Result<Option<Answer>> {
        let values = self.tools.document_values(document.id)?;
        crate::trace!("h) answer_about_document({}) con {} valores", document.path.rsplit('/').next().unwrap_or(""), values.len());
        if values.is_empty() {
            return Ok(None);
        }
        // La pregunta pudo nombrar dos campos de este documento con
        // intenciones opuestas: uno como PREMISA —escrito junto a su valor,
        // «cuyo testador es Felipe Navarro Arias»— y otro como pregunta
        // —«¿cuál es su albacea?»—. La resolución de campo se queda siempre
        // con la premisa: está escrita entera y con su valor, así que puntúa
        // mejor. El resultado era devolverle al usuario, sellado como dato
        // verificado, exactamente lo que él acababa de escribir.
        //
        // La premisa sólo se aparta si, sin ella, la pregunta todavía nombra
        // algún otro campo de este documento. Cuando la premisa es lo único
        // que nombra, la pregunta SÍ trata sobre ella —una confirmación— y
        // nada cambia.
        let roles = QuestionFieldRoles::new(question);
        let is_premise = |value: &tools::DocumentValue| {
            roles.role(&value.field, &value.value) == FieldRole::Restriction
        };
        let values = if values.iter().any(is_premise) {
            let asked = values
                .iter()
                .filter(|value| !is_premise(value))
                .cloned()
                .collect::<Vec<_>>();
            let mut asked_vocabulary: Vec<String> = Vec::new();
            for value in &asked {
                if !asked_vocabulary
                    .iter()
                    .any(|name| normalize_exact(name) == normalize_exact(&value.field))
                {
                    asked_vocabulary.push(value.field.clone());
                }
            }
            if answer::question_names_a_field(question, &asked_vocabulary) {
                crate::trace!(
                    "h) answer_about_document: se apartan las PREMISAS {:?}; la pregunta sigue nombrando otro campo",
                    values
                        .iter()
                        .filter(|value| is_premise(value))
                        .map(|value| value.field.clone())
                        .collect::<Vec<_>>()
                );
                asked
            } else {
                values
            }
        } else {
            values
        };
        // Localizar el documento no autoriza a responder cualquier cosa sobre
        // él: si la pregunta pide un campo que este documento no registra, la
        // respuesta correcta sigue siendo «no encontré evidencia». Sin esta
        // guarda, una pregunta por un dato ausente se contestaba con el valor
        // de otro campo cualquiera del mismo documento.
        // El vocabulario va sin repetidos: un campo que el documento registra
        // varias veces —«Responsable» en cada fila de una tabla de turnos— se
        // empataba consigo mismo y la resolución salía «ambigua», así que una
        // pregunta perfectamente contestable se quedaba sin respuesta.
        let mut vocabulary: Vec<String> = Vec::new();
        for value in &values {
            if !vocabulary
                .iter()
                .any(|name| normalize_exact(name) == normalize_exact(&value.field))
            {
                vocabulary.push(value.field.clone());
            }
        }
        // La palabra interrogativa puede decir QUÉ se pide sin nombrar ningún
        // campo: «¿cuándo…?» pide la fecha de este documento y «¿quién…?» a
        // quien aparece en él. Se resuelve aquí dentro, contra los valores que
        // este documento sí registra, y sólo cuando la pregunta no nombró ya un
        // campo suyo por completo —eso manda siempre—. Una coincidencia parcial
        // no manda: «minuta de ventas» roza «Meta de ventas» sin pedirla.
        crate::trace!("h) field_named_in_full -> {:?}", answer::field_named_in_full(question, &vocabulary));
        if answer::field_named_in_full(question, &vocabulary).is_none() {
            match answer::field_asked_by_category(question, &values) {
                answer::FieldRequest::Resolved(field) => {
                    // Se sintetiza sobre los valores de ESE campo y de este
                    // documento, no sobre todo lo encontrado: si el documento
                    // registra varios (una tabla con varias filas), la
                    // redacción de siempre los enumera en vez de elegir uno.
                    let hits = values
                        .iter()
                        .filter(|value| {
                            normalize_exact(&value.field) == normalize_exact(&field)
                        })
                        .map(|value| SearchHit {
                            title: value.field.clone(),
                            score: 1.0,
                            evidence: value.evidence.clone(),
                        })
                        .collect::<Vec<_>>();
                    if let Some(synthesis) = answer::synthesize(&self.tools, question, &hits)? {
                        let mut answer = Answer::verified(synthesis.text, synthesis.citations);
                        answer.verified &= synthesis.verified;
                        return Ok(Some(answer));
                    }
                }
                answer::FieldRequest::Ambiguous(options) => {
                    return Ok(Some(clarify(Clarification {
                        question: "La pregunta no nombra ningún campo de este documento y más de uno podría responderla. ¿Cuál de estos?".into(),
                        options,
                        reason: "campo_por_categoria_ambiguo".into(),
                    })));
                }
                answer::FieldRequest::NotRequested => {}
            }
        }
        if !answer::question_names_a_field(question, &vocabulary) {
            // Que la pregunta no nombre ningún campo de este documento no
            // significa todavía que no se pueda contestar: puede estar
            // nombrando una FILA de una de sus tablas por su etiqueta —«¿cuál
            // es el valor de "Lubricantes - insumo #2"?»—, que es como se
            // nombra una partida, un arancel o un artículo de una lista de
            // precios. La ronda 8 resolvió eso sólo en la ruta del folio; es
            // la misma pregunta sobre la misma casilla, así que aquí se llama
            // a la misma función, con sus mismas exigencias.
            return Ok(
                answer::labelled_row_in_document(question, &values).map(|synthesis| {
                    let mut answer = Answer::verified(synthesis.text, synthesis.citations);
                    answer.verified &= synthesis.verified;
                    answer
                }),
            );
        }
        let hits = values
            .iter()
            .map(|value| SearchHit {
                title: value.field.clone(),
                score: 1.0,
                evidence: value.evidence.clone(),
            })
            .collect::<Vec<_>>();
        if let Some(synthesis) = answer::synthesize(&self.tools, question, &hits)? {
            let mut answer = Answer::verified(synthesis.text, synthesis.citations);
            answer.verified &= synthesis.verified;
            return Ok(Some(answer));
        }

        // La pregunta trata sobre el documento en sí, no sobre un campo suyo.
        // Aquí sí hay que decir en voz alta de dónde salió la localización: el
        // identificador interno no está escrito en el documento, así que no se
        // puede presentar como si fuera una cita. Se acota a preguntas de
        // existencia/identidad a propósito: una pregunta por un campo que el
        // documento no tiene debe seguir contestándose «no encontré
        // evidencia», no «el documento existe».
        if !asks_whether_the_document_exists(question) {
            return Ok(None);
        }
        let citations = values
            .iter()
            .take(MAX_DOCUMENT_SAMPLE)
            .map(|value| value.evidence.clone())
            .collect::<Vec<_>>();
        let file = document
            .path
            .rsplit('/')
            .next()
            .unwrap_or(document.path.as_str());
        let mut answer = Answer::verified(
            format!(
                "Sí, ese documento está indexado: {file} (carpeta {}). Lo localicé por un \
                 identificador interno de indexación, no por su contenido: esa clave no \
                 aparece escrita en el documento, así que no puedo citarla. Lo que sí puedo \
                 citar son {} campos extraídos de él.",
                document.origin,
                values.len()
            ),
            citations,
        );
        answer.used_context = false;
        Ok(Some(answer))
    }

    /// Operación entre dos campos de un documento concreto.
    ///
    /// Es la ruta que faltaba para «para el documento D#####, ¿cuál es el
    /// resultado de dividir el importe entre la cantidad registrada?». Antes,
    /// esa pregunta acababa contestada con **el valor del divisor** —«El campo
    /// «Cantidad» es 0 piezas»— presentado como respuesta verificada: ni se
    /// dividía, ni se decía que no se podía.
    ///
    /// Las cuatro salidas posibles, en este orden:
    ///
    ///  1. Un operando que el documento no registra → no hay evidencia, y se
    ///     dice cuál falta.
    ///  2. Un operando con varios valores distintos en el mismo documento (una
    ///     tabla de partidas) → aclaración: cuál de ellos, elegir sería
    ///     adivinar.
    ///  3. Un operando que no es un número, o un divisor que vale cero →
    ///     aclaración: la operación es indeterminada, con el valor literal a la
    ///     vista para que se pueda comprobar.
    ///  4. Los dos son números utilizables → el cociente, con la fórmula y las
    ///     dos citas.
    /// ¿Coinciden los valores de un campo entre dos documentos?
    ///
    /// Los dos documentos llegan localizados por su clave interna, así que lo
    /// único que hay que hacer es leer el campo en cada uno y comparar. Las
    /// salidas, con su porqué:
    ///
    ///  * **Coinciden** → respuesta afirmativa con las dos citas. Es un hecho
    ///    comprobable en el texto de los dos documentos.
    ///  * **No coinciden** → aclaración, no respuesta. Omega puede afirmar que
    ///    los dos valores son distintos; lo que no puede es decir cuál de los
    ///    dos es el correcto, y contestar «no coinciden» a secas dejaría al
    ///    usuario creyendo que ya sabe cuál corregir.
    ///  * **Un documento no registra ese campo** → aclaración que lo dice, y
    ///    que ofrece como opción lo que ese documento sí registra. Es el caso
    ///    que más importa hacer bien: el atajo tentador —comparar contra
    ///    «el otro campo monetario» como si fuera el mismo— convertiría una
    ///    premisa falsa de la pregunta en una contradicción inventada entre dos
    ///    campos que nunca fueron el mismo.
    fn compare_field_between_documents(
        &self,
        field: &str,
        scope: &PlannedScope,
    ) -> Result<Answer> {
        let [first, second] = scope.documents.as_slice() else {
            return Ok(no_evidence_answer());
        };
        let mut sides = Vec::new();
        for document in [first, second] {
            let values = self.tools.document_values(*document)?;
            let matching = values
                .iter()
                .filter(|value| canonical_key(&value.field) == canonical_key(field))
                .cloned()
                .collect::<Vec<_>>();
            sides.push((document_label_for(&values), matching, values));
        }
        let [(left_name, left_values, left_all), (right_name, right_values, right_all)] =
            sides.as_slice()
        else {
            return Ok(no_evidence_answer());
        };
        for ((name, matching), all) in [(left_name, left_values), (right_name, right_values)]
            .into_iter()
            .zip([left_all, right_all])
        {
            if matching.is_empty() {
                let mut options = distinct_field_names(all);
                options.truncate(12);
                return Ok(clarify(Clarification {
                    question: format!(
                        "{name} no registra ningún campo llamado «{field}», así que no puedo comparar ese campo entre los dos documentos. No voy a dar por hecho que otro campo suyo es «{field}» sólo porque se le parezca: dime cuál de los suyos quieres comparar."
                    ),
                    options,
                    reason: "campo_ausente_en_un_documento".to_owned(),
                }));
            }
        }
        let describe = |values: &[tools::DocumentValue]| {
            let mut distinct: Vec<String> = Vec::new();
            for value in values {
                let text = value.value.trim().to_owned();
                if !distinct
                    .iter()
                    .any(|seen| normalize_exact(seen) == normalize_exact(&text))
                {
                    distinct.push(text);
                }
            }
            distinct
        };
        let (left_distinct, right_distinct) = (describe(left_values), describe(right_values));
        if left_distinct.len() > 1 || right_distinct.len() > 1 {
            let (name, options) = if left_distinct.len() > 1 {
                (left_name, left_distinct)
            } else {
                (right_name, right_distinct)
            };
            return Ok(clarify(Clarification {
                question: format!(
                    "{name} registra «{field}» con {} valores distintos, así que no hay un solo valor suyo que comparar. ¿Cuál de éstos?",
                    options.len()
                ),
                options,
                reason: "campo_multiple_en_documento".to_owned(),
            }));
        }
        let citations = left_values
            .iter()
            .chain(right_values.iter())
            .map(|value| value.evidence.clone())
            .collect::<Vec<_>>();
        let (left_value, right_value) = (&left_distinct[0], &right_distinct[0]);
        if normalize_exact(left_value) == normalize_exact(right_value) {
            return Ok(Answer::verified(
                format!(
                    "Sí coinciden: {left_name} y {right_name} registran «{field}» con el mismo valor, {left_value}."
                ),
                citations,
            ));
        }
        let mut answer = clarify(Clarification {
            question: format!(
                "No coinciden: {left_name} registra «{field}» = {left_value} y {right_name} registra {right_value}. Los dos valores están citados; cuál de los dos es el correcto no lo dice ninguno de los dos documentos, así que no lo voy a decidir yo."
            ),
            options: vec![],
            reason: "valores_discrepantes".to_owned(),
        });
        answer.citations = citations;
        Ok(answer)
    }

    fn compute_row_in_document(
        &self,
        operation: RowOperation,
        left: &RowOperandSpec,
        right: &RowOperandSpec,
        scope: &PlannedScope,
    ) -> Result<Answer> {
        let [document] = scope.documents.as_slice() else {
            return Ok(no_evidence_answer());
        };
        let values = self.tools.document_values(*document)?;
        // En una división, el divisor se examina primero. No es una
        // preferencia de redacción: si el divisor es cero o no es una cifra, el
        // cociente es indeterminado **sea cual sea** el dividendo, así que
        // reportar antes que falta el dividendo diría algo cierto pero menos
        // pertinente, y dejaría fuera el motivo que de verdad impide calcular.
        let order: [(&RowOperandSpec, bool); 2] = match operation {
            RowOperation::Divide => [(right, true), (left, false)],
            _ => [(left, false), (right, false)],
        };
        let mut numbers: Vec<(Decimal, &tools::DocumentValue)> = Vec::new();
        for (spec, is_divisor) in order {
            let value = match single_value_for(&values, spec) {
                OperandChoice::Missing => {
                    return Ok(Answer::unverified(format!(
                        "No puedo calcularlo: ese documento no registra {}.",
                        describe_operand(spec)
                    )));
                }
                OperandChoice::Ambiguous(options) => {
                    return Ok(clarify(Clarification {
                        question: format!(
                            "Ese documento registra {} valores distintos de {}. No elijo cuál usar por ti: ¿cuál de éstos?",
                            options.len(),
                            describe_operand(spec)
                        ),
                        options,
                        reason: "operando_multiple_en_documento".to_owned(),
                    }));
                }
                OperandChoice::One(value) => value,
            };
            let Some(number) = operand_number(value) else {
                return Ok(indeterminate(
                    spec,
                    value,
                    "de ahí no sale ninguna cifra con la que operar",
                ));
            };
            if is_divisor && number == Decimal::ZERO {
                return Ok(indeterminate(spec, value, "no se puede dividir entre cero"));
            }
            numbers.push((number, value));
        }
        // `numbers` sigue el orden de examen, no el de la fórmula.
        let [(first, first_value), (second, second_value)] = numbers.as_slice() else {
            return Ok(no_evidence_answer());
        };
        let ((dividend, dividend_value), (divisor, divisor_value)) = match operation {
            RowOperation::Divide => ((*second, *second_value), (*first, *first_value)),
            _ => ((*first, *first_value), (*second, *second_value)),
        };
        let result = match operation {
            RowOperation::Divide => dividend.divide(divisor),
            RowOperation::Multiply => dividend.multiply(divisor),
            RowOperation::Subtract => dividend.sub(divisor),
        };
        let text = format!(
            "«{}» ({}) {} «{}» ({}) = {}. Es un cálculo local sobre los dos valores citados de ese documento; el resultado no está escrito en él.",
            dividend_value.field,
            dividend_value.value.trim(),
            operation.symbol(),
            divisor_value.field,
            divisor_value.value.trim(),
            result.render()
        );
        Ok(Answer::verified(
            text,
            vec![
                dividend_value.evidence.clone(),
                divisor_value.evidence.clone(),
            ],
        ))
    }

    /// Censo del acervo: cuántos archivos hay, no cuántos valores se leyeron.
    ///
    /// La cifra puede ser completa —y por eso esta ruta existe— porque el
    /// indexador anota también los archivos que no logró leer. La respuesta
    /// declara siempre la partición: nadie debería tener que deducir de un
    /// total si Omega pudo abrir todos los archivos que contó.
    ///
    /// Las citas son metadato de archivo (ruta y carpeta), no contenido. Eso
    /// las hace no sustantivas y, por el candado de `Answer::verified`, la
    /// respuesta sale sin marcar como verificada: contar archivos no es haber
    /// leído lo que dicen.
    fn census(&self, request: &CensusRequest) -> Result<Answer> {
        if let Some(unknown) = &request.unknown_filter {
            return Ok(Answer::unverified(format!(
                "No sé aplicar el filtro «{unknown}»: no es una carpeta del acervo, ni un tipo de archivo, ni un campo que se haya extraído de ningún documento. No contesto aplicando sólo los filtros que sí entiendo, porque la cifra parecería cumplir todos los que escribiste."
            )));
        }
        let files = self.tools.census_files()?;
        let total = census::count(&files, &request.filter);
        let groups = request
            .group_by_kind
            .then(|| census::by_kind(&files, &request.filter));
        let citations = files
            .iter()
            .filter(|file| request.filter.accepts(file))
            .filter_map(|file| {
                file.document_id
                    .map(|document_id| census_evidence(document_id, file))
            })
            .take(MAX_DOCUMENT_SAMPLE)
            .collect::<Vec<_>>();
        let text = report::census(
            total,
            groups.as_deref(),
            request.filter.origin.as_deref(),
            request.filter.kind.as_deref(),
            request.origin_from_value.as_ref(),
        );
        let mut answer = Answer::verified(text, citations);
        answer.scope = Some(AnswerScope {
            origin: request.filter.origin.clone(),
            document_count: Some(total.discovered as i64),
            ..AnswerScope::default()
        });
        Ok(answer)
    }

    /// Relación byte a byte entre documentos ya nombrados por su clave
    /// interna. El SHA-256 lo calcula el indexador sobre los bytes crudos del
    /// archivo (`insert_document`, al leerlo por primera vez) — no pasa por
    /// texto extraído ni por OCR — así que compararlo es un hecho mecánico
    /// del propio índice, no una síntesis sobre contenido. Por eso puede
    /// responder `verified` sin pasar por `document_values`.
    fn duplicate_comparison(
        &self,
        question: &str,
        kind: DuplicateComparisonKind,
    ) -> Result<Answer> {
        let located = self.tools.locate_documents_by_key(question)?;
        match kind {
            DuplicateComparisonKind::FindByteIdentical => {
                let [target] = located.as_slice() else {
                    return Ok(no_evidence_answer());
                };
                let Some(hash) = self.tools.content_hash(target.id)? else {
                    return Ok(no_evidence_answer());
                };
                let matches = self.tools.documents_sharing_hash(target.id)?;
                let Some(other) = matches.first() else {
                    return Ok(Answer::verified(
                        format!(
                            "No: ningún otro documento del acervo indexado comparte el SHA-256 de {} ({hash}).",
                            document_label(target)
                        ),
                        vec![hash_evidence(target, &hash)],
                    ));
                };
                Ok(Answer::verified(
                    format!(
                        "Sí: {} es byte-idéntico a {} (mismo SHA-256: {hash}). Lo sé por el hash \
                         que el indexador calcula sobre el archivo, no por ningún contenido citado.",
                        document_label(other),
                        document_label(target),
                    ),
                    vec![hash_evidence(target, &hash), hash_evidence(other, &hash)],
                ))
            }
            DuplicateComparisonKind::CompareExact => {
                let [first, second] = located.as_slice() else {
                    return Ok(no_evidence_answer());
                };
                let (Some(hash_a), Some(hash_b)) = (
                    self.tools.content_hash(first.id)?,
                    self.tools.content_hash(second.id)?,
                ) else {
                    return Ok(no_evidence_answer());
                };
                let citations = vec![hash_evidence(first, &hash_a), hash_evidence(second, &hash_b)];
                let text = if hash_a == hash_b {
                    format!(
                        "Sí, es un duplicado exacto: {} y {} comparten el mismo SHA-256 ({hash_a}).",
                        document_label(first),
                        document_label(second),
                    )
                } else {
                    format!(
                        "No es un duplicado exacto, a lo sumo un documento similar: los SHA-256 \
                         de {} y {} son distintos ({hash_a} frente a {hash_b}).",
                        document_label(first),
                        document_label(second),
                    )
                };
                Ok(Answer::verified(text, citations))
            }
        }
    }

    /// Ejecuta el plan clásico. Devuelve, junto a la respuesta, la ruta del
    /// documento del que esa respuesta habla cuando habla de uno solo: es lo
    /// que la conversación necesita para resolver «ese documento» en el turno
    /// siguiente, y no se puede deducir de las citas —la síntesis cita todo lo
    /// que la búsqueda encontró, de varios archivos, y sólo una de esas citas
    /// sostiene la afirmación.
    fn execute(
        &self,
        question: &str,
        plan: &QueryPlan,
        inherited: Option<&str>,
    ) -> Result<(Answer, Option<String>)> {
        // La cadena de fecha entre comillas ya trae el dato completo en la
        // propia pregunta: no hace falta leer el acervo para saber si tiene
        // una lectura de calendario válida, dos distintas o ninguna. Se
        // comprueba antes de localizar el documento porque, si se dejara
        // pasar, la ruta de abajo devolvería esa misma cadena tal cual está
        // escrita como si fuera una fecha ya resuelta, sin advertir el
        // problema.
        if let Some(answer) = date_calendar_clarification(question) {
            crate::trace!("d) RUTA TEMPRANA: date_calendar_clarification DISPARA");
            return Ok((answer, None));
        }
        crate::trace!("d) date_calendar_clarification: no dispara");
        // Preguntar por la fiabilidad de la lectura de un documento no es
        // preguntar por su contenido: se resuelve con el estado de OCR que el
        // índice ya guarda, y va antes de la ruta de localización porque ésta
        // exige que la pregunta nombre un campo del documento —«confianza» no
        // lo es— y devolvería «no encontré evidencia» sobre un dato que Omega
        // sí tiene.
        if let Some(answer) = self.reading_reliability_answer(question)? {
            crate::trace!("d) RUTA TEMPRANA: reading_reliability_answer DISPARA");
            return Ok((answer, None));
        }
        crate::trace!("d) reading_reliability_answer: no dispara");
        // «¿Qué información se puede extraer de este archivo?» sobre un archivo
        // cuya extensión no corresponde a su contenido. Va antes de la ruta de
        // localización porque ésa contesta con el contenido extraído y deja la
        // discrepancia en un aviso al margen: quien pregunta qué se puede
        // sacar de un archivo necesita saber primero que el archivo no es lo
        // que dice ser, no enterarse al final.
        if let Some(answer) = self.disguised_file_answer(question)? {
            crate::trace!("d) RUTA TEMPRANA: disguised_file_answer DISPARA");
            return Ok((answer, None));
        }
        crate::trace!("d) disguised_file_answer: no dispara");
        // Una clave de localización manda sobre el plan: identifica un
        // documento concreto, así que responder sobre él es más preciso que
        // buscar por texto. Si no resuelve a exactamente un documento, la
        // pregunta sigue su curso normal.
        if let Some((answer, subject)) = self.located_answer(question)? {
            crate::trace!("d) RUTA TEMPRANA: located_answer DISPARA -> {subject}");
            return Ok((answer, Some(subject)));
        }
        crate::trace!("d) located_answer (locate_documents_by_key): no dispara");
        // Sin clave escrita, las pistas de la pregunta todavía pueden señalar
        // a un solo documento del acervo. Entonces la respuesta es leerlo y
        // contestar con su valor citado, no devolver una lista de candidatos y
        // dejarle la lectura al usuario. Va después de las rutas anteriores
        // porque todas ellas responden algo que no es un campo del documento
        // —el calendario de una cadena, la fiabilidad de un escaneo— y esta
        // ruta se las robaría.
        //
        // Se intenta para cualquier intent, incluidos conteo/lista/agregación:
        // una pregunta con "cuántas" puede estar pidiendo el valor de UN campo
        // de UN documento concreto ("¿cuántas copias pidió?"), no un conteo
        // real de documentos. `pinned_document` ya se abstiene solo cuando la
        // pregunta no ancla nada (ver tools.rs), y `answer_about_document` más
        // abajo sólo contesta si el documento fijado registra el campo que la
        // pregunta nombra; si no, la rama de conteo/lista de siempre sigue
        // intacta.
        crate::trace!("e) GUARDA DE FIJADO: se intenta para todo intent (intent={:?})", plan.intent);
        let pinned = match self.tools.pinned_document(question, inherited)? {
            Some(document_id) => self.tools.document_by_id(document_id)?,
            None => None,
        };
        crate::trace!("e) pinned = {:?}", pinned.as_ref().map(|d| d.path.clone()));
        // Anclar no es lo mismo que cumplir. Una pregunta con dos condiciones
        // puede quedar anclada por una sola de ellas —basta que un valor sea
        // único en el acervo— y entonces el documento fijado satisface esa
        // condición pero contradice la otra. Contestar desde él sería publicar
        // un dato de un documento que la propia pregunta excluye, y con sello
        // de verificado, porque el valor citado sí está literalmente ahí: lo
        // falso no es la cita, es que ese documento sea el que se preguntaba.
        //
        // El planificador ya calculó las condiciones en `plan.filters`. Si el
        // documento fijado no las cumple todas, se descarta —también como
        // sujeto de la conversación, para que el turno siguiente no herede un
        // documento que estas condiciones excluyen— y la pregunta sigue su
        // camino de siempre: la rama de conteo/lista con esos mismos filtros,
        // que devolverá el conjunto vacío honesto.
        let pinned = match pinned {
            Some(document)
                if !self
                    .tools
                    .document_matches_filters(document.id, &plan.filters)? =>
            {
                crate::trace!(
                    "e) FIJADO DESCARTADO: el documento no cumple los filtros del plan ({:?})",
                    plan.filters
                );
                None
            }
            other => other,
        };
        if let Some(document) = &pinned {
            // Fijar el documento no autoriza a contestar cualquier cosa sobre
            // él: `answer_about_document` sólo responde si la pregunta nombra
            // un campo que ese documento registra. Si no, la pregunta sigue su
            // camino de siempre.
            if let Some(answer) = self.answer_about_document(question, document)? {
                crate::trace!("h) answer_about_document RESUELVE sobre el documento fijado");
                return Ok((answer, Some(document.path.clone())));
            }
            crate::trace!("h) answer_about_document devuelve None sobre el documento fijado");
        }
        let (answer, subject) = match plan.intent.clone() {
            QueryIntent::Inventory => {
                crate::trace!("g) rama ejecutada: Inventory");
                Ok((inventory_answer(self.tools.origin_summaries()?), None))
            }
            QueryIntent::Exact => {
                crate::trace!("g) rama ejecutada: Exact -> legacy_answer(20)");
                self.legacy_answer(question, 20)
            }
            QueryIntent::Aggregate(request) => {
                let result = self.tools.aggregate(&request)?;
                Ok((aggregate_answer(&self.tools, &request, result), None))
            }
            QueryIntent::CountByFormat(request) => {
                let result = self.tools.count_by_format(
                    &plan.filters,
                    plan.origin.as_deref(),
                    &request,
                    MAX_DOCUMENT_SAMPLE,
                )?;
                Ok((
                    format_count_answer(&request, result, &plan.filters, plan.origin.as_deref()),
                    None,
                ))
            }
            QueryIntent::CountDocuments | QueryIntent::ListDocuments => {
                crate::trace!(
                    "g) rama ejecutada: CountDocuments/ListDocuments -> query_documents(filters={:?}, origin={:?})",
                    plan.filters, plan.origin
                );
                let evidence_per_document = plan.filters.len() + usize::from(plan.origin.is_some());
                let sample_limit = if evidence_per_document == 0 {
                    MAX_DOCUMENT_SAMPLE
                } else {
                    (MAX_DOCUMENT_CITATIONS / evidence_per_document)
                        .max(1)
                        .min(MAX_DOCUMENT_SAMPLE)
                };
                let result = self.tools.query_documents(
                    &plan.filters,
                    plan.origin.as_deref(),
                    sample_limit,
                )?;
                crate::trace!(
                    "g) query_documents -> {} documentos; muestra: {:?}",
                    result.document_count,
                    result.evidence.iter().map(|e| e.path.rsplit('/').next().unwrap_or("").to_string()).collect::<std::collections::BTreeSet<_>>()
                );
                crate::trace!("h) composicion: document_answer (fallback generico de conteo)");
                Ok((
                    document_answer(
                        result,
                        &plan.filters,
                        plan.origin.as_deref(),
                        &plan.unapplied_criteria,
                    ),
                    None,
                ))
            }
            QueryIntent::FreeText => {
                crate::trace!("g) rama ejecutada: FreeText -> search_text");
                let result =
                    self.tools
                        .search_text(question, plan.origin.as_deref(), MAX_TEXT_CITATIONS)?;
                crate::trace!("g) search_text -> {} documentos, {} hits", result.document_count, result.hits.len());
                crate::trace!("h) composicion: text_answer");
                Ok((text_answer(question, result), None))
            }
            QueryIntent::BoundedSearch => {
                crate::trace!("g) rama ejecutada: BoundedSearch -> legacy_answer(20)");
                self.legacy_answer(question, 20)
            }
            QueryIntent::LegacySearch => {
                crate::trace!("g) rama ejecutada: LegacySearch -> legacy_answer(MAX)");
                self.legacy_answer(question, usize::MAX)
            }
        }?;
        // Si las pistas fijaron un documento aunque este turno no supiera qué
        // campo se le pedía, ese documento es igualmente del que trata la
        // conversación: la continuación siguiente puede preguntar por él.
        Ok((
            answer,
            subject.or_else(|| pinned.map(|document| document.path)),
        ))
    }

    fn legacy_answer(&self, question: &str, limit: usize) -> Result<(Answer, Option<String>)> {
        // Se consulta un resultado más que el tope para distinguir una lista
        // completa de una recortada. Sin esa señal, un listado que llega justo
        // al tope se presenta igual que uno que agotó el acervo, y las cifras
        // que aparecen en su texto ("20 valores") se leen como un total.
        let mut hits = self.tools.search(question, &[], limit.saturating_add(1))?;
        let truncated = hits.len() > limit;
        hits.truncate(limit);
        crate::trace!("g) search() -> {} hits", hits.len());
        for (i, hit) in hits.iter().take(12).enumerate() {
            crate::trace!(
                "g)   #{} score={:.2} campo={:?} valor={:?} doc={}",
                i + 1, hit.score, hit.evidence.field, hit.evidence.value,
                hit.evidence.path.rsplit('/').next().unwrap_or("")
            );
        }
        if hits.is_empty() {
            crate::trace!("h) legacy_answer: 0 hits -> no_evidence_answer()");
            return Ok((no_evidence_answer(), None));
        }
        // Todas las coincidencias son metadatos sin valor —el nombre del
        // archivo o la carpeta que la propia pregunta acaba de escribir—:
        // dicen que el documento existe y dónde está, no qué contiene.
        // Presentarlas como «1 resultados con evidencia específica» convertía
        // una búsqueda sin hallazgos en una respuesta afirmativa vacía, y
        // además verificada, sobre documentos truncados o ilegibles.
        //
        // El texto nombra QUÉ coincidió, no lo supone: la ruta llega aquí
        // tanto por un nombre de archivo como por una carpeta.
        if !hits.iter().any(|hit| hit.evidence.is_substantive()) {
            crate::trace!("h) legacy_answer: ningun hit es sustantivo -> respuesta de 'solo metadato'");
            let matched = hits
                .iter()
                .filter_map(|hit| hit.evidence.field.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            return Ok((
                Answer::unverified(format!(
                    "Lo único que coincidió fue metadato del índice ({matched}): dice que el documento existe y dónde está, no qué contiene. No encontré en el contenido de ningún documento algo que responda esa pregunta, y repetir lo que tú escribiste no es haber extraído información."
                )),
                None,
            ));
        }
        let (mut answer, subject) =
            if let Some(synthesis) = answer::synthesize(&self.tools, question, &hits)? {
                crate::trace!("h) composicion: answer::synthesize RESUELVE (subject={:?})", synthesis.subject);
                // `Answer::verified` es el candado de confiabilidad: deriva
                // `verified` de las citas y adjunta la advertencia de OCR
                // débil. La síntesis sólo puede bajar ese valor (caso
                // `unresolved`), nunca subirlo.
                let mut answer = Answer::verified(synthesis.text, synthesis.citations);
                answer.verified &= synthesis.verified;
                (answer, synthesis.subject)
            } else {
                crate::trace!("h) composicion: FALLBACK generico 'N resultados con evidencia especifica'");
                (
                    Answer::verified(
                        format!("{} resultados con evidencia específica.", hits.len()),
                        hits.into_iter().map(|hit| hit.evidence).collect(),
                    ),
                    None,
                )
            };
        if truncated {
            note_truncated_sample(&mut answer, limit);
        }
        Ok((answer, subject))
    }

    pub fn answer(&self, question: &str) -> Result<Answer> {
        let mut state = ConversationState::default();
        self.answer_in(question, &mut state)
    }
}

/// ¿La pregunta es por la fiabilidad de la lectura, y no por el contenido?
///
/// Se exigen las dos señales a la vez —el reconocimiento óptico y la confianza
/// o fiabilidad— porque cada una por separado aparece en preguntas normales:
/// «¿qué dice el escaneo?» pide contenido, y «¿es confiable el proveedor?» no
/// habla de OCR.
fn asks_about_reading_reliability(question: &str) -> bool {
    let terms = crate::normalize::search_terms(question);
    let has = |root: &str| terms.iter().any(|term| term.starts_with(root));
    let names_recognition = has("ocr") || has("reconoc") || has("escane");
    let names_reliability = has("confian") || has("confiab") || has("fiab") || has("fiabilid");
    names_recognition && names_reliability
}

fn inventory_answer(summaries: Vec<OriginSummary>) -> Answer {
    let total = summaries
        .iter()
        .map(|item| item.document_count)
        .sum::<i64>();
    let categories = summaries
        .iter()
        .map(|item| format!("- {}: {} documentos", item.origin, item.document_count))
        .collect::<Vec<_>>()
        .join("\n");
    Answer::verified(
        format!(
            "Hay {total} documentos indexados en {} categorías:\n\n{categories}",
            summaries.len()
        ),
        summaries.into_iter().map(|item| item.evidence).collect(),
    )
}

fn document_answer(
    result: DocumentQueryResult,
    filters: &[ToolFilter],
    origin: Option<&str>,
    unapplied_criteria: &[String],
) -> Answer {
    // La pregunta nombró un criterio que no llegó a ser filtro. Contar con los
    // demás daría una cifra donde cada palabra es cierta y el conjunto no: el
    // número existe, pero no es el de los documentos que cumplen lo que se
    // preguntó. Es la misma lección que el censo ya aprendió en su ruta —
    // cuando hay un filtro que no se sabe aplicar, no se cuenta nada; se dice.
    if !unapplied_criteria.is_empty() {
        crate::trace!("h) composicion: criterio nombrado sin aplicar -> {unapplied_criteria:?}");
        return Answer::unverified(format!(
            "No apliqué {} «{}»: la pregunta {} nombra, pero no llegó a convertirse en ningún filtro sobre el acervo. No doy el conteo de los criterios restantes, porque la cifra parecería cumplirlos todos.",
            report::plural(unapplied_criteria.len(), "el criterio", "los criterios"),
            unapplied_criteria.join("», «"),
            report::plural(unapplied_criteria.len(), "lo", "los"),
        ));
    }
    if result.document_count == 0 {
        return no_evidence_answer();
    }
    let mut criteria = filters
        .iter()
        .map(|filter| format!("{} = {}", filter.concept, filter.equals))
        .collect::<Vec<_>>();
    if let Some(origin) = origin {
        criteria.insert(0, format!("carpeta = {origin}"));
    }
    let scope = if criteria.is_empty() {
        String::new()
    } else {
        format!(" ({})", criteria.join("; "))
    };
    Answer::verified(
        format!(
            "{} documentos cumplen simultáneamente los criterios{}.",
            result.document_count, scope
        ),
        result.evidence,
    )
}

/// Conteo por formato con su cobertura declarada.
///
/// Dos cifras, nunca una sola: los documentos de ese formato que el índice
/// tiene, y los archivos del alcance que no se pudieron indexar y por tanto no
/// están en la primera. Sin la segunda, el conteo se leería como si fuera el
/// del acervo completo cuando en realidad excluye en silencio todo lo que no
/// se logró leer.
fn format_count_answer(
    request: &tools::FormatRequest,
    result: tools::FormatCount,
    filters: &[ToolFilter],
    origin: Option<&str>,
) -> Answer {
    let mut criteria = filters
        .iter()
        .map(|filter| format!("{} = {}", filter.concept, filter.equals))
        .collect::<Vec<_>>();
    if let Some(origin) = origin {
        criteria.insert(0, format!("carpeta = {origin}"));
    }
    let scope = if criteria.is_empty() {
        String::new()
    } else {
        format!(" ({})", criteria.join("; "))
    };
    let mut parts = vec![format!(
        "Documentos indexados en formato {}{scope}: {}.",
        request.label, result.matching
    )];
    // Un mismo formato leído de dos maneras distintas es un dato que la cifra
    // única esconde: se declara, sin obligar a preguntarlo aparte.
    if !request.scanned_only && result.scanned > 0 && result.with_text_layer > 0 {
        parts.push(format!(
            "De ésos, {} {} capa de texto y {} hubo que {} por OCR (escaneados).",
            result.with_text_layer,
            report::plural(result.with_text_layer as usize, "tiene", "tienen"),
            result.scanned,
            report::plural(result.scanned as usize, "leerlo", "leerlos")
        ));
    }
    if result.only_in_origin > 0 {
        parts.push(format!(
            "Otro{} {} .{} de la misma carpeta no {} en esa cifra: su «{}» no dice exactamente el valor citado. La pregunta nombró el ámbito una vez y el acervo lo registra de dos formas —la carpeta y un campo del documento—; se cuenta la lectura estricta y se declara la diferencia en vez de elegir una por ti.",
            report::plural(result.only_in_origin as usize, "", "s"),
            format!(
                "{} {}",
                result.only_in_origin,
                report::plural(result.only_in_origin as usize, "archivo", "archivos")
            ),
            request.extension,
            report::plural(result.only_in_origin as usize, "entró", "entraron"),
            filters
                .first()
                .map(|filter| filter.concept.clone())
                .unwrap_or_else(|| "campo de alcance".to_owned())
        ));
    }
    if result.unindexed > 0 {
        parts.push(if result.unindexed_is_scoped {
            format!(
                "Además, {} {} .{} de este alcance no se {} indexar, así que no {} en la cifra anterior.",
                result.unindexed,
                report::plural(result.unindexed as usize, "archivo", "archivos"),
                request.extension,
                report::plural(result.unindexed as usize, "pudo", "pudieron"),
                report::plural(result.unindexed as usize, "entra", "entran")
            )
        } else {
            format!(
                "Además, el acervo tiene {} {} .{} que no se {} indexar. Un archivo sin indexar no tiene valores extraídos, así que no puedo saber cuáles de ellos caen dentro de este alcance: la cifra anterior los excluye a todos.",
                result.unindexed,
                report::plural(result.unindexed as usize, "archivo", "archivos"),
                request.extension,
                report::plural(result.unindexed as usize, "pudo", "pudieron")
            )
        });
    }
    if result.matching == 0 && result.unindexed == 0 {
        return no_evidence_answer();
    }
    let text = parts.join("\n\n");
    let mut answer = Answer::verified(text, result.evidence);
    // El conteo es exacto sobre lo indexado, pero deja fuera lo que no se pudo
    // leer: mientras haya archivos sin indexar en el alcance, la cifra no
    // puede presentarse como el total del acervo.
    if result.unindexed > 0 || result.only_in_origin > 0 {
        answer.verified = false;
        let mut reasons = Vec::new();
        if result.unindexed > 0 {
            reasons.push(format!(
                "{} {} .{} del alcance no se {} indexar",
                result.unindexed,
                report::plural(result.unindexed as usize, "archivo", "archivos"),
                request.extension,
                report::plural(result.unindexed as usize, "pudo", "pudieron")
            ));
        }
        if result.only_in_origin > 0 {
            reasons.push(format!(
                "{} {} .{} de la misma carpeta {} fuera por el filtro de campo",
                result.only_in_origin,
                report::plural(result.only_in_origin as usize, "archivo", "archivos"),
                request.extension,
                report::plural(result.only_in_origin as usize, "quedó", "quedaron")
            ));
        }
        // Se añade al aviso existente, nunca lo sustituye: si la evidencia
        // venía de OCR débil, esa advertencia tiene que sobrevivir.
        let notice = format!("Conteo parcial: {}.", reasons.join("; "));
        answer.warning = Some(match answer.warning.take() {
            Some(existing) => format!("{existing} {notice}"),
            None => notice,
        });
    }
    answer.with_scope(AnswerScope {
        filters: filters.to_vec(),
        origin: origin.map(ToOwned::to_owned),
        document_count: Some(result.matching),
        excluded_count: Some(result.unindexed + result.only_in_origin),
        ..AnswerScope::default()
    })
}

fn aggregate_answer(
    tools: &ToolEngine,
    request: &AggregateRequest,
    result: AggregateResult,
) -> Answer {
    if result.rows.is_empty() {
        return no_evidence_answer();
    }
    let total_values = result.value_count;
    let text = if request.operation == "count" {
        format!(
            "El campo «{}» tiene {total_values} valores con evidencia.",
            request.concept
        )
    } else if result.rows.len() == 1 && result.rows[0].group.is_none() {
        format!(
            "Suma de «{}»: {}, calculada a partir de {total_values} valores.",
            request.concept,
            result.rows[0].value
        )
    } else {
        let table = result.rows
            .iter()
            .map(|row| {
                format!(
                    "| {} | {} | {} |",
                    row.group.as_deref().unwrap_or("Sin valor"),
                    row.value,
                    row.matched_values
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Suma de «{}» agrupada por «{}» ({total_values} valores):\n\n| Grupo | Suma | Valores |\n|---|---:|---:|\n{table}",
            request.concept,
            request.group_by.as_deref().unwrap_or("grupo")
        )
    };

    let mut citations = tools
        .aggregate_calculation_evidence(request, &result)
        .into_iter()
        .collect::<Vec<_>>();
    let mut seen = HashSet::new();
    for evidence in result.rows.iter().flat_map(|row| row.evidence.iter()) {
        if citations.len() >= MAX_AGGREGATE_CITATIONS {
            break;
        }
        if seen.insert(evidence.id.clone()) {
            citations.push(evidence.clone());
        }
    }
    Answer {
        text,
        mode: "local".into(),
        verified: result.verified,
        citations,
        warning: result.warning,
        scope: Some(AnswerScope {
            filters: request.filters.clone(),
            origin: request.origin.clone(),
            concept: Some(request.concept.clone()),
            group_by: request.group_by.clone(),
            currency: request.currency.clone(),
            document_count: Some(result.document_count),
            value_count: Some(result.value_count),
            excluded_count: Some(result.excluded_count),
            ..AnswerScope::default()
        }),
        ..Answer::default()
    }
}

fn text_answer(question: &str, result: TextQueryResult) -> Answer {
    if result.hits.is_empty() {
        return no_evidence_answer();
    }
    let excerpts = result
        .hits
        .iter()
        .take(3)
        .map(|hit| format!("- {}", hit.evidence.excerpt.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let legal = asks_for_legal_guidance(question);
    let mut answer = Answer::verified(
        format!(
            "Encontré evidencia pertinente en {} documentos. Extractos del acervo:\n\n{}{}",
            result.document_count,
            excerpts,
            if legal {
                "\n\nEsta respuesta se limita al material indexado y no sustituye asesoría legal ni una fuente oficial."
            } else {
                ""
            }
        ),
        result.hits.into_iter().map(|hit| hit.evidence).collect(),
    );
    if legal {
        const LEGAL: &str = "Contenido extractivo del acervo local; no constituye asesoría legal.";
        // La nota legal se suma a la advertencia de OCR, no la reemplaza.
        answer.warning = Some(match answer.warning {
            Some(ocr) => format!("{ocr} {LEGAL}"),
            None => LEGAL.to_owned(),
        });
    }
    answer
}

/// Un listado que llega justo al tope interno no puede presentarse como si
/// hubiera agotado el acervo: las cifras de su texto cuentan lo que se muestra,
/// no lo que existe. La respuesta lo declara —en el texto y en la advertencia—
/// en vez de dejar que se lea una muestra parcial como un total.
/// La pregunta es por la existencia o identidad del documento en sí, no por
/// el valor de uno de sus campos.
fn asks_whether_the_document_exists(question: &str) -> bool {
    let normalized = normalize_exact(question);
    normalized
        .split_whitespace()
        .any(|word| word.starts_with("exist"))
}

fn note_truncated_sample(answer: &mut Answer, limit: usize) {
    let note = format!(
        "Muestra recortada: se muestran {limit} documentos y el acervo contiene más. \
         Las cifras de este texto describen la muestra, no un total."
    );
    answer.text.push_str("\n\n");
    answer.text.push_str(&note);
    answer.warning = Some(match answer.warning.take() {
        Some(existing) => format!("{existing} {note}"),
        None => note,
    });
}

fn no_evidence_answer() -> Answer {
    Answer {
        text: "No encontré evidencia local suficiente para responder esa consulta.".into(),
        mode: "local".into(),
        verified: false,
        citations: vec![],
        warning: None,
        ..Answer::default()
    }
}

/// «En el documento D#####, la fecha aparece como "A/B/AAAA". ¿A qué fecha
/// calendario corresponde?» — la cadena entre comillas ya trae el dato
/// completo dentro de la propia pregunta: basta el calendario gregoriano
/// para saber si tiene una lectura válida (DD/MM o MM/DD, pero no ambas),
/// dos lecturas válidas y distintas, o ninguna. No hace falta leer el
/// acervo ni adivinar nada.
///
/// Deliberadamente no cubre la fecha implausible por ser anterior a la
/// fundación de la empresa: ese año de referencia no está escrito en ningún
/// documento del acervo, así que afirmarlo sería inventar un hecho externo
/// que Omega no puede mostrar. Esas preguntas —y cualquier fecha con una
/// única lectura válida— siguen su curso normal, sin que esta función
/// intervenga.
fn date_calendar_clarification(question: &str) -> Option<Answer> {
    if !normalize_exact(question).contains("fecha calendario corresponde") {
        return None;
    }
    let quoted = ToolEngine::quoted_literals(question);
    let [literal] = quoted.as_slice() else {
        return None;
    };
    let (a, b, year) = parse_slash_date(literal)?;
    let day_month_reading = CivilDate::new(year, b, a);
    let month_day_reading = CivilDate::new(year, a, b);
    let clarification = match (day_month_reading, month_day_reading) {
        (None, None) => Clarification {
            question: format!(
                "La fecha «{literal}» no es válida en el calendario gregoriano (día fuera de \
                 rango para el mes indicado); no hay una fecha válida que asignar sin \
                 aclaración."
            ),
            options: vec![],
            reason: "fecha_invalida".to_owned(),
        },
        // Dos lecturas válidas y DISTINTAS. La desigualdad no es un detalle:
        // en «11/11/2025» las dos lecturas son válidas y caen en el mismo día,
        // así que no hay nada que aclarar y declararla ambigua sería afirmar
        // una duda que no existe. Ese caso baja al brazo de interpretación.
        (Some(left), Some(right)) if left != right => Clarification {
            question: format!(
                "La fecha «{literal}» es ambigua entre interpretación DD/MM y MM/DD; no puede \
                 resolverse sin aclaración adicional."
            ),
            options: vec![],
            reason: "fecha_ambigua".to_owned(),
        },
        // Una sola lectura válida —incluida la de «11/11/2025», donde las dos
        // lecturas caen en el mismo día y por tanto no hay dos que distinguir—.
        // No hay ambigüedad que aclarar, pero sí una fecha que dar: sin esto,
        // la ruta de localización devolvía la cadena tal cual, sellada como
        // verificada, que es repetirle al usuario lo que acababa de escribir
        // en vez de contestar a qué fecha corresponde.
        //
        // La lectura no sale del acervo: es el calendario gregoriano aplicado
        // a la cadena que trae la propia pregunta. Por eso la respuesta va sin
        // cita y sin sello de verificada —P0-1 no admite declarar verificado lo
        // que ningún documento sostiene— y su texto dice de dónde sale.
        //
        // Que la fecha resultante sea además implausible por otro motivo
        // (p. ej. anterior a la fundación de la empresa) sigue fuera de
        // alcance: ese año no está escrito en ningún documento del acervo, así
        // que afirmarlo sería inventar un hecho externo.
        (day_month, month_day) => {
            let (reading, kept, discarded) = match (day_month, month_day) {
                (Some(date), Some(same)) if date == same => (date, "DD/MM y MM/DD", None),
                (Some(date), None) => (date, "DD/MM", Some("MM/DD")),
                (None, Some(date)) => (date, "MM/DD", Some("DD/MM")),
                // Las dos lecturas válidas y distintas ya se trataron arriba,
                // y las dos inválidas también.
                _ => return None,
            };
            let note = match discarded {
                Some(other) => format!(
                    "la lectura {other} no existe en el calendario, así que no hay ambigüedad que resolver"
                ),
                None => "las dos lecturas caen en el mismo día, así que no hay ambigüedad que resolver"
                    .to_owned(),
            };
            return Some(Answer::unverified(format!(
                "La fecha «{literal}» corresponde al {} (interpretación {kept}): {note}. La lectura sale de aplicar el calendario gregoriano a la cadena de la pregunta, no del contenido del documento.",
                dates::spanish_long_date(reading)
            )));
        }
    };
    Some(clarify(clarification))
}

/// `"A/B/AAAA"` en sus tres componentes numéricos, sin decidir todavía cuál
/// es el día y cuál el mes.
fn parse_slash_date(literal: &str) -> Option<(u32, u32, i32)> {
    let mut parts = literal.split('/');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let year = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((a, b, year))
}

/// El identificador interno (`D#####`) de un documento ya localizado, para
/// nombrarlo en una respuesta que compara dos documentos entre sí. Es el
/// mismo prefijo numérico del nombre de archivo que `locate_documents_by_key`
/// usa para resolverlo — nunca se afirma que esa clave esté escrita dentro
/// del documento, sólo se usa para señalar cuál es cuál, igual que el usuario
/// ya la usó en la pregunta.
fn document_label(document: &LocatedDocument) -> String {
    let file_name = document
        .path
        .rsplit('/')
        .next()
        .unwrap_or(document.path.as_str());
    let digits = file_name
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    if digits.len() == 5 {
        format!("D{digits}")
    } else {
        file_name.to_owned()
    }
}

/// Evidencia de un hecho estructural del propio índice (el SHA-256 que el
/// indexador calculó sobre los bytes del archivo), no de un valor extraído de
/// su texto. Siempre fiable: el hash no depende de si el OCR pudo leer el
/// documento o de qué tan bien lo hizo.
/// Cómo nombrar un documento en una respuesta que habla de dos.
///
/// El nombre del archivo, que es lo que el usuario puede abrir. Sale de la
/// evidencia de cualquiera de sus valores: la ruta ya viaja ahí.
fn document_label_for(values: &[tools::DocumentValue]) -> String {
    values
        .first()
        .map(|value| {
            value
                .evidence
                .path
                .rsplit('/')
                .next()
                .unwrap_or(value.evidence.path.as_str())
                .to_owned()
        })
        .unwrap_or_else(|| "ese documento".to_owned())
}

fn distinct_field_names(values: &[tools::DocumentValue]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for value in values {
        if !names
            .iter()
            .any(|seen| canonical_key(seen) == canonical_key(&value.field))
        {
            names.push(value.field.clone());
        }
    }
    names
}

/// La operación no se puede hacer con lo que el documento registra.
///
/// Es una aclaración y no una respuesta sin evidencia porque hay evidencia: el
/// valor está ahí, citado y a la vista, y lo que falta es una decisión que
/// sólo el usuario puede tomar —usar otro campo, o aceptar que la pregunta no
/// tiene respuesta con este documento—. Omega dice cuál es el obstáculo y con
/// qué valor exacto se topó, en vez de devolver una cifra inventada o el valor
/// de uno de los operandos disfrazado de resultado.
fn indeterminate(spec: &RowOperandSpec, value: &tools::DocumentValue, why: &str) -> Answer {
    clarify(Clarification {
        question: format!(
            "Indeterminado: {} vale «{}» en ese documento ({}), y {why}. La operación no es calculable con lo que el documento registra y no voy a suponer otro valor. Si querías operar con otro campo, dime cuál.",
            describe_operand(spec),
            value.value.trim(),
            value.evidence.location
        ),
        options: vec![],
        reason: "operacion_indeterminada".to_owned(),
    })
}

/// La cifra de un valor, para operar con ella.
///
/// Primero lo que el índice ya tipó como número o dinero. Si no, la lectura de
/// una medida escrita con su unidad («0 metros», «143.47 litros»): el
/// indexador guarda esos valores como texto —la unidad es parte de lo que el
/// documento escribió y no se descarta— así que la cifra se lee aquí, en la
/// consulta, sin cambiar nada de lo indexado. «N/D kg» no trae ninguna cifra
/// que leer y devuelve `None`, que es exactamente lo que la respuesta necesita
/// para poder declarar el cálculo indeterminado.
fn operand_number(value: &tools::DocumentValue) -> Option<Decimal> {
    let typed = crate::extract::classify_value(&value.field, &value.value);
    if let Some(number) = typed.numeric_value.and_then(Decimal::from_f64) {
        return Some(number);
    }
    measured_number(&value.value).and_then(Decimal::from_f64)
}

/// Un número seguido de su unidad, y nada más. La exigencia de que sólo quede
/// una palabra detrás evita leer como medida lo que no lo es: «12 de marzo de
/// 2024» empieza por un número y no es una cantidad de nada.
fn measured_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let split_at = trimmed
        .find(|character: char| character.is_alphabetic())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split_at);
    let unit = unit.trim();
    let is_unit = unit.is_empty()
        || (unit.split_whitespace().count() == 1
            && unit.chars().all(|character| character.is_alphabetic()));
    if !is_unit {
        return None;
    }
    let number = number.trim().replace(',', "");
    (!number.is_empty()).then(|| number.parse().ok()).flatten()
}

/// Resultado de buscar en un documento el único valor que un operando designa.
enum OperandChoice<'a> {
    One(&'a tools::DocumentValue),
    /// Varios valores distintos: el documento no determina cuál es «el»
    /// valor, así que el motor tampoco.
    Ambiguous(Vec<String>),
    Missing,
}

/// El valor que un operando designa dentro de un documento.
///
/// Un campo nombrado se busca por su nombre; una categoría genérica («el
/// importe»), por el tipo del valor. En los dos casos vale la misma regla que
/// `collect_category_operands`: sólo hay operando cuando el documento registra
/// **un** valor, porque entonces cuál es lo decide el documento. Los repetidos
/// idénticos no cuentan como varios — un mismo importe escrito dos veces no
/// crea una elección.
fn single_value_for<'a>(
    values: &'a [tools::DocumentValue],
    spec: &RowOperandSpec,
) -> OperandChoice<'a> {
    let matching = values
        .iter()
        .filter(|value| match spec {
            RowOperandSpec::Field(name) => canonical_key(&value.field) == canonical_key(name),
            RowOperandSpec::Category(kind) => {
                &crate::extract::classify_value(&value.field, &value.value)
                    .kind
                    .as_str()
                    == kind
            }
        })
        .collect::<Vec<_>>();
    let mut distinct: Vec<String> = Vec::new();
    for value in &matching {
        let normalized = normalize_exact(&value.value);
        if !distinct.iter().any(|seen| normalize_exact(seen) == normalized) {
            distinct.push(value.value.clone());
        }
    }
    match distinct.len() {
        0 => OperandChoice::Missing,
        1 => OperandChoice::One(matching[0]),
        _ => OperandChoice::Ambiguous(distinct),
    }
}

fn describe_operand(spec: &RowOperandSpec) -> String {
    match spec {
        RowOperandSpec::Field(name) => format!("el campo «{name}»"),
        RowOperandSpec::Category("money") => "el importe (su único valor monetario)".to_owned(),
        RowOperandSpec::Category(_) => "la cantidad (su único valor numérico)".to_owned(),
    }
}

/// Cita de un archivo contado por el censo.
///
/// Señala el archivo, no lo que dice: la ubicación empieza por `metadato:` a
/// propósito, para que `Evidence::is_substantive` la reconozca como lo que es
/// y ninguna respuesta apoyada sólo en estas citas pueda declararse verificada.
fn census_evidence(document_id: i64, file: &census::CensusFile) -> Evidence {
    let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_owned();
    Evidence {
        id: format!("censo-{document_id}"),
        document_id,
        path: file.path.clone(),
        origin: file.origin.clone(),
        location: "metadato: nombre de archivo".to_owned(),
        excerpt: name.clone(),
        normalized_value: Some(normalize_exact(&name)),
        value: None,
        matched: Some(name),
        field: Some("nombre de archivo".to_owned()),
        match_kind: "metadato".to_owned(),
        reliable: true,
        ocr_status: Some("not_required".to_owned()),
        ocr_confidence: None,
        confidence: None,
    }
}

fn hash_evidence(document: &LocatedDocument, hash: &str) -> Evidence {
    Evidence {
        id: format!("hash-{}", document.id),
        document_id: document.id,
        path: document.path.clone(),
        origin: document.origin.clone(),
        location: "metadato del archivo (SHA-256 calculado por el indexador)".to_owned(),
        excerpt: hash.to_owned(),
        normalized_value: Some(normalize_exact(hash)),
        value: Some(hash.to_owned()),
        matched: Some(hash.to_owned()),
        field: Some("SHA-256".to_owned()),
        match_kind: "metadato".to_owned(),
        reliable: true,
        ocr_status: Some("not_required".to_owned()),
        ocr_confidence: None,
        confidence: None,
    }
}

/// ¿La pregunta es «qué se puede extraer de este archivo»?
///
/// Se exige que hable de extraer/obtener/sacar información **y** que nombre un
/// archivo: sin lo segundo, «¿qué información hay sobre X?» es una búsqueda
/// corriente y no debe desviarse aquí.
fn asks_what_can_be_extracted(question: &str) -> bool {
    let normalized = normalize_exact(question);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let verb = words.iter().any(|word| {
        word.starts_with("extra") || word.starts_with("obten") || word.starts_with("sacar")
    });
    let about_a_file = words
        .iter()
        .any(|word| word.starts_with("archiv") || word.starts_with("fichero"));
    verb && about_a_file
}

fn asks_for_legal_guidance(question: &str) -> bool {
    let normalized = normalize_exact(question);
    [
        "legal",
        "ley",
        "leyes",
        "norma",
        "normativa",
        "regulacion",
        "obligacion",
        "obligaciones",
        "asesoria",
    ]
    .iter()
    .any(|term| normalized.split_whitespace().any(|word| word == *term))
}

// ---------------------------------------------------------------------------
// Ejecución del plan estructurado
//
// Cada rama consulta el índice, calcula con el motor aritmético y redacta con
// `report`. Ninguna inventa una cifra: si no hay operandos con evidencia, lo
// dice. Al terminar, deja en el contexto los hechos estructurados del turno.
// ---------------------------------------------------------------------------

/// Muestra de operandos que acompaña a un cálculo. No es el conjunto entero:
/// la respuesta declara cuántos valores usó y la interfaz ofrece ver más.
const MAX_OPERAND_CITATIONS: usize = 12;

/// Notas de cálculo por respuesta. Una comparación necesita las dos; un
/// ranking, las de los primeros grupos, no una por cada fila.
const MAX_CALCULATION_CITATIONS: usize = 4;

impl Agent {
    fn compute(
        &self,
        operation: Operation,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let Some(concept) = scope.concept.clone() else {
            // Conteo de documentos: la operación no necesita un campo.
            let citations = self
                .tools
                .evidence_for_documents(&scope.documents, MAX_OPERAND_CITATIONS)?;
            let text = with_scope(report::document_count(scope.documents.len()), scope);
            self.remember_scope(scope, state);
            return Ok(Answer::verified(text, citations).with_scope(answer_scope(scope, None)));
        };
        // Sin filtrar por moneda en la consulta: filtrar aquí después de
        // calcular permite distinguir un documento sin el campo de uno que sí
        // lo tiene, pero en otra moneda, y declarar cada uno por su motivo.
        let operands = self.tools.collect_operands(&ValueQuery {
            concept: &concept,
            documents: Some(&scope.documents),
            group_by: scope.group_by.as_deref(),
            ..ValueQuery::default()
        })?;
        let all_buckets = calc::compute(operation, &operands);
        let (buckets, currency_excluded) = split_by_requested_currency(all_buckets, scope.currency.as_deref());
        let used_documents = operand_document_set(&buckets);
        let with_field = operands
            .iter()
            .map(|operand| operand.document_id)
            .collect::<HashSet<_>>();
        let missing_field = scope
            .documents
            .iter()
            .filter(|id| !with_field.contains(id))
            .count();
        // Un mismo documento puede contener más de un valor. Si alguno sí
        // coincide con la moneda pedida, el documento cuenta como calculado,
        // no también como excluido por otro valor suyo en otra moneda.
        let currency_only = currency_excluded
            .iter()
            .filter(|id| !used_documents.contains(id))
            .copied()
            .collect::<HashSet<_>>();
        let invalid_value = if operation.needs_numbers() {
            with_field
                .iter()
                .filter(|id| !used_documents.contains(id) && !currency_only.contains(id))
                .count()
        } else {
            0
        };
        let exclusions = report::ScopeExclusions {
            missing_field,
            invalid_value,
            currency_mismatch: currency_only.len(),
        };
        if buckets.is_empty() {
            return Ok(no_usable_sum_answer(
                &concept,
                scope.currency.as_deref(),
                exclusions,
                scope,
            ));
        }
        let calculated_documents = used_documents.len();
        let text = with_scope_of(
            report::computation(
                operation,
                &concept,
                &buckets,
                exclusions,
                scope.group_by.as_deref(),
            ),
            scope,
            scope.documents.len(),
        );
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        let has_unreliable_ocr = buckets
            .iter()
            .any(|bucket| bucket.has_unreliable_evidence);
        let partial = !exclusions.is_empty() || has_unreliable_ocr;
        self.remember_scope(scope, state);
        self.remember_computation(operation.label(), &concept, &buckets, state);
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: !partial,
            citations,
            warning: partial.then(|| {
                if has_unreliable_ocr {
                    "Resultado no verificado: al menos un operando procede de OCR de baja confianza.".to_owned()
                } else {
                    "Resultado parcial: algunos documentos del alcance no participaron en el cálculo.".to_owned()
                }
            }),
            ..Answer::default()
        };
        let mut answer_scope = answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            scope.documents.len(),
        );
        answer_scope.excluded_count = Some(exclusions.total() as i64);
        debug_assert_eq!(
            calculated_documents + exclusions.total(),
            scope.documents.len(),
            "alcance = calculados + excluidos para cálculos de un campo"
        );
        Ok(answer.with_scope(answer_scope))
    }

    /// Calcula sobre la CATEGORÍA de valor cuando el campo que la pregunta
    /// nombró no tiene ni un valor en el alcance.
    ///
    /// Esta ruta existe para dejar de elegir entre «todo o nada». Antes, una
    /// suma cuyo campo no estuviera en el alcance se contestaba sólo con «no
    /// encontré valores de X», sin decir si el alcance contenía algo
    /// equivalente ni cuánto. Ahora se calcula sobre los documentos que
    /// **determinan por sí mismos** un único valor de esa categoría, y la
    /// respuesta declara siempre la cobertura: cuántos de cuántos, y el motivo
    /// por el que cada uno de los demás quedó fuera.
    ///
    /// Nunca se presenta como verificada, aunque la cobertura fuera completa:
    /// la cifra es de campos distintos del que se pidió, y eso es una
    /// sustitución que el usuario tiene que ver declarada, no un resultado que
    /// pueda darse por bueno en silencio.
    fn compute_category(
        &self,
        operation: Operation,
        requested: Option<&str>,
        value_type: &str,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let collected = self
            .tools
            .collect_category_operands(value_type, &scope.documents)?;
        let all_buckets = calc::compute(operation, &collected.operands);
        let (buckets, currency_excluded) =
            split_by_requested_currency(all_buckets, scope.currency.as_deref());
        let used_documents = operand_document_set(&buckets);
        let determinate = collected
            .operands
            .iter()
            .map(|operand| operand.document_id)
            .collect::<HashSet<_>>();
        let currency_only = currency_excluded
            .iter()
            .filter(|id| !used_documents.contains(id))
            .copied()
            .collect::<HashSet<_>>();
        let invalid_value = determinate
            .iter()
            .filter(|id| !used_documents.contains(id) && !currency_only.contains(id))
            .count();
        let coverage = report::CategoryCoverage {
            scope_documents: scope.documents.len(),
            used_documents: used_documents.len(),
            without_category: collected.without_documents,
            ambiguous_category: collected.ambiguous_documents,
            invalid_value,
            currency_mismatch: currency_only.len(),
        };
        if buckets.is_empty() {
            // Ningún operando sobrevivió (todos en otra moneda, o ninguno
            // numérico): la negativa vuelve a ser la respuesta correcta, pero
            // ahora acompañada del recuento de por qué.
            let mut answer = Answer::unverified(with_scope(
                match requested {
                    Some(requested) => format!(
                        "No encontré ningún valor de «{requested}» en este alcance, y tampoco pude calcular sobre el campo {} de cada documento.\n\n{}",
                        report::category_adjective(value_type),
                        report::coverage_note(coverage)
                    ),
                    None => format!(
                        "No pude calcular sobre el campo {} de ningún documento de este alcance.\n\n{}",
                        report::category_adjective(value_type),
                        report::coverage_note(coverage)
                    ),
                },
                scope,
            ));
            answer.warning =
                Some("Resultado parcial: ningún valor del alcance pudo calcularse.".to_owned());
            return Ok(answer.with_scope(AnswerScope {
                document_count: Some(scope.documents.len() as i64),
                value_count: Some(0),
                excluded_count: Some(coverage.excluded() as i64),
                ..answer_scope(scope, None)
            }));
        }
        let text = with_scope(
            report::category_computation(
                operation,
                requested,
                value_type,
                &buckets,
                &collected.fields,
                coverage,
            ),
            scope,
        );
        let label = format!("campo {}", report::category_adjective(value_type));
        let citations = calculation_citations(operation.label(), &label, &buckets);
        let has_unreliable_ocr = buckets.iter().any(|bucket| bucket.has_unreliable_evidence);
        self.remember_scope(scope, state);
        // Sustituir el campo pedido por su categoría nunca se da por bueno en
        // silencio. Cuando la categoría ES lo que se pidió no hay sustitución
        // que declarar, y la cifra vale lo que valga su cobertura: completa y
        // con evidencia fiable, se verifica como cualquier otro cálculo.
        let substituted = requested.is_some();
        let complete = coverage.excluded() == 0;
        let warning = match (requested, has_unreliable_ocr) {
            (Some(requested), true) => Some(format!(
                "Resultado no verificado: «{requested}» no tiene valores en este alcance, la cifra es del campo {} de cada documento y cubre {} de {} documentos; además, al menos un operando procede de OCR de baja confianza.",
                report::category_adjective(value_type),
                coverage.used_documents,
                coverage.scope_documents
            )),
            (Some(requested), false) => Some(format!(
                "Resultado no verificado: «{requested}» no tiene valores en este alcance. La cifra es del campo {} de cada documento y cubre {} de {} documentos del alcance.",
                report::category_adjective(value_type),
                coverage.used_documents,
                coverage.scope_documents
            )),
            (None, true) => Some(
                "Resultado no verificado: al menos un operando procede de OCR de baja confianza."
                    .to_owned(),
            ),
            (None, false) => (!complete).then(|| {
                format!(
                    "Resultado parcial: la cifra cubre {} de {} documentos del alcance; el resto no aportó un valor {} único.",
                    coverage.used_documents,
                    coverage.scope_documents,
                    report::category_adjective(value_type)
                )
            }),
        };
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: !substituted && !has_unreliable_ocr && complete,
            citations,
            warning,
            ..Answer::default()
        };
        let mut answer_scope = answer_scope(scope, Some(total_values(&buckets)));
        answer_scope.excluded_count = Some(coverage.excluded() as i64);
        debug_assert_eq!(
            coverage.used_documents + coverage.excluded(),
            scope.documents.len(),
            "alcance = cubiertos + excluidos en un cálculo por categoría"
        );
        Ok(answer.with_scope(answer_scope))
    }

    /// Calcula la misma operación para varios campos sobre un único conjunto
    /// de documentos, en vez de exigir que el usuario elija uno solo.
    ///
    /// Un campo sin valores en el alcance se dice explícitamente en la tabla
    /// («Sin datos»); nunca se inventa un cero ni se omite la fila.
    fn compute_many(
        &self,
        operation: Operation,
        concepts: &[String],
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let mut results = Vec::with_capacity(concepts.len());
        let mut documents = HashSet::new();
        let mut total = 0i64;
        for concept in concepts {
            let operands = self.tools.collect_operands(&ValueQuery {
                concept,
                documents: Some(&scope.documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?;
            let buckets = calc::compute(operation, &operands);
            documents.extend(buckets.iter().flat_map(|bucket| bucket.document_ids.iter().copied()));
            total += total_values(&buckets);
            results.push((concept.clone(), buckets));
        }
        if results.iter().all(|(_, buckets)| buckets.is_empty()) {
            let names = concepts.join("», «");
            return Ok(missing_values_answer(&names, scope));
        }
        let text = with_scope_of(
            report::computation_many(operation, &results),
            scope,
            documents.len(),
        );
        let mut citations = Vec::new();
        let mut seen = HashSet::new();
        for (concept, buckets) in &results {
            if buckets.is_empty() {
                continue;
            }
            for evidence in calculation_citations(operation.label(), concept, buckets) {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence);
                }
            }
        }
        self.remember_scope(scope, state);
        // Falta uno de los campos pedidos, o alguno de ellos mezcla monedas
        // distintas dentro de sí mismo: la tabla es real y no inventa nada,
        // pero no puede declararse enteramente verificada porque no cubre
        // todo lo que se pidió con una sola cifra por campo.
        let fully_verified = calc::multi_field_is_fully_verified(&results);
        let has_unreliable_ocr = results
            .iter()
            .flat_map(|(_, buckets)| buckets)
            .any(|bucket| bucket.has_unreliable_evidence);
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: fully_verified && !has_unreliable_ocr,
            citations,
            warning: (!fully_verified || has_unreliable_ocr).then(|| {
                if has_unreliable_ocr {
                    "Resultado no verificado: la evidencia incluye OCR de baja confianza.".to_owned()
                } else {
                    "Resultado parcial: no todos los campos pedidos tienen una cifra única y verificable en este alcance.".to_owned()
                }
            }),
            ..Answer::default()
        };
        Ok(answer.with_scope(answer_scope_of(scope, Some(total), documents.len())))
    }

    /// Combina dos campos numéricos del mismo documento («Cantidad ×
    /// Precio unitario»), documento por documento, y suma los resultados si
    /// hay más de uno. Nunca opera entre los totales globales de cada
    /// campo: esa sería una cifra distinta de la que se pidió.
    fn compute_row(
        &self,
        operation: RowOperation,
        left: &str,
        right: &str,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        if scope.documents.is_empty() {
            return Ok(empty_scope_answer(scope));
        }
        let left_operands = self.tools.collect_operands(&ValueQuery {
            concept: left,
            documents: Some(&scope.documents),
            ..ValueQuery::default()
        })?;
        let right_operands = self.tools.collect_operands(&ValueQuery {
            concept: right,
            documents: Some(&scope.documents),
            ..ValueQuery::default()
        })?;
        if left_operands.is_empty() {
            return Ok(missing_values_answer(left, scope));
        }
        if right_operands.is_empty() {
            return Ok(missing_values_answer(right, scope));
        }
        let computation =
            calc::compute_row(operation, &left_operands, &right_operands, &scope.documents);
        let breakdown = computation.breakdown;
        debug_assert!(
            breakdown.is_exhaustive(),
            "cada documento del alcance debe caer en exactamente una categoría: {breakdown:?}"
        );
        // El alcance declarado es el filtro original completo, no sólo los
        // documentos que además tenían ambos campos: un documento excluido
        // (por moneda, por valor inválido, por faltarle un campo o por no
        // traer ninguno de los dos) sigue habiendo estado en el alcance de la
        // pregunta.
        let text = with_scope_of(
            report::row_computation(operation, left, right, &computation),
            scope,
            scope.documents.len(),
        );
        if computation.outcomes.is_empty() {
            let mut answer = Answer::unverified(text);
            answer.used_context = scope.inherited;
            let mut empty_scope = answer_scope_of(scope, Some(0), scope.documents.len());
            empty_scope.excluded_count = Some(breakdown.excluded() as i64);
            return Ok(answer.with_scope(empty_scope));
        }
        let mut citations = Vec::new();
        let mut seen = HashSet::new();
        for outcome in computation.outcomes.iter().take(MAX_OPERAND_CITATIONS) {
            for evidence in [&outcome.left_evidence, &outcome.right_evidence] {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence.clone());
                }
            }
        }
        // Un documento excluido también se rastrea a sus dos operandos: la
        // explicación de por qué no entró debe poder verificarse igual que
        // la cifra que sí se publicó.
        for skip in computation.skipped.iter().take(MAX_OPERAND_CITATIONS) {
            for evidence in [&skip.left_evidence, &skip.right_evidence] {
                if seen.insert(evidence.id.clone()) {
                    citations.push(evidence.clone());
                }
            }
        }
        self.remember_scope(scope, state);
        // Cualquier documento del alcance que no produjo una cifra —unidades
        // incompatibles, división entre cero, valor inválido, sólo uno de los
        // dos campos, o ninguno de los dos— se cuenta y se explica, y le
        // quita a la respuesta el derecho a declararse verificada. La cuenta
        // sale del reparto del alcance completo: mirar sólo `skipped` dejaba
        // fuera a los documentos sin ninguno de los dos campos, que es
        // exactamente como una respuesta sobre 140 de 600 documentos podía
        // presentarse como verificada.
        let has_issues = breakdown.excluded() > 0;
        let has_unreliable_ocr = computation.has_unreliable_evidence;
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: !has_issues && !has_unreliable_ocr,
            citations,
            warning: (has_issues || has_unreliable_ocr).then(|| {
                if has_unreliable_ocr {
                    "Resultado no verificado: la evidencia incluye OCR de baja confianza.".to_owned()
                } else {
                    "Resultado parcial: algunos documentos del alcance no participaron en el cálculo.".to_owned()
                }
            }),
            ..Answer::default()
        };
        // Alcance, uso y exclusión se guardan por separado y cuadran entre
        // sí: `document_count` es el filtro original completo,
        // `value_count` los documentos que de verdad produjeron una cifra, y
        // `excluded_count` todos los demás. Por construcción,
        // `document_count == value_count + excluded_count`.
        let mut answer_scope = answer_scope_of(
            scope,
            Some(breakdown.calculated as i64),
            scope.documents.len(),
        );
        answer_scope.excluded_count = Some(breakdown.excluded() as i64);
        Ok(answer.with_scope(answer_scope))
    }

    fn rank(
        &self,
        operation: Operation,
        descending: bool,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let (Some(concept), Some(group)) = (scope.concept.clone(), scope.group_by.clone()) else {
            return Ok(empty_scope_answer(scope));
        };
        let operands = self.tools.collect_operands(&ValueQuery {
            concept: &concept,
            documents: Some(&scope.documents),
            group_by: Some(&group),
            currency: scope.currency.as_deref(),
            ..ValueQuery::default()
        })?;
        let mut buckets = calc::compute(operation, &operands);
        if buckets.is_empty() {
            return Ok(missing_values_answer(&concept, scope));
        }
        buckets.sort_by(|left, right| {
            if descending {
                right.value.cmp(&left.value)
            } else {
                left.value.cmp(&right.value)
            }
            .then_with(|| left.group.cmp(&right.group))
        });
        let documents = operand_documents(&buckets);
        let text = with_scope_of(
            report::ranking(operation, &concept, &group, &buckets, descending),
            scope,
            documents,
        );
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        self.remember_scope(scope, state);
        self.remember_computation(operation.label(), &concept, &buckets, state);
        state.group_by = Some(group);
        let has_unreliable_ocr = buckets
            .iter()
            .any(|bucket| bucket.has_unreliable_evidence);
        let answer = Answer {
            text,
            mode: "local".into(),
            verified: !has_unreliable_ocr,
            citations,
            warning: has_unreliable_ocr.then(|| {
                "Resultado no verificado: al menos un operando procede de OCR de baja confianza.".to_owned()
            }),
            ..Answer::default()
        };
        Ok(answer.with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    fn compare_groups(
        &self,
        operation: Operation,
        dimension: &ComparisonDimension,
        left: &str,
        right: &str,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let Some(concept) = scope.concept.clone() else {
            return Ok(missing_values_answer("el campo pedido", scope));
        };
        let mut sides = Vec::new();
        for value in [left, right] {
            // Cada lado acota lo que le corresponde: un valor de campo entra
            // como filtro, y una carpeta como origen. La carpeta no es un
            // campo de ningún documento, así que pedirla como filtro no
            // encontraría nada y los dos lados saldrían vacíos.
            let mut filters = scope.filters.clone();
            let mut origin = scope.origin.clone();
            match dimension {
                ComparisonDimension::Concept(name) => filters.push(ToolFilter {
                    concept: name.clone(),
                    equals: value.to_owned(),
                }),
                ComparisonDimension::Origin => origin = Some(value.to_owned()),
            }
            let documents = self
                .tools
                .documents_matching(&filters, origin.as_deref(), scope.date.as_ref())?
                .into_iter()
                .map(|document| document.id)
                .collect::<Vec<_>>();
            let operands = self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?;
            sides.push(calc::compute(operation, &operands));
        }
        let (left_buckets, right_buckets) = (sides[0].clone(), sides[1].clone());
        let mut buckets = left_buckets.clone();
        buckets.extend(right_buckets.clone());
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        let documents = operand_documents(&buckets);

        if left_buckets.len() > 1 || right_buckets.len() > 1 {
            self.remember_scope(scope, state);
            return Ok(multi_currency_comparison(
                operation,
                &concept,
                dimension.label(),
                (left, &left_buckets),
                (right, &right_buckets),
                scope,
                documents,
            ));
        }
        let text = with_scope_of(
            report::comparison(
                operation,
                &concept,
                dimension.label(),
                (left, left_buckets.first()),
                (right, right_buckets.first()),
            ),
            scope,
            documents,
        );
        // Una comparación a la que le falta un lado o que mezcla monedas no
        // resolvió la pregunta: se responde explicándolo, sin marcarla como
        // verificada.
        let complete = left_buckets.first().is_some()
            && right_buckets.first().is_some()
            && left_buckets.first().map(|bucket| bucket.currency.clone())
                == right_buckets.first().map(|bucket| bucket.currency.clone())
            && buckets.iter().all(|bucket| !bucket.has_unreliable_evidence);
        self.remember_scope(scope, state);
        state.concept = Some(concept.clone());
        state.comparison = Some(ComparisonMemory {
            concept: concept.clone(),
            dimension: dimension.label().to_owned(),
            left_label: left.to_owned(),
            right_label: right.to_owned(),
            left: side_memory(left_buckets.first()),
            right: side_memory(right_buckets.first()),
            evidence: citations.clone(),
            has_unreliable_evidence: buckets
                .iter()
                .any(|bucket| bucket.has_unreliable_evidence),
        });
        Ok(Answer {
            text,
            mode: "local".into(),
            verified: complete,
            citations,
            warning: (!complete).then(|| {
                "Resultado no verificado: falta evidencia completa, hay monedas incompatibles u OCR de baja confianza.".to_owned()
            }),
            ..Answer::default()
        }
        .with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    fn compare_periods(
        &self,
        operation: Operation,
        previous: &DateConstraint,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let Some(concept) = scope.concept.clone() else {
            return Ok(missing_values_answer("el campo pedido", scope));
        };
        let current = calc::compute(
            operation,
            &self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&scope.documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?,
        );
        let previous_documents = self
            .tools
            .documents_matching(&scope.filters, scope.origin.as_deref(), Some(previous))?
            .into_iter()
            .map(|document| document.id)
            .collect::<Vec<_>>();
        let earlier = calc::compute(
            operation,
            &self.tools.collect_operands(&ValueQuery {
                concept: &concept,
                documents: Some(&previous_documents),
                currency: scope.currency.as_deref(),
                ..ValueQuery::default()
            })?,
        );
        let current_label = scope
            .range
            .as_ref()
            .map(|range| range.label())
            .unwrap_or_else(|| "periodo actual".into());
        let previous_label = format!("{} a {}", previous.from, previous.to);
        let text = report::periods(
            operation,
            &concept,
            &previous.concept,
            (&previous_label, earlier.first()),
            (&current_label, current.first()),
        );
        let mut buckets = earlier.clone();
        buckets.extend(current.clone());
        let citations = calculation_citations(operation.label(), &concept, &buckets);
        let documents = operand_documents(&buckets);
        let complete = earlier.first().is_some()
            && current.first().is_some()
            && earlier.first().map(|bucket| bucket.currency.clone())
                == current.first().map(|bucket| bucket.currency.clone())
            && buckets.iter().all(|bucket| !bucket.has_unreliable_evidence);
        self.remember_scope(scope, state);
        state.concept = Some(concept.clone());
        state.comparison = Some(ComparisonMemory {
            concept: concept.clone(),
            dimension: previous.concept.clone(),
            left_label: previous_label,
            right_label: current_label,
            left: side_memory(earlier.first()),
            right: side_memory(current.first()),
            evidence: citations.clone(),
            has_unreliable_evidence: buckets
                .iter()
                .any(|bucket| bucket.has_unreliable_evidence),
        });
        Ok(Answer {
            text: with_scope_of(text, scope, documents),
            mode: "local".into(),
            verified: complete,
            citations,
            warning: (!complete).then(|| {
                "Resultado no verificado: falta evidencia completa, hay monedas incompatibles u OCR de baja confianza.".to_owned()
            }),
            ..Answer::default()
        }
        .with_scope(answer_scope_of(
            scope,
            Some(total_values(&buckets)),
            documents,
        )))
    }

    /// Diferencia o porcentaje de la comparación anterior, sin volver a
    /// consultar el acervo: los dos lados ya están en el contexto con su
    /// evidencia.
    fn comparison_follow_up(&self, percentage: bool, state: &ConversationState) -> Answer {
        let Some(comparison) = &state.comparison else {
            return no_evidence_answer();
        };
        let (Some(left), Some(right)) = (&comparison.left, &comparison.right) else {
            let missing = if comparison.left.is_none() {
                &comparison.left_label
            } else {
                &comparison.right_label
            };
            let mut answer = Answer::unverified(format!(
                "La comparación anterior no tiene valores para «{missing}», así que su diferencia no está definida."
            ));
            answer.used_context = true;
            return answer;
        };
        if left.currency != right.currency {
            let mut answer = Answer::unverified(format!(
                "No puedo restar los dos lados de la comparación anterior: «{}» está en {} y «{}» en {}.",
                comparison.left_label,
                left.currency.clone().unwrap_or_else(|| "sin moneda".into()),
                comparison.right_label,
                right.currency.clone().unwrap_or_else(|| "sin moneda".into())
            ));
            answer.used_context = true;
            return answer;
        }
        let first = Decimal::from_raw(left.units);
        let second = Decimal::from_raw(right.units);
        let difference = second.sub(first);
        let text = if percentage {
            match Decimal::percent_change(first, second) {
                Some(change) => format!(
                    "Respecto a «{}» ({}), «{}» ({}) representa una variación de {} %. La diferencia absoluta es {}. Cálculo local de Omega sobre {} y {} valores con evidencia.",
                    comparison.left_label,
                    left.rendered,
                    comparison.right_label,
                    right.rendered,
                    change.render_signed(),
                    calc::render_amount(difference.abs(), left.currency.as_deref()),
                    left.value_count,
                    right.value_count
                ),
                None => format!(
                    "La variación porcentual no está definida: «{}» vale {} y no se puede dividir entre cero. La diferencia absoluta es {}.",
                    comparison.left_label,
                    left.rendered,
                    calc::render_amount(difference.abs(), left.currency.as_deref())
                ),
            }
        } else {
            format!(
                "Diferencia entre «{}» ({}) y «{}» ({}) en «{}», comparados por «{}»: {}; en valor absoluto, {}. Cálculo local de Omega sobre {} y {} valores con evidencia.",
                comparison.right_label,
                right.rendered,
                comparison.left_label,
                left.rendered,
                comparison.concept,
                comparison.dimension,
                calc::render_amount(difference, left.currency.as_deref()),
                calc::render_amount(difference.abs(), left.currency.as_deref()),
                left.value_count,
                right.value_count
            )
        };
        // La diferencia se deriva de la comparación anterior: hereda su
        // procedencia. Si alguno de aquellos operandos venía de OCR débil,
        // esta cifra tampoco puede declararse verificada aunque las citas
        // que se muestran aquí sí sean fiables.
        let mut answer = if comparison.has_unreliable_evidence {
            let mut derived = Answer::unverified(text);
            derived.citations = comparison.evidence.clone();
            derived.warning = Some(
                "Resultado no verificado: al menos un operando procede de OCR de baja confianza."
                    .to_owned(),
            );
            derived
        } else {
            Answer::verified(text, comparison.evidence.clone())
        };
        answer.used_context = true;
        answer
    }

    /// Responde sobre el documento que ocupa una posición del conjunto
    /// anterior («¿cuál es el Responsable del primero?»).
    ///
    /// La posición se aplica sobre el conjunto **reevaluado**, no sobre una
    /// lista guardada: el predicado se resuelve otra vez contra el índice y su
    /// orden es el mismo orden estable con el que se enumeró en el turno
    /// anterior. Si el conjunto encogió y la posición ya no existe, se dice —
    /// nunca se devuelve el documento más cercano.
    fn document_in_context(
        &self,
        question: &str,
        selection: &DocumentSelection,
        scope: &PlannedScope,
        state: &mut ConversationState,
    ) -> Result<Answer> {
        let position = match selection {
            DocumentSelection::Position(position) => *position,
            DocumentSelection::LastCited(path) => {
                let Some(document) = self.tools.document_by_path(path)? else {
                    let mut answer = Answer::unverified(
                        "El documento del que hablaba la respuesta anterior ya no está en el índice, así que no puedo responder sobre él.",
                    );
                    answer.used_context = true;
                    return Ok(answer);
                };
                // La referencia se conserva para el turno siguiente: «¿y el
                // Responsable?» debe seguir hablando del mismo documento.
                state.document = Some(document.path.clone());
                return self.answer_about_the_document(question, &document, None);
            }
        };
        self.remember_scope(scope, state);
        let total = scope.documents.len();
        if total == 0 {
            return Ok(empty_scope_answer(scope));
        }
        let index = match position {
            OrdinalPosition::Nth(index) => index,
            OrdinalPosition::Last => total - 1,
        };
        let Some(&document_id) = scope.documents.get(index) else {
            let mut answer = Answer::unverified(with_scope(
                format!(
                    "El conjunto anterior tiene {total} documento{}, así que no existe el número {}. No voy a responder con el más cercano.",
                    if total == 1 { "" } else { "s" },
                    index + 1
                ),
                scope,
            ));
            answer.used_context = true;
            return Ok(answer);
        };
        let Some(document) = self.tools.document_by_id(document_id)? else {
            return Ok(no_evidence_answer());
        };
        let ordinal_scope = AnswerScope {
            document_count: Some(total as i64),
            ..answer_scope(scope, None)
        };
        state.document = Some(document.path.clone());
        self.answer_about_the_document(question, &document, Some(ordinal_scope))
    }

    /// Redacta la respuesta sobre un documento ya señalado por el contexto, o
    /// dice con precisión que ese documento no registra lo que se le pide.
    fn answer_about_the_document(
        &self,
        question: &str,
        document: &LocatedDocument,
        scope: Option<AnswerScope>,
    ) -> Result<Answer> {
        let file = document
            .path
            .rsplit('/')
            .next()
            .unwrap_or(document.path.as_str());
        let mut answer = match self.answer_about_document(question, document)? {
            Some(answer) => answer,
            // El documento existe, pero no registra el campo que la pregunta
            // pide. Nombrarlo es más útil —y más honesto— que la negativa
            // genérica: el usuario sabe entonces que la referencia se resolvió
            // y que lo que falta es el dato.
            None => Answer::unverified(format!(
                "El documento al que se refiere la pregunta es {file} (carpeta {}), pero no encontré en él un campo que responda esa pregunta.",
                document.origin
            )),
        };
        answer.used_context = true;
        Ok(match scope {
            Some(scope) => answer.with_scope(scope),
            None => answer,
        })
    }

    fn evidence_for_last(&self, state: &ConversationState) -> Answer {
        let Some(previous) = &state.last_result else {
            return no_evidence_answer();
        };
        let text = report::supporting_documents(
            &previous.operation,
            &previous.concept,
            &previous.rendered,
            previous.value_count,
            &previous.evidence,
        );
        // Los documentos que respaldan un total heredan la procedencia de ese
        // total. `previous.evidence` está recortada, así que la señal se lee
        // de la memoria del cálculo y no de las citas que sobrevivieron.
        let mut answer = if previous.has_unreliable_evidence {
            let mut derived = Answer::unverified(text);
            derived.citations = previous.evidence.clone();
            derived.warning = Some(
                "Resultado no verificado: al menos un operando procede de OCR de baja confianza."
                    .to_owned(),
            );
            derived
        } else {
            Answer::verified(text, previous.evidence.clone())
        };
        answer.used_context = true;
        answer
    }

    fn relation_without_key(&self, question: &str) -> Result<Answer> {
        let subject = self
            .tools
            .filters_from_query(question, None, true)?
            .into_iter()
            .map(|filter| filter.equals)
            .next()
            .or_else(|| subject_after_container(question));
        let Some(subject) = subject else {
            return Ok(Answer::unverified(
                "No puedo afirmar una relación: no encontré en la pregunta un valor con clave estable, y unir documentos por parecido de nombres no es evidencia.",
            ));
        };
        let mentions = relations::mentions_without_key(&self.tools, &subject)?;
        let citations = mentions
            .iter()
            .map(|mention| mention.evidence.clone())
            .collect::<Vec<_>>();
        let text = report::relation_without_key(&subject, &mentions);
        // Hay evidencia de menciones, pero no de un vínculo: la respuesta se
        // marca como no verificada porque lo que se pidió —una relación— no
        // está respaldado.
        Ok(Answer {
            text,
            mode: "local".into(),
            verified: false,
            citations,
            ..Answer::default()
        })
    }

    fn contradictions(
        &self,
        key: Option<&str>,
        compared: Option<&str>,
        identifier: Option<&str>,
    ) -> Result<Answer> {
        // Con la clave nombrada se mira ese expediente y sólo ése. Sin ella,
        // el barrido global conserva su tope: recorrer el acervo entero por
        // cada pregunta no es viable, pero decir «no hay contradicciones»
        // después de mirar 400 claves de miles sí era engañoso cuando el
        // usuario había nombrado la suya.
        let found = match identifier {
            Some(canonical) => relations::contradictions_for(&self.tools, canonical, compared)?,
            None => relations::contradictions(&self.tools, key, compared)?,
        };
        if found.is_empty() {
            if let Some(canonical) = identifier {
                return Ok(Answer::unverified(format!(
                    "No encontré evidencia de contradicción entre los documentos que comparten «{canonical}»: los campos que los dos declaran coinciden."
                )));
            }
            return Ok(Answer::unverified(match (key, compared) {
                (Some(key), Some(compared)) => format!(
                    "No encontré evidencia de contradicción: ningún valor de «{key}» se repite en dos documentos con «{compared}» distintos."
                ),
                (Some(key), None) => format!(
                    "No encontré evidencia de contradicción: ningún valor de «{key}» se repite en dos documentos con campos incompatibles."
                ),
                _ => "No encontré documentos que compartan una clave estable y declaren valores incompatibles.".to_owned(),
            }));
        }
        let citations = found
            .iter()
            .flat_map(|item| item.entries.iter().map(|entry| entry.evidence.clone()))
            .take(MAX_OPERAND_CITATIONS)
            .collect::<Vec<_>>();
        Ok(Answer::verified(report::contradictions(&found), citations))
    }

    fn dossier(&self, canonical: &str, state: &mut ConversationState) -> Result<Answer> {
        let Some(dossier) = relations::dossier(&self.tools, canonical)? else {
            return Ok(no_evidence_answer());
        };
        let mut citations = dossier
            .group
            .documents
            .iter()
            .map(|document| document.evidence.clone())
            .collect::<Vec<_>>();
        let mut seen = citations
            .iter()
            .map(|evidence| evidence.id.clone())
            .collect::<HashSet<_>>();
        for field in &dossier.fields {
            for value in &field.values {
                if citations.len() >= 60 {
                    break;
                }
                if seen.insert(value.evidence.id.clone()) {
                    citations.push(value.evidence.clone());
                }
            }
        }
        let text = report::dossier(&dossier);
        state.identifier = Some(canonical.to_owned());
        state.set = Some(crate::conversation::DocumentSet {
            identifier: Some(canonical.to_owned()),
            document_count: dossier.group.documents.len() as i64,
            ..Default::default()
        });
        Ok(Answer::verified(text, citations))
    }

    fn remember_scope(&self, scope: &PlannedScope, state: &mut ConversationState) {
        state.set = Some(scope.as_document_set());
        state.concept = scope.concept.clone();
        state.currency = scope.currency.clone();
        state.identifier = scope.identifier.clone();
    }

    fn remember_computation(
        &self,
        operation: &str,
        concept: &str,
        buckets: &[Bucket],
        state: &mut ConversationState,
    ) {
        let Some(first) = buckets.first() else {
            return;
        };
        let rendered = if buckets.len() == 1 {
            report::bucket_amount(first)
        } else {
            buckets
                .iter()
                .map(report::bucket_amount)
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let evidence = buckets
            .iter()
            .flat_map(|bucket| bucket.evidence.iter().cloned())
            .collect::<Vec<_>>();
        state.last_result = Some(ComputationMemory::new(
            operation,
            concept,
            rendered,
            total_values(buckets) as usize,
            evidence,
            // Se lee de TODOS los buckets, no de la evidencia recordada: el
            // operando de OCR débil puede quedar fuera de la muestra.
            buckets.iter().any(|bucket| bucket.has_unreliable_evidence),
        ));
        state.currency = first.currency.clone();
    }
}

/// Sujeto escrito después de la palabra que nombra al contenedor: «resume el
/// expediente X» habla de X, aunque X no exista como valor de ningún campo.
fn subject_after_container(question: &str) -> Option<String> {
    const CONTAINERS: &[&str] = &[
        "expediente",
        "expedientes",
        "documento",
        "documentos",
        "registro",
        "registros",
        "archivo",
        "archivos",
        "caso",
        "casos",
    ];
    let words = question.split_whitespace().collect::<Vec<_>>();
    let position = words
        .iter()
        .position(|word| CONTAINERS.contains(&normalize_exact(word).as_str()))?;
    let tail = words
        .get(position + 1..)
        .map(|rest| rest.join(" "))
        .unwrap_or_default();
    let tail = tail
        .trim()
        .trim_end_matches(['.', '?', '!', ',', ';'])
        .trim()
        .to_owned();
    (!tail.is_empty() && tail.split_whitespace().count() <= 6).then_some(tail)
}

fn total_values(buckets: &[Bucket]) -> i64 {
    buckets.iter().map(|bucket| bucket.value_count as i64).sum()
}

/// Documentos que realmente aportaron un operando. El alcance publicado debe
/// ser este, no el tamaño del conjunto consultado: decir «600 documentos»
/// cuando sólo 140 tenían el campo describe mal el cálculo.
fn operand_documents(buckets: &[Bucket]) -> usize {
    operand_document_set(buckets).len()
}

fn operand_document_set(buckets: &[Bucket]) -> HashSet<i64> {
    buckets
        .iter()
        .flat_map(|bucket| bucket.document_ids.iter().copied())
        .collect()
}

/// Separa los cubos de un cálculo entre los que están en la moneda pedida y
/// los que no, cuando la pregunta pidió una moneda concreta. Sin moneda
/// pedida, todos los cubos se conservan y no hay excluidos.
///
/// Se aplica después de `calc::compute` en vez de filtrar en la consulta SQL
/// para poder distinguir, más adelante, un documento sin el campo de uno que
/// sí lo tiene pero en otra moneda: filtrado en SQL, el segundo caso
/// desaparece sin dejar rastro.
fn split_by_requested_currency(
    buckets: Vec<Bucket>,
    wanted: Option<&str>,
) -> (Vec<Bucket>, HashSet<i64>) {
    let Some(wanted) = wanted else {
        return (buckets, HashSet::new());
    };
    let mut matching = Vec::new();
    let mut excluded = HashSet::new();
    for bucket in buckets {
        if bucket
            .currency
            .as_deref()
            .is_some_and(|code| code.eq_ignore_ascii_case(wanted))
        {
            matching.push(bucket);
        } else {
            excluded.extend(bucket.document_ids.iter().copied());
        }
    }
    (matching, excluded)
}


/// Añade al texto la línea de alcance con los mismos datos que consultó el
/// motor. Un cálculo sin su alcance no es reproducible.
fn with_scope(text: String, scope: &PlannedScope) -> String {
    with_scope_of(text, scope, scope.documents.len())
}

fn with_scope_of(text: String, scope: &PlannedScope, documents: usize) -> String {
    let mut parts = Vec::new();
    if scope.inherited {
        parts.push("resultado anterior".to_owned());
    }
    if let Some(origin) = &scope.origin {
        parts.push(format!("carpeta = {origin}"));
    }
    for filter in &scope.filters {
        parts.push(format!("{} = {}", filter.concept, filter.equals));
    }
    if let Some(identifier) = &scope.identifier {
        parts.push(format!("identificador = {identifier}"));
    }
    if let Some(date) = &scope.date {
        parts.push(date.label());
    }
    parts.push(format!(
        "{documents} {}",
        report::plural(documents, "documento", "documentos")
    ));
    match report::scope_line(&parts) {
        Some(line) => format!("{text}\n\n{line}"),
        None => text,
    }
}

fn answer_scope_of(
    scope: &PlannedScope,
    value_count: Option<i64>,
    documents: usize,
) -> AnswerScope {
    AnswerScope {
        document_count: Some(documents as i64),
        ..answer_scope(scope, value_count)
    }
}

fn answer_scope(scope: &PlannedScope, value_count: Option<i64>) -> AnswerScope {
    AnswerScope {
        filters: scope.filters.clone(),
        origin: scope.origin.clone(),
        concept: scope.concept.clone(),
        group_by: scope.group_by.clone(),
        date: scope.date.clone(),
        currency: scope.currency.clone(),
        document_count: Some(scope.documents.len() as i64),
        value_count,
        excluded_count: None,
        inherited: scope.inherited,
    }
}

/// Citas de un cálculo: primero la nota que declara la operación local, después
/// los operandos que la respaldan.
fn calculation_citations(operation: &str, concept: &str, buckets: &[Bucket]) -> Vec<Evidence> {
    let mut citations = Vec::new();
    for bucket in buckets.iter().take(MAX_CALCULATION_CITATIONS) {
        citations.push(report::calculation_evidence(
            operation,
            concept,
            &report::bucket_amount(bucket),
            bucket.value_count,
            bucket.evidence.first(),
        ));
    }
    let mut seen = citations
        .iter()
        .map(|evidence| evidence.id.clone())
        .collect::<HashSet<_>>();
    // Muestra por turnos entre grupos: una comparación o un ranking deben
    // enseñar evidencia de todos sus lados, no llenar el cupo con el primero.
    let mut round = 0;
    while citations.len() < MAX_OPERAND_CITATIONS + citations.len().min(MAX_CALCULATION_CITATIONS) {
        let mut added = false;
        for bucket in buckets {
            let Some(evidence) = bucket.evidence.get(round) else {
                continue;
            };
            added = true;
            if seen.insert(evidence.id.clone()) {
                citations.push(evidence.clone());
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    // Cada ronda añade una evidencia por grupo antes de volver a comprobar el
    // tope, así que una agrupación con varios grupos puede rebasarlo. El
    // límite final es el mismo que aplica la ruta clásica de agregación: una
    // sola política, también para el tamaño de la muestra.
    //
    // Recortar aquí no puede ocultar un OCR débil: la señal se calcula sobre
    // todos los operandos (`Bucket::has_unreliable_evidence`), nunca sobre las
    // citas que sobrevivieron a este corte.
    citations.truncate(MAX_AGGREGATE_CITATIONS);
    citations
}

fn side_memory(bucket: Option<&Bucket>) -> Option<ComparisonSide> {
    bucket.map(|bucket| ComparisonSide {
        rendered: report::bucket_amount(bucket),
        value_count: bucket.value_count,
        currency: bucket.currency.clone(),
        units: bucket.value.raw(),
    })
}

fn multi_currency_comparison(
    operation: Operation,
    concept: &str,
    dimension: &str,
    left: (&str, &[Bucket]),
    right: (&str, &[Bucket]),
    scope: &PlannedScope,
    documents: usize,
) -> Answer {
    let mut rows = Vec::new();
    for (label, buckets) in [left, right] {
        for bucket in buckets {
            rows.push(vec![
                label.to_owned(),
                bucket
                    .currency
                    .clone()
                    .unwrap_or_else(|| "Sin moneda".into()),
                report::bucket_amount(bucket),
                bucket.value_count.to_string(),
            ]);
        }
    }
    let text = with_scope_of(
        format!(
            "No puedo comparar «{}» y «{}» con una sola cifra: hay más de una moneda en «{concept}» y las cantidades no se combinan. Cada moneda por separado:\n\n{}",
            left.0,
            right.0,
            report::table(
                &[dimension, "Moneda", report::operation_title(operation), "Valores"],
                &rows
            )
        ),
        scope,
        documents,
    );
    let mut buckets = left.1.to_vec();
    buckets.extend(right.1.to_vec());
    // La pregunta pedía una comparación y no se pudo dar: la respuesta informa,
    // pero no se presenta como verificada.
    Answer {
        text,
        mode: "local".into(),
        verified: false,
        citations: calculation_citations(operation.label(), concept, &buckets),
        ..Answer::default()
    }
    .with_scope(answer_scope_of(scope, Some(total_values(&buckets)), documents))
}

fn empty_scope_answer(scope: &PlannedScope) -> Answer {
    let mut answer = Answer::unverified(with_scope(
        "No encontré documentos con evidencia local que cumplan ese alcance.".to_owned(),
        scope,
    ));
    answer.used_context = scope.inherited;
    answer
}

/// Todos los valores de este campo en el alcance existen, pero en una moneda
/// distinta de la pedida. Es un caso distinto de «no hay valores»: los datos
/// están, sólo no en la moneda que se pidió.
fn no_usable_sum_answer(
    concept: &str,
    wanted_currency: Option<&str>,
    exclusions: report::ScopeExclusions,
    scope: &PlannedScope,
) -> Answer {
    let reason = match wanted_currency {
        Some(currency) if exclusions.currency_mismatch > 0 => format!(
            "No encontré valores calculables de «{concept}» en {currency}; los valores disponibles tienen moneda incompatible."
        ),
        _ => format!(
            "No encontré valores numéricos calculables de «{concept}» en ese alcance."
        ),
    };
    let note = report::exclusion_note(exclusions).unwrap_or_default();
    let text = with_scope(
        [reason, note]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        scope,
    );
    let mut answer = Answer::unverified(text);
    answer.warning = Some("Resultado parcial: ningún valor del alcance pudo calcularse.".to_owned());
    answer.with_scope(AnswerScope {
        document_count: Some(scope.documents.len() as i64),
        value_count: Some(0),
        excluded_count: Some(exclusions.total() as i64),
        ..answer_scope(scope, None)
    })
}

fn missing_values_answer(concept: &str, scope: &PlannedScope) -> Answer {
    let mut answer = Answer::unverified(with_scope(
        format!("No encontré valores de «{concept}» con evidencia en ese alcance, así que no puedo calcular nada sobre ellos."),
        scope,
    ));
    answer.used_context = scope.inherited;
    answer
}

fn clarify(clarification: Clarification) -> Answer {
    let text = if clarification.options.is_empty() {
        clarification.question.clone()
    } else {
        format!(
            "{}\n\n{}",
            clarification.question,
            clarification
                .options
                .iter()
                .map(|option| format!("- {option}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Answer {
        text,
        mode: "local".into(),
        verified: false,
        clarification: Some(clarification),
        ..Answer::default()
    }
}
