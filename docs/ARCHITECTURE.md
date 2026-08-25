# Arquitectura de Omega

## Flujo de datos y límites de confianza

1. El usuario autoriza una carpeta en modo de lectura.
2. Un parser local produce texto y una secuencia común de registros `etiqueta + valor + ubicación`.
   CSV/XLSX y texto narrativo entran al mismo clasificador; no existen dos catálogos semánticos.
3. El indexador asigna un concepto a todo registro y escribe documentos, FTS, valores, entidades
   y evidencia en una transacción SQLite.
4. Las herramientas consultan SQLite y devuelven JSON estructurado más evidencia. Ninguna
   herramienta genera prosa libre.
5. La fase actual devuelve resultados de recuperación: ruta real, carpeta de origen, campo,
   líneas y fragmento. No compone resúmenes ni respuestas narrativas.
6. El agente local compone únicamente respuestas extractivas y cálculos respaldados. Si no hay
   evidencia suficiente, responde que no la encontró en lugar de inferir o inventar contenido.

## Conversación y razonamiento local

El flujo anterior resuelve una pregunta aislada. Sobre él vive una segunda capa,
igual de determinista, que resuelve una **conversación**:

1. `planner::plan_structured` produce un plan tipado (`Command` + `PlannedScope`)
   a partir de señales léxicas genéricas, del contexto de la conversación y de un
   reloj inyectado. No conoce ningún giro de negocio.
2. `agent` ejecuta ese plan: consulta `tools`, calcula con `calc` y redacta con
   `report`. Cada respuesta declara su alcance (`AnswerScope`) y, si hereda el
   turno anterior, lo marca (`used_context`).
3. `conversation` guarda el resultado del turno como **hechos estructurados**:
   predicado del conjunto, campo calculado, agrupación, rango de fechas, moneda y
   evidencia usada. Nunca guarda una interpretación libre del texto.

### Puertas de entrada

El razonamiento sólo se hace cargo de una pregunta cuando aporta algo que la
recuperación clásica no sabe hacer: promedio, máximo, mínimo, comparación entre
grupos o periodos, agrupación con superlativo, filtro por fecha anclado, o una
referencia al turno anterior. Cualquier otra consulta cae intacta en el
planificador clásico, cuyo comportamiento ya verificado no cambia. Una pregunta
con un literal entrecomillado es siempre una búsqueda literal.

Las señales son palabras completas, no raíces: «contratada» no es «contra» y
«compareciente» no es «comparar». Una raíz compartida bastaba para robarle una
consulta legítima al motor de recuperación.

### El conjunto se guarda como predicado, no como identificadores

Reindexar borra y reinserta las filas de `documents`, de modo que los `rowid` se
reasignan. Un contexto que guardara identificadores internos apuntaría, después
de reindexar, a documentos distintos de los que el usuario vio. Por eso
`conversation::DocumentSet` guarda el **predicado** —filtros, carpeta, rango de
fechas anclado a un campo, o una clave estable— y se reevalúa en cada turno: el
conjunto sigue el estado real del índice y las fuentes revocadas desaparecen
solas. Al reindexar o revocar, la evidencia ya citada se descarta
(`invalidate_results`) para que ninguna respuesta apunte a una cita que ya no
existe.

### Aritmética exacta

`calc::Decimal` es un entero de escala fija (cuatro decimales). Sumar y restar
son exactos; el promedio redondea a esa escala; la variación porcentual devuelve
`None` cuando la base es cero, en vez de una cifra inventada. Las monedas viajan
en la clave del acumulador: dos importes de monedas distintas no pueden caer en
la misma suma aunque la pregunta no mencione ninguna moneda. Toda cifra derivada
se cita como cálculo local (`match_kind = "cálculo"`), con el número de valores
que la produjeron.

### Fecha inyectable

`dates::Clock` decide qué día es «hoy». `OmegaEngine::open` usa el reloj del
sistema; `open_with_clock` permite fijarlo. Todo rango relativo («el mes
anterior») se resuelve a fechas concretas que aparecen en la respuesta y en el
alcance, de modo que nunca hay una fecha implícita.

### Lo que el usuario escribe manda

Un par «Campo: valor» escrito en la pregunta se exige completo. Si el acervo no
tiene ese valor exacto, Omega dice que no lo encontró o pregunta ofreciendo los
valores emparentados, pero nunca responde con un valor más corto que se le
parezca: pedir «Estado: Pendiente de emisión» y recibir el conteo de «Pendiente»
es una respuesta a otra pregunta. Cuando la pregunta trae pares escritos, no se
infiere ningún filtro adicional por coincidencia de palabras.

La misma regla gobierna el campo de un cálculo. El campo que la pregunta nombra
se busca en todo el acervo, no sólo en el alcance actual: si existe pero no está
en esos documentos, la respuesta es que ahí no tiene valores; si no existe, que
el acervo no tiene ese campo. En ningún caso se sustituye por el campo que la
conversación venía usando. **El contexto rellena lo que el usuario omitió; nunca
reemplaza lo que escribió.**

### Aclaraciones con memoria

