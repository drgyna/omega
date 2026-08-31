# OCR local

`LocalDocumentParser` distingue un PDF con texto nativo de uno escaneado. En macOS, un auxiliar
compilado junto con Omega usa Vision y PDFKit de forma local para imágenes y PDF escaneado; no se
envían archivos a red. La evidencia OCR conserva página, zona y confianza. Una confianza menor a
0.55 se muestra como no confiable y no se completa con texto inventado.

En plataformas sin ese proveedor, el archivo queda **sin evidencia indexada** y el reporte de
indexación lo cuenta como `unavailable` (OCR no disponible en este equipo). No se declara como
procesado ni se sustituye por texto de otro documento. Si el proveedor se ejecuta pero no produce
texto utilizable, el estado es `failed`; si lo produce por debajo del umbral, es
`low_confidence`. Ninguno de esos estados puede sostener una respuesta verificada.

La interfaz de extensión es:

```rust
pub trait OcrEngine: Send + Sync {
    fn recognize(&self, path: &Path) -> OcrOutcome;
}
```

Un proveedor alternativo debe ejecutar OCR en el equipo, devolver fragmentos con ubicación por
página y pasar por el mismo extractor, catálogo e índice. No se permite un proveedor de red por
defecto ni un camino semántico especial para OCR.

## Dependencias de plataforma y pase manual de distribución

- El proveedor incluido sólo está disponible en macOS y depende de **Vision** y **PDFKit**. En
  otras plataformas se debe observar `unavailable` y un aviso explícito, nunca una cita vacía ni
  una respuesta verificada.
- Vision Text Recognition requiere macOS 10.15 o posterior. El empaquetado final debe declarar y
  comprobar ese mínimo antes de habilitar OCR.
- El binario auxiliar se materializa localmente con permisos de propietario (`0700`); no usa red,
  credenciales ni servicios externos.
- Antes de distribuir, ejecutar la prueba OCR real en el contexto nativo del paquete (no sólo en
  un sandbox): definir `OMEGA_OCR_FIXTURE` y `OMEGA_OCR_QUERY`, y correr
  `cargo test --test ocr_state_regressions the_real_local_ocr_engine_reads_a_scanned_fixture -- --ignored`.
  La fixture debe ser un PDF o imagen escaneada con texto conocido y debe comprobarse página,
  ubicación, confianza y recuperación por el literal. Si faltan esas variables o fixture, la
  prueba se mantiene ignorada.
- También probar manualmente un PDF truncado, una imagen ilegible y un PDF con texto nativo:
  deben informar respectivamente `failed`, `failed` y `not_required`, sin contaminar una
  indexación completa.
