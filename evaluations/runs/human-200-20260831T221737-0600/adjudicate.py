#!/usr/bin/env python3
"""Adjudicación reproducible, posterior a la corrida, de Omega human-200.

Este programa nunca formula preguntas a Omega. Sólo lee la captura ya cerrada,
el reporte de indexación y el corpus/oráculo para producir los entregables.
"""

from __future__ import annotations

import json
import math
import statistics
from collections import Counter
from pathlib import Path


RUN = Path("/Users/davidramirez/Documents/ChatGPT/omega/evaluations/runs/human-200-20260831T221737-0600")
RAW = RUN / "raw-engine-answers.jsonl"
INDEX = RUN / "index-report.json"

PASS = {
    # Negocio: campo y evidencia correctos.
    5, 6, 15, 24, 28, 38, 41, 46, 50, 55, 71, 72, 80, 84,
    # Relaciones: CASE-#### no existe en ningún documento; negativa segura.
    136, 137, 138, 140, 141, 142, 147, 154, 155,
    # Ausencia/archivo y ubicaciones navegables.
    156, 157, 158, 166, 167, 169,
    # Conversación: los dos turnos que sí contestaron con evidencia correcta.
    188, 194,
}

NEEDS_REVIEW = {
    # Referente inexistente en conversación nueva; Omega pide aclaración.
    92,
    # CASE-#### sólo existe en el oráculo; Omega pide contexto adicional.
    139, 143, 144,
    # Archivos vacíos/truncados; Omega niega sin inventar, pero no diagnostica
    # el daño de forma específica.
    161, 162, 163,
}

CUSTOM_FAILURES = {
    95: "Contó 249 documentos de septiembre de 2025 en lugar de recuperar el importe y la moneda del plan estratégico; la respuesta verificada no atiende la pregunta.",
    101: "Enumeró valores del campo Moneda en lugar de sumar los importes de Calidad en MXN; no entregó total ni cantidad de operandos.",
    126: "Devolvió sólo el importe cotizado y omitió dividirlo entre la cantidad registrada en kilogramos.",
    127: "Devolvió sólo el costo de mantenimiento y no comprobó ni explicó la compatibilidad de la unidad para calcular costo por metro.",
    128: "Devolvió sólo el importe facturado y omitió dividirlo entre la cantidad en litros.",
    129: "Devolvió sólo el costo de la acción correctiva y omitió dividirlo entre la cantidad en piezas.",
    132: "Interpretó «área» como el campo numérico y no calculó ni mostró los máximos EUR de Dirección y TI.",
    133: "Presentó extractos de 282 documentos como si respondieran el total global MXN; no calculó el total y mezcló evidencia no pertinente.",
    145: "Afirmó que no encontró contradicciones, aunque el rótulo CASE-0086 no existe en las fuentes y por ello no podía sostener esa negativa sobre el expediente.",
    146: "Afirmó ausencia de contradicción sin poder vincular CASE-0113 a documentos del corpus.",
    148: "Buscó repeticiones del campo Documento y concluyó que no había contradicción; ese criterio no corresponde a CASE-0040 ni a sus claves documentales.",
    149: "El reporte de indexación detectó 94 grupos (188 documentos) idénticos byte por byte, pero Omega respondió que no encontró evidencia y no dio el par pedido.",
    150: "No identificó ningún par parecido no idéntico ni explicó la diferencia solicitada, pese a que el corpus contiene pares de casi duplicados.",
    151: "No respondió la política de conteo de copias idénticas ni citó un grupo real, aunque el índice conservó y contó 94 grupos duplicados.",
    152: "No recuperó un expediente enlazado por claves documentales con recepción y factura discrepantes; respondió sin evidencia.",
    153: "Afirmó ausencia de documentos incompatibles sin poder resolver el rótulo CASE-0001, que no aparece en las fuentes.",
    159: "Repitió la fecha imposible 31/02/2025 como valor verificado, pero no negó su conversión ni explicó que febrero no puede tener 31 días.",
    160: "Repitió 07/01/2024 como si fuera utilizable y no pidió aclarar si el formato era DD/MM o MM/DD.",
    164: "No contestó si un OCR de baja confianza puede marcarse verified ni razonó sobre confiabilidad de la evidencia.",
    165: "No informó la confianza OCR del archivo ni que su texto no puede citarse como confiable; sólo devolvió una negativa genérica.",
    168: "El CSV sí contiene Documento en la fila 1, pero Omega sólo halló metadatos y no dio la ubicación solicitada.",
    170: "No explicó la semántica de porcentaje ni aportó el caso real solicitado.",
    171: "Mostró extractos de 1,215 documentos, incluida una moneda EUR ajena a la celda hipotética, en vez de declarar moneda desconocida/no inferible.",
    172: "No explicó que una fórmula marcada para recálculo no debe usar su resultado almacenado como valor verificable.",
    173: "No distinguió OCR no disponible de OCR ejecutado y fallido.",
    174: "No explicó si el conteo incluye archivos no legibles ni declaró el alcance indexados/omitidos.",
    175: "No distinguió la confiabilidad de PDF nativo frente a OCR de baja confianza.",
    176: "No recuperó el importe y moneda del pedido al primer turno; interpretó el texto posterior al folio como valor del campo PED.",
    179: "Contestó con el folio REC en vez de la fecha, por lo que la sesión C-02 inició con un referente incorrecto.",
    182: "Contó 980 documentos de Compras en vez de sumar sus importes EUR; la sesión C-03 quedó sin el conjunto calculado solicitado.",
    185: "No pudo resolver CASE-0028, rótulo ausente de las fuentes; la sesión no estableció el expediente y los turnos siguientes quedaron sin base.",
    197: "Contó 1,450 documentos de Finanzas en vez de calcular el total MXN; la sesión C-08 no estableció el cálculo solicitado.",
}


