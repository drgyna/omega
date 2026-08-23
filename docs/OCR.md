# OCR local

`LocalDocumentParser` distingue un PDF con texto nativo de uno escaneado. En macOS, un auxiliar
compilado junto con Omega usa Vision y PDFKit de forma local para imágenes y PDF escaneado; no se
envían archivos a red. La evidencia OCR conserva página, zona y confianza. Una confianza menor a
0.55 se muestra como no confiable y no se completa con texto inventado.

En plataformas sin ese proveedor, el archivo queda indexado con estado `pending` para que una
reindexación lo procese cuando exista un OCR local. La interfaz de extensión es:

```rust
pub trait OcrProvider: Send + Sync {
    fn recognize(&self, path: &Path) -> Result<ParsedDocument>;
}
```

Un proveedor alternativo debe ejecutar OCR en el equipo, emitir el mismo `ParsedDocument` con
ubicaciones por página y pasar por el mismo extractor, catálogo e índice. No se permite un
proveedor de red por defecto ni un camino semántico especial para OCR.

Pendiente para la siguiente ronda:

- conservar cajas/página para abrir una cita en la ubicación visual exacta;
- añadir fixtures OCR externos por plataforma al pase de integración;
- medir calidad y tiempo por página, y exponer progreso/cancelación en la pantalla de fuentes.
