# Omega — 200 preguntas de evaluación humana

Corpus objetivo: `/Users/davidramirez/omega-synthetic-corpus/corpus`.

Estas preguntas se escribieron a partir del manifiesto completo de 10,001 documentos de negocio y de una revisión directa de documentos reales de cada área. No contienen los IDs internos `D#####` que el índice de evaluación usa: los folios, nombres de archivo y expedientes son los datos de negocio que una persona podría conocer.

## Reglas de ejecución

- Para las preguntas 1–175, inicia una conversación nueva por pregunta.
- Las preguntas 176–200 son turnos conversacionales. Respeta cada sesión y no la reinicies entre sus turnos.
- No des por buena una respuesta sólo porque suena razonable. Una respuesta afirmativa debe incluir una cita al archivo y al valor que la respalda. Si falta evidencia, la conducta correcta es aclarar o responder que no encontró evidencia.
- Conserva la respuesta textual, `verified`, advertencia, citas, archivo, valor, ubicación y tiempo de cada ejecución.

## Consultas de una sola pregunta

### Ventas

1. ¿En qué planta se atendió el pedido `PED-2024-00063`?
2. Necesito revisar el pedido `PED-2023-00124`: ¿qué proveedor quedó relacionado con él?
3. ¿De qué sucursal salió la cotización `PED-2023-00129`?
4. ¿Qué planta aparece en el contrato comercial `EXP-2023-00196`?
5. Para el pedido `PED-2023-00023`, ¿en qué moneda está registrado el importe?
6. ¿Quién es el cliente relacionado con el pedido `PED-2025-00138`?
7. ¿Cuánto se pidió en total en `PED-2023-00116`? Indica la moneda.
8. ¿Qué fecha tiene el pedido `PED-2023-00056`?
9. ¿Qué cantidad se registró en el pedido `PED-2023-00019` y en qué unidad?
10. ¿En qué planta se firmó el contrato comercial `EXP-2023-00136`?

### Compras y logística

11. Estoy rastreando la orden `OC-2024-00114`; ¿qué cliente está asociado?
12. ¿Qué proveedor figura en el contrato `EXP-2025-00183`?
13. ¿Qué cantidad se movió en el envío `ENV-2023-00070`?
14. ¿A qué planta corresponde el envío `ENV-2023-00067`?
15. Para el contrato de proveedor `EXP-2024-00224`, ¿quién aparece como proveedor relacionado?
16. ¿En qué planta quedó registrada la orden de compra `OC-2025-00035`?
17. ¿Qué moneda maneja la orden `OC-2025-00021`?
18. ¿Qué cliente está vinculado con el contrato de proveedor `EXP-2023-00111`?
19. ¿Quién quedó como responsable del envío `ENV-2024-00066`?
20. ¿En qué planta se registró el contrato `EXP-2024-00064`?

### Operaciones

21. ¿Qué cliente está ligado a la bitácora de mantenimiento `MTTO-2023-00111`?
22. ¿En qué moneda está la recepción de almacén `REC-2025-00057`?
23. ¿Qué moneda se registró en la bitácora de producción `LOTE-2023-00125`?
24. ¿Qué cliente aparece relacionado con el lote `LOTE-2025-00017`?
25. ¿Quién fue responsable de la bitácora de producción `LOTE-2024-00036`?
26. Para la recepción `REC-2025-00041`, ¿qué proveedor se indica?
27. ¿Quién quedó como responsable de la bitácora `MTTO-2023-00102`?
28. ¿Qué producto o SKU está relacionado con la bitácora `LOTE-2024-00244`?
29. ¿Qué SKU se relaciona con la orden de mantenimiento `MTTO-2025-00122`?
30. ¿Qué fecha tiene la recepción de almacén `REC-2024-00079`?

### Finanzas