def read_jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def short(text: str, limit: int = 180) -> str:
    clean = " ".join(text.split())
    return clean if len(clean) <= limit else clean[: limit - 1] + "…"


def verdict_for(question_id: int) -> str:
    if question_id in PASS:
        return "pass"
    if question_id in NEEDS_REVIEW:
        return "needs_review"
    return "fail"


def pass_reason(question_id: int) -> str:
    if question_id in {136, 137, 138, 140, 141, 142, 147, 154, 155}:
        return "Negativa segura: el rótulo CASE-#### no aparece en ningún documento ni valor indexado, por lo que no hay evidencia fuente para vincularlo sin usar el oráculo."
    if question_id in {156, 157, 158}:
        return "Negativa clara y segura ante un identificador inexistente; no sugirió coincidencias parecidas ni inventó evidencia."
    if question_id in {166, 167, 169}:
        return "Recuperó el archivo correcto y dio una ubicación navegable y coherente con el campo/valor fuente."
    if question_id in {188, 194}:
        return "Respondió el dato solicitado con archivo, valor y ubicación correctos dentro de la sesión correspondiente."
    return "Respondió el campo solicitado y la cita coincide con el archivo, valor y ubicación de la fuente."


def review_reason(question_id: int) -> str:
    if question_id == 92:
        return "La pregunta se ejecutó en conversación nueva y «ese reporte» no tiene antecedente; Omega pidió aclaración en lugar de adivinar."
    if question_id in {139, 143, 144}:
        return "El rótulo CASE-#### no está presente en las fuentes; Omega pidió contexto adicional. Requiere revisión porque el oráculo sí agrupa documentos bajo ese rótulo externo."
    return "La fuente está vacía o truncada; Omega no inventó contenido y negó evidencia suficiente, pero no diagnosticó explícitamente el tipo de daño."


def generic_fail_reason(row: dict) -> str:
    qid = row["question_id"]
    if qid in CUSTOM_FAILURES:
        return CUSTOM_FAILURES[qid]
    answer = short(row.get("answer_text", ""))
    citations = row.get("citations", [])
    if qid <= 90:
        if citations:
            first = citations[0]
            field = first.get("field") or "sin campo"
            value = first.get("value") or "sin valor"
            return f"Recuperó el folio, pero respondió/citó «{field}={value}» en vez del dato solicitado; es evidencia real pero no sustenta la respuesta pedida."
        return "Existe un documento fuente para el folio y el campo solicitado, pero Omega no lo recuperó ni aportó citas."
    if qid <= 100:
        return f"No resolvió ni desambiguó el documento de Dirección solicitado; respuesta obtenida: «{answer}»."
    if qid <= 135:
        return f"No ejecutó el cálculo, censo o comparación solicitado; respuesta obtenida: «{answer}»."
    if qid <= 155:
        return f"No entregó la relación, duplicado o contradicción pedida con evidencia reproducible; respuesta obtenida: «{answer}»."
    if qid <= 175:
        return f"No aplicó la conducta de integridad/formato requerida ni aportó la evidencia específica; respuesta obtenida: «{answer}»."
    return f"Falló la recuperación o continuidad dentro de la sesión; respuesta obtenida: «{answer}»."


