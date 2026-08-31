fn main() {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--release-smoke")) {
        let Some(database) = arguments.next() else {
            eprintln!("uso: omega --release-smoke BASE FIXTURE CONSULTA");
            std::process::exit(2);
        };
        let Some(fixture) = arguments.next() else {
            eprintln!("uso: omega --release-smoke BASE FIXTURE CONSULTA");
            std::process::exit(2);
        };
        let Some(query) = arguments.next() else {
            eprintln!("uso: omega --release-smoke BASE FIXTURE CONSULTA");
            std::process::exit(2);
        };
        if arguments.next().is_some() {
            eprintln!("uso: omega --release-smoke BASE FIXTURE CONSULTA");
            std::process::exit(2);
        }
        match omega_core::run_release_smoke(
            std::path::PathBuf::from(database),
            std::path::PathBuf::from(fixture),
            &query.to_string_lossy(),
        ) {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("reporte serializable")
                );
                return;
            }
            Err(error) => {
                eprintln!("release smoke: {error}");
                std::process::exit(1);
            }
        }
    }
    omega_core::run();
}
