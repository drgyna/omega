# Validación del paquete macOS

El comando oficial del repositorio es:

```sh
npm run tauri:build
```

Genera `Omega.app` y el `.dmg` bajo `src-tauri/target/release/bundle/`. La construcción local puede
hacerse sin publicar ni aportar credenciales nuevas. Si no hay una identidad `Developer ID
Application`, el artefacto queda sin firma de distribución y no puede darse por notarizado.

## Smoke test del mismo binario empaquetado

El ejecutable dentro de `Omega.app` incluye un modo headless que no abre ni cambia la UI:

```sh
Omega.app/Contents/MacOS/omega --release-smoke \
  /tmp/omega-package-smoke.db \
  /ruta/fixture-escaneado.pdf \
  'LITERAL-VISIBLE'
```

El comando abre una base limpia, autoriza la carpeta de la fixture, indexa con el OCR Vision/PDFKit
embebido, exige una cita `complete`, fiable y ubicada, cierra el motor, reabre SQLite y verifica que
la cita siga apuntando al mismo archivo. Debe ejecutarse fuera de un sandbox de automatización que
bloquee Vision.

Vision Text Recognition requiere macOS 10.15 o posterior. En una plataforma sin Vision el helper
devuelve el estado explícito `unavailable`; no se indexa texto ni se publica una respuesta
verificada.

## Pendiente para distribución general

En el equipo de firma se debe seleccionar una identidad Developer ID válida, construir el paquete,
notarizarlo con Apple, adjuntar el ticket (`stapler`) y comprobarlo con `codesign`, `spctl` y una
instalación limpia desde el DMG. Estos pasos no se simulan sin las credenciales reales.

