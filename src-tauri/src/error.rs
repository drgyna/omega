use thiserror::Error;

#[derive(Debug, Error)]
pub enum OmegaError {
    #[error("Base de datos: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Archivo: {0}")]
    Io(#[from] std::io::Error),
    #[error("Documento no compatible: {0}")]
    Unsupported(String),
    #[error("Documento inválido: {0}")]
    Parse(String),
    #[error("Ruta no autorizada: {0}")]
    UnauthorizedPath(String),
    #[error("Argumentos inválidos: {0}")]
    InvalidArguments(String),
    #[error("La respuesta no superó la verificación: {0}")]
    Verification(String),
}

pub type Result<T> = std::result::Result<T, OmegaError>;

impl serde::Serialize for OmegaError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
