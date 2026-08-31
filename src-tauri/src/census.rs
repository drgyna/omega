//! Censo del acervo: cuántos archivos hay, por carpeta y por tipo de nombre.
//!
//! Es la única ruta de Omega que responde sobre el acervo **como conjunto de
//! archivos** en vez de sobre su contenido. La distinción no es cosmética:
//!
//!  * Un conteo de contenido («¿cuántos documentos registran Moneda = EUR?»)
//!    sólo puede hablar de lo que se logró leer. Un documento que no se pudo
//!    indexar no registra nada, así que no puede entrar ni salir de esa cifra
//!    sin adivinar.
//!  * Un conteo de archivos («¿cuántos documentos hay en la carpeta rh?») sí
//!    puede ser completo, porque el indexador **anota también los que no pudo
//!    leer** (`unindexed_documents`). Contar los dos conjuntos y declarar la
//!    partición es un hecho mecánico del índice, no una inferencia.
//!
//! La segunda observación es lo que esta ronda encontró y las anteriores no
//! podían usar: cuando se decidió no dar cifras exactas en conteos amplios
//! (F4 opción (a)), el motivo textual era que «en tiempo de consulta Omega no
//! tiene registro de qué documentos del alcance no logró indexar». Esa tabla
//! existe hoy, así que el motivo ya no aplica **para los conteos de archivo**
//! —y sigue aplicando, intacto, para los conteos por campo extraído—.
//!
//! El «tipo» de un documento se lee del nombre del archivo, con el mismo
//! criterio que ya usa `locate_documents_by_key` para resolver `D#####`: el
//! prefijo numérico es el identificador de indexación y lo que sigue es el
//! nombre descriptivo. Ninguna respuesta de este módulo presenta eso como
//! contenido citado: el texto siempre dice que se contó por el nombre del
//! archivo.

use crate::normalize::{normalize_exact, normalize_spanish};

/// Tipo de documento tal y como lo escribe el nombre del archivo.
///
/// `operaciones/01147_bitacora_mantenimiento.docx` → `bitacora_mantenimiento`.
/// Devuelve `None` cuando el nombre no deja nada descriptivo (`00042.pdf`),
/// que es justo el caso en el que no hay tipo que afirmar.
pub fn kind_of_path(path: &str) -> Option<String> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => file,
    };
    let descriptive = stem
        .split_once(['_', '-'])
        .filter(|(prefix, _)| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, rest)| rest)
        .unwrap_or(stem);
    let normalized = normalize_exact(descriptive).replace(' ', "_");
    // Un nombre que sólo son dígitos no dice nada del tipo del documento: es
    // un número de archivo. Devolver «00042» como si fuera un tipo llenaría el
    // reparto por tipo de grupos de un solo documento que no significan nada.
    let says_something = normalized
        .chars()
        .any(|character| !character.is_ascii_digit() && character != '_');
    (says_something && !normalized.is_empty()).then_some(normalized)
}

/// ¿Es este el tipo que la pregunta nombró? Se compara por raíces, para que
/// «orden_mantenimiento», «orden de mantenimiento» y «Órdenes de
/// mantenimiento» sean el mismo tipo y una diferencia de escritura no borre
/// documentos de un conteo en silencio.
pub fn kind_matches(kind: &str, asked: &str) -> bool {
    normalize_spanish(&kind.replace('_', " ")) == normalize_spanish(&asked.replace('_', " "))
}

/// Un archivo descubierto por el indexador, esté o no indexado.
#[derive(Clone, Debug)]
pub struct CensusFile {
    pub path: String,
    pub origin: String,
    pub indexed: bool,
    /// Identificador de fila cuando el archivo sí se indexó; sirve para citar.
    pub document_id: Option<i64>,
}

impl CensusFile {
    pub fn kind(&self) -> Option<String> {
        kind_of_path(&self.path)
    }
}

/// Resultado de un censo: cuántos archivos y cómo se reparten entre leídos y
/// no leídos. Los tres números viajan juntos a propósito — una cifra de
/// cobertura que hay que restar a mano se lee mal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CensusCount {
    pub discovered: usize,
    pub indexed: usize,
    pub unindexed: usize,
}

impl CensusCount {
    pub fn add(&mut self, indexed: bool) {
        self.discovered += 1;
        if indexed {
            self.indexed += 1;
        } else {
            self.unindexed += 1;
        }
    }
}

/// Qué acota el censo. Todo lo que se puede acotar aquí es metadato de
/// archivo; no hay ningún filtro de contenido, y ésa es la razón por la que la
/// cifra puede ser completa.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CensusFilter {
    pub origin: Option<String>,
    pub kind: Option<String>,
}