31. ¿Quién autorizó o figura como responsable de la factura `FAC-2023-00187`?
32. ¿Qué responsable aparece en el pago `FAC-2024-00042`?
33. ¿Qué producto está relacionado con la factura `FAC-2023-00254`?
34. Para la factura `FAC-2024-00022`, ¿qué SKU se registró?
35. ¿A qué planta corresponde el pago `FAC-2024-00145`?
36. ¿Qué cantidad aparece en la factura `FAC-2023-00161`?
37. ¿Qué cantidad se asentó en el pago `FAC-2025-00244`?
38. ¿Qué SKU está relacionado con la factura `FAC-2024-00010`?
39. En la factura `FAC-2025-00212`, ¿qué producto se facturó?
40. ¿En qué planta se emitió la factura `FAC-2025-00245`?

### Calidad y seguridad

41. ¿Qué SKU está relacionado con la inspección de calidad `QC-2023-00056`?
42. ¿Qué cantidad se registró en la auditoría de seguridad `AUD-2023-00027`?
43. ¿Qué producto está vinculado con la auditoría `AUD-2023-00113`?
44. ¿Qué cantidad reporta la inspección `QC-2024-00053`?
45. ¿Quién aparece como responsable en la auditoría `AUD-2024-00037`?
46. ¿Qué cliente está relacionado con la auditoría de seguridad `AUD-2023-00020`?
47. ¿Qué cliente quedó asociado a la acción correctiva `AC-2025-00042`?
48. ¿Qué proveedor se menciona en la acción correctiva `AC-2025-00004`?
49. ¿Qué cliente figura en la auditoría `AUD-2023-00013`?
50. ¿En qué moneda se registró la no conformidad `NC-2023-00126`?

### Servicio al cliente

51. ¿Qué fecha tiene el incidente de cliente `INC-2025-00085`?
52. ¿Qué cantidad se reportó en el ticket de servicio `TIC-2023-00071`?
53. ¿Qué producto se devolvió o quedó relacionado con `INC-2025-00087`?
54. ¿Qué cliente está vinculado con el incidente `INC-2024-00030`?
55. ¿De cuánto fue el importe reclamado en el incidente `INC-2025-00095`?
56. ¿Qué SKU está asociado a la devolución `INC-2023-00197`?
57. ¿Qué cantidad aparece en el ticket `TIC-2023-00008`?
58. ¿Qué proveedor quedó ligado al incidente de cliente `INC-2023-00115`?
59. ¿Cuándo se registró la devolución `INC-2025-00144`?
60. ¿En qué moneda está registrado el incidente `INC-2024-00003`?

### Recursos humanos

61. ¿Qué fecha se registró en la incidencia de RH `INC-2025-00190`?
62. ¿Qué moneda tiene el expediente de empleado `EXP-2024-00173`?
63. ¿Qué SKU está ligado a la incidencia `INC-2025-00044`?
64. ¿Qué producto aparece relacionado con la nómina `NOM-2024-00044`?
65. ¿Qué SKU se asocia al expediente de empleado `EXP-2025-00134`?
66. ¿En qué planta está registrada la nómina `NOM-2024-00080`?
67. ¿Qué cantidad se reportó en la incidencia de RH `INC-2024-00159`?
68. ¿Qué proveedor aparece en el expediente de empleado `EXP-2024-00251`?
69. ¿Quién es el responsable de la incidencia `INC-2024-00104`?
70. ¿Qué proveedor se vinculó con la incidencia de RH `INC-2023-00159`?

### Jurídico y cumplimiento

71. ¿Qué proveedor está relacionado con la auditoría interna `AUD-2023-00108`?
72. ¿Cuántos hallazgos abiertos tiene la auditoría `AUD-2023-00008`?
73. ¿En qué planta se ubica el contrato jurídico `EXP-2025-00082`?
74. ¿Quién aparece como responsable en la auditoría `AUD-2024-00028`?
75. ¿Quién está a cargo del contrato jurídico `EXP-2024-00130`?
76. ¿Qué fecha aparece en el contrato jurídico `EXP-2024-00175`?
77. ¿Qué cliente se relaciona con la auditoría `AUD-2024-00057`?
78. ¿Qué proveedor aparece en el expediente legal `EXP-2024-00017`?
79. ¿Quién es responsable de la auditoría interna `AUD-2023-00078`?
80. ¿Qué producto o SKU está relacionado con la auditoría `AUD-2025-00041`?

