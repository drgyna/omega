# Corpus de prueba — agencia de yates

La carpeta `corpus-prueba-agencia-yates/` contiene exactamente 600 documentos Markdown
sintéticos para probar la indexación y recuperación de Omega. No contiene datos personales,
clientes, permisos, facturas ni operaciones reales.

## Contenido

| Carpeta | Documentos | Ejemplos de información |
| --- | ---: | --- |
| `01_ventas` | 100 | prospectos, precio, anticipo, embarcación y entrega |
| `02_reservas_charter` | 140 | rutas, pasajeros, pagos, capitanes y cancelaciones |
| `03_contratos` | 80 | alcance, condiciones, responsabilidad y privacidad |
| `04_mantenimiento` | 70 | diagnóstico, sistemas, costo y liberación de flota |
| `05_personal` | 45 | puestos, capacitación, desempeño y acceso a datos |
| `06_permisos_cumplimiento` | 45 | permisos, vigencias, renovación y auditoría |
| `07_facturas` | 45 | importes, impuestos, pagos y cobranza |
| `08_incidentes` | 30 | eventos, investigación y acciones correctivas |
| `09_proveedores` | 20 | evaluación, diligencia y condiciones de compra |
| `10_inventario` | 15 | activos, inspecciones y custodia |
| `11_politicas` | 10 | privacidad, fraude, anticorrupción y quejas |

Los archivos usan campos `Etiqueta: valor` (por ejemplo, `Estado`, `Folio`, `Ciudad base`,
`Embarcación`, `Importe total`), para que Omega pueda recuperar evidencia precisa y realizar
consultas sobre valores estructurados.

## Uso

En Omega, autoriza o selecciona la carpeta completa:

`/Users/davidramirez/Documents/ChatGPT/omega/corpus-prueba-agencia-yates`

Para regenerar los mismos 600 archivos después de una modificación deliberada del generador,
ejecuta desde la raíz del proyecto:

```sh
node scripts/generate_yacht_agency_corpus.mjs
```

La regeneración reemplaza sólo esa carpeta de corpus.
