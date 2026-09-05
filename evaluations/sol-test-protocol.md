# Protocolo para que Sol evalúe Omega

Usa este protocolo junto con `omega-human-200.md`. El archivo de preguntas no contiene respuestas esperadas ni IDs internos del índice, por lo que puede enviarse a Omega sin revelar el oráculo.

## Preparación

1. Crea una base limpia de Omega.
2. Autoriza exclusivamente `/Users/davidramirez/omega-synthetic-corpus/corpus`.
3. Indexa la carpeta completa y guarda el reporte de indexación: descubiertos, indexados, omitidos, estados OCR, avisos y duración.
4. Anota la versión exacta de Omega: commit y hash de los cambios sin confirmar.
5. Ejecuta las preguntas 1–175 en una conversación nueva por pregunta. Ejecuta 176–200 respetando las ocho sesiones indicadas.

## Qué debe registrar Sol en cada resultado

```json
{
  "question_id": 1,
  "question": "…",
  "answer_text": "…",
  "verified": true,
  "warning": null,
  "used_context": false,
  "citations": [
    {"path": "…", "location": "…", "field": "…", "value": "…", "reliable": true}
  ],
  "latency_ms": 0,
  "verdict": "pass | fail | needs_review",
  "reason": "…"
}
```

## Regla de evaluación

Sol debe comprobar el resultado contra los documentos del corpus, no contra intuición ni contra el texto de la pregunta.

- **Pass:** la respuesta atiende la pregunta; las citas apuntan al archivo correcto, valor correcto y ubicación real; si afirma un hecho, éste está respaldado.
- **Fail:** valor incorrecto, documento equivocado, cita sin el valor afirmado, mezcla de monedas, cálculo incompleto presentado como total, contexto conversacional equivocado, o una respuesta `verified=true` con evidencia OCR no confiable.
- **Needs review:** la pregunta es genuinamente ambigua y Omega pide aclaración; o la fuente está dañada/no legible y Omega explica el límite sin inventar contenido.

Una respuesta sin evidencia no puede aprobarse sólo por coincidir textualmente. Una respuesta negativa o una aclaración sí puede aprobarse si es la conducta segura correcta.

## Puertas de salida

Reporta resultados globales y por bloque: consultas de negocio (1–100), cálculo y censo (101–135), relaciones (136–155), integridad/formato (156–175) y conversación (176–200).

El reporte debe separar al menos:

- recuperación del documento correcto;
- exactitud del valor o cálculo;
- exactitud de cita (archivo, valor, ubicación);
- falsas verificaciones;
- negativas/clarificaciones correctas;
- continuidad conversacional;
- latencia p50, p95 y peor caso.

Una falsa verificación es bloqueante: no se compensa con respuestas correctas en otras preguntas.
