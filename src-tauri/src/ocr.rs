use std::{fs, path::Path, process::Command};

use serde::Deserialize;

use crate::model::{OcrStatus, ParsedChunk};

pub struct OcrOutcome {
    pub chunks: Vec<ParsedChunk>,
    pub status: OcrStatus,
    pub confidence: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct VisionLine {
    page: usize,
    text: String,
    confidence: f64,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// OCR local de macOS. El auxiliar de Vision/PDFKit se compila durante el
/// build, se empaqueta dentro del binario y se ejecuta localmente. No requiere
/// red ni herramientas de desarrollo instaladas en la máquina del usuario.
#[cfg(target_os = "macos")]
pub fn recognize(path: &Path) -> OcrOutcome {
    let Some(helper) = materialize_helper() else {
        return failed();
    };
    let Ok(output) = Command::new(&helper).arg(path).output() else {
        return failed();
    };
    if !output.status.success() {
        return failed();
    }
    let Ok(lines) = serde_json::from_slice::<Vec<VisionLine>>(&output.stdout) else {
        return failed();
    };
    if lines.is_empty() {
        return OcrOutcome {
            chunks: vec![],
            status: OcrStatus::Complete,
            confidence: Some(1.0),
        };
    }
    let confidence = lines.iter().map(|line| line.confidence).sum::<f64>() / lines.len() as f64;
    let status = if confidence < 0.55 {
        OcrStatus::LowConfidence
    } else {
        OcrStatus::Complete
    };
    let chunks = lines
        .into_iter()
        .filter_map(|line| {
            let content = line.text.trim();
            (!content.is_empty()).then(|| ParsedChunk {
                location: format!(
                    "página {}, zona x={:.2}, y={:.2}, ancho={:.2}, alto={:.2}",
                    line.page, line.x, line.y, line.width, line.height
                ),
                content: content.to_owned(),
            })
        })
        .collect();
    OcrOutcome {
        chunks,
        status,
        confidence: Some(confidence),
    }
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

#[cfg(not(target_os = "macos"))]
pub fn recognize(_path: &Path) -> OcrOutcome {
    pending()
}

#[cfg(not(target_os = "macos"))]
fn pending() -> OcrOutcome {
    OcrOutcome {
        chunks: vec![],
        status: OcrStatus::Pending,
        confidence: None,
    }
}

fn failed() -> OcrOutcome {
    OcrOutcome {
        chunks: vec![],
        status: OcrStatus::Failed,
        confidence: None,
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn bundled_vision_helper_runs_locally() {
        let outcome = recognize(Path::new("icons/32x32.png"));
        assert_ne!(outcome.status, OcrStatus::Failed);
    }
}
