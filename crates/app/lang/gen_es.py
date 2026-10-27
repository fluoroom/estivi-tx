#!/usr/bin/env python3
"""Generate lang/es/LC_MESSAGES/estivi.po from @tr() strings in the UI."""
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "ui"
OUT = ROOT / "lang/es/LC_MESSAGES/estivi.po"

# English msgid -> Spanish. {{braces}} are literal in Slint @tr.
TR = {
    "Cancel": "Cancelar",
    "Save": "Guardar",
    "Grid block": "Bloque de la grilla",
    "Block name": "Nombre del bloque",
    "e.g. Night on 80 m": "Ej.: Noche en 80 m",
    "Days": "Días",
    "Mo": "L",
    "Tu": "M",
    "We": "X",
    "Th": "J",
    "Fr": "V",
    "Sa": "S",
    "Su": "D",
    "Starts ({})": "Comienza ({})",
    "Repeat": "Repetir",
    "Every (minutes)": "Cada (minutos)",
    "Until ({})": "Hasta ({})",
    "If Until is earlier than the start, the block ends the next day. If a transmission lasts longer than the interval, the next one waits for the minimum pause.": "Si «hasta» es más temprano que el comienzo, el bloque termina al día siguiente. Si una emisión dura más que el intervalo, la siguiente espera la pausa mínima.",
    "What goes on air": "Qué sale al aire",
    "Frequency (MHz, optional — set via CAT if enabled)": "Frecuencia (MHz, opcional — se fija por CAT si está habilitado)",
    "e.g. 14.230": "Ej.: 14.230",
    "Block enabled": "Bloque habilitado",
    "Project": "Proyecto",
    "Station": "Estación",
    "Channel / project name": "Nombre del canal / proyecto",
    "Callsign": "Indicativo",
    "Operator": "Operador",
    "These fields can appear on the graphics as {{call}}, {{op}}, {{qth}} and {{loc}}. SSTV mode is chosen in the main bar and is also saved with the project.": "Estos datos se pueden mostrar en la gráfica con {{call}}, {{op}}, {{qth}} y {{loc}}. El modo SSTV se elige en la barra principal y también se guarda con el proyecto.",
    "Transmission": "Transmisión",
    "Minimum pause between transmissions (seconds) — protects the radio": "Pausa mínima entre emisiones (segundos) — protege el equipo",
    "Grid delay — every TX starts this long after the block time": "Retraso de la grilla — todas las TX salen esta cantidad después de la hora del bloque",
    "min": "min",
    "s": "s",
    "Blocks are written on the hour (14:00). The delay lets several channels share a frequency: if another occupies 14:00–14:03, set 3 min and yours start at 14:03.": "Los bloques se escriben en hora redonda (14:00). El retraso sirve para compartir frecuencia: si otro canal ocupa 14:00–14:03, poné 3 min y las tuyas empiezan a las 14:03.",
    "Robot / PD color range": "Escala de color Robot / PD",
    "Full 0–255 (QSSTV, slowrx; recommended)": "Completa 0–255 (QSSTV, slowrx; recomendada)",
    "Studio BT.601 16–235": "Estudio BT.601 16–235",
    "Preamble tones before VIS (open VOX)": "Tonos de preámbulo antes del VIS (abren el VOX)",
    "FSK ID at the end (callsign readable by MMSSTV)": "FSK ID al final (indicativo legible por MMSSTV)",
    "CW identification before VIS (preamble)": "Identificación en CW antes del VIS (preámbulo)",
    "CW identification at the end of each transmission": "Identificación en CW al final de cada emisión",
    "CW text": "Texto CW",
    "Speed (wpm)": "Velocidad (ppm)",
    "Tone (Hz)": "Tono (Hz)",
    "The preamble comes after the VOX tones and before VIS. The post comes after the picture (and FSK ID if enabled). The text accepts {{call}}.": "El preámbulo va después de los tonos VOX y antes del VIS. El post, después de la imagen (y del FSK ID si está). El texto admite {{call}}.",
    "Listen and triggers": "Escucha y disparadores",
    "Decode CW": "Decodificar CW",
    "Fixed speed (wpm)": "Velocidad fija (ppm)",
    "Decode DTMF": "Decodificar DTMF",
    "TRIGGERS": "DISPARADORES",
    "Add": "Agregar",
    "DTMF: the sequence ends with # or after 2.5 s of silence (e.g. *12#). CW: fires when received text ends with those words (e.g. QRV SSTV).": "DTMF: la secuencia termina con # o tras 2,5 s de silencio (ej.: *12#). CW: se dispara cuando el texto recibido termina con esas palabras (ej.: QRV SSTV).",
    "Transmit": "Transmitir",
    "Pause grid": "Pausar grilla",
    "Resume grid": "Reanudar grilla",
    "Equipment: audio, PTT and CAT": "Equipo: audio, PTT y CAT",
    "Audio": "Audio",
    "Output (to the transmitter)": "Salida (hacia el transmisor)",
    "Input (from the receiver, for CW / DTMF)": "Entrada (desde el receptor, para CW / DTMF)",
    "Sample rate": "Frecuencia de muestreo",
    "Device default": "La del dispositivo",
    "Output channel": "Canal de salida",
    "Both": "Ambos",
    "Left": "Izquierdo",
    "Right": "Derecho",
    "Output level: {} %": "Nivel de salida: {} %",
    "Sound-card clock correction (ppm) — if pictures come out slanted": "Corrección del reloj de la placa (ppm) — si las imágenes salen inclinadas",
    "Test tone 5 s (with PTT)": "Tono de prueba 5 s (con PTT)",
    "Rescan devices": "Volver a buscar dispositivos",
    "With the test tone, raise the level until the radio ALC just starts to move.": "Con el tono de prueba ajustá el nivel hasta que el ALC del equipo apenas se mueva.",
    "PTT and CAT": "PTT y CAT",
    "How the radio is keyed": "Cómo se pone en transmisión el equipo",
    "None (audio only)": "Ninguno (solo audio)",
    "Radio VOX": "VOX del equipo",
    "Serial port (RTS / DTR)": "Puerto serie (RTS / DTR)",
    "CAT via Hamlib (rigctld)": "CAT por Hamlib (rigctld)",
    "Detected port": "Puerto detectado",
    "or another (e.g. COM7, /dev/ttyACM0)": "u otro (ej.: COM7, /dev/ttyACM0)",
    "Line": "Línea",
    "RTS and DTR": "RTS y DTR",
    "Inverted (active low)": "Invertida (activa en bajo)",
    "rigctld host": "Host de rigctld",
    "Port": "Puerto",
    "Set each block frequency before transmitting": "Fijar la frecuencia de cada bloque antes de transmitir",
    "Example: rigctld -m <model> -r /dev/ttyUSB0 -s 38400. Hamlib supports hundreds of radios.": "Ejemplo: rigctld -m <modelo> -r /dev/ttyUSB0 -s 38400. Hamlib soporta cientos de equipos.",
    "Lead before audio (ms)": "Retardo antes del audio (ms)",
    "Tail after (ms)": "Cola después (ms)",
    "Maximum time (s)": "Tiempo máximo (s)",
    "Test PTT (2 s)": "Probar PTT (2 s)",
    "General": "General",
    "Start with the grid on air (unattended stations)": "Arrancar con la grilla al aire (estaciones desatendidas)",
    "Draw the UI with the GPU (OpenGL; uses more memory, requires restart)": "Dibujar la interfaz con la GPU (OpenGL; usa más memoria, requiere reiniciar)",
    "Light theme": "Tema claro",
    "Language": "Idioma",
    "This configuration belongs to this PC (audio, port, PTT) and is saved apart from the project, in:\n{}": "Esta configuración es de esta PC (audio, puerto, PTT) y se guarda aparte del proyecto, en:\n{}",
    "Rename": "Renombrar",
    "Program": "Programa",
    "Pictures": "Imágenes",
    "Program name": "Nombre del programa",
    "Picture order": "Orden de las imágenes",
    "In order": "En orden",
    "Shuffle (no repeats)": "Aleatorio (sin repetir)",
    "Program SSTV mode (default for its pictures)": "Modo SSTV del programa (default de sus imágenes)",
    "Drop or pick pictures": "Soltá o elegí imágenes",
    "MISSING": "FALTA",
    "A program is a group of pictures. Drop several or pick them with the button.": "Un programa es un grupo de imágenes. Arrastrá varias o elegilas con el botón.",
    "Program enabled": "Programa habilitado",
    "Use the project CW": "Usar el CW del proyecto",
    "CW before VIS": "CW antes del VIS",
    "CW at the end": "CW al final",
    "Text (empty = project text)": "Texto (vacío = el del proyecto)",
    "Activations": "Activaciones",
    "TRIGGERS FOR THIS PROGRAM": "DISPARADORES DE ESTE PROGRAMA",
    "When the code is heard, this program is transmitted (or the grid is paused / resumed). DTMF ends with #; CW, when the text ends with those words.": "Si se oye el código, se transmite este programa (o se pausa / reanuda la grilla). DTMF termina en #; CW, cuando el texto acaba en esas palabras.",
    "Channel graphics — {}": "Gráfica del canal — {}",
    "Text": "Texto",
    "PNG image": "Imagen PNG",
    "Bring forward": "Al frente",
    "Send back": "Atrás",
    "Duplicate": "Duplicar",
    "Delete": "Borrar",
    "Preview with program": "Ver con el programa",
    "LAYERS": "CAPAS",
    "Base picture": "Imagen base",
    "the program that goes on air": "el programa que sale al aire",
    "{} × {} px · {} · drag to move, corners to resize, arrows to nudge": "{} × {} px · {} · arrastrá para mover, las esquinas para cambiar el tamaño, flechas para ajustar",
    "BASE PICTURE": "IMAGEN BASE",
    "This is each program's picture. It always stays at the bottom and is fitted to the mode size.": "Es la imagen de cada programa. Queda siempre abajo de todo y se adapta al tamaño del modo.",
    "How it is fitted": "Cómo se ajusta",
    "Crop to fill": "Recortar para llenar",
    "Fit (with bars)": "Entera (con bandas)",
    "Stretch": "Estirar",
    "Background color (bars)": "Color de fondo (bandas)",
    "Layer name": "Nombre de la capa",
    "Text (can include fields in braces)": "Texto (puede llevar datos entre llaves)",
    "Data from a web API → {{api}}": "Dato desde una API web → {{api}}",
    "JSON field (empty = all)": "Campo JSON (vacío = todo)",
    "Every (s)": "Cada (s)",
    "Test": "Probar",
    "TYPE": "LETRA",
    "Bold": "Negrita",
    "Size: {} % of height": "Tamaño: {} % del alto",
    "Color": "Color",
    "Alignment": "Alineación",
    "Center": "Centro",
    "Top": "Arriba",
    "Middle": "Medio",
    "Bottom": "Abajo",
    "Shrink the type if it does not fit the box": "Achicar la letra si no entra en la caja",
    "ON-AIR READABILITY": "LEGIBILIDAD EN EL AIRE",
    "Outline: {} %": "Contorno: {} %",
    "Shadow": "Sombra",
    "Background box (transparent = none)": "Caja de fondo (transparente = sin caja)",
    "File": "Archivo",
    "Drop or pick the picture": "Soltá o elegí la imagen",
    "Keep aspect ratio": "Mantener proporción",
    "POSITION AND SIZE (px)": "POSICIÓN Y TAMAÑO (px)",
    "Width": "Ancho",
    "Height": "Alto",
    "Opacity: {} %": "Opacidad: {} %",
    "Changes apply immediately and are saved with the project (Save in the main window).": "Los cambios se aplican al momento y se guardan con el proyecto (Guardar en la ventana principal).",
    "Drop the file here": "Soltá el archivo aquí",
    "Drag from the file manager or pick with the button.": "Arrastrá desde el explorador o elegí con el botón.",
    "Drop…": "Soltá…",
    "Browse…": "Elegir…",
    "OK": "Aceptar",
    "Drop the file or folder here": "Soltá el archivo o la carpeta aquí",
    "Or type the path": "O escribí la ruta",
    "SSTV Channel": "Canal SSTV",
    "No transmissions on the grid": "Sin emisiones en la grilla",
    "Nothing transmitted yet": "Nada transmitido todavía",
    "Off": "Desactivada",
    "unsaved": "sin guardar",
    "New project (no file yet)": "Proyecto nuevo (todavía sin archivo)",
    "New": "Nuevo",
    "Open": "Abrir",
    "Save as…": "Guardar como…",
    "Graphics": "Gráfica",
    "Equipment": "Equipo",
    "Dark": "Oscuro",
    "Light": "Claro",
    "UTC · {} · local {}": "UTC · {} · local {}",
    "ON AIR": "AL AIRE",
    "PTT…": "PTT…",
    "TAIL": "COLA",
    "STANDBY": "EN ESPERA",
    "PAUSED": "PAUSADA",
    "TRIGGER WAITING": "DISPARADOR EN ESPERA",
    "NEXT TRANSMISSION": "PRÓXIMA EMISIÓN",
    "Error: {}": "Error: {}",
    "REMAINING": "FALTAN",
    "GRID PAUSED": "GRILLA PAUSADA",
    "Project mode": "Modo del proyecto",
    "Grid on air": "Grilla al aire",
    "Transmit now": "Transmitir ahora",
    "Cut": "Cortar",
    "PROGRAMS": "PROGRAMAS",
    "Drop pictures or an .estivi": "Soltá imágenes o un .estivi",
    "A program is a group of pictures, with order, CW and activations.": "Un programa es un grupo de imágenes, con orden, CW y activaciones.",
    "Sequential rotation": "Rotación en orden",
    "GRID": "GRILLA",
    "UTC time": "Horario UTC",
    "Local time": "Horario local",
    "upcoming": "por emitir",
    "aired": "emitida",
    "error": "con error",
    "on air": "al aire",
    "BLOCK": "BLOQUE",
    "DAYS": "DÍAS",
    "TIME": "HORARIO",
    "PROGRAM": "PROGRAMA",
    "FREQUENCY": "FRECUENCIA",
    "The grid is empty. Add a block: for example, 21:00 to 23:00 every 5 minutes.": "La grilla está vacía. Agregá un bloque: por ejemplo, de 21:00 a 23:00 cada 5 minutos.",
    "Block": "Bloque",
    "Edit": "Editar",
    "LAST TRANSMISSION": "ÚLTIMA EMISIÓN",
    "No transmissions yet": "Sin emisiones todavía",
    "NEXT": "SIGUIENTE",
    "Add programs to see the next picture": "Agregá programas para ver la próxima imagen",
    "Edit channel graphics": "Editar gráfica del canal",
    "TRANSMISSION LOG": "REGISTRO DE EMISIONES",
    "LISTEN": "ESCUCHA",
    "Triggers are set in Project → Listen.": "Los disparadores se configuran en Proyecto → Escucha.",
    "Utils — SSTV cheat sheet": "Utils — machete SSTV",
    "SSTV mode cheat sheet": "Machete de modos SSTV",
    "The project mode is the default. A program can set another for its pictures, and each picture can set its own. Channel graphics scale to the canvas of the mode that goes on air.": "El modo del proyecto es el default. Un programa puede fijar otro para sus imágenes, y cada imagen puede fijar el suyo. La gráfica del canal se escala al lienzo del modo que sale al aire.",
    "Mode": "Modo",
    "Canvas": "Lienzo",
    "Picture": "Imagen",
    "M2, M4 and S2 scan in half the time of M1/S1: the canvas stays 320 px, written as 160(320). Robot 36 alternates R-Y/B-Y (4:2:0); Robot 72 sends both chromas on every line (4:2:2). Martin and Scottie send G, B, R in sequence.": "M2, M4 y S2 barren en la mitad de tiempo que M1/S1: el lienzo sigue en 320 px, pero se escriben 160(320). Robot 36 alterna R-Y/B-Y (4:2:0); Robot 72 manda ambos croma en cada línea (4:2:2). Martin y Scottie envían G, B, R en secuencia.",
    "Close": "Cerrar",
}

