use std::fs;

use omega_core::evaluation::{CaseStatus, EvaluationOptions, run_evaluations};

#[test]
fn isolated_factory_indexes_asks_and_writes_all_reports() {
    let fixture = tempfile::tempdir().unwrap();
    let corpus = fixture.path().join("corpus");
    for (folder, state, city, start) in [
        ("01_alpha", "Abierto", "Norte", 1),
        ("02_beta", "Cerrado", "Sur", 4),
    ] {
        let directory = corpus.join(folder);
        fs::create_dir_all(&directory).unwrap();
        for number in start..start + 3 {
            fs::write(
                directory.join(format!("{number}.md")),
                format!(
                    "# Registro de prueba\n\nFolio: QA-26-{number:04}\nTipo de documento: Registro {folder}\nEstado: {state}\nCiudad base: {city}\nImporte: ${}.00\n\nEl personal autorizado conserva evidencia suficiente para realizar el seguimiento operativo correspondiente sin alterar los documentos de origen.\n",
                    number * 100
                ),
            )
            .unwrap();
        }
    }
    let manifest = fixture.path().join("manifest.json");
    fs::write(
        &manifest,
        r#"{
          "version": 1,
          "seed": 5600,
          "corpora": [{"id":"fixture","path":"corpus","enabled":true}]
        }"#,
    )
    .unwrap();
    let report_root = fixture.path().join("reports");
    let output = run_evaluations(&EvaluationOptions {
        manifest_path: manifest,
        corpus_id: None,
        report_root,
        stress: false,
    })
    .unwrap();

    assert_eq!(output.summary.corpora, 1);
    assert_eq!(output.summary.documents, 6);
    assert_eq!(output.summary.failed, 0, "{:#?}", output.results);
    assert!(
        output
            .results
            .iter()
            .all(|result| result.status != CaseStatus::Failed)
    );
    for name in ["resultados.jsonl", "resumen.json", "reporte.md"] {
        assert!(output.report_directory.join(name).is_file(), "falta {name}");
    }
    assert!(
        fs::read_dir(&corpus)
            .unwrap()
            .all(|entry| entry.unwrap().path().is_dir()),
        "la fábrica no debe escribir su SQLite dentro del corpus"
    );
}
