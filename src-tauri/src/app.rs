use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    agent::Agent,
    conversation::ConversationMemory,
    dates::Clock,
    db::Database,
    error::{OmegaError, Result},
    indexer::Indexer,
    model::{Answer, AppStatus, ConceptSummary, IndexReport, SearchHit, SourceSummary},
    parser::LocalDocumentParser,
    tools::ToolEngine,
};

#[derive(Clone)]
pub struct OmegaEngine {
    database: Database,
    tools: ToolEngine,
    /// Fuente de la fecha actual. Inyectarla es un requisito, no una comodidad:
    /// «el mes pasado» debe poder fijarse en una prueba y mostrarse resuelto.
    clock: Clock,
    /// Memoria de conversación en proceso. No se persiste: cerrar la
    /// aplicación borra todo contexto.
    conversations: ConversationMemory,
}

impl OmegaEngine {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_clock(path, Clock::System)
    }

    pub fn open_with_clock(path: impl AsRef<Path>, clock: Clock) -> Result<Self> {
        let database = Database::open(path)?;
        let tools = ToolEngine::new(database.clone());
        Ok(Self {
            database,
            tools,
            clock,
            conversations: ConversationMemory::default(),
        })
    }

    pub fn authorize_source(&self, path: &Path) -> Result<i64> {
        Indexer::new(&self.database, &LocalDocumentParser).authorize(path)
    }

    pub fn index_source(&self, source_id: i64) -> Result<IndexReport> {
        let report = Indexer::new(&self.database, &LocalDocumentParser).index_source(source_id)?;
        // Reindexar reasigna los identificadores internos y regenera la
        // evidencia. El predicado de cada conversación sobrevive —se reevalúa
        // contra el índice nuevo—, pero la evidencia ya citada se descarta para
        // que ninguna respuesta apunte a una cita que ya no existe.
        self.conversations.invalidate_results();
        Ok(report)
    }

    pub fn revoke_source(&self, source_id: i64) -> Result<()> {
        self.database.purge_source(source_id, true)?;
        self.conversations.invalidate_results();
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<SourceSummary>> {
        self.database.list_sources()
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.tools.search(query, &[], 12)
    }

    pub fn concepts(&self, query: Option<&str>) -> Result<Vec<ConceptSummary>> {
        self.tools.list_concepts(query)
    }

    /// Pregunta suelta, sin conversación: equivale a una conversación nueva
    /// que se descarta al terminar.
    pub fn ask(&self, question: &str) -> Result<Answer> {
        self.validate(question)?;
        Agent::new(self.tools.clone(), self.clock).answer(question)
    }

    /// Pregunta dentro de una conversación identificada. El contexto sólo se
    /// lee y se escribe bajo esa clave: dos conversaciones nunca se mezclan.
    pub fn ask_in_conversation(&self, conversation: &str, question: &str) -> Result<Answer> {
        self.validate(question)?;
        let mut state = self.conversations.state(conversation);
        let answer = Agent::new(self.tools.clone(), self.clock).answer_in(question, &mut state)?;
        self.conversations.store(conversation, state);
        Ok(answer)
    }

    /// Inicia una conversación nueva: el contexto anterior desaparece.
    pub fn reset_conversation(&self, conversation: &str) {
        self.conversations.reset(conversation);
    }

    fn validate(&self, question: &str) -> Result<()> {
        if question.trim().is_empty() {
            return Err(OmegaError::InvalidArguments(
                "la pregunta está vacía".into(),
            ));
        }
        Ok(())
    }

    pub fn status(&self) -> Result<AppStatus> {
        self.database.status()
    }

    pub fn open_document(&self, path: &Path) -> Result<()> {
        if !self.database.is_authorized_document(path)? {
            return Err(OmegaError::UnauthorizedPath(path.display().to_string()));
        }
        open_with_system(path)
    }

    pub fn database_path(&self) -> PathBuf {
        self.database.path().to_path_buf()
    }
}

fn open_with_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut cmd = Command::new("open");
        cmd.arg(path);
        cmd
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut cmd = Command::new("explorer");
        cmd.arg(path);
        cmd
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path);
        cmd
    };
    command.spawn()?;
    Ok(())
}