def failure_types(row: dict, verdict: str) -> list[str]:
    if verdict != "fail":
        return []
    qid = row["question_id"]
    if qid <= 100:
        kinds = ["recuperacion"]
    elif qid <= 135:
        kinds = ["calculo_moneda"]
    elif qid <= 155:
        kinds = ["relaciones_recuperacion"]
    elif qid <= 175:
        kinds = ["ocr_integridad"]
    else:
        kinds = ["conversacion"]
        if qid in {182, 183, 184, 197, 198, 199, 200}:
            kinds.append("calculo_moneda")
    if row.get("citations"):
        kinds.append("cita")
    if row.get("verified"):
        kinds.append("falsa_verificacion")
    return kinds


def block_for(qid: int) -> tuple[str, str]:
    if qid <= 100:
        return "negocio_1_100", "Negocio 1–100"
    if qid <= 135:
        return "calculos_censo_101_135", "Cálculos/censo 101–135"
    if qid <= 155:
        return "relaciones_136_155", "Relaciones 136–155"
    if qid <= 175:
        return "integridad_formato_156_175", "Integridad/formato 156–175"
    return "conversacion_176_200", "Conversación 176–200"


def nearest_rank(values: list[int], percentile: float) -> int:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(percentile * len(ordered)) - 1)]


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def write_jsonl(path: Path, rows: list[dict]) -> None:
    path.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")


