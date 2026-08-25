# Corpus de prueba multirrubro

Estos cinco acervos son completamente sintéticos. Se diseñaron para comprobar que Omega funcione
con documentos de negocios distintos sin incluir reglas de ningún rubro en el motor.

| Corpus | Documentos | Carpetas y casos principales |
| --- | ---: | --- |
| `corpus-prueba-notaria` | 100 | escrituras, poderes, testamentos, certificaciones, cumplimiento, expedientes y facturas |
| `corpus-prueba-despacho-legal` | 100 | asuntos, contratos, escritos, dictámenes, cumplimiento, horas y facturas |
| `corpus-prueba-ferreteria` | 100 | ventas, compras, inventario, proveedores, entregas, seguridad y facturas |
| `corpus-prueba-seguros` | 100 | pólizas, siniestros, suscripción, pagos, renovaciones, agentes y cumplimiento |
| `corpus-prueba-restaurante` | 100 | reservaciones, comandas, proveedores, personal, sanidad, facturas e incidentes |

Cada archivo Markdown incluye campos reutilizables como `Folio`, `Tipo de documento`, `Área
responsable`, `Estado`, `Empresa`, `Ciudad base` y `Fecha de registro`, además de campos propios
del rubro: importes, expedientes, pólizas, productos, cantidades, reservas, incidencias y texto
operativo. Esto permite probar conteos, carpetas, filtros con AND, identificadores, sumas,
agrupaciones, búsquedas extractivas y casos sin resultado.

## Regeneración

Desde la raíz del proyecto:

```sh
node scripts/generate_multi_business_corpora.mjs
```

El generador sólo escribe los 500 archivos sintéticos en las cinco carpetas anteriores. No uses
este contenido como documento legal, fiscal, comercial, de seguros o sanitario real.
