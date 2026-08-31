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

## Benchmark reproducible y sobre operativo

El benchmark release genera un corpus sintético local y mide por separado indexación, búsqueda
exacta, filtros, ranking, construcción de citas, respuesta completa y máximo residente del
proceso. No escribe corpus ni reportes dentro del repositorio:

```sh
cargo run --manifest-path src-tauri/Cargo.toml --release --bin omega-bench -- \
  --sizes 1000,10000,50000 --report /tmp/omega-bench.json
```

Cada consulta tiene una ejecución de calentamiento y siete muestras; el reporte JSON conserva
mejor, mediana y peor tiempo. La indexación se mide una vez por tamaño. Las cifras son una línea
base del equipo que las ejecuta, no un SLA ni una proyección a tamaños no medidos.

Como límite operativo inicial, la beta debe limitar cada fuente a **10,000 documentos** y capturar
el reporte anterior en el hardware objetivo antes de aceptar más. Producción puede habilitar hasta
**50,000 documentos por fuente** sólo tras reproducir el benchmark en el paquete y hardware de
distribución, verificar espacio para al menos el tamaño del índice medido más margen operativo y
revisar la mediana/peor caso de respuesta completa. Fuentes mayores requieren un nuevo benchmark,
una decisión explícita de capacidad y no deben quedar cubiertas por inferencia lineal.

## Escenarios conversacionales

Además de las preguntas sueltas, la fábrica encadena turnos sobre una misma
conversación (`ask_in_conversation`) y comprueba que el contexto se comporte:

- **Continuidad.** Un conteo con filtro establece un conjunto; el turno siguiente
  suma sobre ese conjunto y no sobre el acervo. El oráculo calcula por su cuenta
  el total del subconjunto y el total del acervo: si el motor devolviera el
  segundo, el caso falla. Después se pide la evidencia del total y se verifica
  que las citas sean exactamente los documentos usados.
- **Ambigüedad de campo.** Si el conjunto tiene más de un campo numérico, el
  oráculo espera una **aclaración**, no una cifra. El oráculo decide esto
  releyendo el corpus y reimplementando la regla del índice (el tipo de un campo
  lo fija su primera aparición), sin llamar a la lógica de producción.
- **Elección de la aclaración.** Cuando el motor pregunta qué campo usar, el
  escenario responde con una de las opciones ofrecidas y comprueba que la suma
  se calcule sobre el conjunto anterior, con su alcance heredado y sus citas
  —no sobre todo el acervo—.
- **Valor inexistente.** El valor real más una palabra que no está en el acervo
  debe producir una aclaración con motivo `valor_inexistente`, nunca el conteo
  del valor recortado.
- **Referencia sin contexto.** «¿Cuánto suman esos?» en una conversación nueva
  debe pedir aclaración con el motivo `referencia_sin_contexto`.
- **Contexto borrado.** Tras `reset_conversation`, la misma continuación que
  antes funcionaba debe volver a pedir aclaración: «Nueva conversación» olvida de
  verdad.

Estos casos se generan a partir del propio corpus —un campo repetido que parta el
acervo y un campo monetario presente en esos documentos—, así que no dependen del
vocabulario de ningún giro concreto.

## Cobertura conversacional en `cargo test`

`src-tauri/tests/conversational_reasoning.rs` cubre con fixtures propias los
casos que un corpus no siempre contiene: variación porcentual con base cero, lado
sin datos, monedas incompatibles, relación válida por identificador, falsa
relación por nombres parecidos, contradicciones, resumen de expediente,
reindexación sin contaminar el contexto y ausencia de evidencia. También
comprueba, leyendo el propio código fuente, que el motor de producción no
contenga vocabulario de los corpus de control de calidad.
