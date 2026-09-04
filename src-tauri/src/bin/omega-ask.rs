//! Arnés de prueba manual. NO forma parte del motor: sólo indexa un corpus y
//! corre un archivo de preguntas para poder comparar las respuestas de Omega
//! contra una clave escrita a mano.
//!
//! Uso:
//!   omega-ask --db RUTA --index CARPETA
//!   omega-ask --db RUTA --ask ARCHIVO_DE_PREGUNTAS
//!
//! En el archivo de preguntas, una línea vacía inicia una CONVERSACIÓN NUEVA
//! y las líneas que empiezan con `#` son comentarios.

use std::{path::PathBuf, time::Instant};

use omega_core::OmegaEngine;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut database: Option<PathBuf> = None;
    let mut index: Option<PathBuf> = None;
    let mut ask: Option<PathBuf> = None;

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--db" => database = arguments.next().map(PathBuf::from),
            "--index" => index = arguments.next().map(PathBuf::from),
            "--ask" => ask = arguments.next().map(PathBuf::from),
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }

    let database = database.ok_or("falta --db")?;
    let engine = OmegaEngine::open(&database).map_err(|error| error.to_string())?;

    if let Some(folder) = index {
        let started = Instant::now();
        let source = engine
            .authorize_source(&folder)
            .map_err(|error| error.to_string())?;
        let report = engine
            .index_source(source)
            .map_err(|error| error.to_string())?;
        println!(
            "indexado: {} descubiertos, {} indexados, {} omitidos, OCR pendiente {}, {:?}",
            report.discovered,
            report.indexed,
            report.skipped,
            report.ocr_pending,
            started.elapsed()
        );
        return Ok(());
    }

    let questions = ask.ok_or("falta --ask o --index")?;
    let content = std::fs::read_to_string(&questions).map_err(|error| error.to_string())?;

    let mut conversation = 0usize;
    let mut number = 0usize;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            // Conversación nueva: el contexto anterior desaparece.
            conversation += 1;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        number += 1;
        let key = format!("prueba-{conversation}");
        let answer = engine
            .ask_in_conversation(&key, line)
            .map_err(|error| error.to_string())?;
        println!("\n=== P{number} [conv {conversation}] ===");
        println!("PREGUNTA : {line}");
        println!(
            "RESPUESTA: {}",
            answer.text.replace('\n', "\n           ")
        );
        println!(
            "SELLO    : {}",
            if answer.verified {
                "VERIFICADA"
            } else {
                "sin sello"
            }
        );
        if let Some(warning) = &answer.warning {
            println!("AVISO    : {warning}");
        }
        if let Some(clarification) = &answer.clarification {
            println!("ACLARA   : {}", clarification.question);
        }
        // Rutas distintas citadas, en orden de aparición: sirve para medir si
        // el documento correcto ya estaba citado y en qué posición.
        let mut vistas: Vec<&str> = Vec::new();
        for evidence in &answer.citations {
            if !vistas.contains(&evidence.path.as_str()) {
                vistas.push(evidence.path.as_str());
            }
        }
        for (posicion, ruta) in vistas.iter().enumerate() {
            println!("DOC{:<3}   : {}", posicion + 1, ruta);
        }
    }
    Ok(())
}
