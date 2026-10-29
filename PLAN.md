# Estivi — plan de los cambios de canal, grilla y transmisión

**Estado: correcciones incorporadas. No implementar nada de este archivo hasta que se apruebe.**

Este documento es la especificación y el orden de trabajo. Guarda el pedido completo (copiado al final, sin recortar) y la lectura con la que se va a programar. Si al leerlo algo no coincide con lo que querés, se corrige este archivo primero y recién después se ejecuta.

Lo que contestaste ya está cerrado en [Decisiones que se cierran al aprobar](#decisiones-que-se-cierran-al-aprobar). No queda nada pendiente de esa lista.

### Correcciones ya incorporadas

- Exportación: 11025, 22050, 44100 y **48000**. Default 44100.
- Cortina a color: modo nuevo **Robot 12 color** (`R12`). Cortina gris: `BW8`. El `BW12` que ya existe es el Robot 12 en blanco y negro y no se usa como ese default.
- AX.25 siempre a 1200 baudios. No hay selector.
- Un solo PTT a la vez.
- Pausar y reanudar la grilla son acciones de una activación. No transmiten.
- El preámbulo VOX se queda. No es un paso del bloque: es del proyecto y va delante de cualquier TX (grilla, instantáneo o activación).
- Dentro de un bloque no hay rotación. El orden es el de la lista.
- No se escribe un conversor de proyectos viejos. La alfa no tiene archivos que conservar.
- El FSK ID no va. El VIS sí: delante de cada SSTV, aunque el modo ya viaje en el AX.25.
- El editor de gráficas del canal gana una capa QR. El texto sale de variables de la grilla, de un comando local, o de una API. Las activaciones también pueden consultar una API y transmitir la respuesta.

## Cómo está armado hoy el programa

Estivi es un ejecutable de escritorio (Rust + Slint, sin navegador). Lo que no es pantalla vive en `crates/core`. Lo que se ve vive en `crates/app`.

Hay dos archivos distintos, y eso se mantiene:

| Archivo | Qué es |
|---|---|
| `*.estivi` | El canal. TOML, versión de formato en el campo `format` (hoy `1`). |
| `estivi.toml` | Esta PC: audio, PTT, CAT, idioma. No viaja con el canal. |

Hoy el modelo está al revés de lo que tiene que quedar:

- Un **programa** (`Program` en `crates/core/src/project.rs`) es una lista de imágenes, con rotación, modo SSTV opcional, CW propio y activaciones.
- Un **bloque** (`Slot` en la grilla) es una ventana semanal (`days`, `start`, `end`, `every_min`) que apunta a un programa. La frecuencia está en el bloque (`freq_hz`), no en el canal.
- El **offset** en segundos sí es del proyecto (`grid.offset_s`) y atrasa todos los arranques. Eso se queda.
- Una transmisión es una sola imagen SSTV: preámbulo VOX opcional, CW opcional, VIS, imagen, FSK ID opcional, CW final. No hay AX.25, no hay cortinas, no hay corte de PTT al fin del horario.
- El motor (`crates/core/src/engine.rs`) salta una salida que llega con más de 90 s de atraso (`MAX_LATE`) y respeta una pausa mínima entre salidas (`min_pause_s`, default 60 s).
- Escuchar CW y DTMF existe, y dispara “transmitir un programa”, “pausar la grilla” o “reanudar la grilla”. DTMF solo se decodifica: no hay generador para transmitirlo.
- La grilla de la ventana es una lista de bloques más una tira del día. No es un calendario.
- El tema claro ya define texto oscuro en `crates/app/ui/theme.slint`, pero el botón Light/Dark deja los widgets estándar en esquema oscuro. El `init` de la ventana hace `Palette.color-scheme = Theme.light ? light : dark`. El click hace `set-light(!Theme.light)` y después `Palette.color-scheme = !Theme.light ? light : dark`, o sea el esquema contrario al que acaba de quedar. Con el fondo claro, esos widgets siguen dibujando texto blanco. La interfaz nueva va al final; este dato queda anotado para esa fase.

La frase “base de datos” de acá en adelante significa el archivo `.estivi`. No se agrega un motor SQL. Un bloque que “está en la base” es una tabla TOML. Un bloque que “no está” se calcula desde la configuración del programa y no se escribe.

## Vocabulario de este plan

- **Proyecto / canal.** Un archivo `.estivi`. Una frecuencia.
- **Programa.** Plantilla que se repite. Tiene nombre, días de semana, rango horario, imagen de bienvenida, imagen de despedida, fecha de término, activado y un color (solo el matiz). No tiene lista de imágenes de aire.
- **Bloque.** Una ocurrencia en el calendario: un día concreto y un rango horario, con su lista de imágenes. Es la tarjeta de la grilla.
- **Bloque de programa.** Hereda nombre, días y rango del programa. Si no tiene imágenes, no se guarda: la grilla lo dibuja igual, transparente, calculándolo.
- **Bloque común.** No tiene programa. No se repite. Tiene fecha, rango horario, nombre propio, imágenes y activado. Si está vacío y no lo borraron, sigue guardado y se dibuja transparente.
- **Imagen normal.** Una transmisión dentro del bloque.
- **Bumper / cortina.** Imagen pegada a una imagen normal, antes (prebumper, preámbulo de esa imagen) o después (postbumper, postámbulo de esa imagen). No ocupa un turno propio del intervalo.
- **Intervalo.** Cada cuánto arranca una imagen, en minutos enteros. En un programa es de cada día de la semana, junto con el rango horario. En un bloque común es de esa fecha. `0` = un solo arranque. El offset sigue siendo del proyecto.
- **TX.** Una salida completa al aire, en el orden de la sección [Secuencia de una transmisión](#secuencia-de-una-transmisión).
- **Activación.** Disparador de CW o DTMF con una lista ordenada de acciones. No es un objeto de la grilla y no vive dentro de un programa.

## Orden de ejecución

La interfaz es la última fase. Antes se construye la lógica, con tests, y la pantalla vieja solo se toca lo mínimo para que el proyecto siga compilando. No se redibuja de a pedazos.

El orden sigue la dependencia: nada de audio se puede componer sin el modelo, nada se puede exportar sin la secuencia de TX, y la grilla visual necesita las mismas cuentas que el motor ya va a usar en el aire.

| Fase | Qué queda resuelto | Depende de |
|---|---|---|
| 0 | Este documento aprobado | — |
| 1 | Modo Robot 12 color | 0 |
| 2 | Formato nuevo del `.estivi`, sin conversor del viejo | 0 |
| 3 | Calendario: ocurrencias, solapes, días restantes, turnos | 2 |
| 4 | AX.25: trama, audio y duración, siempre 1200 baudios | 2 |
| 5 | Una TX completa, duraciones y corte de PTT | 1, 3, 4 |
| 6 | La grilla real usa esa TX (motor, sin pantalla nueva) | 5 |
| 7 | Cortinas, sufijos `-bp`/`-bn`, importar carpetas | 1, 2, 5 |
| 8 | Exportar audio del rango horario | 5, 6 |
| 9 | Reloj por internet | 2 |
| 10 | Activaciones, incluidas pausar y reanudar | 4, 5, 6 |
| 11 | TX instantáneo | 5, 6 |
| 12 | Consultas de grilla mes / semana / día | 3, 5 |
| 13 | Interfaz completa, al final | 1–12 |

Las fases 9 y 7 pueden escribirse en paralelo con sus vecinas cuando se ejecute, porque no se pisan los archivos. El orden de la tabla es el orden para integrarlas. Cada fase termina con `cargo test --workspace` y `cargo build -p estivi` en verde. La fase 13, además, se prueba abriendo la ventana.

## Decisiones que se cierran al aprobar

Estas lecturas están tomadas del pedido. Están escritas acá, aparte, para poder tachar una línea sin releer todo.

1. **Un canal, una frecuencia.** `freq_hz` pasa al proyecto. Se borra del bloque. El CAT, si está activado, pone esa frecuencia antes de cada salida de la grilla.
2. **El offset es del proyecto. El intervalo y el rango horario son de cada día del programa, y de cada bloque común.** El offset en segundos sigue como está y no mueve el fin escrito. El intervalo es minutos enteros, default 6, y `0` deja un solo arranque. Un programa guarda siete ventanas (lunes primero): hora de inicio, hora de fin e intervalo. Cambiar el lunes no cambia el sábado. Al encender un día que no tiene horario, copia el de un día que ya lo tenga. Un bloque común guarda los suyos. Al convertir un bloque de programa en común, se copian el horario y el intervalo de ese día. Un archivo con un solo `start`/`end`/`interval_min` en el programa los reparte a los siete días al abrirlo. Un `interval_min` en la raíz del canal se reparte igual, a los programas y bloques comunes que no tengan el suyo. Al guardar, ni la raíz ni el programa llevan un intervalo único.
3. **Un bloque común es una sola fecha.** No tiene días de semana y no se repite al semana siguiente. Un programa sí se repite, por máscara de días, hasta su fecha de término o para siempre.
4. **Las imágenes son de esa ocurrencia.** Cargar fotos el martes 26 no llena los martes siguientes. Cada fecha con imágenes es su propia entrada.
5. **Vacío de programa no se guarda.** Vacío común sí, hasta que el usuario borra la tarjeta. La frase del pedido “no vamos a guardar un bloque si tiene imágenes” se lee al revés de lo literal, porque la oración siguiente es la regla: se crea la entrada al asignar imágenes y se borra la entrada de un bloque de programa cuando se queda sin imágenes. El texto original queda copiado al final.
6. **Bienvenida y despedida son del programa, y se aplican en cada bloque, no solo en el primero y el último del programa.** Dentro de un bloque, la bienvenida va solo en la primera TX y la despedida solo en la última. Un bloque de 14:00 a 14:30 con intervalo de 6 min sale a las 14:00, 14:06, 14:12, 14:18 y 14:24: bienvenida a las 14:00, despedida a las 14:24, ninguna de las dos en el medio. Si el bloque tiene una sola TX, van las dos en esa TX.
7. **14:30 no es un arranque** si el bloque termina a las 14:30. El turno siguiente arrancaría en el instante en que se acaba el permiso. Si el bloque fuera 14:00–14:33 y la sexta imagen (arranque 14:30) dura menos de 3 min contando toda su TX, sí entra.
8. **No se arranca una TX que no entra entera** antes del próximo turno y antes del fin del bloque. La duración incluye lead y tail de PTT, la ráfaga VOX si está activa, CW, AX.25, bienvenida o despedida si ese turno las lleva, cortinas y la imagen. Si no entra, se salta ese turno. Las imágenes de más, por encima de la cantidad de turnos, no se transmiten, y sus cortinas tampoco. Las cortinas no consumen turno.
9. **Si aun así el bloque se acaba en el aire, se corta.** Se suelta el PTT y se corta el audio juntos, en el último instante del permiso. Un fin a las 14:30 suelta en 14:29:59.99, no en 14:29:59.0. En la implementación el margen máximo es 1 ms: el PTT tiene que estar suelto antes del instante `fin`, sin adelantar un segundo a propósito. El offset no corre esa hora de fin: el fin es el fin escrito del bloque. El offset solo corre los arranques.
10. **El único que transmite fuera de horario, y al que no se le suelta el PTT por la hora, es el tab de TX instantáneo.**
11. **Un solo PTT a la vez.** Con la grilla armada pero en silencio, el tab instantáneo se puede usar. Con la grilla transmitiendo, no. Una acción de activación que transmite, si dispara durante una TX de grilla, espera a que el canal quede libre, después corre completa, y el fin de un bloque no la corta. Pausar y reanudar no usan el PTT y no esperan (decisión 20). Si por esa espera el turno de grilla llega con más de 90 s de atraso, ese turno se salta, como ya hace el motor.
12. **Dentro de un bloque el orden es el de la lista.** Se va la rotación al azar y también la secuencial que elegía “la próxima foto”: no hay un puntero que recuerde por dónde iba. Cada turno toma la siguiente imagen normal habilitada, de arriba hacia abajo. La ruleta de programas del canal también desaparece, porque un bloque ya no pide “el próximo programa”: tiene sus propias imágenes. El CW propio de cada programa sale: el CW es uno, el del canal. La pausa mínima entre salidas se queda, como protección de la radio: si entre el fin real de una TX y el turno siguiente hay menos que `min_pause_s`, ese turno no arranca.
13. **AX.25 en el aire es siempre 1200 baudios** (1200/2200 Hz). No es una opción del proyecto ni de la activación. Trama UI, PID `0xF0`, destino `ESTIVI`, origen el indicativo de la estación. El texto exacto está más abajo. Preámbulo y postámbulo de info llevan el mismo texto.
14. **Importar un programa** lee carpetas `block` + número (`block01`, `block1`, sin distinguir mayúsculas). El número es la enésima ocurrencia de ese programa desde ahora, contando también las vacías. `block02` sola llena la segunda, no la primera.
15. **El sufijo de cortina** es la última cosa del nombre, antes de la extensión, sin distinguir mayúsculas: `foto-bp.png` y `foto-BP.png` sí; `foto-bp-2.png` no.
16. **Reloj.** No se cambia la hora del sistema. Estivi guarda un corrimiento contra un servidor NTP y con eso decide la grilla. Default: preguntar en el primer arranque, servidor `pool.ntp.org`, repetir cada 6 h. Después se cambia en la configuración de la PC.
17. **Color de programa.** Se guarda solo el matiz, 0–359. Saturación y brillo son constantes del dibujo (saturación 0,45, luminosidad 0,42) para que el texto de la tarjeta se lea en los dos temas.
18. **Días restantes**, tal como está el pedido. `0` significa infinito, o sin fecha, o más de 99 días. `1` significa que hoy es el último día de transmisión, no que mañana es el último. `2`…`99` son los días que faltan contando hoy. Se calcula en la zona horaria de la grilla.
19. **El preámbulo VOX se queda.** Es del proyecto, apagado por defecto como ahora. No es un paso del contenido del bloque. Si está activo, la ráfaga de 800 ms (los ocho tonos de 100 ms que ya arma `VOX_PREAMBLE` en `crates/core/src/encoder.rs`) va delante de toda TX: grilla, tab instantáneo y activación, antes del CW. En la lista del bloque se muestra como fila no editable al tope de cada TX, solo cuando está activo, porque ocupa aire y entra en la cuenta de si la TX se pasa del turno. No se reordena y no se configura por bloque.
20. **Pausar y reanudar la grilla son acciones de activación.** No transmiten y no bajan el PTT, así que no esperan a que el canal esté libre: se aplican en cuanto coincide el disparador, aunque haya una TX en el aire. Pausar deja la agenda en falso y no corta la TX que ya salió; los turnos siguientes no arrancan. Reanudar vuelve a armar la agenda. Es el mismo efecto que `TriggerAction::Pause` y `Resume` en `engine.rs` hoy.
21. **Frecuencias de exportación:** 11025, 22050, 44100 y 48000. Default 44100, FLAC.
22. **Cortina recién marcada:** si la imagen tiene color, modo `R12` (Robot 12 color, el de la fase 1). Si es gris, `BW8`. “Tiene color” = algún píxel con diferencia entre canales mayor a 8. Después el usuario puede cambiar el modo. `BW12` sigue existiendo como Robot 12 blanco y negro, elegible a mano, y no es el default.
23. **No hay conversor de `.estivi` viejos.** La app está en alfa temprana y no hay proyectos guardados que haya que leer. El formato nuevo reemplaza al actual. Un archivo con la forma vieja se rechaza con un error claro. No se diseña la conversión “para más adelante” en esta tanda: cuando existan archivos que valga la pena conservar, se hace ahí.
24. **No hay FSK ID.** Se va la opción del proyecto y no se llama a `fskid_segments`. El indicativo digital de esta TX es el AX.25 y, si está activo, el CW.
25. **Cada SSTV lleva su VIS delante**, el header que ya arma `vis_segments` (300 ms + 10 ms + 300 ms de líder, y los bits de 30 ms con el código del modo). En Scottie se mantiene el sync de 9 ms que este encoder ya agrega pegado al VIS. Va en toda pieza: bienvenida, cada cortina, la imagen y la despedida, y también en una imagen de activación o del tab instantáneo. El AX.25 de modo no lo reemplaza: ese texto es para quien lee el paquete; el VIS es lo que un decodificador SSTV (QSSTV, MMSSTV, el resto) necesita para enganchar la imagen. No es opcional y no tiene interruptor.
26. **Capa QR en la gráfica del canal**, con el texto armado de una de tres maneras, y una acción nueva de activación que consulta una API. El detalle está en [Capa QR](#capa-qr).

## Robot 12 color

Modo nuevo. No es `BW12` (“Robot 12 B/N”, VIS `0x06`, solo luminancia, línea de 100 ms con sync de 7 ms y Y de 93 ms). El de color es el que el handbook de SSTV lista como el modo de color más corto de la familia Robot.

Fuente: Martin Bruchanov OK2MNM, *Image Communication on Short Waves*, tabla 4.3, y la lista de modos del mismo handbook (VIS 0). QSSTV y slowrx no lo transmiten; slowrx solo tiene el Robot 12 blanco y negro. El test de esta fase es la duración publicada, no una decodificación en QSSTV.

| | |
|---|---|
| Nombre | Robot 12 |
| Código corto, también en el AX.25 de modo | `R12` |
| VIS | `0x00` |
| Tamaño | 160×120, 120 líneas |
| Color | YCrCb 4:2:0, una crominancia por línea, alternando R−Y y B−Y |
| Línea | 100 ms (600 líneas/min). 120 × 100 ms = 12,0 s de imagen, más el VIS |
| Sync | 7,0 ms a 1200 Hz |
| Porch | 3,0 ms a 1500 Hz |
| Y | 60 ms |
| Croma | 30 ms en total, y después la línea cierra |

7 + 3 + 60 + 30 = 100. La tabla no parte los 30 ms. Se parten con la misma convención que el Robot 36 de este encoder (separador, porch de 1900 Hz, barrido), escalada para que sumen 30: el paquete de croma del Robot 36 es 4,5 + 1,5 + 44 = 50 ms, y 30/50 da separador 2,7 ms, porch 0,9 ms y barrido 26,4 ms. Línea par: separador a 1500 Hz y se manda R−Y. Línea impar: separador a 2300 Hz y se manda B−Y. La croma es el promedio del par de líneas, igual que en `ScanImage::line` para el Robot 36. Así entra en `Timing::Robot { alternating: true }` y no hace falta otro sintetizador: el encoder de Robot color que ya existe recorre la tabla.

El VIS de la imagen no está incluido en los 12 s, igual que los 36 s del Robot 36 de esta tabla son 240 × 150 ms de imagen y el header va aparte.

## Modelo que se guarda

El archivo nuevo lleva `format = 2` para poder rechazar un archivo de una versión futura. No se convierte un `format = 1` ni la forma vieja de programas y bloques: se rechaza. Un `format` mayor que 2 también se rechaza, como ahora.

### Proyecto

Se mantienen estación (indicativo, operador, QTH, locator), gráficos, una sola zona horaria (`utc` o `local`) que mueve el reloj, la grilla, los finales y los nombres de exportación, offset en segundos, pausa mínima, color de SSTV (rango completo o estudio), CW de canal (preámbulo sí/no, postámbulo sí/no, texto, WPM, tono) y el log al lado del proyecto. El bloque y el programa tienen gráficos propios: se pintan en cadena, primero el bloque, después el programa y al final el canal, cada uno sobre el cuadro anterior.

Entran:

- `freq_hz` del canal. `0` = no tocar la radio.
- AX.25 general: activado sí/no.
- AX.25 de modo (el preámbulo corto delante de cada pieza SSTV): activado sí/no. Es independiente del general, porque son las dos funciones distintas del pedido.
- Cuántos bloques próximos entran en la info: por cantidad, o por horizonte de horas. Default: 8 bloques, sin deduplicar.
- Deduplicar próximos: si está activo, entra solo el próximo bloque de cada programa distinto, y no entra ningún bloque del programa que se está transmitiendo ahora.
- Programas, bloques comunes, bloques de programa que tengan imágenes, activaciones.

Dejan de existir, sin ruta de lectura de los archivos actuales: la lista de imágenes colgada del programa, los slots semanales, la rotación, el CW propio del programa, la frecuencia por bloque y el intervalo viejo (`every_min`), los triggers viejos como única forma de activar, y `tx.fsk_id`. `tx.vox_preamble` se queda. El intervalo nuevo es `interval_min` del programa o del bloque común, no el campo viejo del bloque.

`listen` se queda con los interruptores de decodificar CW y DTMF, y con la velocidad y el tono de CW de recepción. Los disparadores viven en las activaciones.

### Programa

| Campo | Regla |
|---|---|
| `id` | Estable. |
| `name` | Lo heredan sus bloques y sale en el AX.25. |
| `enabled` | Apaga la transmisión de todos sus bloques. No los borra. |
| `weekdays` | Siete bool, lunes primero. Puede estar todo en falso: el programa existe y no ocupa la grilla. |
| `days` | Siete ventanas, lunes primero. Cada una tiene `start`, `end` e `interval_min`. El día apagado conserva la ventana. `end` anterior a `start` cruza medianoche. |
| `welcome` | Imagen opcional y su modo SSTV. |
| `bye` | Imagen opcional y su modo SSTV. |
| `ends` | Fecha y hora de término, opcional. Sin valor = infinito = `daysleft 0`. |
| `hue` | 0–359. Solo para las tarjetas. |
| `mode` | Modo SSTV opcional. Vacío = el del canal. |

Un programa se crea en la ventana de programas, no desde una tarjeta y no desde un doble click. Nace sin días y sin horas. No se inventa un horario. En esa ventana se elige, para cada día, si está prendido y su comienzo, fin e intervalo. Eso genera las tarjetas. El día y las horas no son campos del bloque: se heredan. Un doble click en la grilla vacía crea solo un bloque común, en esa fecha, con la hora del click y el intervalo del diálogo (empieza en 6). El fin propuesto es el inicio más ese intervalo, y cambia si se cambia el intervalo antes de confirmar. Si eso pisara algo, no se crea y se avisa qué tarjeta está en el camino. Arrastrar el borde de un bloque común cambia el horario de ese bloque. Arrastrar el borde de una tarjeta de programa no cambia el horario: se edita en la ventana de programas.

### Bloque

| Campo | Bloque de programa | Bloque común |
|---|---|---|
| `id` | sí | sí |
| `program` | el id del programa | vacío |
| `date` | la fecha de esa ocurrencia | la única fecha |
| `name` | no se guarda: es el del programa | propio |
| `start`, `end` | no se guardan: son los del día de la semana de esa fecha | propios, `HH:MM` |
| `interval_min` | no se usa: es el de ese día. Al guardar se reescribe con ese valor | propio. Default 6. `0` = un arranque |
| `weekdays` | no se guardan nunca en el bloque | no tiene |
| `enabled` | sí. Apaga solo esa ocurrencia | sí |
| `images` | la lista de abajo | la misma lista |
| `mode` | Modo SSTV opcional. Vacío = el del programa, o el del canal si no hay programa | el mismo |

El modo de una imagen, si está puesto, gana. Si no, gana el del bloque. Si el bloque tampoco tiene, gana el del programa. Si nadie lo puso, queda el del canal. La bienvenida y la despedida usan el suyo, después el del programa y después el del canal.

Un bloque de programa entra al archivo cuando tiene una imagen, gráficos propios o un modo SSTV propio. Cuando se queda sin las tres cosas, la entrada se borra y la tarjeta vuelve a ser el hueco calculado.

Un bloque común vacío sí se guarda. Borrarlo es una acción distinta de quitarle las fotos.

### Ítem de imagen

Lista ordenada. Cada ítem tiene id estable, path relativo al proyecto cuando el archivo vive dentro de la carpeta (la regla actual de `relativize`), `enabled`, modo SSTV opcional, y un rol:

- `normal`
- `pre` de una imagen normal (por id de esa imagen)
- `post` de una imagen normal (por id)

Invariante, rechazada por la función que edita la lista y también al cargar el archivo:

- Toda cortina apunta a una imagen normal de la misma lista.
- Las precortinas de una imagen van juntas, inmediatamente arriba, en el orden de transmisión.
- Las postcortinas van juntas, inmediatamente abajo.
- Una cortina no apunta a otra cortina.
- Dos imágenes normales no quedan unidas por cortinas. Son dos TX separadas por el intervalo, y el programa no tiene un estado que represente esa unión.

Al marcar una imagen como cortina se le pone `R12` si tiene color y `BW8` si es gris, como la decisión 22. El usuario puede cambiar el modo después. Desligar la vuelve normal y le deja el modo que tiene en ese momento.

### Activación

No cuelga de un programa ni de un bloque.

- `enabled`
- `name`
- `triggers`: lista. Cada uno es origen `cw` o `dtmf`, más el texto o la secuencia. Con que coincida uno, la activación corre (OR). El match de DTMF sigue aceptando la `#` final como opcional, como `dtmf_matches` hoy. El de CW sigue siendo el de `cw_matches`.
- `actions`: lista ordenada. Se ejecutan en orden, una TX detrás de la otra, mientras esta activación tenga el PTT. Acciones posibles:
  1. Transmitir CW (texto, WPM, tono).
  2. Transmitir AX.25 con el mismo formato de info de la grilla. Cada campo es opcional. Puede sumar un comentario.
  3. Transmitir DTMF (secuencia). Hay que escribir el generador: hoy solo existe el decodificador.
  4. Correr un comando en esta PC y transmitir su salida estándar en CW, en DTMF o en AX.25. Solo stdout. Tope de espera 10 s. Si no hay salida, no se transmite esa acción y queda en el log. El comando corre con el usuario del programa.
  5. Transmitir una imagen en un modo.
  6. Transmitir una imagen al azar de un grupo, en un modo.
  7. Pausar la grilla.
  8. Reanudar la grilla.
  9. Consultar una API y transmitir la respuesta en CW, en DTMF o en AX.25.

Las acciones 7 y 8 no arman audio y no toman el PTT. El resto, cuando les toca, espera un canal libre y después transmite. Si la lista es “pausar, después una imagen”, la agenda se apaga al instante y la imagen espera a que suelte el PTT quien lo tenga. Las acciones de imagen usan la pieza SSTV (VIS + imagen), con el preámbulo AX.25 de modo si esa opción del proyecto está activa, y con la ráfaga VOX delante si el proyecto la tiene activa. No arrastran la bienvenida de un programa ni el horario de un bloque.

La acción 9 usa el mismo `ApiSource` que una capa de texto: URL, camino JSON opcional (`current.temperature_2m`), y si el camino está vacío se usa el cuerpo tal cual. Se llama en el momento de la acción, con el tope de 8 s que ya tiene `dynamic::fetch`. No usa la caché de fondo de las capas: la activación existe para mandar la respuesta de esa consulta. Si falla, viene vacía o pasa de 500 caracteres, no se transmite esa acción y queda en el log. Los 500 son el tope que `fetch` ya aplica a un texto de capa.

### Sin conversor

No hay migración ni lector del formato viejo, ni un guardado que vuelva a escribir ese formato, ni copia `.bak`. El exportar audio del rango es otra cosa y sigue en la fase 8. Los tests de esta fase son el ida y vuelta TOML del formato nuevo, el rechazo de un `format` mayor, y el rechazo de un archivo con la forma actual (programa con `path` o con `slots`, bloque con `freq_hz`) sin reescribirlo.

## Calendario

Zona horaria: una sola, UTC o local (`GridTz`). El reloj de la ventana y la grilla usan la misma. Los huecos de horario de verano se saltan; los solapes de hora usan el primer instante, como `to_utc` ahora.

### Turnos dentro de un bloque

```
arranque = inicio_del_bloque + offset
mientras arranque < fin_del_bloque:
    este instante es un turno
    arranque = arranque + intervalo
```

El fin no es un turno. 14:00–14:30 con intervalo 6 y offset 0 da cinco turnos: 14:00, 14:06, 14:12, 14:18, 14:24.

A cada turno se le asigna la siguiente imagen normal habilitada, en orden de lista. Las que sobran no se asignan. Una imagen normal deshabilitada se saltea y no consume turno. Sus cortinas tampoco salen.

Después se mide la TX de ese turno (fase 5). Si no entra entera en el hueco hasta el próximo turno o hasta el fin, el que llegue antes, ese turno no se emite. La imagen no se corre al turno siguiente: el turno siguiente sigue siendo la imagen que le tocaba. Si el turno descartado era el último, la despedida se va con él y no pasa al anterior. Si era el primero, la bienvenida se va con él y no pasa al siguiente.

La pausa mínima se aplica sobre ese mismo hueco: el tiempo libre después de la duración tiene que alcanzar `min_pause_s` antes del próximo arranque, salvo que no haya próximo arranque dentro del bloque (el silencio hasta el fin del bloque no tiene que cumplir la pausa mínima; la pausa mínima protege el arranque siguiente).

### Qué ocupa la grilla

En una fecha dada, ocupan lugar, y por lo tanto pueden prohibir un solape:

- cada bloque común guardado, tenga imágenes o no, esté activo o no
- cada bloque de programa guardado
- cada hueco calculado de un programa (día de la semana marcado, fecha dentro de `ends` o sin `ends`, aunque no exista entrada)

No ocupan lugar los programas con la máscara vacía o sin horario.

Dos ocupaciones se pisan si están en la misma fecha y los rangos horarios se cruzan. Tocar el borde (`fin` de uno = `inicio` del otro) no es solape: a las 14:30 uno ya soltó el PTT y el otro puede tomarlo.

La cuenta no recorre de a un día “por si acaso” hasta el infinito. Dos programas se comparan por intersección de máscara de días y de rango horario, recortada por sus fechas de término. Un programa contra un bloque común se compara si el día de la semana de esa fecha está en la máscara, la fecha cae dentro de la vigencia, y los horarios se cruzan. Dos bloques comunes se comparan por fecha y horario. Desactivado o transparente no afloja la regla: si se dibuja, ocupa.

Si una edición de programa, un arrastre o un alta no pasa esta cuenta, no se aplica nada de esa edición. La grilla queda como estaba. El mensaje nombra la tarjeta que estorba (programa, fecha, horario). El usuario la mueve o la borra a mano.

El límite de “tres años hacia el futuro” es de navegación de la pantalla, no de la cuenta. Un programa sin fecha de término ocupa también el año cuatro, aunque la grilla no deje scrollear hasta ahí. Hacia el pasado la navegación no tiene límite.

### Días restantes

En la zona de la grilla, sea `último` la fecha calendario del último día en que el programa todavía tiene permiso, e `hoy` la fecha de esa misma zona.

- sin `ends`, o `último - hoy > 98` días: `0`
- `último == hoy`: `1`
- en otro caso: `(último - hoy) + 1`, que cae entre 2 y 99

Un programa vencido no se dibuja y no se anuncia.

### Arrastrar el borde

- Borde de un bloque de programa: cambia `start` o `end` de ese día de la semana. Se mueven las tarjetas de ese día, llenas y vacías, en todas las semanas. Los otros días no se mueven.
- Borde de un bloque común: cambia solo esa tarjeta.

Si el borde nuevo pisa algo, el arrastre no se confirma.

### Sacar un día del programa

Al desmarcar un día que tenía ocurrencias, se pregunta, y hasta elegir no se guarda el cambio:

- borrar los bloques futuros no vacíos de ese día de la semana, de ese programa
- convertir a bloques comunes, solo los no vacíos
- convertir a bloques comunes, incluyendo vacíos, hasta un horizonte que el usuario elige, con tope a 3 años desde hoy

“Convertir” copia a cada bloque común la fecha, el horario del programa y, si existían, las imágenes. El bloque común queda independiente: después el programa ya no lo mueve. Los pasados no se tocan: el pedido dice futuros.

### Borrar un programa

Se pregunta, y hasta elegir no se borra:

- borrar todos sus bloques
- dejar los no vacíos (convertidos a comunes) y borrar el resto
- dejar todos, vacíos incluidos, hasta un horizonte elegido (tope 3 años), convertidos a comunes

Dejar = convertir a bloque común. El programa después no existe.

### Doble click en hueco vacío

Pregunta: bloque común, o programa. El programa, por defecto, repite solo el día de la semana del click. El bloque común no repite. Los dos proponen el horario del click hasta el click más el intervalo. Si un bloque futuro, guardado o solo calculado, pisa ese horario, no se crea y se avisa.

## Secuencia de una transmisión

Esta es la TX de un turno de grilla. Las piezas opcionales se omiten enteras, junto con su preámbulo de modo, cuando esa opción está apagada o esa imagen no existe.

Si el proyecto tiene el VOX activo, antes del paso 1 va la ráfaga de 800 ms. También va delante de una TX instantánea y de cada acción que transmita. No se numera acá porque no es contenido del bloque.

1. CW preámbulo del canal, si está activo y el texto no está vacío.
2. AX.25 de info general, si está activo.
3. Si este turno es el primero del bloque y el programa tiene bienvenida: AX.25 de modo de esa imagen, VIS de ese modo, y después la SSTV de bienvenida.
4. Por cada precortina, en orden: AX.25 de modo con marca de bumper y el modo SSTV, VIS de ese modo, y después la SSTV de la cortina.
5. AX.25 de modo de la imagen normal, VIS de ese modo, y después la SSTV de la imagen.
6. Por cada postcortina, en orden: AX.25 de modo con marca de bumper y el modo SSTV, VIS de ese modo, y después la SSTV de la cortina.
7. Si este turno es el último del bloque y el programa tiene despedida: AX.25 de modo de esa imagen, VIS de ese modo, y después la SSTV de despedida.
8. AX.25 de info general, el mismo texto que en el paso 2, si está activo.
9. CW postámbulo del canal, si está activo y el texto no está vacío.

El ejemplo del pedido, escrito en vertical como va a verse en la lista:

```
CW 5s
AX.25 info 2s
AX.25 modo 0.25s
VIS 0.9s
bumper 12s
AX.25 modo 0.25s
VIS 0.9s
img 114s
AX.25 info 2s
CW 5s
```

El ejemplo original del pedido no listaba el VIS. Acá sí, porque ahora entra en toda SSTV y ocupa aire (el header de `vis_segments` dura 0,91 s; en Scottie se le suma el sync de 9 ms). Esos segundos siguen siendo un ejemplo de cómo se muestra, no los tiempos fijos del programa. Los tiempos salen de la duración real de cada pieza. Debajo de cada TX, la suma. Entre un turno y el siguiente, una fila de silencio con lo que falta hasta el próximo arranque, por ejemplo `silencio 8m 10s`. Si el offset corre el primer arranque dentro del bloque, la primera fila es ese silencio.

Las filas de CW, AX.25, VIS, bienvenida, despedida y silencio no se editan y no se arrastran. Existen para ver si CW + imagen + cortinas se pasan del tiempo del turno. Las filas de imagen y de cortina sí son los ítems de la lista. El VIS no es un interruptor: si hay una SSTV, la fila está.

El arranque programado es el instante en que el PTT se baja. Después corre el `lead_ms` ya existente, después el audio, después el `tail_ms`. Esos tres entran en la duración que tiene que caber. El corte de fin de bloque los incluye: si la hora llega durante el tail, el tail se corta igual.

El tab instantáneo y las activaciones no usan los pasos 3 y 7 (bienvenida y despedida de programa), salvo que el instantáneo esté armando “una TX de este bloque” para prueba: en ese caso usa la secuencia completa de la imagen que el usuario eligió, con las cortinas de esa imagen, y puede cambiar el modo de las cortinas respecto de la imagen para probar. El corte por horario no se le aplica.

### AX.25 de info general

Texto UTF-8, una línea por campo, terminador `\n`. Versión de texto `1` para que un cliente imaginario pueda ignorar un formato futuro.

```
ESTIVI 1
FREQ 145825000
CALL LU1ABC
NAME Noticias
RANGE 14:00-14:30
DAYS Mon,Tue,Wed,Thu,Fri
DAYSLEFT 12
NEXT Noticias|14:00-14:30|Mon,Tue,Wed,Thu,Fri|12
NEXT Clima|18:00-18:15|Sat,Sun|0
```

| Línea | Cuándo va | Contenido |
|---|---|---|
| `ESTIVI 1` | siempre | Marca y versión. |
| `FREQ` | siempre | Hertz del canal. |
| `CALL` | siempre | Indicativo. Sirve de identificativo y para que otra estación reconozca quién ocupa el canal. |
| `NAME` | siempre | Nombre del programa, o el nombre del bloque común. |
| `RANGE` | siempre | Horario del bloque, `HH:MM-HH:MM`, el heredado si es de programa. |
| `DAYS` | solo si el bloque es de un programa | Días en que se repite, en inglés corto, lunes a domingo: `Mon`…`Sun`. Un bloque común no la lleva: no tiene días de semana, y el cliente usa la ausencia para no repetirlo en su grilla. |
| `DAYSLEFT` | solo si el bloque es de un programa | El número definido arriba. |
| `NEXT` | cero o más, en orden cronológico | `nombre\|horario\|días\|daysleft`. En un bloque común los días van vacíos y `daysleft` va `0`: `Nombre\|18:00-18:15\|\|0`. |

El tope es el de la configuración del proyecto: N bloques, o los que arranquen dentro de las próximas H horas, lo que el proyecto tenga elegido. Con deduplicar activo, de cada programa entra solo el próximo bloque, y no entra ninguno del programa de esta TX. El bloque que se está transmitiendo no entra en `NEXT`.

Una acción AX.25 de activación usa las mismas líneas, todas opcionales, y puede agregar al final:

```
COMMENT texto libre en una línea
```

### AX.25 de modo

Trama aparte, corta, inmediatamente antes de la pieza SSTV. No va mezclada con la info general.

```
ESTIVI MODE R36
ESTIVI MODE R12 BUMPER PRE
ESTIVI MODE BW8 BUMPER POST
ESTIVI MODE M1 WELCOME
ESTIVI MODE R24 BYE
```

El código de modo es el `short` que ya existe en `crates/core/src/modes.rs` (`M1`, `R36`, `BW8`, `PD120`, …). `BUMPER PRE` / `BUMPER POST` es la marca de cortina. `WELCOME` y `BYE` distinguen las imágenes del programa.

Las duraciones de ejemplo `2s` y `0.25s` no se hardcodean. La duración es la del audio AFSK real más un silencio de cola de 100 ms para que el decodificador cierre la trama. Los tests comparan contra esa cuenta, no contra 0,25.

## Lista de imágenes de un bloque

Es la lista de una ocurrencia, no la de un programa.

- Arrastrar reordena.
- **Linkear arriba** convierte el ítem elegido en postcortina de la imagen normal de arriba. Si el elegido ya tiene cortinas propias, se rechaza: primero hay que desligarlas, para no fusionar dos TX.
- **Linkear abajo** lo convierte en precortina de la imagen normal de abajo, con la misma restricción.
- Se puede desligar. La cortina vuelve a ser imagen normal, colocada justo fuera del grupo, sin quedar puente entre dos normales.
- Una imagen normal puede tener varias precortinas y varias postcortinas.
- El grupo (precortinas + normal + postcortinas) se ve unido por una cadena o una línea.
- Arrastrar la imagen normal, o el canal de la cadena, mueve el grupo entero y conserva el orden interno.
- Arrastrar una cortina solo se puede dentro de su lado: las pre siguen arriba, las post siguen abajo. No se puede soltar una cortina entre dos grupos.
- Al soltar archivos o al usar Agregar, el orden es el que entrega el selector. Al final de cada nombre se prueba el sufijo:

  | Sufijo | Efecto |
  |---|---|
  | `-bp`, `-BP` | Postcortina de la imagen normal anterior. |
  | `-bn`, `-BN` | Precortina de la imagen normal siguiente. |

  Si `-bp` queda primero y no hay imagen anterior, o `-bn` queda último y no hay siguiente, el archivo se agrega como imagen normal y se avisa. No se inventa una pareja.

La cadena es un dato (el rol y el id al que apunta), no solo un dibujo. La fase 7 prueba el invariante sin abrir la ventana. La fase 13 lo dibuja.

### Importar un programa desde carpetas

Botón, no drop. El usuario elige una carpeta raíz. Adentro se leen solo directorios cuyo nombre sea `block` seguido de un entero, sin distinguir mayúsculas: `block01`, `block1`, `BLOCK02`. El entero es la posición.

- `block01` = la próxima ocurrencia de este programa con arranque `>= ahora`, sea hueco o ya tenga fotos.
- `block02` = la de después.
- Si en la raíz solo está `block02`, se llena la segunda y la primera no se toca.

Dentro de cada carpeta, los archivos de imagen se ordenan con orden natural (`2` antes de `10`) y se les aplican los mismos sufijos. Esa ocurrencia pasa a existir en el archivo porque ahora tiene imágenes. Una carpeta con número que no corresponde a ninguna ocurrencia vigente (el programa termina antes) se informa y se salta.

No se crean ocurrencias nuevas más allá de las que el programa ya define. La carpeta no agrega días ni horarios.

## Exportar audio

Es una acción distinta de guardar el proyecto. No escribe el `.estivi`.

El usuario elige:

- rango: día y hora de inicio, día y hora de fin
- forma del archivo (una de estas cinco)
- formato: FLAC, MP3 VBR máximo, o AAC VBR máximo
- frecuencia de muestreo. Default `44100`. Opciones: `11025`, `22050`, `44100`, `48000`

El audio se sintetiza directo a esa frecuencia, que es lo que el sintetizador ya sabe hacer (`TxPlan::synth` recibe `fs`). No se genera a 44100 para después remuestrear.

**Aviso de calidad.** El default es FLAC a 44100, sin pérdida. En el momento en que el usuario se aleja de eso (otra frecuencia, o MP3, o AAC) se muestra, antes de exportar, que está comprobado que cualquier ajuste por debajo de 44100 sin pérdida introduce cambios leves en la imagen que se decodifica, que no aseguran una transmisión perfecta, y que igual es usable. El aviso aparece cada vez que se cambia el control, no una sola vez para siempre.

El rango se respeta siempre. La duración exportada, sumando todos los archivos, es la del rango. El instante 0 del conjunto es el inicio del rango: ahí es cuando hay que darle play.

- Si el inicio del rango cae en silencio, el primer audio es ese silencio, aunque no coincida con el inicio de un bloque.
- Si el inicio del rango cae en el medio de una TX, entra el resto de esa TX, no lo que ya hubiera sonado antes del rango.
- Una TX que el motor hubiera cortado por fin de bloque se exporta cortada igual, y después silencio.
- El fin del rango corta lo que esté sonando.

El nombre describe cómo reproducirlo. La hora del nombre es la hora de la grilla en que hay que darle play a ese archivo, en la zona horaria de la grilla, con el desplazamiento numérico (`-0300`, `Z`). El orden de reproducción de una carpeta es el orden alfabético común, así que el índice numérico va primero, con ceros, y el texto después.

Archivo entero (un solo archivo, play en el inicio del rango):

```
20261026T141000-0300_entero.flac
```

Partido en TX y silencio. Cada TX es un archivo (toda la secuencia de ese turno) y cada silencio entre medias es otro, para que cualquier reproductor, al pasar al archivo de silencio, espere esa media hora:

```
001_20261026T140000-0300_tx.flac
002_20261026T140200-0300_silencio.flac
003_20261026T143000-0300_tx.flac
```

Partido también entre imágenes y cortinas. Misma idea, más fino: cada pieza es su archivo, en orden, con su hora de inicio. Etiquetas de texto después del índice: `cw`, `ax25`, `bienvenida`, `bumper`, `img`, `despedida`, `silencio`.

```
001_20261026T140000-0300_cw.flac
002_20261026T140005-0300_ax25.flac
003_20261026T140007-0300_bumper.flac
004_20261026T140019-0300_img.flac
```

Partido cada X. El usuario elige X. Son rebanadas de esa duración sobre la línea de tiempo ya armada, silencio incluido. La última es más corta si el rango no da un múltiplo. El nombre lleva índice y hora de inicio de la rebanada (`trozo`).

Por bloque. Un archivo por cada bloque que toca el rango, con el silencio interno entre sus turnos, recortado al rango. Entre bloques, un archivo de silencio, también con índice, para que la carpeta siga alineada al reloj. Etiqueta `bloque` más el nombre, saneado para un nombre de archivo.

El índice crece a 4 dígitos si hace falta (`1000_…`), siempre con el mismo ancho en esa exportación, para que `999` no quede después de `1000` en orden alfabético.

FLAC, MP3 y AAC se codifican dentro del ejecutable. No se exige ffmpeg ni otra herramienta instalada. AAC se entrega en `.m4a` para que un reproductor común lo abra. MP3 en `.mp3`. FLAC en `.flac`. “VBR máximo” es el preset más alto del codificador que se embeba. En la fase 8 se eligen las bibliotecas mirando que compilen en Linux y en Windows y que la licencia cierre con la del proyecto; si AAC no se puede embeber así, se frena esa opción y se avisa en vez de depender de un programa externo.

## Reloj

Configuración de la PC (`estivi.toml`), no del canal.

- En el primer arranque se ofrece activar la sincronización exacta por internet, cada X horas. Si el usuario la rechaza, no se vuelve a preguntar. `time_sync_prompted` queda en verdadero.
- En Equipo se puede activar después. Servidor editable, default `pool.ntp.org`. Período editable, default 6 h.
- Se consulta un NTP, se guarda el corrimiento respecto del reloj del sistema y la hora de la última consulta. La grilla, el corte de PTT, el “ahora” del importador `block01` y el rango de exportación usan `reloj del sistema + corrimiento`.
- Si el servidor no responde, se sigue con el último corrimiento y se muestra el error. No se bloquea la transmisión.

## TX instantáneo

Tab propio.

- Se puede usar con la grilla armada, mientras no haya una TX de grilla en el aire. Si la hay, los controles de transmitir quedan deshabilitados y se dice por qué.
- Sirve para probar una sola TX: CW sí/no, AX.25 de info sí/no, imagen, y cortinas con un modo distinto del de la imagen.
- Puede tomar como punto de partida una imagen de un bloque (con sus cortinas) para no armarla de cero.
- No lo corta el fin del bloque. Puede salir fuera de cualquier horario.

## Activaciones

Tab propio, al lado del instantáneo, no dentro de un programa y no en la grilla.

La especificación del objeto está en el modelo. En el motor:

- Escuchar CW y DTMF sigue como ahora.
- Un match lanza la activación si está habilitada.
- Si el canal está ocupado (grilla, instantáneo u otra activación), queda encolada una vez. Un segundo match de la misma activación, mientras la primera espera o corre, no acumula otra copia.
- Al tomar el canal, corre sus acciones en orden.
- El fin de horario de un bloque no la corta.
- Cada acción queda en el log con su resultado.

## Capa QR

Es una capa más del editor de gráficas del canal, al lado de las de texto y de imagen (`LayerKind` en `crates/core/src/graphics`). Se arrastra y se cambia de tamaño como las otras. Se pinta en cada cuadro que ya lleva la gráfica del canal: imagen normal, cortina, bienvenida y despedida. No es una capa de una sola foto.

El contenido es uno de estos tres, y se elige cuál:

| Origen | Qué se codifica |
|---|---|
| Texto | Una plantilla. Se expande con `dynamic::expand` en el momento de armar el cuadro, con los datos de la TX que está por salir. |
| Comando | Un comando de esta PC, igual que la acción 4 de una activación. La línea del comando se expande con las mismas variables; la salida estándar no se vuelve a expandir, para que un `{` impreso por el programa quede literal. Solo stdout. Tope 10 s. Corre con el usuario de la app. |
| API | La respuesta de un `ApiSource` (URL, camino JSON, refresco). Es el cuerpo entero del QR, sin pasar por `expand`, así un JSON conserva las llaves. |

En origen texto la capa puede tener además una API, y `{api}` dentro de la plantilla inserta esa respuesta, igual que una capa de texto hoy. Los otros dos orígenes no usan la plantilla: el QR es la salida del comando o la respuesta de la API.

Variables nuevas, además de las que ya existen (`{call}`, `{utc}`, `{program}`, `{freq}`, `{mode}`, …). Valen en el QR y en cualquier capa de texto, porque es el mismo `expand`. Salen de la TX que se está armando, con las mismas reglas que el AX.25 de info:

| Variable | Texto |
|---|---|
| `{block}` | Nombre del bloque o del programa. |
| `{range}` | `14:00-14:30` |
| `{days}` | Días de la semana si es un programa. Vacío en un bloque común. |
| `{daysleft}` | El número ya definido, o vacío si el bloque no es de un programa. |
| `{next}` | Los próximos bloques que entrarían en el AX.25, una línea por bloque, con el mismo tope y la misma deduplicación del proyecto. |

Si el comando tarda o falla, o la API no tiene respuesta, o el texto no entra en un QR (versión 40), esa capa no se dibuja, el error va al log y el resto del cuadro se transmite igual. La API de una capa usa la caché de fondo que ya refresca los textos (`refresh_s`, tope 8 s): la TX no espera a la red si hay un valor reciente. Si nunca hubo respuesta, se intenta una vez al armar el cuadro y, si no llega, se sigue sin el QR.

El dibujo es módulos negros sobre blanco, con la zona quieta de 4 módulos adentro del rectángulo de la capa, a píxel entero (sin suavizado), para que sobreviva el SSTV. Corrección de error por defecto M; se puede elegir L, M, Q o H. La opacidad de la capa sigue valiendo. En el editor, si en el modo SSTV que se está previsualizando el módulo quedaría de menos de 2 píxeles, se avisa: en Robot 8 o en un QR largo el teléfono no va a poder leerlo. Se dibuja igual.

La matriz la genera la crate `qrcode`, dentro del binario. El cuadro lo sigue armando `compose::frame` antes de codificar el SSTV.

## Interfaz (fase 13, última)

Se rehace la ventana completa. No se parchea el tema claro sobre la distribución actual. Los dos temas tienen que leerse: texto oscuro sobre fondo claro, texto claro sobre fondo oscuro, incluidos los widgets estándar (`Palette.color-scheme` alineado con `Theme.light` en el `init` y en cada toggle; el bug del botón Light/Dark no se “arregla solo” en una fase anterior).

Aspecto de estación, no de formulario genérico: la grilla es el centro, el aire y el que sigue son visibles sin cazarlos en otra pantalla, y los ajustes pesados viven en paneles y no en la tarjeta. Se mantienen inglés y español en todos los textos nuevos. El editor de gráficos del canal se queda (capas, campos `{call}`, `{utc}`, y el resto) y suma la capa QR de la sección [Capa QR](#capa-qr): origen texto, comando o API, y las variables nuevas de grilla también en las capas de texto. El modo de una imagen normal sigue la cascada imagen, bloque, programa, canal. Las cortinas usan la misma cascada. La bienvenida y la despedida usan la suya, el programa y el canal. El panel Next muestra la próxima transmisión que va a salir (la primera con hora futura), con los gráficos de ese bloque, de su programa y del canal. Se vuelve a dibujar cuando cambia esa transmisión o cualquiera de esos gráficos.

La grilla:

- Zoom de mes, de semana y de día. Meses hacia adelante hasta 3 años desde hoy. Hacia atrás, sin límite.
- Tarjetas. El color es el matiz del programa. Un bloque común usa un gris neutro.
- Llena y activa: opaca.
- Desactivada, por el bloque o por el programa: más transparente. Si la apagó el programa, además va en gris. Menos transparente que un vacío.
- Vacía (hueco de programa, o común sin fotos): la más transparente. Sigue ocupando y sigue prohibiendo solapes.
- Doble click en vacío: el diálogo de bloque común, con el rechazo si algo futuro está en el camino. Un programa se crea en su ventana.
- Arrastrar el borde de un bloque común edita su rango. El borde de una tarjeta de programa no edita el horario: el día y las horas se heredan y se cambian en la ventana de programas.
- Un bloque común se puede borrar. Un bloque de programa no se borra como tarjeta suelta: se vacía (y desaparece del archivo) o se desactiva.

La lista del bloque muestra los grupos encadenados, los botones de linkear arriba, linkear abajo y desligar, las filas no editables de CW / AX.25 / bienvenida / despedida, el total de cada TX y el silencio entre turnos.

Diálogos nuevos, con las preguntas de este documento y sin un default silencioso: exportar, días del programa, borrar programa, TX instantáneo, activaciones, aviso de calidad de audio, oferta de reloj la primera vez.

Frecuencia: un campo del proyecto. No aparece en el bloque.

Al cerrar esta fase se actualiza el `README.md` para que describa canal, programa, bloque, cortina, exportación y activaciones como quedaron. Hoy el README dice que el programa es la lista de fotos y que el bloque apunta a un programa: eso va a ser falso.

## Qué no entra en esta tanda

- No se construye el cliente que escucharía el AX.25 y armaría su propia grilla. Solo se emite el texto para que ese proyecto, si existe, pueda leerlo.
- No se fija una ventana de 3 minutos ni un cupo de estaciones. Offset e intervalo los pone el usuario.
- No se rehace el editor de gráficos desde cero. Se le agrega la capa QR y las variables de grilla.
- No se abre un servidor web.
- No se cambia el reloj del sistema operativo.
- No hay FSK ID. El identificativo digital de la salida es el AX.25, y el CW si está activo.
- No se convierte un proyecto guardado con el formato de esta alfa. Si más adelante hay archivos que importen, el conversor se diseña contra esos archivos, no ahora.

## Fases, en checklist

### Fase 1 — Robot 12 color

- [ ] `Mode::Robot12` en la tabla de `modes.rs`, código `R12`, VIS `0x00`, 160×120, línea de 100 ms con los tiempos de la sección [Robot 12 color](#robot-12-color).
- [ ] El camino de `Timing::Robot` con `alternating` lo transmite sin un sintetizador nuevo.
- [ ] Test: 120 líneas × 100 ms = 12,0 s de imagen, y el VIS no está metido en esos 12 s.
- [ ] Aparece en el combo de modos. La pantalla no se rediseña en esta fase.

### Fase 2 — Formato nuevo

- [ ] Tipos nuevos en `crates/core/src/project.rs`: proyecto, programa, bloque, ítem, activación (con pausar y reanudar en la lista de acciones).
- [ ] Invariante de cortinas en una función pura, con tests de las uniones prohibidas.
- [ ] `format = 2`. Sin conversor. Un archivo con la forma vieja o con `format` mayor se rechaza y no se reescribe.
- [ ] Ida y vuelta TOML del formato nuevo.
- [ ] La app compila contra el modelo nuevo. La pantalla vieja puede mostrar los datos de forma provisoria y fea. No se rediseña.

### Fase 3 — Calendario

- [ ] Turnos de un bloque con offset, intervalo y fin exclusivo.
- [ ] Ocurrencias virtuales y guardadas en un rango de fechas.
- [ ] Solape entre programas, entre programa y bloque común, entre comunes. El borde exacto no pisa.
- [ ] Rechazo de una edición que pisa, sin aplicar una parte.
- [ ] `daysleft` en los bordes: hoy, mañana, 99, 100, sin fecha, vencido.
- [ ] Asignación de imágenes a turnos, sobrantes ignoradas, deshabilitadas salteadas.
- [ ] Tests de medianoche y de horario de verano, junto a los que ya tiene `schedule.rs`.

### Fase 4 — AX.25

- [ ] Armar la trama UI con los textos de info y de modo definidos arriba.
- [ ] AFSK fijo a 1200 baudios, sin parámetro de velocidad. Duración incluida la cola de 100 ms.
- [ ] Roundtrip de los campos, incluido `NEXT` con deduplicado y con tope por cantidad o por horas.
- [ ] Un bloque común no emite `DAYS` ni `DAYSLEFT`.

### Fase 5 — La TX y el corte

- [ ] Lista de piezas de un turno, con bienvenida solo en el primero y despedida solo en el último.
- [ ] La ráfaga VOX, si el proyecto la tiene activa, queda delante y entra en la duración. No es una pieza editable del bloque.
- [ ] Cada pieza SSTV lleva su VIS delante. No hay FSK ID ni opción para activarlo.
- [ ] Duración que incluye lead, audio y tail.
- [ ] Un turno que no entra no se agenda; la despedida no retrocede.
- [ ] El caso 14:00–14:30 con cinco imágenes, y el caso 14:00–14:33 con una sexta que entra solo si dura menos que el resto.
- [ ] Reloj de corte: PTT y audio se sueltan antes del instante de fin, con menos de 1 ms de margen, y no un segundo antes. Test con un fin sintético, sin radio.
- [ ] El camino de TX instantáneo no recibe ese corte.

### Fase 6 — Motor de grilla

- [ ] El hilo de `engine.rs` agenda con la fase 3 y transmite con la fase 5.
- [ ] Un solo PTT. La grilla en el aire bloquea al instantáneo. Una activación de audio todavía no existe en esta fase: el gancho de “canal ocupado” sí.
- [ ] Atraso mayor a 90 s sigue saltando el turno.
- [ ] Pausa mínima sigue pudiendo correr un arranque, y si ya no entra, se salta.
- [ ] El log gana columnas al final (id de bloque, id de programa, cortado sí/no) sin reordenar las viejas.
- [ ] CAT usa la frecuencia del proyecto.
- [ ] Al armar el cuadro, una capa QR se resuelve (texto con variables de grilla, comando, o API) y se dibuja en negro sobre blanco. Si no hay payload o no entra, el cuadro sale igual, sin esa capa.

### Fase 7 — Cortinas e importación

- [ ] Linkear arriba, linkear abajo, desligar, mover grupo, reordenar dentro del lado. Todo rechazando el puente entre dos normales.
- [ ] Sufijos en un lote agregado, con el caso del `-bp` inicial y el `-bn` final avisados y convertidos a imagen normal.
- [ ] Importar `blockNN` sobre la enésima ocurrencia desde ahora, con `block02` sola dejando la primera intacta.
- [ ] Orden natural de archivos dentro de la carpeta.
- [ ] Al llenar un hueco, aparece la entrada en el archivo. Al vaciar un bloque de programa, desaparece.

### Fase 8 — Exportación

- [ ] Las cinco formas de partir, sobre el mismo rango, con la suma de duraciones igual al rango.
- [ ] Silencio inicial cuando el rango no cae en un arranque. Recorte cuando cae en el medio de una TX. Corte de fin de bloque igual al del aire.
- [ ] Nombres con índice primero, hora de play, y orden alfabético = orden de reproducción.
- [ ] FLAC, MP3 VBR máximo, AAC VBR máximo, dentro del binario.
- [ ] El aviso de calidad se dispara al alejarse de FLAC 44100. El texto es el del pedido, en los dos idiomas.
- [ ] Frecuencias: 11025, 22050, 44100 y 48000.

### Fase 9 — Reloj

- [ ] Cliente NTP, corrimiento guardado en `estivi.toml`, sin tocar el reloj del sistema.
- [ ] Oferta solo la primera vez. Configurable después: servidor y período.
- [ ] La grilla usa el reloj corregido. Si la consulta falla, queda el último corrimiento.

### Fase 10 — Activaciones

- [ ] Ejecución ordenada de las nueve acciones. Pausar y reanudar no toman el PTT y no esperan canal libre.
- [ ] Generador DTMF, que hoy no existe.
- [ ] Comando local: stdout, 10 s, sin salida = no se transmite esa acción.
- [ ] API: la respuesta se transmite en CW, DTMF o AX.25. Fallo, vacío o más de 500 caracteres = no se transmite esa acción.
- [ ] Cola de una sola copia de las acciones que sí transmiten, mientras el canal está ocupado. El fin de bloque no las corta.
- [ ] Match CW y DTMF como los actuales.
- [ ] VOX delante de cada acción que transmite, si el proyecto lo tiene activo.

### Fase 11 — TX instantáneo

- [ ] Armar una TX de prueba con CW, AX.25 de info y cortinas en otro modo.
- [ ] Poder partir de una imagen de un bloque.
- [ ] Negado, con motivo, mientras la grilla tiene el PTT. Permitido con la grilla armada y en silencio.
- [ ] Sin corte por horario.
- [ ] VOX delante, con la misma opción del proyecto.

### Fase 12 — Consultas para la pantalla

- [ ] Ventana visible de mes, semana y día, con tarjetas llenas, desactivadas y vacías, y el “ahora”.
- [ ] Futuro limitado a 3 años en la consulta de navegación. Pasado sin límite.
- [ ] Misma función de solape que la fase 3, para que la pantalla no tenga otra regla.

### Fase 13 — Interfaz

- [ ] Sistema visual nuevo, tema claro y tema oscuro, widgets estándar incluidos. Verificar leyendo la ventana, no solo el color en el código: el fallo conocido es texto blanco sobre fondo claro.
- [ ] Grilla calendario con zoom, arrastre de bordes, doble click, estados de opacidad y el gris del programa desactivado.
- [ ] Lista del bloque con cadena, botones, filas de tiempo y totales.
- [ ] En el editor de gráficas, capa QR: elegir texto, comando o API, corrección de error, aviso si el módulo queda chico, y botón de probar para comando y API. La plantilla de texto se previsualiza con la grilla actual.
- [ ] Diálogos de exportar, programa, borrar, instantáneo, activaciones, calidad y reloj.
- [ ] Frecuencia solo en el proyecto.
- [ ] Textos en inglés y en español.
- [ ] `README.md` reescrito a este modelo.
- [ ] Recorrer en la ventana: crear un programa en su ventana y ver las tarjetas, rechazar un solape, marcar una cortina, ver el total de la TX, desactivar un programa y ver las tarjetas, exportar un rango corto a FLAC, abrir el tab instantáneo con la grilla armada y comprobar que se niega mientras hay una TX.

## Pedido original

Copiado completo, para que ningún detalle quede solo en el chat.

> -la frecuencia no es por programa, es por proyecto (un proyecto es un canal)
>
> -una opción para exportar proyecto (no guardar), en audio flac. que se pueda elegir un rango horario(diahora inicio-diahora fin). que se pueda elegir si archivo entero, separado en [tx-01.flac, silencio-01.flac (para que cualquier reproductor espere media hora), tx-02.flac, etc...], o separar entre imagenes y bumpers tambien, o separado cada X tiempo, o por bloque. siempre respetar el rango horario, incluir silencio al inicio si es necesario para respetarlo aunque el inicio de rango no coincida con inicio de bloque. el nombre de archivo debe describir bien como reproducirlo. si es entero, fecha y hora desde la cual hay que ponerle play (inicio en grid). si está dividido tambien, cada archivo con su hora de inicio (pero siempre ordenar numericamente antes de caracteres de texto para cuidar orden de reproduccion). elegir sample rate (default 44100, opciones 11025 22050 44100 4800). ofrecer mp3 vbr max y aac vbr max. al cambiar calidad default, avisar que está comprobado que cualquier setting abajo de 44100 lossless introducirá cambios levmente visuales en la imagen resultante que no aseguran una transmisión perfecta pero de todas formas sí es usable.
>
> -ax25 pre/post image for channel info: frequency, next blocks/programs and hourrange/weekdays, etc. image mode (R36,RBW8, M1, etc) then goes apart and is image preamble, not info general tx pre/postamble, because of the next point:
>
> -programs can have images marked as bumper/cortina, than will go right after (or right before) one normal image and once marked will default to robot12/bw8 depending on image colors with an ax25 preamble after normal image with bumper mark and bumper sstv mode
>
> -so ax25 does two things: general tx pre/postamble and image mode preamble. a general tx is:
> CW preamble -> (ax25 general tx info preamble) -> (ax25 sstv mode of optional program welcome + img) -> (ax25 sstv mode of prebumper -> actual sstv prebumper) x (prebumper count) -> (ax25 sstv mode of img -> actual sstv image) -> (ax25 sstv mode of postbumper -> actual sstv postbumper) x (postbumper count)-> (ax25 sstv mode of optional program byebye + img) -> (ax25 general tx info postamble) -> CW postamble
>
> -la ui light sigue con texto blanco y no se lee nada. rehace la ui (toda) mas profesionalmente. la ui es lo ultimo, primero toda la lógica y features (el backend, aunque no estemos en web)
>
> -enable importing blocks and auto marking files with -bp -bn or -BP -BN last in the name, as bumpers for previous image (postamble) or next image (preamble). when dropping several files to the program images list or hitting add
> -enable importing (no drop, just button) a full program reading a folder hierarchy of root/block[01]/(images-bumpers)
>  applying the images to the blocks of the program being block 01 the next one in grid from current time. if user folder is only 02, import to 2nd next block, not next one. respect the number.
>
> en la lista de imagenes de un bloque, poder draggear para reordenar. botones para "linkear arriba" (convertir en postbumper de imagen de arriba) o "linkear abajo". unir con una linea o cadena las linkeadas y que drageen/reordenen juntas. permitir tener mas de un prebumper o postbumper linkeado a una imagen normal. los bumpers son relativos a una imagen normal y por naturaleza no se permite (ni el software estará preparado para que sea posible) unir dos imagenes normales a traves de bumpers, ya que dos imagenes normales son dos tx diferentes espaciados por intervalos.
> en la lista de imagenes incluir items "no editables, ni ordenables manualmente) que sean (si activados en proyecto) los cw y ax25, para ver el tiempo que le suman a cada tx (y por ejemplo ver si entre cw, img y bumper me paso de 3 minutos).  ejemplo: [CW 5s],[AX.25 info 2s], [ax25mode 0.25s],[bumper 12s], [ax25mode 0.25s],[img 114s],[ax25 info 2s], [cw 5s] (pero vertical, en los items de la lista). y para cada tx sumar el tiempo total de tx.
> entre intervalos, item con [silencio 8m 10s] (por ejemplo)
>
> -bloques y programas: el aproach actual está mal
> actualmente no se entiende para que sirve cada uno. la organizacion va a ser una sola:
> habrá una grilla tipo google calendar (que no permita superposicion de bloques, tampoco desde config de bloque/programa) (con zoom mes, semana, dia) que tendrá bloques (cards)
> un bloque es una lista de imagenes que se tx cada X tiempo, algunas marcadas como bumpers. el bloque no tiene offset de tiempo propio. eso es general del proyecto y debe seguir asi y los bloques respetarlo.
> los programas tienen bloques. los bloques pueden pertenecer a un programa o no. al crear un bloque hay opciones "single block" y "Blocks Program" para armar un programa.
> los bloques tienen:
> -solo si pertenecen a un programa para heredarlos:
> --nombre
> --weekdays
> --rango horario
> -propio:
> --rango horario (si no hereda)
> --imagenes/bumpers.
> --activo/inactivo (tx activado o no, porque si pertenece a un programa no puede eliminarse como un bloque "comun" (porque depende de su programa) pero puede desactivarse el tx de un bloque único manualmente. disponible tambien para bloques "comunes" sin programa padre para no borrarlos)
>
> un programa puede:
> -tener un nombre (y heredarlo a los bloques)
> -tener una definicion de rango horario y dias que se repite, para incluir en el ax25 info de cada tx de sus bloques que heredan esta info (los bloques solo saben su horario, los weekdays los heredan si pertenecen a un programa, pero sino no tienen weekdays/meses, no son propios de su objeto. si los heredan los informan.)
> -tener una imagen de bienvenida y despedida con sus digimodos cada una. se insertan en el caso de bienvenida, luego de cw y ax25 info, antes de la primer imagen del bloque. para despedida, luego de la ultima imagen del bloque. lo dejé en el flow mas arriba igual. esto se aplica en cada bloque del programa, no solo en el primer bloque del programa y en el ultimo. o sea, un bloque de 5 imagenes (+bumpers) de 30 min que transmite 14:00 14:06 14:12 14:18 14:24, se inserta solo bienvenida en 14:00 y solo despedida en 14:24.
> no hay tx 14:30 en un bloque 14:00-14:30 porque 14:30 TERMINA su permiso de tx y comienza otro bloque (o nada). si fuera 14:00-14:33 y el tiempo del ultimo tx del bloque (ahora de 6 imagenes) fuera menor a 3 min, el programa sí deberia incluirlo porque la cuenta daría.
> -tener fecha/hora de término (se transmite como daysleft)
> -estar activado/desactivado (su tx) (desactivado el programa, sus bloques aparecen en gris y al transparentes (no tanto como los vacios).
> -color (para sus cards en el grid solamente) (solo hue. sat y brightness fijos)
>
> bloques desactivados (por bloque o por programa) aparecen más transparentes
>
> bloques respetan su rango de tiempo de tx. si el tiempo de intervalo y el tiempo de bloque alcanza para 5 imágenes (exceptuando bumpers) y hay 6, se ignora y no se transmite esa imagen y cualquier otra sobrante. si se está transmitiendo algo perteneciente a un bloque y este termina en horario, debe cortarse PTT inmediatamente sin terminar el tx. si termina 14:30 debe soltar cualquier ptt activado a las 14h:29m:59.99999999s (no 59.0, si 59.99)
> el unico que puede hacer tx (y no soltarlo) fuera de horario de bloque es el tab manual.
>
> el programa debe ofecer activar sincronizar tiempo exacto cada X horas via internet en su primer uso (y si no se usa, poder activarlo luego en config, con un server default pero pudiendo cambiarlo.
>
> la grilla tiene zoom hasta mes, pero se puede ir pasando meses hasta futuro 3 años y pasado infinito.
>
> seamless edit: cuando creamos un programa se crean sus bloques con el dia y horario default y con fecha de termino infinita.hacerle eso a la BBDD seria mala idea, así que no vamos a guardar un bloque si tiene imagenes (sean importadas via bloque o via programa con jerarquia de carpetas).
> o sea, deben aparecer en grilla si el programa los tiene seteados, pero semitransparentes (indicador de vacío). recien cuando se le asignan imagenes se crea en la db como un bloque real y programado (y full opacidad). asi mismo cuando se le borran las imagenes se borra de db y vuelve a transparente. esto es posible porque se guardan parametros(modos,o si es bumper-N/P) de las imagenes, que cuando las imagenes no estan pierden sentido, como tambien el estar activo/inactivo.
> transparente significa "el horario/weekday de un programa indica que debería estar acá pero estoy vacío por ahora, no existo en la DB y si el daysleft de mi programa es 0 me veras graficado en todos les meses futuros en transparente" o tambien "soy un bloque 'comun sin programa' y estoy vacio y no me borraron".
>
> cuando en la config de un programa deschequeas un dia, debe haber un warning para elegir: borrar los bloques futuros no-vacios de ese día de ese programa? convertir a bloques comunes (elige max futuro(con vacios)/dejar solo no-vacios)?
> al borrar un programa, warning elegir: borrar todos sus bloques? dejar no-vacios? dejar todos(elige futuro max)?(dejar=convertir a comun)
>
> doble click en grilla vacia inserta a eleccion bloque comun o programa en ese horario (que por default repite solo ese día (o avisa que no se puede porque un bloque futuro existente(o planeado) se pone en el camino))
>
> dragging a block border edits it's time range. if it belongs to a program, it edits the time range of the program (so all its full or empty blocks change too)
>
> al crear programa comenzar sin dias/horas, sin default, el usuario lo pone. excepto que venga de click en grilla vacia. no permitir en creacion de bloques comunes ni en rango/weekdays de programas, que se superpongan bloques de ningun tipo con otros, activos o no, vacíos o no-vacío, en db o no en db (calcular con config de otros programas a futuro qunque no esten en db los bloques), no se superpone nada que en la grilla se muestre, transparente o no. si la config no es compatible con la grilla actual, no se aplica. el usuario moverá o borrará a mano el bloque que detiene poder aplicar la config deseada.
>
> los bloques comunes pueden borrarse, son independientes de un programa.
>
> -agregar un tab aparte de tx instantáneo, aún si la grilla está corriendo (no permitir uso si la grilla esta en tx). con opciones (cw,ax25 info) y bumpers con distinto modo (como para testear solo un tx de un bloque)
>
> -the general info ax25 is:
> --channel freq
> --call sign (for ID and cause other hams could run their programs in the same channel and time range, minute offseted in turns with me, after organizing via phone a 3minute-3minute-3minute window for 3 hams use the same channel (freq) at same time. this is an example case, not a constant for the software)
> --actual block/prgm name
> --actual block/prgm time range (14:00-15:30)
> --weekdays (if is program)
> --daysleft (99-2 days left, 1 = today is last tx day/date(not tomorrow), 0 = infinite/not set/higher than 99) (so imaginary client grid can stop repeating for next week if program won't be txed)
> --upcoming blocks (name, time, weekdays (so the -imaginary external project- client can identify it as a program and repeat in its visual grid), daysleft). (configurable how much next blocks to include in info by count or by hours/days, and dedup option for "include only next block of every different program and omit blocks from actual txing program")
>
> -"activations" (cw-dtmf) are completely out of relation with hour grid objects. activations are a separate tab (like manual tx). they are actions with a trigger that can activate a cw tx, a ax25tx (in the same info format as grid tx (optional fields) + a comment), a dtmf tx, run a specific command in the pc and tx it's output in cw or dtmf or ax25, tx and image in a mode, tx a random image from a pool in a mode. an activation has multiple triggers and multiple ordered actions from mentioned options.
