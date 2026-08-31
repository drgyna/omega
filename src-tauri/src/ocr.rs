use std::{fs, path::Path, process::Command};

use serde::Deserialize;

use crate::model::{OcrStatus, ParsedChunk};

pub struct OcrOutcome {
    pub chunks: Vec<ParsedChunk>,
    pub status: OcrStatus,
    pub confidence: Option<f64>,
}

impl OcrOutcome {
    /// El motor corrió y no entregó texto utilizable.
    pub fn failed() -> Self {
        Self {
            chunks: vec![],
            status: OcrStatus::Failed,
            confidence: None,
        }
    }

    /// No hay motor OCR en este equipo: el archivo queda sin procesar y
    /// visible como omitido, nunca como procesado correctamente.
    pub fn unavailable() -> Self {
        Self {
            chunks: vec![],
            status: OcrStatus::Unavailable,
            confidence: None,
        }
    }

    /// Hay motor, pero el archivo todavía no se procesó.
    pub fn pending() -> Self {
        Self {
            chunks: vec![],
            status: OcrStatus::Pending,
            confidence: None,
        }
    }
}

/// Línea reconocida por un motor OCR local, con su página, su confianza y su
/// caja. Es la unidad mínima que conserva ubicación verificable.
#[derive(Debug, Clone, Deserialize)]
pub struct RecognizedLine {
    pub page: usize,
    pub text: String,
    pub confidence: f64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Motor OCR puramente local. Omega incorpora Vision en macOS; este contrato
/// existe para sustituirlo por otro proveedor local —y para que las pruebas
/// puedan fijar un estado OCR concreto sin depender del equipo que las
/// ejecuta—. No se admite un proveedor de red.
pub trait OcrEngine: Send + Sync {
    fn recognize(&self, path: &Path) -> OcrOutcome;
}

/// Motor OCR del sistema operativo anfitrión.
#[derive(Default)]
pub struct SystemOcr;

impl OcrEngine for SystemOcr {
    fn recognize(&self, path: &Path) -> OcrOutcome {
        recognize(path)
    }
}

/// Confianza mínima para considerar fiable una línea reconocida.
pub const RELIABLE_CONFIDENCE: f64 = 0.55;

/// Traduce las líneas de un motor local al estado OCR del documento.
pub fn outcome_from_lines(lines: Vec<RecognizedLine>) -> OcrOutcome {
    // Una línea en blanco no es texto reconocido. Contarla en el promedio
    // permitía que una página de la que no salió nada —o de la que salió un
    // resto ilegible— se declarara «completa» con la confianza que el motor
    // atribuye al vacío.
    let recognized = lines
        .into_iter()
        .filter(|line| !line.text.trim().is_empty())
        .collect::<Vec<_>>();
    if recognized.is_empty() {
        return OcrOutcome::failed();
    }
    let confidence =
        recognized.iter().map(|line| line.confidence).sum::<f64>() / recognized.len() as f64;
    let status = if confidence < RELIABLE_CONFIDENCE {
        OcrStatus::LowConfidence
    } else {
        OcrStatus::Complete
    };
    let chunks = recognized
        .into_iter()
        .map(|line| ParsedChunk {
            location: format!(
                "OCR, página {}, zona x={:.2}, y={:.2}, ancho={:.2}, alto={:.2}",
                line.page, line.x, line.y, line.width, line.height
            ),
            content: line.text.trim().to_owned(),
        })
        .collect();
    OcrOutcome {
        chunks,
        status,
        confidence: Some(confidence),
    }
}

/// OCR local de macOS. El auxiliar de Vision/PDFKit se compila durante el
/// build, se empaqueta dentro del binario y se ejecuta localmente. No requiere
/// red ni herramientas de desarrollo instaladas en la máquina del usuario.
#[cfg(target_os = "macos")]
pub fn recognize(path: &Path) -> OcrOutcome {
    let Some(helper) = materialize_helper() else {
        return OcrOutcome::unavailable();
    };
    let Ok(output) = Command::new(&helper).arg(path).output() else {
        return OcrOutcome::unavailable();
    };
    outcome_from_helper_output(output.status.code(), &output.stdout)
}

/// Traduce el protocolo de salida del auxiliar macOS a un estado que el índice
/// pueda publicar honestamente. El código 66 es deliberado: el auxiliar lo
/// usa cuando Vision Text Recognition no existe en esa versión de macOS. Es
/// distinto de un OCR que sí se ejecutó pero no pudo leer el archivo.
#[cfg(target_os = "macos")]
fn outcome_from_helper_output(exit_code: Option<i32>, stdout: &[u8]) -> OcrOutcome {
    if exit_code == Some(66) {
        return OcrOutcome::unavailable();
    }
    if exit_code != Some(0) {
        return OcrOutcome::failed();
    }
    let Ok(lines) = serde_json::from_slice::<Vec<RecognizedLine>>(stdout) else {
        return OcrOutcome::failed();
    };
    outcome_from_lines(lines)
}

#[cfg(target_os = "macos")]
fn materialize_helper() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    const BINARY: &[u8] = include_bytes!(env!("OMEGA_VISION_OCR"));
    let helper = std::env::temp_dir().join("omega-vision-ocr");
    (fs::write(&helper, BINARY).is_ok()
        && fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).is_ok())
    .then_some(helper)
}

