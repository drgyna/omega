# Informe independiente — Omega human-200

## Resumen ejecutivo

Omega aprobó **31 de 200** preguntas, falló **162** y dejó **7** en revisión. La corrida usó el motor local real del commit `6311ebec31587c16b15605474b0a8b4145fa92f5`, una SQLite limpia en `/tmp` y una única fuente autorizada: `/Users/davidramirez/omega-synthetic-corpus/corpus`.

Una falsa verificación es bloqueante. Se observaron 114 respuestas `verified=true` con veredicto `fail`; el patrón dominante fue citar el propio folio o contar documentos en vez de responder el campo o cálculo solicitado.

## Resultados por bloque

| Bloque | Pass | Fail | Needs review | Total |
|---|---:|---:|---:|---:|
| Negocio 1–100 | 14 | 85 | 1 | 100 |
| Cálculos/censo 101–135 | 0 | 35 | 0 | 35 |
| Relaciones 136–155 | 9 | 8 | 3 | 20 |
| Integridad/formato 156–175 | 6 | 11 | 3 | 20 |
| Conversación 176–200 | 2 | 23 | 0 | 25 |

## Fallas observadas

- **Falsas verificaciones:** 114 preguntas. IDs: 1, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20, 21, 22, 23, 25, 26, 27, 29, 30, 31, 32, 33, 34, 35, 36, 37, 39, 40, 42, 43, 44, 45, 47, 48, 49, 51, 52, 53, 54, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 73, 74, 75, 76, 77, 78, 79, 81, 82, 83, 85, 86, 87, 88, 89, 90, 95, 101, 102, 103, 104, 105, 106, 108, 110, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 126, 127, 128, 129, 133, 134, 135, 159, 160, 171, 179, 181, 182, 191, 192, 195, 196, 197.
- **Fallas de citas:** 114 respuestas fallidas incluyeron citas reales pero irrelevantes o insuficientes para lo pedido. IDs: 1, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20, 21, 22, 23, 25, 26, 27, 29, 30, 31, 32, 33, 34, 35, 36, 37, 39, 40, 42, 43, 44, 45, 47, 48, 49, 51, 52, 53, 54, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 73, 74, 75, 76, 77, 78, 79, 81, 82, 83, 85, 86, 87, 88, 89, 90, 95, 101, 102, 103, 104, 105, 106, 108, 110, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 126, 127, 128, 129, 133, 134, 135, 159, 160, 171, 179, 181, 182, 191, 192, 195, 196, 197.
- **Recuperación:** 85 fallas en consultas de negocio; el caso típico recuperó el documento por folio, pero sintetizó el folio en vez del campo solicitado.
- **Cálculo o moneda:** 42 fallas. No se observó una mezcla aritmética explícita de monedas porque, en general, el motor no llegó a calcular; devolvió conteos, listados o negativas.
- **Conversación:** 23 fallas en el bloque conversacional. Sólo 188 y 194 respondieron correctamente; varios primeros turnos ya establecieron un alcance erróneo y los siguientes perdieron o reinterpretaron el referente.
- **OCR/integridad:** 11 fallas. La indexación omitió 1041 archivos y registró 1011 OCR fallidos; no hubo evidencia OCR de baja confianza publicada como `verified=true` (`ocr_low_confidence=0`).

## Latencia

La latencia de respuesta fue p50 **830 ms**, p95 (rango más cercano) **1342 ms** y peor caso **6418 ms** en la pregunta 95. La indexación completa tardó **274414 ms**.

## Indexación y alcance

Omega descubrió 10000 formatos soportados, indexó 8959, omitió 1041 y extrajo 171340 valores. Detectó 94 grupos idénticos (188 documentos). `source_folders` contiene una sola autorización, la del corpus objetivo.

## Causas probables priorizadas

1. **La señal exacta por folio corta demasiado pronto hacia recuperación exacta.** En `src-tauri/src/planner.rs:79` cualquier identificador toma `QueryIntent::Exact`. Después, `src-tauri/src/answer.rs:352-366` acepta sin más el único grupo de campo recuperado; en decenas de casos ese grupo fue `PED`, `FAC`, `INC`, etc., no el campo solicitado. La evidencia son, entre otras, las preguntas 1, 3, 7–14, 16–23 y 25–37.
2. **«Total» se clasifica como conteo pero no como suma.** `src-tauri/src/planner.rs:41-42` incluye la raíz `total` en `asks_count` y reserva suma para `sum`, `totaliz` o `add`. Esto reproduce directamente respuestas como 102–106, 110 y 182/197: conteos de documentos donde se pidió importe acumulado.
3. **Una recuperación fallida reemplaza el estado conversacional.** `src-tauri/src/agent.rs:210-232` reinicia `ConversationState` en cada turno de recuperación; además `src-tauri/src/agent.rs:52-56` borra el documento señalado antes de ejecutar el nuevo plan. Tras un primer turno mal sintetizado, referencias como «ese mismo pedido» o «de ese documento» quedan sin antecedente útil.
4. **El manejo seguro de metadatos evita algunas invenciones, pero deja consultas respondibles sin salida.** `src-tauri/src/agent.rs:936-949` convierte coincidencias sólo de metadatos en negativa no verificada. Es seguro, pero apareció en preguntas con evidencia estructurada disponible (125, 130, 131 y 168), indicando una falla anterior de planificación/recuperación.
5. **OCR local dominó y falló para todos los escaneos intentados.** El reporte atribuye 245,739 ms a `pdf_ocr`, con 1,011 OCR fallidos y cero resultados de baja confianza. Esto explica la imposibilidad de responder 165, pero Omega no expuso el estado/confianza solicitado.

## Limitaciones de la evaluación

- Ningún texto `CASE-####` aparece en el corpus ni en la tabla `extracted_values`; esos rótulos sólo están en `oracle/relations.jsonl`. Penalizar una negativa segura habría exigido a Omega conocer el oráculo, lo cual estaba prohibido. Por eso 136–155 mezcla passes seguros, aclaraciones en revisión y fallos sólo cuando la respuesta hizo una afirmación no sustentada o cuando la pregunta era global (duplicados/recepción–factura).
- La pregunta 92 se ejecutó correctamente como conversación nueva conforme al protocolo; «ese reporte» no tenía antecedente y la aclaración se dejó en `needs_review`.

## Reproducibilidad

- Commit: `6311ebec31587c16b15605474b0a8b4145fa92f5`.
- Estado inicial de Git: `?? evaluations/omega-human-200.md`, `?? evaluations/sol-test-protocol.md`; sin diferencias en archivos versionados.
- SHA-256 del diff versionado vacío: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
- SHA-256 canónico del estado no confirmado (incluye no rastreados): `c310962ec5994f969d30bdbc6f722a98a8f8922213f8b8d364bd5f02f925c93d`.
- Preguntas SHA-256: `89f7f973eb65c655702bb4109626847a32f1c933404b97a54a5fa37569cece1f`.
- Protocolo SHA-256: `e011acd69ae8767228a3afa70860f14cf2a0e2b2aa0570aedbf182598d227987`.
- `answers.jsonl` conserva literalmente texto, `verified`, advertencia, contexto, alcance, aclaración, citas completas y latencia de cada turno. `failures.jsonl` es el subconjunto con veredicto `fail`.
