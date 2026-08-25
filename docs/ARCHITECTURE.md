# Arquitectura de Omega

## Flujo de datos y límites de confianza

1. El usuario autoriza una carpeta en modo de lectura.
2. Un parser local produce texto y una secuencia común de registros `etiqueta + valor + ubicación`.
   CSV/XLSX y texto narrativo entran al mismo clasificador; no existen dos catálogos semánticos.
3. El indexador asigna un concepto a todo registro y escribe documentos, FTS, valores, entidades
   y evidencia en una transacción SQLite.
4. Las herramientas consultan SQLite y devuelven JSON estructurado más evidencia. Ninguna
   herramienta genera prosa libre.
5. La fase actual devuelve resultados de recuperación: ruta real, carpeta de origen, campo,
   líneas y fragmento. No compone resúmenes ni respuestas narrativas.
6. El agente local compone únicamente respuestas extractivas y cálculos respaldados. Si no hay
   evidencia suficiente, responde que no la encontró en lugar de inferir o inventar contenido.

## Catálogo semántico

`concepts` define clave y tipo. `concept_aliases` conserva el origen y estado de cada alias.
`extracted_values` exige concepto,
tipo, valor normalizado, ubicación y `evidence_id`. Las entidades conservan el concepto que actuó
como rol y distinguen propietario/mención.

La normalización española vive únicamente en `normalize_spanish`. Preguntas, aliases y valores
usan la misma raíz para acentos, género y número; por ejemplo, `pagado` y `pagadas` convergen sin
parches por campo.

## Operaciones de mantenimiento

La revocación ejecuta, dentro de una transacción:

```sql
DELETE FROM chunks_fts
WHERE document_id IN (SELECT id FROM documents WHERE source_id = ?);

DELETE FROM documents WHERE source_id = ?;

DELETE FROM concepts
WHERE NOT EXISTS (
  SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id
);
```

Las cascadas eliminan fragmentos, valores, entidades y aliases. La reindexación usa el mismo
patrón, pero purga e inserta dentro de una única transacción; si un parser falla, el índice anterior
se conserva.

## Decisiones adicionales

- Cada operación abre una conexión corta sobre una base WAL; el estado Tauri comparte solo la ruta,
  evitando guardar una conexión SQLite no concurrente en el frontend.
- El indexado de archivos es necesariamente iterativo para leer formatos distintos, pero las
  escrituras se agrupan en una sola transacción. Las eliminaciones nunca iteran por documento.
- Los resultados de agregación incluyen una evidencia de cálculo local, además de los operandos,
  para verificar cifras que son derivadas y no aparecen literalmente en un solo archivo.
- Las rutas solicitadas al abrir una cita se vuelven a validar contra documentos pertenecientes a
  una fuente activa antes de invocar el visor del sistema.
- El motor no realiza solicitudes de red ni depende de modelos o credenciales externas.
