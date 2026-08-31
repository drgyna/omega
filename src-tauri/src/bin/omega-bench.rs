//! Benchmarks locales de Omega, separados por fase.
//!
//! Cada fase se mide por separado —indexación, búsqueda exacta, filtros,
//! ranking, construcción de citas y respuesta completa— y cada tamaño de
//! acervo se mide de verdad. **Ninguna cifra de este reporte es una
//! proyección**: si un tamaño no se ejecutó, no aparece.
//!
//! El acervo sintético y el reporte viven fuera del repositorio (carpeta
//! temporal del sistema), y el acervo se borra al terminar.
//!
//! ```bash
//! cargo run --release --bin omega-bench -- --sizes 1000,10000,50000
//! ```

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use omega_core::{Clock, Database, OmegaEngine, ToolEngine, ToolFilter};

const TODAY: &str = "2026-08-25";
/// Repeticiones de cada fase de consulta. La indexación se mide una vez por
/// tamaño: es la fase cara y repetirla no aporta nada que el resto no diga.
const REPETITIONS: usize = 7;

struct Options {
    sizes: Vec<usize>,
    report: PathBuf,
    keep_corpus: bool,
}

struct Measurement {
    phase: &'static str,
    detail: String,
    runs: usize,
    best: Duration,
    median: Duration,
    worst: Duration,
}

impl Measurement {
    fn json(&self) -> String {
        format!(
            r#"{{"fase":"{}","detalle":"{}","ejecuciones":{},"mejor_ms":{:.3},"mediana_ms":{:.3},"peor_ms":{:.3}}}"#,
            self.phase,
            self.detail.replace('"', "'"),
            self.runs,
            millis(self.best),
            millis(self.median),
            millis(self.worst)
        )
    }
}