### Tecnología

81. ¿Qué proveedor está ligado al incidente de TI `TIC-2025-00049`?
82. ¿En qué planta se abrió el ticket de soporte `TIC-2025-00117`?
83. ¿Quién atendió o quedó como responsable del incidente `TIC-2023-00051`?
84. ¿Qué cliente está relacionado con el ticket de soporte `TIC-2023-00111`?
85. ¿Qué cliente se menciona en el incidente de TI `TIC-2023-00103`?
86. ¿Qué fecha tiene el incidente `TIC-2024-00059`?
87. ¿Quién aparece como responsable del incidente `TIC-2023-00133`?
88. ¿Qué fecha se registró en el ticket `TIC-2025-00089`?
89. ¿Quién es responsable del ticket de soporte `TIC-2024-00132`?
90. ¿Qué proveedor figura en el incidente de TI `TIC-2023-00019`?

### Dirección

91. En el reporte ejecutivo de junio de 2025, ¿qué planta se reporta?
92. ¿Quién aparece como responsable en ese reporte ejecutivo de junio de 2025?
93. En el tablero de indicadores de enero de 2025, ¿cuál fue el cumplimiento de meta?
94. ¿Qué planta se indica en el tablero de indicadores de enero de 2025?
95. ¿Cuánto se estima invertir en el plan estratégico de septiembre de 2025 y en qué moneda?
96. ¿Quién figura como responsable en el plan estratégico de septiembre de 2025?
97. En la minuta de dirección de abril de 2024, ¿qué proveedor está relacionado?
98. ¿Qué cliente se vincula con la minuta de dirección de abril de 2024?
99. ¿Qué producto o SKU está relacionado con el tablero de indicadores de agosto de 2025?
100. ¿En qué planta se elaboró el tablero de indicadores de agosto de 2025?

### Conteos, cálculos y panorama

101. Necesito el acumulado de importes de Calidad en MXN; no mezcles otras monedas.
102. ¿Cuál es el total de importes válidos del área de Servicio al cliente que están en EUR?
103. Para Compras, ¿cuánto suman los importes en EUR?
104. Dame el total de Finanzas en MXN, indicando cuántos valores pudiste usar.
105. ¿Cuál es el total de Operaciones en USD?
106. ¿Cuánto suman los importes de Ventas en EUR?
107. ¿Cuál es el promedio de importe en Recursos Humanos para los registros en USD?
108. ¿Qué total de importes hay en TI en moneda MXN?
109. Para Dirección, ¿cuál es el importe acumulado en EUR?
110. ¿Cuánto suman los importes de Jurídico en MXN?
111. Dentro de Calidad en EUR, ¿cuál es el importe más bajo y en qué documento aparece?
112. Dentro de Servicio en USD, ¿cuál es el importe más alto y de qué documento proviene?
113. ¿Cuántos documentos de vacaciones hay en Recursos Humanos?
114. ¿Cuántas facturas hay en el área de Finanzas?
115. ¿Cuántas recepciones de almacén tiene registradas Operaciones?
116. ¿Cuántos tickets de servicio al cliente hay en el acervo?
117. ¿Cuántas auditorías internas hay en Jurídico?
118. ¿Cuántas órdenes de compra hay en Compras?
119. ¿Cuántas políticas de crédito hay en Ventas?
120. ¿Cuántas acciones correctivas hay en Calidad?
121. ¿Cuántos incidentes de TI se tienen registrados?
122. ¿Cuántos reportes ejecutivos hay en Dirección?
123. Dame un desglose por tipo de documento de Servicio al cliente y valida que el total cuadre.
124. Hazme un resumen de cuántas facturas, pagos, conciliaciones, pólizas, presupuestos y reportes de impuestos hay en Finanzas.
125. ¿Cómo se reparte el archivo de Ventas entre pedidos, cotizaciones, contratos, minutas y políticas de crédito?
126. En la cotización `PED-2023-00010`, ¿cuál sería el importe cotizado por kilogramo si divides el importe cotizado entre la cantidad registrada?
127. Para la orden de mantenimiento `MTTO-2025-00027`, calcula el costo de mantenimiento por metro. Si el dato no es compatible, dilo.
128. En la factura `FAC-2023-00244`, ¿cuál es el importe facturado por litro?
129. Para la acción correctiva `AC-2024-00043`, ¿cuánto representa el costo por pieza?
130. ¿Cuál es el promedio de importe de Ventas en USD?
131. ¿Cuál es el promedio de importe de Finanzas en EUR?
132. ¿Qué área tiene el importe máximo más alto entre Dirección y TI si sólo consideramos EUR? Muestra ambos valores antes de comparar.
133. ¿Cuál es el total global en MXN? No conviertas ni agregues USD o EUR.
134. ¿Cuál es el total global en USD? No combines monedas.
135. ¿Cuál es el total global en EUR? No combines monedas.