impl CensusFilter {
    pub fn accepts(&self, file: &CensusFile) -> bool {
        if let Some(origin) = &self.origin {
            if normalize_exact(&file.origin) != normalize_exact(origin) {
                return false;
            }
        }
        if let Some(kind) = &self.kind {
            let Some(actual) = file.kind() else {
                return false;
            };
            if !kind_matches(&actual, kind) {
                return false;
            }
        }
        true
    }
}

/// Cuenta los archivos que cumplen el filtro.
pub fn count(files: &[CensusFile], filter: &CensusFilter) -> CensusCount {
    let mut total = CensusCount::default();
    for file in files.iter().filter(|file| filter.accepts(file)) {
        total.add(file.indexed);
    }
    total
}

/// Reparto por tipo dentro del filtro, ordenado por nombre de tipo para que
/// dos ejecuciones den la misma respuesta.
pub fn by_kind(files: &[CensusFile], filter: &CensusFilter) -> Vec<(String, CensusCount)> {
    let mut groups: std::collections::BTreeMap<String, CensusCount> =
        std::collections::BTreeMap::new();
    for file in files.iter().filter(|file| filter.accepts(file)) {
        let Some(kind) = file.kind() else {
            continue;
        };
        groups.entry(kind).or_default().add(file.indexed);
    }
    groups.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, origin: &str, indexed: bool) -> CensusFile {
        CensusFile {
            path: path.into(),
            origin: origin.into(),
            indexed,
            document_id: None,
        }
    }

    #[test]
    fn the_kind_is_the_descriptive_part_of_the_file_name() {
        assert_eq!(
            kind_of_path("/acervo/rh/01147_bitacora_mantenimiento.docx").as_deref(),
            Some("bitacora_mantenimiento")
        );
        assert_eq!(
            kind_of_path("07962_evaluacion_desempeno.xlsx").as_deref(),
            Some("evaluacion_desempeno")
        );
    }

    #[test]
    fn a_name_without_a_numeric_prefix_keeps_its_whole_stem() {
        assert_eq!(
            kind_of_path("/acervo/ventas/contrato_marco.pdf").as_deref(),
            Some("contrato_marco")
        );
    }

    #[test]
    fn a_name_that_is_only_a_number_has_no_kind_to_claim() {
        assert_eq!(kind_of_path("/acervo/ventas/00042.pdf"), None);
        assert_eq!(kind_of_path("/acervo/ventas/00042_.pdf"), None);
    }

    #[test]
    fn writing_the_kind_with_spaces_or_accents_counts_the_same_documents() {
        assert!(kind_matches("orden_mantenimiento", "orden mantenimiento"));
        assert!(kind_matches("evaluacion_desempeno", "Evaluación desempeño"));
        assert!(!kind_matches("orden_compra", "orden_mantenimiento"));
    }

    #[test]
    fn the_count_declares_what_could_not_be_read() {
        let files = vec![
            file("/a/rh/00001_vacaciones.pdf", "rh", true),
            file("/a/rh/00002_vacaciones.pdf", "rh", false),
            file("/a/rh/00003_nomina.pdf", "rh", true),
            file("/a/ventas/00004_vacaciones.pdf", "ventas", true),
        ];
        let all = count(&files, &CensusFilter::default());
        assert_eq!(all.discovered, 4);
        assert_eq!(all.indexed, 3);
        assert_eq!(all.unindexed, 1);

        let scoped = count(
            &files,
            &CensusFilter {
                origin: Some("rh".into()),
                kind: Some("vacaciones".into()),
            },
        );
        assert_eq!(scoped.discovered, 2);
        assert_eq!(scoped.indexed, 1);
        assert_eq!(scoped.unindexed, 1);
    }

    #[test]
    fn the_breakdown_by_kind_keeps_the_unread_files_in_their_group() {
        let files = vec![
            file("/a/rh/00001_vacaciones.pdf", "rh", true),
            file("/a/rh/00002_vacaciones.pdf", "rh", false),
            file("/a/rh/00003_nomina.pdf", "rh", true),
        ];
        let groups = by_kind(
            &files,
            &CensusFilter {
                origin: Some("rh".into()),
                kind: None,
            },
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "nomina");
        assert_eq!(groups[0].1.discovered, 1);
        assert_eq!(groups[1].0, "vacaciones");
        assert_eq!(groups[1].1.discovered, 2);
        assert_eq!(groups[1].1.unindexed, 1);
    }
}
