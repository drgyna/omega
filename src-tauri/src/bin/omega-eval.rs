use std::{env, path::PathBuf, process::ExitCode};

use omega_core::evaluation::{EvaluationOptions, run_evaluations};

fn main() -> ExitCode {
    match parse_arguments().and_then(|options| run_evaluations(&options)) {
        Ok(output) => {
            println!(
                "Evaluación terminada: {} corpus, {} documentos, {} aprobados, {} fallidos, {} omitidos.",
                output.summary.corpora,
                output.summary.documents,
                output.summary.passed,
                output.summary.failed,
                output.summary.skipped
            );
            println!("Reporte: {}", output.report_directory.display());
            if output.summary.failed == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            eprintln!("omega-eval: {error}");
            ExitCode::from(2)
        }
    }
}

fn parse_arguments() -> Result<EvaluationOptions, String> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri debe vivir dentro del repositorio")
        .to_path_buf();
    let mut manifest_path = repository.join("evaluation-corpora.json");
    let mut report_root = repository.join("artifacts/evaluations");
    let mut corpus_id = None;
    let mut all = false;
    let mut stress = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--all" => all = true,
            "--stress" => stress = true,
            "--corpus" => {
                corpus_id = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--corpus requiere un identificador".to_string())?,
                );
            }
            "--manifest" => {
                manifest_path = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--manifest requiere una ruta".to_string())?,
                );
            }
            "--report-root" => {
                report_root = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--report-root requiere una ruta".to_string())?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "Uso: omega-eval (--all | --corpus ID | --stress) [--manifest RUTA] [--report-root RUTA]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }
    let selections = usize::from(all) + usize::from(corpus_id.is_some()) + usize::from(stress);
    if selections > 1 {
        return Err("usa solo una opción entre --all, --corpus y --stress".into());
    }
    if selections == 0 {
        return Err("indica --all, --corpus ID o --stress".into());
    }
    Ok(EvaluationOptions {
        manifest_path,
        corpus_id,
        report_root,
        stress,
    })
}
