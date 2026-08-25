use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    agent::Agent,
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
}

impl OmegaEngine {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let database = Database::open(path)?;
        let tools = ToolEngine::new(database.clone());
        Ok(Self { database, tools })
    }

    pub fn authorize_source(&self, path: &Path) -> Result<i64> {
        Indexer::new(&self.database, &LocalDocumentParser).authorize(path)
    }

    pub fn index_source(&self, source_id: i64) -> Result<IndexReport> {
        Indexer::new(&self.database, &LocalDocumentParser).index_source(source_id)
    }

    pub fn revoke_source(&self, source_id: i64) -> Result<()> {
        self.database.purge_source(source_id, true)
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

    pub fn ask(&self, question: &str) -> Result<Answer> {
        if question.trim().is_empty() {
            return Err(OmegaError::InvalidArguments(
                "la pregunta está vacía".into(),
            ));
        }
        let agent = Agent::new(self.tools.clone());
        // La configuración de IA se conserva para fases posteriores, pero la
        // fase de recuperación no permite generar ni resumir contenido.
        agent.answer(question)
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
