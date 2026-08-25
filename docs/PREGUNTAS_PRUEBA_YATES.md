# Batería de preguntas para Omega — corpus de yates

Antes de probar, autoriza e indexa la carpeta `corpus-prueba-agencia-yates/`.
Las respuestas esperadas se calcularon directamente sobre los 600 archivos generados.
Para las pruebas de recuperación, verifica que Omega muestre evidencia del campo indicado,
no sólo el nombre del archivo ni un párrafo genérico.

## 1. Panorama general

1. **Pregunta:** `¿Cuántos documentos hay indexados y qué categorías contiene el acervo?`
   **Esperado:** 600 documentos y 11 categorías.

2. **Pregunta:** `¿Cuántos documentos pertenecen a la carpeta 02_reservas_charter?`
   **Esperado:** 140 documentos; la evidencia debe indicar la carpeta de origen.

3. **Pregunta:** `¿Cuántos documentos pertenecen a la carpeta 08_incidentes?`
   **Esperado:** 30 documentos.

## 2. Identificadores y búsquedas exactas

4. **Pregunta:** `Encuentra exactamente el folio MAY-26-0001-AZM.`
   **Esperado:** un solo expediente de venta: `001_may-26-0001_01_ventas.md`.

5. **Pregunta:** `Busca el permiso con número de control PERM-QUI-00535.`
   **Esperado:** un solo expediente de permiso de Cancún; la cita debe contener el número de control.

6. **Pregunta:** `Encuentra exactamente el folio fiscal interno FAC-MAY-26-0481.`
   **Esperado:** una sola factura, en `07_facturas`.

7. **Pregunta:** `Encuentra exactamente MAY.`
   **Esperado:** ningún resultado. Es un identificador incompleto y no debe expandirse a todos los folios que comienzan con MAY.

8. **Pregunta:** `Encuentra exactamente el identificador OMEGA-YATE-INEXISTENTE-999.`
   **Esperado:** ningún resultado ni cita.

## 3. Campos estructurados y filtros

9. **Pregunta:** `¿Cuántas reservas tienen Tipo de documento: Reserva de renta náutica?`
   **Esperado:** 140 documentos.

10. **Pregunta:** `¿Cuántas reservas tienen Estado: Pendiente de pago?`
    **Esperado:** 11 documentos.

11. **Pregunta:** `¿Cuántas facturas tienen Estado de la factura: Vencida?`
    **Esperado:** 4 documentos; las citas deben ser de la línea `Estado de la factura: Vencida`.

12. **Pregunta:** `¿Cuántos permisos tienen Estado: Por renovar?`
    **Esperado:** 4 documentos.

13. **Pregunta:** `¿Cuántos incidentes tienen Estado: Investigación abierta?`
    **Esperado:** 5 documentos.

14. **Pregunta:** `¿Cuántos documentos registran Clase de embarcación: catamarán?`
    **Esperado:** 120 documentos. La pregunta cuenta documentos, no embarcaciones únicas.

15. **Pregunta:** `Muestra documentos con Ciudad base: Cancún y Tipo de documento: Reserva de renta náutica.`
    **Esperado:** 11 reservas; cada resultado debe cumplir ambos campos simultáneamente.

16. **Pregunta:** `Muestra los expedientes de mantenimiento cerrados de embarcaciones en Puerto Vallarta.`
    **Esperado:** 5 documentos de `04_mantenimiento` con evidencia de ciudad y estado, sin mezclar otros tipos documentales.

## 4. Importes y cálculos

17. **Pregunta:** `Suma el campo Total facturado de todas las facturas en MXN.`
    **Esperado:** **$5,585,748.00 MXN**, obtenido de 45 valores.

18. **Pregunta:** `Suma el campo Precio de lista de todos los expedientes de venta en MXN.`
    **Esperado:** **$1,273,950,000.00 MXN**, obtenido de 100 valores.

19. **Pregunta:** `Suma el campo Tarifa contratada de todas las reservas en MXN.`
    **Esperado:** **$15,089,550.00 MXN**, obtenido de 140 valores.

20. **Pregunta:** `¿Cuántos valores tiene el campo Anticipo recibido?`
    **Esperado:** 140 valores, uno por reserva.

21. **Pregunta:** `Agrupa la suma de Total facturado por Ciudad base.`
    **Esperado:** resultados separados por ciudad, con evidencia de cálculo y sin mezclar otras etiquetas monetarias.

## 5. Recuperación por contenido y relaciones operativas

22. **Pregunta:** `¿Qué reglas de cancelación se aplican a una renta de yate?`
    **Esperado:** contenido de reservas sobre el anticipo, el plazo de siete días, reprogramación y cierre de puerto; debe citar documentos de `02_reservas_charter`.

23. **Pregunta:** `¿Quién tiene la autoridad final para cambiar una ruta durante un charter?`
    **Esperado:** el capitán, con una cita procedente de un contrato o reserva.

24. **Pregunta:** `¿Qué impide liberar una embarcación para renta después de mantenimiento?`
    **Esperado:** fallas que afecten propulsión, comunicación, achique, extinción, navegación o casco; cita de `04_mantenimiento`.

25. **Pregunta:** `¿Qué controles se aplican a un pago inusual o realizado por un tercero?`
    **Esperado:** verificación, escalamiento a cumplimiento y prohibición de evadir controles; cita de facturas, ventas o políticas.

26. **Pregunta:** `¿Qué se debe hacer cuando un permiso vence o queda suspendido?`
    **Esperado:** bloquear la operación afectada hasta documentar una liberación válida; cita de `06_permisos_cumplimiento`.

27. **Pregunta:** `¿Qué datos no puede compartir el personal con terceros?`
    **Esperado:** información de pasajeros, pagos, expedientes e itinerarios; cita de personal o política de cumplimiento.

28. **Pregunta:** `Busca incidentes relacionados con lesión leve atendida a bordo.`
    **Esperado:** reportes de incidente que citen exactamente ese tipo de incidente.

## 6. Casos de robustez

29. **Pregunta:** `¿Cuántos documentos mencionan una flota de submarinos?`
    **Esperado:** ningún resultado. No debe inventar documentos ni cifras.

30. **Pregunta:** `Resume las obligaciones legales exactas de México para operar un yate comercial.`
    **Esperado:** Omega debe limitarse al contenido sintético del corpus y advertir que no sustituye asesoría legal; no debe presentar los textos ficticios como normativa real.

31. **Pregunta:** `Busca a Sofía Valdés Romero.`
    **Esperado:** varios documentos relacionados con esa persona ficticia. Revisa que las citas señalen el valor de un campo como `Cliente potencial`, `Responsable interno` o `Colaborador`.

32. **Pregunta:** `¿Existe documentación sobre protección de datos personales?`
    **Esperado:** memorandos de `11_politicas` y referencias en contratos, reservas o expedientes de personal.

## Criterios rápidos de aprobación

- Los resultados exactos no deben devolver documentos con sólo una parte del folio.
- Una pregunta con dos campos debe exigir ambos en el mismo documento.
- Los cálculos deben indicar el número de valores usados y conservar evidencia.
- Las consultas sin coincidencias no deben fabricar respuesta ni cita.
- Las respuestas legales deben reconocer que el corpus es ficticio y no asesoría profesional.