fn main() -> ExitCode {
    let options = match parse_arguments() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("omega-bench: {error}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("omega-bench: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(options: &Options) -> Result<(), String> {
    println!("Omega — benchmarks locales por fase");
    println!("Tamaños medidos: {:?}", options.sizes);
    println!(
        "Cada fila es una medición real sobre ese tamaño. No hay ninguna proyección lineal.\n"
    );

    let mut sections = Vec::new();
    for size in &options.sizes {
        let section = measure_size(*size, options)?;
        sections.push(section);
    }

    let report = format!(
        "{{\"generado\":\"{}\",\"repeticiones_por_consulta\":{REPETITIONS},\"tamaños\":[{}]}}",
        TODAY,
        sections.join(",")
    );
    fs::write(&options.report, report).map_err(|error| error.to_string())?;
    println!("\nReporte: {}", options.report.display());
    Ok(())
}

fn measure_size(size: usize, options: &Options) -> Result<String, String> {
    println!("── {size} documentos ───────────────────────────────────────");
    let corpus = env::temp_dir().join(format!("omega-bench-corpus-{size}"));
    let _ = fs::remove_dir_all(&corpus);
    fs::create_dir_all(&corpus).map_err(|error| error.to_string())?;

    let generation = Instant::now();
    generate_corpus(&corpus, size)?;
    println!(
        "  acervo generado en {:.2} s ({} archivos)",
        generation.elapsed().as_secs_f64(),
        size
    );

    let database_path = env::temp_dir().join(format!("omega-bench-{size}.db"));
    let _ = fs::remove_file(&database_path);
    let engine = OmegaEngine::open_with_clock(&database_path, Clock::fixed(TODAY).unwrap())
        .map_err(|error| error.to_string())?;
    let source = engine
        .authorize_source(&corpus)
        .map_err(|error| error.to_string())?;

    // ── Fase 1: indexación ────────────────────────────────────────────
    let started = Instant::now();
    let report = engine
        .index_source(source)
        .map_err(|error| error.to_string())?;
    let indexing = started.elapsed();
    println!(
        "  indexación            {:>9.1} ms  ({} documentos, {} valores, {:.0} doc/s)",
        millis(indexing),
        report.indexed,
        report.values,
        report.indexed as f64 / indexing.as_secs_f64().max(f64::MIN_POSITIVE)
    );

    let tools = ToolEngine::new(Database::open(&database_path).map_err(|e| e.to_string())?);
    let mut measurements = vec![Measurement {
        phase: "indexación",
        detail: format!("{} documentos, {} valores", report.indexed, report.values),
        runs: 1,
        best: indexing,
        median: indexing,
        worst: indexing,
    }];

    // ── Fase 2: búsqueda exacta ───────────────────────────────────────
    let needle = format!("FAC-{:07}", size / 2);
    measurements.push(measure(
        "búsqueda exacta",
        &format!("identificador «{needle}»"),
        || {
            tools.exact_lookup(&needle, 20).map(|hits| hits.len())
        },
    )?);

    // ── Fase 3: filtros ───────────────────────────────────────────────
    let filters = vec![ToolFilter {
        concept: "Estado".into(),
        equals: "Cerrada".into(),
    }];
    measurements.push(measure("filtros", "Estado: Cerrada", || {
        tools
            .query_documents(&filters, None, 20)
            .map(|result| result.document_count as usize)
    })?);

    // ── Fase 4: ranking ───────────────────────────────────────────────
    // Es la fase de la que se retiró el corte previo al ranking, así que es la
    // que más importa medir a escala.
    measurements.push(measure(
        "ranking",
        "texto libre sobre todo el acervo",
        || {
            tools
                .search_text("registro cerrado del periodo", None, 12)
                .map(|result| result.hits.len())
        },
    )?);

    // ── Fase 5: construcción de citas ─────────────────────────────────
    let documents = tools
        .query_documents(&filters, None, 1)
        .map(|result| result.document_count)
        .unwrap_or(0)
        .min(50);
    let ids = (1..=documents).collect::<Vec<i64>>();
    measurements.push(measure(
        "construcción de citas",
        &format!("{} documentos", ids.len()),
        || tools.evidence_for_documents(&ids, 50).map(|e| e.len()),
    )?);

    // ── Fase 6: respuesta completa ────────────────────────────────────
    measurements.push(measure("respuesta", "suma de «Importe»", || {
        engine
            .ask_in_conversation("bench", "¿Cuánto suma el Importe?")
            .map(|answer| answer.citations.len())
    })?);

    for measurement in measurements.iter().skip(1) {
        println!(
            "  {:<21} {:>9.1} ms  (mediana de {} · mejor {:.1} · peor {:.1}) — {}",
            measurement.phase,
            millis(measurement.median),
            measurement.runs,
            millis(measurement.best),
            millis(measurement.worst),
            measurement.detail
        );
    }

    let database_bytes = fs::metadata(&database_path).map(|m| m.len()).unwrap_or(0);
    let peak = peak_memory_bytes();
    println!(
        "  índice en disco       {:>9.1} MB   ·  memoria máxima del proceso {:.1} MB",
        database_bytes as f64 / 1_048_576.0,
        peak as f64 / 1_048_576.0
    );
    println!();

    if !options.keep_corpus {
        let _ = fs::remove_dir_all(&corpus);
        let _ = fs::remove_file(&database_path);
    }

    Ok(format!(
        r#"{{"documentos":{size},"indice_bytes":{database_bytes},"memoria_maxima_bytes":{peak},"fases":[{}]}}"#,
        measurements
            .iter()
            .map(Measurement::json)
            .collect::<Vec<_>>()
            .join(",")
    ))
}

fn measure<T>(
    phase: &'static str,
    detail: &str,
    mut operation: impl FnMut() -> omega_core::Result<T>,
) -> Result<Measurement, String> {
    // Una ejecución de calentamiento fuera de la muestra: la primera consulta
    // paga la apertura de la base y el llenado de la caché de páginas, y
    // mezclarla con el resto convertiría la mediana en otra cosa.
    operation().map_err(|error| error.to_string())?;
    let mut samples = Vec::with_capacity(REPETITIONS);
    for _ in 0..REPETITIONS {
        let started = Instant::now();
        operation().map_err(|error| error.to_string())?;
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    Ok(Measurement {
        phase,
        detail: detail.to_owned(),
        runs: samples.len(),
        best: samples[0],
        median: samples[samples.len() / 2],
        worst: samples[samples.len() - 1],
    })
}

/// Acervo sintético genérico: facturas con folio, fecha, estado e importe, y
/// una parte en CSV de inventario. No describe ningún giro de negocio.
fn generate_corpus(root: &Path, size: usize) -> Result<(), String> {
    const ESTADOS: [&str; 3] = ["Cerrada", "Abierta", "En revisión"];
    let folders = ["2024", "2025", "2026"];
    for folder in folders {
        fs::create_dir_all(root.join(folder)).map_err(|error| error.to_string())?;
    }
    for index in 0..size {
        let folder = folders[index % folders.len()];
        let estado = ESTADOS[index % ESTADOS.len()];
        let dia = (index % 28) + 1;
        let mes = (index % 12) + 1;
        let contents = format!(
            "# Registro\n\nFolio: FAC-{index:07}\nFecha de emisión: 20{}-{mes:02}-{dia:02}\nEstado: {estado}\nImporte: ${}.00 MXN\nResponsable: Equipo {}\nNota: registro cerrado del periodo con seguimiento y anexos.\n",
            24 + (index % 3),
            (index % 900) + 100,
            index % 17
        );
        fs::write(
            root.join(folder).join(format!("factura-{index:07}.md")),
            contents,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Pico de memoria residente del proceso.
fn peak_memory_bytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    let raw = usage.ru_maxrss as u64;
    // macOS reporta bytes; Linux, kilobytes.
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

fn parse_arguments() -> Result<Options, String> {
    let mut sizes = vec![1_000usize, 10_000, 50_000];
    let mut report = env::temp_dir().join("omega-bench.json");
    let mut keep_corpus = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--sizes" => {
                let raw = arguments
                    .next()
                    .ok_or_else(|| "--sizes requiere una lista como 1000,10000".to_string())?;
                sizes = raw
                    .split(',')
                    .map(|value| {
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| format!("tamaño inválido: {value}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "--report" => {
                report = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--report requiere una ruta".to_string())?,
                );
            }
            "--keep-corpus" => keep_corpus = true,
            "--help" | "-h" => {
                println!(
                    "Uso: omega-bench [--sizes 1000,10000,50000] [--report RUTA] [--keep-corpus]\n\n\
                     Mide por separado indexación, búsqueda exacta, filtros, ranking,\n\
                     construcción de citas y respuesta completa. Cada tamaño se ejecuta de\n\
                     verdad: el reporte no contiene proyecciones.\n\n\
                     El acervo sintético y el reporte viven en la carpeta temporal del\n\
                     sistema, nunca dentro del repositorio."
                );
                std::process::exit(0);
            }
            other => return Err(format!("argumento desconocido: {other}")),
        }
    }
    if sizes.is_empty() {
        return Err("indica al menos un tamaño".into());
    }
    Ok(Options {
        sizes,
        report,
        keep_corpus,
    })
}