Una aclaración no es un callejón sin salida: `ConversationState::pending` guarda
la pregunta original, el predicado completo del conjunto y las opciones
ofrecidas. Cuando el usuario responde con una de ellas, Omega vuelve a
planificar **la pregunta original** con ese campo ya fijado y sobre el mismo
conjunto. Tratar la respuesta como una consulta nueva perdería el alcance que
motivó la pregunta —sumaría todo el acervo en lugar de los documentos que el
usuario tenía delante—. La aclaración vive un solo turno: o se responde, o la
siguiente pregunta la sustituye.

### Comparaciones y agrupaciones

Los dos grupos de una comparación se reconocen porque la pregunta escribe sus
valores literalmente, no por coincidencia de raíces; entre varios campos que
contengan esos valores gana el que cubre más documentos. Los filtros inferidos
con esos mismos valores se descartan: «Veracruz» puede ser a la vez una ciudad y
un estado, y recortar la comparación con la segunda lectura respondería otra
pregunta. Una comparación reconocida nunca se degrada a búsqueda: si falta un
grupo, se pregunta.

En una agrupación, «más» y «menos» indican la dirección del orden, no la
operación: «qué ciudad tiene el mayor anticipo» pide el total por ciudad, no el
anticipo individual más alto. El campo agrupador se reconoce por su nombre
completo o por una palabra distintiva inequívoca; si varios campos comparten esa
palabra, se pregunta.

### El alcance publicado es el del cálculo

Una respuesta declara los documentos que **aportaron un operando**, no el tamaño
del conjunto consultado. Decir «600 documentos» cuando sólo 140 tenían el campo
describe mal el cálculo y hace irreproducible la cifra.

### Relaciones sólo por clave estable

`relations::stable_key` decide qué puede vincular dos documentos, y es
deliberadamente restrictivo porque el coste de equivocarse es inventar una
relación:

- Un importe, una fecha, un porcentaje o un número suelto nunca identifican a
  nadie, por mucho que dos documentos compartan la cifra.
- Un identificador no lleva espacios, salvo que el campo se llame como un
  identificador (folio, expediente, contrato, póliza, código, referencia…). Así
  «10 pasajeros» en «Capacidad autorizada» queda fuera, mientras que
  «Folio: SEG 26 0024» sigue dentro.
- El valor debe mezclar letras y dígitos y tener cuerpo suficiente: un nombre de
  ciudad, de persona o de producto no produce clave.
- Un valor compartido por decenas de documentos sin un campo identificador
  detrás no es un expediente, sino vocabulario repetido.

Sobre esa base, `relations` vincula documentos por `identifier_canonical`, que
`normalize::canonical_identifier` sólo produce cuando el valor mezcla letras y
dígitos. Dos nombres parecidos no generan clave y por tanto no pueden
relacionarse: la respuesta lo dice explícitamente y muestra, como mucho,
menciones literales. Las contradicciones comparan el **mismo campo** entre documentos que comparten
esa clave, exigiendo que cada documento declare un solo valor de ese campo —un
archivo tabular con muchas filas no es una contradicción—, y nunca deciden cuál
valor es el correcto. Dos documentos con campos distintos no se contradicen: no
hay nada que comparar. Cuando la pregunta nombra la clave y el campo («¿hay
folios con estados diferentes?»), la búsqueda se restringe a esos dos campos y,
si no encuentra repeticiones incompatibles, lo dice — nunca lista los valores
existentes como sustituto.

## Catálogo semántico

`concepts` define clave y tipo. `concept_aliases` conserva el origen y estado de cada alias.
`extracted_values` exige concepto,
tipo, valor normalizado, ubicación y `evidence_id`. Las entidades conservan el concepto que actuó
como rol y distinguen propietario/mención.

La normalización española vive únicamente en `normalize_spanish`. Preguntas, aliases y valores
usan la misma raíz para acentos, género y número; por ejemplo, `pagado` y `pagadas` convergen sin
parches por campo.

## Operaciones de mantenimiento

La revocación ejecuta, dentro de una transacción:

```sql
DELETE FROM chunks_fts
WHERE document_id IN (SELECT id FROM documents WHERE source_id = ?);

DELETE FROM documents WHERE source_id = ?;

DELETE FROM concepts
WHERE NOT EXISTS (
  SELECT 1 FROM extracted_values v WHERE v.concept_id = concepts.id
);
```

Las cascadas eliminan fragmentos, valores, entidades y aliases. La reindexación usa el mismo
patrón, pero purga e inserta dentro de una única transacción; si un parser falla, el índice anterior
se conserva.

## Decisiones adicionales

- Cada operación abre una conexión corta sobre una base WAL; el estado Tauri comparte solo la ruta,
  evitando guardar una conexión SQLite no concurrente en el frontend.
- El indexado de archivos es necesariamente iterativo para leer formatos distintos, pero las
  escrituras se agrupan en una sola transacción. Las eliminaciones nunca iteran por documento.
- Los resultados de agregación incluyen una evidencia de cálculo local, además de los operandos,
  para verificar cifras que son derivadas y no aparecen literalmente en un solo archivo.
- Las rutas solicitadas al abrir una cita se vuelven a validar contra documentos pertenecientes a
  una fuente activa antes de invocar el visor del sistema.
- El motor no realiza solicitudes de red ni depende de modelos o credenciales externas.