def main() -> None:
    raw = read_jsonl(RAW)
    if len(raw) != 200 or [row["question_id"] for row in raw] != list(range(1, 201)):
        raise SystemExit("captura inválida: se requieren 200 filas ordenadas 1..200")
    index = json.loads(INDEX.read_text(encoding="utf-8"))

    answers: list[dict] = []
    for original in raw:
        row = dict(original)
        verdict = verdict_for(row["question_id"])
        if verdict == "pass":
            reason = pass_reason(row["question_id"])
        elif verdict == "needs_review":
            reason = review_reason(row["question_id"])
        else:
            reason = generic_fail_reason(row)
        row["verdict"] = verdict
        row["reason"] = reason
        row["failure_types"] = failure_types(row, verdict)
        answers.append(row)

    failures = [row for row in answers if row["verdict"] == "fail"]
    totals = Counter(row["verdict"] for row in answers)
    block_rows: dict[str, dict] = {}
    for row in answers:
        key, label = block_for(row["question_id"])
        item = block_rows.setdefault(key, {"label": label, "range": [], "total": 0, "pass": 0, "fail": 0, "needs_review": 0})
        item["total"] += 1
        item[row["verdict"]] += 1
    for key, item in block_rows.items():
        start, end = {
            "negocio_1_100": (1, 100),
            "calculos_censo_101_135": (101, 135),
            "relaciones_136_155": (136, 155),
            "integridad_formato_156_175": (156, 175),
            "conversacion_176_200": (176, 200),
        }[key]
        item["range"] = [start, end]

    latencies = [int(row["latency_ms"]) for row in answers]
    worst = max(answers, key=lambda row: int(row["latency_ms"]))
    category_counts = Counter(kind for row in failures for kind in row["failure_types"])
    false_ids = [row["question_id"] for row in failures if row.get("verified")]
    citation_ids = [row["question_id"] for row in failures if row.get("citations")]
    summary = {
        "run_id": "human-200-20260831T221737-0600",
        "repository": "/Users/davidramirez/Documents/ChatGPT/omega",
        "corpus": "/Users/davidramirez/omega-synthetic-corpus/corpus",
        "database": "/tmp/omega-human-200-20260831T221737-0600.sqlite3",
        "engine_commit": "6311ebec31587c16b15605474b0a8b4145fa92f5",
        "git_status_before": ["?? evaluations/omega-human-200.md", "?? evaluations/sol-test-protocol.md"],
        "tracked_diff_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "uncommitted_state_sha256": "c310962ec5994f969d30bdbc6f722a98a8f8922213f8b8d364bd5f02f925c93d",
        "question_file_sha256": "89f7f973eb65c655702bb4109626847a32f1c933404b97a54a5fa37569cece1f",
        "protocol_file_sha256": "e011acd69ae8767228a3afa70860f14cf2a0e2b2aa0570aedbf182598d227987",
        "totals": {"total": 200, "pass": totals["pass"], "fail": totals["fail"], "needs_review": totals["needs_review"]},
        "blocks": block_rows,
        "failure_categories": dict(sorted(category_counts.items())),
        "false_verification_question_ids": false_ids,
        "citation_failure_question_ids": citation_ids,
        "latency_ms": {
            "p50": statistics.median(latencies),
            "p95_nearest_rank": nearest_rank(latencies, 0.95),
            "worst": int(worst["latency_ms"]),
            "worst_question_id": worst["question_id"],
        },
        "indexing": {
            key: index[key]
            for key in ["discovered", "indexed", "skipped", "ocr_failed", "ocr_low_confidence", "ocr_unavailable", "duplicate_groups", "duplicate_documents", "values", "elapsed_ms"]
        },
        "evaluation_limitations": [
            "Los rótulos CASE-#### no aparecen en ningún archivo del corpus ni en extracted_values; sólo existen en el oráculo. Se aprobaron negativas seguras y se marcaron para revisión las aclaraciones, sin usar el oráculo para fingir que Omega podía resolver esos rótulos.",
            "Las preguntas 1–175 se ejecutaron sin contexto entre sí; por eso la referencia «ese reporte» de la pregunta 92 es genuinamente indeterminada.",
        ],
    }

    write_jsonl(RUN / "answers.jsonl", answers)
    write_jsonl(RUN / "failures.jsonl", failures)
    write_json(RUN / "summary.json", summary)

    lines = [
        "# Informe independiente — Omega human-200",
        "",
        "## Resumen ejecutivo",
        "",
        f"Omega aprobó **{totals['pass']} de 200** preguntas, falló **{totals['fail']}** y dejó **{totals['needs_review']}** en revisión. La corrida usó el motor local real del commit `6311ebec31587c16b15605474b0a8b4145fa92f5`, una SQLite limpia en `/tmp` y una única fuente autorizada: `/Users/davidramirez/omega-synthetic-corpus/corpus`.",
        "",
        "Una falsa verificación es bloqueante. Se observaron " + str(len(false_ids)) + " respuestas `verified=true` con veredicto `fail`; el patrón dominante fue citar el propio folio o contar documentos en vez de responder el campo o cálculo solicitado.",
        "",
        "## Resultados por bloque",
        "",
        "| Bloque | Pass | Fail | Needs review | Total |",
        "|---|---:|---:|---:|---:|",
    ]
    for item in block_rows.values():
        lines.append(f"| {item['label']} | {item['pass']} | {item['fail']} | {item['needs_review']} | {item['total']} |")

    lines += [
        "",
        "## Fallas observadas",
        "",
        f"- **Falsas verificaciones:** {len(false_ids)} preguntas. IDs: {', '.join(map(str, false_ids))}.",
        f"- **Fallas de citas:** {len(citation_ids)} respuestas fallidas incluyeron citas reales pero irrelevantes o insuficientes para lo pedido. IDs: {', '.join(map(str, citation_ids))}.",
        f"- **Recuperación:** {category_counts['recuperacion']} fallas en consultas de negocio; el caso típico recuperó el documento por folio, pero sintetizó el folio en vez del campo solicitado.",
        f"- **Cálculo o moneda:** {category_counts['calculo_moneda']} fallas. No se observó una mezcla aritmética explícita de monedas porque, en general, el motor no llegó a calcular; devolvió conteos, listados o negativas.",
        f"- **Conversación:** {category_counts['conversacion']} fallas en el bloque conversacional. Sólo 188 y 194 respondieron correctamente; varios primeros turnos ya establecieron un alcance erróneo y los siguientes perdieron o reinterpretaron el referente.",
        f"- **OCR/integridad:** {category_counts['ocr_integridad']} fallas. La indexación omitió {index['skipped']} archivos y registró {index['ocr_failed']} OCR fallidos; no hubo evidencia OCR de baja confianza publicada como `verified=true` (`ocr_low_confidence={index['ocr_low_confidence']}`).",
        "",
        "## Latencia",
        "",
        f"La latencia de respuesta fue p50 **{statistics.median(latencies):g} ms**, p95 (rango más cercano) **{nearest_rank(latencies, 0.95)} ms** y peor caso **{worst['latency_ms']} ms** en la pregunta {worst['question_id']}. La indexación completa tardó **{index['elapsed_ms']} ms**.",
        "",
        "## Indexación y alcance",
        "",
        f"Omega descubrió {index['discovered']} formatos soportados, indexó {index['indexed']}, omitió {index['skipped']} y extrajo {index['values']} valores. Detectó {index['duplicate_groups']} grupos idénticos ({index['duplicate_documents']} documentos). `source_folders` contiene una sola autorización, la del corpus objetivo.",
        "",
        "## Causas probables priorizadas",
        "",
        "1. **La señal exacta por folio corta demasiado pronto hacia recuperación exacta.** En `src-tauri/src/planner.rs:79` cualquier identificador toma `QueryIntent::Exact`. Después, `src-tauri/src/answer.rs:352-366` acepta sin más el único grupo de campo recuperado; en decenas de casos ese grupo fue `PED`, `FAC`, `INC`, etc., no el campo solicitado. La evidencia son, entre otras, las preguntas 1, 3, 7–14, 16–23 y 25–37.",
        "2. **«Total» se clasifica como conteo pero no como suma.** `src-tauri/src/planner.rs:41-42` incluye la raíz `total` en `asks_count` y reserva suma para `sum`, `totaliz` o `add`. Esto reproduce directamente respuestas como 102–106, 110 y 182/197: conteos de documentos donde se pidió importe acumulado.",
        "3. **Una recuperación fallida reemplaza el estado conversacional.** `src-tauri/src/agent.rs:210-232` reinicia `ConversationState` en cada turno de recuperación; además `src-tauri/src/agent.rs:52-56` borra el documento señalado antes de ejecutar el nuevo plan. Tras un primer turno mal sintetizado, referencias como «ese mismo pedido» o «de ese documento» quedan sin antecedente útil.",
        "4. **El manejo seguro de metadatos evita algunas invenciones, pero deja consultas respondibles sin salida.** `src-tauri/src/agent.rs:936-949` convierte coincidencias sólo de metadatos en negativa no verificada. Es seguro, pero apareció en preguntas con evidencia estructurada disponible (125, 130, 131 y 168), indicando una falla anterior de planificación/recuperación.",
        "5. **OCR local dominó y falló para todos los escaneos intentados.** El reporte atribuye 245,739 ms a `pdf_ocr`, con 1,011 OCR fallidos y cero resultados de baja confianza. Esto explica la imposibilidad de responder 165, pero Omega no expuso el estado/confianza solicitado.",
        "",
        "## Limitaciones de la evaluación",
        "",
        "- Ningún texto `CASE-####` aparece en el corpus ni en la tabla `extracted_values`; esos rótulos sólo están en `oracle/relations.jsonl`. Penalizar una negativa segura habría exigido a Omega conocer el oráculo, lo cual estaba prohibido. Por eso 136–155 mezcla passes seguros, aclaraciones en revisión y fallos sólo cuando la respuesta hizo una afirmación no sustentada o cuando la pregunta era global (duplicados/recepción–factura).",
        "- La pregunta 92 se ejecutó correctamente como conversación nueva conforme al protocolo; «ese reporte» no tenía antecedente y la aclaración se dejó en `needs_review`.",
        "",
        "## Reproducibilidad",
        "",
        "- Commit: `6311ebec31587c16b15605474b0a8b4145fa92f5`.",
        "- Estado inicial de Git: `?? evaluations/omega-human-200.md`, `?? evaluations/sol-test-protocol.md`; sin diferencias en archivos versionados.",
        "- SHA-256 del diff versionado vacío: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.",
        "- SHA-256 canónico del estado no confirmado (incluye no rastreados): `c310962ec5994f969d30bdbc6f722a98a8f8922213f8b8d364bd5f02f925c93d`.",
        "- Preguntas SHA-256: `89f7f973eb65c655702bb4109626847a32f1c933404b97a54a5fa37569cece1f`.",
        "- Protocolo SHA-256: `e011acd69ae8767228a3afa70860f14cf2a0e2b2aa0570aedbf182598d227987`.",
        "- `answers.jsonl` conserva literalmente texto, `verified`, advertencia, contexto, alcance, aclaración, citas completas y latencia de cada turno. `failures.jsonl` es el subconjunto con veredicto `fail`.",
        "",
    ]
    (RUN / "report.md").write_text("\n".join(lines), encoding="utf-8")


if __name__ == "__main__":
    main()
