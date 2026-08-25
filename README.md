# Omega

Omega es una aplicación de escritorio local para conversar con los documentos de un negocio,
calcular sobre datos extraídos y conservar una fuente navegable para cada respuesta. Está
construida con Tauri v2, Rust, SQLite/FTS5 y React/TypeScript.

> Omega no se usa abriendo `index.html`. Ese archivo es un recurso interno que Tauri empaqueta
> dentro de la ventana nativa. Para instalar la aplicación en macOS, abre el `.dmg` generado en
> `src-tauri/target/release/bundle/dmg/` y arrastra **Omega** a Aplicaciones.

## Garantías del diseño

- La búsqueda, el parsing, la extracción, los cálculos y las citas funcionan sin red.
- Los valores extraídos tienen siempre `concept_id`, tipo y evidencia; el esquema no permite
  valores huérfanos.
- La revocación elimina FTS, documentos, valores, entidades y aliases derivados mediante SQL
  de conjunto y dentro de una sola transacción.
- Omega no incluye modelos remotos, API, claves ni tráfico de red: los archivos y las preguntas
  no salen del equipo.
- El planificador local decide la ruta de búsqueda; Rust calcula y devuelve evidencia. Un
  verificador bloquea cualquier cifra, fecha, identificador o nombre propio no respaldado.

## Desarrollo

Requisitos: Node.js 20 o posterior, Rust estable y las dependencias de sistema de Tauri v2.

```sh
npm install
npm run tauri:dev
```

Compilación verificable sin crear un instalador:

```sh
npm run build
npm run tauri -- build --debug --no-bundle
npm run tauri:build
```

`npm run tauri:build` genera tanto `Omega.app` como el instalador `.dmg` en macOS.

## Pruebas

Las pruebas normales validan el motor y sus parsers. La fábrica local agrega siete corpus
sintéticos y aislados —incluidos formatos extremos— para detectar regresiones sin tocar la base
real de la aplicación.

```sh
cd src-tauri
cargo test --all
```

Desde la raíz del repositorio, para ejecutar todos los corpus de evaluación:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --bin omega-eval -- --all
```

Consulta [docs/EVALUACIONES.md](docs/EVALUACIONES.md) para ejecutar un corpus concreto, la carga
de 5,000 documentos y conocer los límites de OCR del entorno.

## Formatos y OCR

- Soportados y verificados por pruebas de recuperación: TXT, Markdown, CSV, XLSX, DOCX y PDF
  con texto nativo. CSV/XLSX devuelven celda y encabezado; DOCX devuelve párrafo; PDF devuelve
  página y línea.
- XLS se procesa mediante el mismo lector local de libros de cálculo que XLSX. La compatibilidad
  depende de que el archivo no esté cifrado ni dañado.
- DOC binario no se interpreta: al indexarlo Omega informa que requiere una conversión local a
  DOCX o PDF para conservar evidencia verificable.
- En macOS, PNG, JPG/JPEG, TIFF, BMP, WEBP y HEIC (cuando ImageIO pueda abrirlo), así como PDF
  escaneado, se envían exclusivamente al proveedor local Vision/PDFKit incluido con el binario.
  La evidencia OCR conserva página/zona y confianza. En otras plataformas queda `pendiente` hasta
  instalar un proveedor OCR local equivalente; nunca se usa un servicio remoto ni se inventa texto.

Para verificar un archivo OCR externo en macOS:

```sh
OMEGA_OCR_FIXTURE=/ruta/a/imagen-o-pdf-escaneado \
OMEGA_OCR_QUERY='texto exacto visible' \
cargo test --test format_retrieval retrieves_ocr_fixture_when_explicitly_configured -- --nocapture
```

## Estructura

- `src-tauri/src/parser.rs`: TXT, Markdown, CSV, XLSX, DOCX, PDF de texto y contrato OCR.
- `src-tauri/src/indexer.rs`: indexado transaccional y extracción unificada etiqueta/valor.
- `src-tauri/src/tools.rs`: catálogo, búsqueda, lookup exacto y agregaciones.
- `src-tauri/src/agent.rs`: planificador local, búsqueda, cálculo y respuestas con citas.
- `src-tauri/src/verifier.rs`: verificación de respaldo literal antes de publicar una respuesta.
- `src/`: conversación, fuentes autorizadas, citas y configuración de privacidad.
- `docs/ARCHITECTURE.md`: límites de confianza y decisiones técnicas.
- `docs/OCR.md`: punto de extensión y plan de la siguiente implementación.
