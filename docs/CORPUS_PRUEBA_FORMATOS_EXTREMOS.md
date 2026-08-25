# Corpus de prueba: formatos extremos

`corpus-prueba-formatos-extremos/` es un acervo sintético para la segunda ronda de QA de Omega.
No contiene datos reales. Está diseñado para comprobar extracción, OCR, tablas, reindexación,
archivos problemáticos y respuesta honesta ante evidencia incompleta.

| Carpeta | Contenido | Finalidad |
| --- | ---: | --- |
| `01_pdf_texto` | 12 PDF digitales | texto, tablas, campos e importes en PDF |
| `02_pdf_escaneado_ocr` | 8 PDF hechos como imagen | OCR, acentos y texto visual |
| `03_word_docx` | 8 Word | encabezados, tablas y párrafos largos |
| `04_excel_xlsx` | 6 Excel | tablas, fechas, moneda y fórmulas |
| `05_csv` | 6 CSV | columnas, valores repetidos y fechas |
| `06_markdown_largo` | 10 Markdown | referencia de texto y filtros estructurados |
| `07_archivos_problematicos` | 4 archivos | vacíos, truncados, extensión falsa y nombre Unicode |

Los primeros seis grupos contienen **50 documentos válidos**. Los cuatro archivos del último grupo
no deben bloquear la indexación: Omega debe informar los no legibles de forma localizada, indexar
lo que sí puede leer y nunca inventar contenido de ellos.

## Regeneración

Los constructores se ejecutan desde la raíz del proyecto usando los runtimes incluidos por Codex:

```sh
/Users/davidramirez/.cache/codex-runtimes/codex-primary-runtime/dependencies/python/bin/python3 scripts/build_format_stress_documents.py
/Users/davidramirez/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node scripts/build_format_stress_workbooks.mjs
```

El conjunto prueba deliberadamente formatos y errores; no debe usarse como plantilla comercial o
legal real.
