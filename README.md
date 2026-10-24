# Estivi

Programación de un canal SSTV como si fuera un canal de TV: una **grilla** de bloques horarios, **programas** (las imágenes que salen al aire), **gráfica del canal** encima (zócalos, hora, número de emisión, logo, datos de una API) y un **registro de emisiones**. Corre en Windows y Linux, es un solo ejecutable liviano (Rust + Slint, sin navegador) y no necesita internet.

- **Transmite** SSTV: Martin M1–M4, Scottie S1/S2/DX, Robot 24/36/72, Robot B/N 8/12/24 y PD 50/90/120/160/180/240/290. Con VIS, preámbulo VOX opcional, FSK ID y CW ID.
- **Escucha** CW (velocidad fija) y DTMF, para **disparadores** remotos: «si oigo `*1#`, transmití», «si oigo `QRV SSTV` en CW, transmití», pausar o reanudar la grilla.
- **PTT** por VOX, puerto serie (RTS/DTR, normal o invertido) o CAT con Hamlib (`rigctld`), que además puede fijar la frecuencia de cada bloque.

## Dos archivos distintos

| Archivo | Qué guarda | Dónde |
|---|---|---|
| `estivi.toml` (configuración) | Lo de **esta PC**: placas de audio, puerto serie, PTT, CAT, calibración del reloj | Junto al ejecutable si se puede escribir ahí (instalación portable en un pendrive); si no, en `~/.config/estivi/` o `%APPDATA%\estivi\` |
| `*.estivi` (proyecto) | Lo del **canal**: modo SSTV, estación, programas, grilla, gráfica, identificación, escucha y disparadores | Donde lo guardes. Las imágenes dentro de la carpeta del proyecto se guardan con ruta relativa, así la carpeta viaja entera |
| `*-registro.csv` | Una línea por emisión (inicio, fin, número, modo, programa, frecuencia, origen, resultado) | Junto al proyecto |

Las fuentes `.ttf`/`.otf` que pongas en una carpeta `fuentes/` junto al proyecto o al ejecutable aparecen en el editor de gráfica (además de las Noto incluidas).

## La ventana principal

- **Barra al aire**: estado (EN ESPERA / AL AIRE / PAUSADA), próxima emisión con cuenta regresiva, modo del proyecto, interruptor «Grilla al aire», *Transmitir ahora* y *Cortar*.
- **Programas**: las imágenes del canal, con rotación en orden o aleatoria sin repetir. Doble clic para renombrar; ▶ lo transmite ya.
- **Grilla**: bloques con días, horario (una vez, o cada N minutos entre dos horas, aunque crucen la medianoche), programa fijo o rotación y frecuencia opcional. Arriba, la línea de tiempo del día.
- **Monitores**: lo que está al aire (con la línea de barrido avanzando) y una vista previa exacta de lo que sale después.
- **Registro** y **Escucha** (nivel de entrada, texto CW y secuencias DTMF recibidas).

La **pausa mínima** entre emisiones (Proyecto → Transmisión) protege el equipo: si un bloque cae antes, sale cuando termina la pausa. Las emisiones que se atrasan más de 90 s (por ejemplo, si la PC estuvo suspendida) se saltean en vez de salir tarde.

## Gráfica del canal

Ventana aparte (**Gráfica**). Abajo de todo va la imagen base (el programa), que se adapta al tamaño de cada modo: recortar para llenar, entera con bandas o estirar. Encima, capas de **texto** e **imagen** (PNG con transparencia) que se arrastran, se redimensionan desde las esquinas (con imán al centro y a los bordes) y se ajustan desde el inspector. Las posiciones se guardan en proporción, así la misma gráfica sirve de 160×120 (Robot 24) a 800×616 (PD 290).

Los textos aceptan datos entre llaves:

| Dato | Muestra |
|---|---|
| `{call}` `{op}` `{qth}` `{loc}` | Datos de la estación |
| `{utc}` `{date}` `{local}` | Hora UTC, fecha UTC, hora local. Con formato: `{utc:%d/%m %H:%M}` |
| `{serial}` `{serial:4}` | Número de emisión (con ceros: `0042`) |
| `{mode}` `{program}` `{freq}` | Modo, nombre del programa, frecuencia del bloque |
| `{cw}` `{cwcall}` `{dtmf}` | Último texto CW, último indicativo oído en CW, última secuencia DTMF |
| `{api}` | Respuesta de una URL (con campo JSON opcional, p. ej. `current.temperature_2m`), refrescada cada N segundos en segundo plano |

Para que se lea con ruido, cada texto puede llevar contorno, sombra y caja de fondo, y achicarse solo si no entra.

## Compilar

Requiere Rust estable (1.85 o más nuevo).

```sh
cargo build --release
# el ejecutable queda en target/release/estivi (estivi.exe en Windows)
```

- **Linux**: hacen falta los paquetes de desarrollo de ALSA y fontconfig (`alsa-lib` y `fontconfig` en Arch; `libasound2-dev libfontconfig1-dev` en Debian/Ubuntu).
- **Windows**: compilar en Windows con la toolchain MSVC. En las versiones *release* no se abre consola.
- La interfaz se dibuja por software (≈50 MB de RAM y ~3 % de CPU en reposo, sin depender de drivers de video). Equipo → General permite usar la GPU (OpenGL) si se prefiere.

Herramientas de línea de comandos (útiles para probar sin radio):

```sh
# cuadro final de un programa a PNG y la emisión completa a WAV (para decodificar con QSSTV/MMSSTV)
cargo run -p estivi-core --example render -- ejemplo/canal-demo.estivi cuadro.png audio.wav 0 PD120
# dispositivos de audio (el id es lo que va en estivi.toml) y puertos serie
cargo run -p estivi-core --example dispositivos
```

`ejemplo/canal-demo.estivi` es un canal de prueba (indicativo ficticio `N0CALL`) con tres programas, tres bloques, zócalo, hora/número, logo y una capa de clima por API (oculta).

## Estado de verificación

Pruebas automáticas (`cargo test --workspace`): tiempos de línea de todos los modos contra los valores publicados, VIS y paridad, bordes de píxel sin deriva acumulada, ida y vuelta de frecuencias en Martin M1, decodificadores CW y DTMF con ruido, grilla (incluido el cruce de medianoche), pausa mínima y emisiones atrasadas, rotación y formatos de archivo.

Decodificado con **QSSTV 9** por audio real (ALSA loopback), imagen y resolución correctas:

| Modo | Resultado |
|---|---|
| Martin M1 | OK, transmitido en vivo desde la app (motor → audio → QSSTV) |
| Martin M2 · Scottie S1, S2, DX | OK |
| Robot 24, 36, 72 · Robot B/N 8, 12 | OK |
| PD 50, 120, 290 | OK |
| PD 90, 160, 180, 240 | Mismo código que los PD probados; no se decodificaron por separado |
| Martin M3, M4 · Robot B/N 24 | QSSTV no los soporta: siguen la tabla de slowrx, falta probarlos |

- Escala de color de Robot/PD: QSSTV decodifica luminancia en **escala completa** (medido con franjas: 0→2 y 255→255; con escala de estudio llegaba 18 y 237, imagen lavada). Por eso es el valor por defecto; la escala de estudio queda como opción en Proyecto → Transmisión.
- FSK ID sigue el formato de MMSSTV (como pySSTV); falta confirmarlo con MMSSTV.
- PTT serie y CAT (rigctld) están implementados pero no se probaron con un equipo real.
- En Linux, el PCM `front` de algunas placas pasa por un volumen por software del mezclador ALSA: si el nivel sale muy bajo, elegí la variante `plughw` del dispositivo o subí ese control.

## Licencias

Las fuentes Noto incluidas (`crates/core/assets/fonts`) son SIL Open Font License 1.1 (`OFL.txt`). Slint se puede usar bajo GPLv3 o bajo su licencia libre de regalías para escritorio, que pide mostrar la atribución «Made with Slint»: hay que elegir una antes de distribuir.
