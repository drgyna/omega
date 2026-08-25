# Fábrica local de evaluaciones

La fábrica indexa cada corpus en una base SQLite temporal independiente y hace las preguntas por
el mismo recorrido público que usa Omega: `OmegaEngine::open`, `authorize_source`, `index_source`
y `ask`. No usa red, modelos, API ni la base de datos real de la aplicación. Los corpus son
material de control de calidad del repositorio: el empaquetado de Omega sólo incluye el frontend
compilado y no distribuye estas carpetas con la aplicación.

Desde la raíz del repositorio:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --bin omega-eval -- --all
```

Para evaluar un solo corpus activado:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --bin omega-eval -- --corpus ferreteria
```

Para la ronda multiformato con oráculo explícito:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --bin omega-eval -- --corpus formatos-extremos
```

Para una carga temporal reproducible de 5,000 documentos:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --bin omega-eval -- --stress
```

El manifiesto versionado es `evaluation-corpora.json`. Para añadir un corpus basta con agregar su
identificador y ruta relativa; `enabled: false` lo conserva sin ejecutarlo. El lector independiente
descubre archivos Markdown, carpetas, campos, valores, números y párrafos en cada ejecución.
`evaluation-formatos-extremos.json` es un fixture pequeño y versionado, independiente de los
parsers de producción. Declara conteos, identificadores, campos, agregados, ubicaciones y avisos
esperados para PDF con texto, PDF escaneado, DOCX, XLSX, CSV, Markdown y archivos problemáticos.

La ronda de formatos también trabaja sobre una copia temporal para eliminar, modificar y agregar
archivos; comprueba que la reindexación no deje evidencia fantasma, reabre la SQLite, rechaza una
base corrupta de forma controlada y prueba rutas Unicode, nombres largos y un symlink fuera de la
fuente autorizada. Nunca muta el corpus original ni la base real de la aplicación.

Cada corrida crea `artifacts/evaluations/<marca-de-tiempo>/` con:

- `resultados.jsonl`: un objeto por caso, incluidos fallos y omisiones.
- `resumen.json`: conteos, duración, métricas por etapa y memoria máxima aproximada.
- `reporte.md`: informe legible por caso con formato, archivo, pregunta, esperado, obtenido,
  citas, errores de indexación, duración y estado.

El comando devuelve código distinto de cero si existe cualquier caso fallido. Las familias que no
pueden generarse con evidencia suficiente se registran como `omitida` con su razón.

En macOS el OCR usa Vision/PDFKit local. Un sandbox de automatización que niegue la aceleración de
Vision puede devolver cero líneas aunque la aplicación nativa funcione; por eso la validación OCR
debe ejecutarse en el mismo contexto nativo en que se distribuirá Omega. No hay OCR remoto ni una
ruta de red alternativa.