PLURALS = {
    ("{n} program", "{n} programs"): ("{n} programa", "{n} programas"),
    ("{n} picture", "{n} pictures"): ("{n} imagen", "{n} imágenes"),
}


def unescape_slint(s: str) -> str:
    return s.replace(r"\"", '"').replace(r"\n", "\n").replace(r"\\", "\\")


def po_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def extract():
    pat = re.compile(r'@tr\(\s*"((?:\\.|[^"\\])*)"(?:\s*\|\s*"((?:\\.|[^"\\])*)")?', re.S)
    items = []
    seen = set()
    for f in sorted(UI.rglob("*.slint")):
        for m in pat.finditer(f.read_text()):
            msgid = unescape_slint(m.group(1))
            plural = unescape_slint(m.group(2)) if m.group(2) else None
            key = (msgid, plural)
            if key in seen:
                continue
            seen.add(key)
            items.append(key)
    return items


def main():
    items = extract()
    missing = []
    lines = [
        'msgid ""',
        "msgstr \"\"",
        '"Project-Id-Version: estivi 0.1.0\\n"',
        '"Language: es\\n"',
        '"MIME-Version: 1.0\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        '"Plural-Forms: nplurals=2; plural=(n != 1);\\n"',
        "",
    ]
    for msgid, plural in items:
        if plural is not None:
            es0, es1 = PLURALS.get((msgid, plural), (msgid, plural))
            if (msgid, plural) not in PLURALS:
                missing.append(f"{msgid} | {plural}")
            lines += [
                f'msgid "{po_escape(msgid)}"',
                f'msgid_plural "{po_escape(plural)}"',
                f'msgstr[0] "{po_escape(es0)}"',
                f'msgstr[1] "{po_escape(es1)}"',
                "",
            ]
            continue
        es = TR.get(msgid)
        if es is None:
            missing.append(msgid)
            es = msgid
        lines += [
            f'msgid "{po_escape(msgid)}"',
            f'msgstr "{po_escape(es)}"',
            "",
        ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(items)} entries, {len(missing)} missing)")
    for m in missing:
        print("MISSING", m[:120])


if __name__ == "__main__":
    main()