### Relaciones, expedientes, duplicados y contradicciones

136. Necesito entender el expediente `CASE-0028`: ¿qué documentos lo componen y cuál parece ser el flujo operativo?
137. Muéstrame la trazabilidad documental completa del expediente `CASE-0037`.
138. ¿Qué documentos están relacionados dentro de `CASE-0064`? Ordénalos como se mueve la operación.
139. Para `CASE-0095`, ¿qué documentos forman parte del mismo caso y qué identificadores los conectan?
140. Revisa `CASE-0118`: ¿qué evidencia documental hay y en qué orden ocurrió?
141. ¿Hay diferencias en los importes registrados dentro de `CASE-0093`? Si las hay, no elijas uno como correcto.
142. ¿Los importes del expediente `CASE-0070` son consistentes entre los documentos relacionados?
143. Antes de sumar `CASE-0055`, confirma si todos sus documentos están en la misma moneda.
144. ¿Los documentos de `CASE-0006` manejan una moneda compatible para sumarlos juntos?
145. Revisa si hay una contradicción de importe en `CASE-0086`; necesito ver ambos valores y sus fuentes.
146. ¿Qué contradicción aparece en el expediente `CASE-0113`?
147. ¿El expediente `CASE-0021` permite calcular un total único o mezcla monedas?
148. ¿Qué documentos forman el expediente `CASE-0040` y existe alguna discrepancia entre ellos?
149. ¿Hay documentos con contenido exactamente duplicado dentro del acervo? Dame un par y no confundas similitud con duplicado exacto.
150. ¿Puedes identificar dos archivos parecidos que no sean copias exactas y explicar la diferencia entre “parecido” y “duplicado”?
151. Si un documento se repite byte por byte en dos carpetas, ¿debe contarse dos veces en un conteo de archivos? Explica la evidencia que usas.
152. ¿Hay algún expediente donde una recepción y una factura no coincidan en el importe? No supongas cuál es correcto.
153. Busca el expediente `CASE-0001` y dime si hay contradicciones documentales que requieran revisión humana.
154. Para `CASE-0107`, resume sólo lo que esté respaldado por documentos vinculados.
155. ¿Qué se puede afirmar con seguridad sobre `CASE-0087` y qué no se puede concluir sin más evidencia?

### Ausencia, integridad, OCR y formato

