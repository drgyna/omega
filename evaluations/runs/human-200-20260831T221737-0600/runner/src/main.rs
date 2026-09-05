use std::{
    env,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use omega_core::OmegaEngine;
use serde_json::{Map, Value, json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("omega-human-200-runner: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 6 {
        return Err("uso: runner BASE CORPUS PREGUNTAS INDEX_REPORT RAW_ANSWERS".into());
    }
    let database = PathBuf::from(&args[1]);
    let corpus = PathBuf::from(&args[2]);
    let questions_path = PathBuf::from(&args[3]);
    let index_path = PathBuf::from(&args[4]);
    let answers_path = PathBuf::from(&args[5]);

    refuse_existing_database(&database)?;
    let questions = read_questions(&questions_path)?;
    if questions.len() != 200 || questions.first().map(|q| q.0) != Some(1) || questions.last().map(|q| q.0) != Some(200) {
        return Err(format!("se esperaban exactamente 200 preguntas numeradas 1..200; se leyeron {}", questions.len()).into());
    }

    let engine = OmegaEngine::open(&database)?;
    let authorized_at = Instant::now();
    let source_id = engine.authorize_source(&corpus)?;
    let authorize_ms = authorized_at.elapsed().as_millis();
    let report = engine.index_source(source_id)?;

    let mut index = match serde_json::to_value(report)? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    index.insert("source_path".into(), json!(corpus));
    index.insert("database_path".into(), json!(database));
    index.insert("authorize_ms".into(), json!(authorize_ms));
    write_json_pretty(&index_path, &Value::Object(index))?;

    let mut output = BufWriter::new(File::create(&answers_path)?);
    for (question_id, question) in questions {
        let session = session_for(question_id);
        let started = Instant::now();
        let result = if let Some(session_id) = session {
            engine.ask_in_conversation(session_id, &question)
        } else {
            engine.ask(&question)
        };
        let latency_ms = started.elapsed().as_millis();
        let row = match result {
            Ok(answer) => json!({
                "question_id": question_id,
                "question": question,
                "session": session,
                "answer_text": answer.text,
                "mode": answer.mode,
                "verified": answer.verified,
                "warning": answer.warning,
                "used_context": answer.used_context,
                "scope": answer.scope,
                "clarification": answer.clarification,
                "citations": answer.citations,
                "latency_ms": latency_ms,
                "engine_error": null,
            }),
            Err(error) => json!({
                "question_id": question_id,
                "question": question,
                "session": session,
                "answer_text": "",
                "mode": "local",
                "verified": false,
                "warning": format!("Error del motor: {error}"),
                "used_context": false,
                "scope": null,
                "clarification": null,
                "citations": [],
                "latency_ms": latency_ms,
                "engine_error": error.to_string(),
            }),
        };
        serde_json::to_writer(&mut output, &row)?;
        output.write_all(b"\n")?;
        output.flush()?;
        eprintln!("pregunta {question_id}/200: {latency_ms} ms");
    }
    Ok(())
}

fn refuse_existing_database(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        return Err(format!("la base ya existe; se requiere una base limpia: {}", path.display()).into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn read_questions(path: &Path) -> Result<Vec<(u16, String)>, Box<dyn std::error::Error>> {
    let mut result = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        let Some((prefix, question)) = line.split_once(". ") else { continue };
        let Ok(id) = prefix.parse::<u16>() else { continue };
        if (1..=200).contains(&id) {
            result.push((id, question.to_owned()));
        }
    }
    Ok(result)
}

fn session_for(question_id: u16) -> Option<&'static str> {
    match question_id {
        176..=178 => Some("C-01"),
        179..=181 => Some("C-02"),
        182..=184 => Some("C-03"),
        185..=187 => Some("C-04"),
        188..=190 => Some("C-05"),
        191..=193 => Some("C-06"),
        194..=196 => Some("C-07"),
        197..=200 => Some("C-08"),
        _ => None,
    }
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut output = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}
