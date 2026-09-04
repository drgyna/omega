//! INSTRUMENTACIÓN TEMPORAL DE AUDITORÍA. Borrar al terminar.
//! Se activa sólo con OMEGA_TRACE=1; sin la variable no imprime nada.

use std::sync::OnceLock;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("OMEGA_TRACE").as_deref() == Ok("1"))
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::trace::enabled() {
            eprintln!("[TRACE] {}", format!($($arg)*));
        }
    };
}