156. Busco el proveedor `PROV-2099-99999`. Si no existe, dímelo claramente y no me sugieras uno parecido.
157. ¿Hay información para el pedido `PED-2099-99999`? Si no existe, necesito una negativa clara.
158. Revisa el expediente `EXP-2099-88888`: ¿hay evidencia documental o no?
159. En el inventario `operaciones/04404_inventario.pdf` aparece la fecha `31/02/2025`. ¿La puedes convertir en una fecha válida? Explica por qué sí o por qué no.
160. Si una fecha aparece como `07/01/2024` y el documento no aclara el formato, ¿qué fecha debes usar?
161. ¿Qué información confiable puedes recuperar del archivo `compras/01115_cotizacion_proveedor.xlsx`?
162. ¿Qué se puede extraer de `ventas/04221_cotizacion.pdf`? No inventes texto si el archivo está truncado.
163. ¿Qué información utilizable hay en `operaciones/07195_orden_mantenimiento.xlsx`?
164. En un PDF escaneado con OCR de baja confianza, ¿puedes marcar la respuesta como verificada? Razona a partir de la evidencia disponible.
165. ¿Cuál es el nivel de confianza OCR de `finanzas/07915_poliza_contable.pdf` y se puede citar su texto como confiable?
166. En `calidad/07550_inspeccion_calidad.xlsx`, localiza dónde aparece `Planta/Sucursal`; necesito una ubicación navegable, no sólo el valor.
167. En `operaciones/01575_orden_mantenimiento.docx`, ¿puedes señalar la tabla y fila exactas donde aparece `Empresa`?
168. En `rh/02708_evaluacion_desempeno.csv`, ¿en qué fila está el campo `Documento`?
169. En `juridico/01240_politica_cumplimiento.pdf`, ¿en qué página aparece la nota de control documental?
170. Cuando un valor viene de una hoja de cálculo con formato de porcentaje, ¿lo tratas como porcentaje o como número entero? Dame evidencia de un caso real.
171. Si una celda muestra dinero sin código ISO de moneda, ¿qué moneda debes reportar?
172. ¿Qué debería hacer Omega si el resultado almacenado de una fórmula XLSX está marcado para recálculo?
173. ¿Qué diferencia hay entre un archivo no indexado porque el OCR no está disponible y un archivo cuyo OCR falló?
174. ¿El conteo de archivos de una carpeta debe incluir los documentos que no pudieron leerse? Explica el alcance de la cifra.
175. ¿Puedo confiar por igual en una cita de PDF con texto nativo y en una cita OCR de baja confianza? Justifica la respuesta.

## Conversaciones: ejecuta cada bloque en la misma sesión

### C-01

176. Quiero revisar el pedido `PED-2023-00116`: ¿cuál es su importe y moneda?
177. ¿Y qué cliente está relacionado con ese mismo pedido?
178. Ahora dime en qué planta se registró.

### C-02

179. Busca la recepción de almacén `REC-2024-00079` y dime la fecha.
180. De ese documento, ¿quién es el proveedor relacionado?
181. ¿Qué producto o SKU quedó ligado a la recepción?

### C-03

182. Dame el total de importes de Compras en EUR.
183. ¿Cuál es el promedio de ese mismo conjunto?
184. ¿Y cuál fue el importe más alto, con su documento de respaldo?

### C-04

185. Revisa el expediente `CASE-0028` y enumera los documentos disponibles.
186. De esos, ¿hay alguna contradicción de importe?
187. Si las monedas no son compatibles, no las sumes: ¿qué explicación corresponde?

### C-05

188. En el incidente de cliente `INC-2025-00095`, ¿cuál fue el importe reclamado?
189. ¿Qué cliente está vinculado con él?
190. ¿Qué fecha tiene el mismo incidente?

### C-06

191. ¿Cuántos documentos de vacaciones hay en RH?
192. ¿Y cuántos expedientes de empleado hay en esa misma área?
193. Resume los tipos documentales de Recursos Humanos sin salirte de esa área.

### C-07

194. Encuentra la auditoría interna `AUD-2023-00108` y dime el proveedor relacionado.
195. ¿Cuántos hallazgos abiertos tiene esa auditoría?
196. ¿Qué responsable aparece en el documento anterior?

### C-08

197. Necesito el total de Finanzas en MXN.
198. Ahora compáralo contra el total de Finanzas en EUR, pero no los sumes ni conviertas.
199. ¿Cuál de los dos conjuntos tiene más documentos con valores utilizables?
200. Muéstrame los documentos que respaldan el resultado anterior, dejando claro si son una muestra o el conjunto completo.
