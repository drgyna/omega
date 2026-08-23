# Reporte de fases

## Fase 0 — Cimientos

Se construyeron el esquema SQLite/FTS5, autorización de fuentes, seis parsers, extracción unificada,
normalización española, conceptos/aliases con origen y estado, valores tipados, entidades, evidencia
y purga transaccional por lote.

Validación: pruebas unitarias e integración contra un corpus inyectado mediante configuración,
sin valores huérfanos ni fragmentos ausentes en FTS.

## Fase 1 — Motor como herramientas

Se implementaron `list_concepts`, `search_documents`, `exact_lookup` y `aggregate_values` con JSON
Schema estricto, validación de argumentos, filtros, fechas, moneda, agrupación y evidencia.

Validación: lookup por identificador, búsqueda por campo y valor, agregación, filtros y argumentos
inválidos sobre un fixture configurado externamente.

## Fase 2 — Bucle de agente

Se dejó preparado el contrato de herramientas y validación para una fase posterior. La ruta actual
es de recuperación local y no compone prosa.

Validación: salida respaldada aceptada, número no respaldado rechazado, respuesta local con citas y
consulta sin evidencia con negativa explícita. No se realizó una llamada facturable a OpenAI ni se
requirió una clave para las pruebas.

## Fase 3 — Interfaz

Se construyeron conversación, fuentes, reindexación/revocación, citas navegables, estado del motor,
catálogo y configuración de IA con consentimiento. La clave solo cruza una orden Tauri de escritura
hacia el llavero; no existe una orden para leerla al frontend.

La autorización de fuentes usa el selector nativo de carpetas de Tauri. La distribución genera una
aplicación `.app` y un instalador `.dmg`; `index.html` es únicamente un recurso interno del bundle.

Validación: TypeScript/Vite y el binario Tauri compilan; revisión visual de conversación, fuentes y
configuración en layout amplio y compacto; nombres accesibles para navegación y controles.

## Fase 4 — OCR

El punto de extensión, estado de base y detección de PDF escaneado están listos. La ejecución OCR
nativa y las cajas visuales por página quedan documentadas en `docs/OCR.md` como siguiente paso,
sin requerir rediseñar extracción, catálogo ni herramientas.