/// Sin motor OCR local en esta plataforma. El archivo no se procesa y su
/// estado lo dice; no se inventa texto ni se marca como completo.
#[cfg(not(target_os = "macos"))]
pub fn recognize(_path: &Path) -> OcrOutcome {
    OcrOutcome::unavailable()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn bundled_vision_helper_runs_locally() {
        let outcome = recognize(Path::new("icons/32x32.png"));
        assert_ne!(outcome.status, OcrStatus::Unavailable);
    }

    #[test]
    fn a_macos_without_vision_is_reported_as_unavailable() {
        let outcome = outcome_from_helper_output(Some(66), b"");
        assert_eq!(outcome.status, OcrStatus::Unavailable);
        assert!(outcome.chunks.is_empty());
    }
}

/// Motor OCR con caché de resultado por contenido del archivo.
///
/// Decora cualquier otro motor local sin cambiar su contrato: el trabajo lo
/// sigue haciendo el motor de dentro, y lo único que se añade es no repetirlo
/// sobre bytes que ya se reconocieron. La clave es el SHA-256 del archivo, no
/// su ruta ni su fecha: un archivo movido o retocado sin cambiar contenido
/// acierta, y uno cuyo contenido cambió falla la caché por definición, así que
/// no hace falta ninguna política de invalidación aparte.
///
/// **Aquí sólo se LEE.** Escribir desde este punto significaría abrir una
/// segunda conexión y hacer un `INSERT` mientras el indexador tiene su
/// transacción de escritura abierta sobre la misma base: cada documento se
/// quedaba esperando los 5 s de `busy_timeout` y luego fallaba en silencio, de
/// modo que la caché nunca se llenaba y la indexación tardaba más de una hora
/// extra. Quien guarda es el indexador, dentro de su propia transacción y con
/// el resultado que ya tiene en las manos (`ParsedDocument` lleva el estado,
/// la confianza y los fragmentos del OCR). Bajo WAL, leer mientras otro
/// escribe no bloquea, así que la lectura sí puede vivir aquí.
pub struct CachedOcr<E: OcrEngine> {
    inner: E,
    database: crate::db::Database,
}

impl<E: OcrEngine> CachedOcr<E> {
    pub fn new(inner: E, database: crate::db::Database) -> Self {
        Self { inner, database }
    }

    fn lookup(&self, hash: &str) -> Option<OcrOutcome> {
        let connection = self.database.connect().ok()?;
        let (status, confidence, chunks): (String, Option<f64>, String) = connection
            .query_row(
                "SELECT status, confidence, chunks FROM ocr_cache WHERE content_hash = ?1",
                [hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok()?;
        let chunks: Vec<ParsedChunk> = serde_json::from_str::<Vec<(String, String)>>(&chunks)
            .ok()?
            .into_iter()
            .map(|(location, content)| ParsedChunk { location, content })
            .collect();
        Some(OcrOutcome {
            chunks,
            status: OcrStatus::from_stored(&status),
            confidence,
        })
    }
}

impl<E: OcrEngine> OcrEngine for CachedOcr<E> {
    fn recognize(&self, path: &Path) -> OcrOutcome {
        let Some(hash) = file_hash(path) else {
            return self.inner.recognize(path);
        };
        match self.lookup(&hash) {
            Some(cached) => cached,
            None => self.inner.recognize(path),
        }
    }
}

fn file_hash(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    Some(format!("{:x}", Sha256::digest(fs::read(path).ok()?)))
}
