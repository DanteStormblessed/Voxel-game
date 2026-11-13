#!/usr/bin/env python3
"""Genera los modelos .vox de ejemplo de content/base/models/.

Los modelos de verdad se dibujan en MagicaVoxel; estos se generan por código para que el
repositorio tenga ejemplos reproducibles. Uso: python3 tools/make_vox.py

Ejes del mundo (los mismos que usa el motor): x = este, y = arriba, z = sur. El frente
del modelo es el lado -Z (norte). En el .vox: su x = x, su y = (sz - 1 - z), su z = y.
"""

import os
import struct

OUT = os.path.join(os.path.dirname(__file__), "..", "content", "base", "models")


def hex_color(text):
    text = text.lstrip("#")
    return tuple(int(text[i:i + 2], 16) for i in (0, 2, 4))


def write_vox(name, size, voxels, palette):
    """voxels: {(x, y, z): índice de color 1..}; palette: lista de '#RRGGBB'."""
    sx, sy, sz = size
    points = b"".join(
        struct.pack("4B", x, sz - 1 - z, y, color) for (x, y, z), color in sorted(voxels.items())
    )

    def chunk(tag, body):
        return tag + struct.pack("<II", len(body), 0) + body

    rgba = b"".join(struct.pack("4B", *hex_color(c), 255) for c in palette)
    rgba += b"\0\0\0\xff" * (256 - len(palette))
    children = (
        chunk(b"SIZE", struct.pack("<III", sx, sz, sy))
        + chunk(b"XYZI", struct.pack("<I", len(voxels)) + points)
        + chunk(b"RGBA", rgba)
    )
    data = b"VOX " + struct.pack("<I", 150) + b"MAIN" + struct.pack("<II", 0, len(children)) + children
    with open(os.path.join(OUT, name + ".vox"), "wb") as f:
        f.write(data)


def box(voxels, x0, y0, z0, x1, y1, z1, color):
    for x in range(x0, x1):
        for y in range(y0, y1):
            for z in range(z0, z1):
                voxels[(x, y, z)] = color


def main():
    os.makedirs(OUT, exist_ok=True)

    # Flor: tallo, una hoja y una corola roja con el centro amarillo.
    v = {}
    box(v, 1, 0, 1, 2, 4, 2, 1)
    v[(0, 2, 1)] = 1
    box(v, 0, 4, 0, 3, 5, 3, 2)
    v[(1, 4, 1)] = 3
    v[(1, 5, 1)] = 3
    write_vox("flower", (3, 6, 3), v, ["#2E9E3A", "#E8413C", "#FFD23F"])

    # Losa de piedra: medio bloque, con la cara de arriba algo más clara.
    v = {}
    box(v, 0, 0, 0, 8, 3, 8, 1)
    box(v, 0, 3, 0, 8, 4, 8, 2)
    write_vox("stone_slab", (8, 4, 8), v, ["#8E959F", "#A9B0BA"])

    # Escalera: el peldaño bajo al frente (norte, hacia quien la coloca) y el alto detrás.
    v = {}
    box(v, 0, 0, 0, 8, 4, 8, 1)
    box(v, 0, 4, 4, 8, 8, 8, 1)
    box(v, 0, 3, 0, 8, 4, 4, 2)
    box(v, 0, 7, 4, 8, 8, 8, 2)
    write_vox("stone_stairs", (8, 8, 8), v, ["#8E959F", "#A9B0BA"])

    # Puerta de 8×16×2 (dos celdas): marco oscuro, tablas claras y un pomo al frente.
    v = {}
    box(v, 0, 0, 0, 8, 16, 2, 1)
    box(v, 1, 1, 0, 7, 15, 2, 2)
    box(v, 1, 7, 0, 7, 8, 2, 1)
    v[(6, 7, 0)] = 3
    write_vox("door", (8, 16, 2), v, ["#6B3F1F", "#B5793E", "#E3C04A"])

    # Poste de valla.
    v = {}
    box(v, 0, 0, 0, 2, 8, 2, 1)
    write_vox("fence_post", (2, 8, 2), v, ["#A9632B"])

    # Farol: base y tapa oscuras, cristal amarillo en medio.
    v = {}
    box(v, 0, 0, 0, 4, 1, 4, 1)
    box(v, 0, 1, 0, 4, 5, 4, 2)
    box(v, 0, 5, 0, 4, 6, 4, 1)
    v[(1, 6, 1)] = 1
    write_vox("lantern", (4, 7, 4), v, ["#3B3B42", "#FFD46B"])

    # Espada (modelo de ítem, más grande que una celda): hoja, guarda y empuñadura.
    v = {}
    box(v, 1, 5, 0, 2, 16, 1, 1)
    box(v, 0, 4, 0, 3, 5, 1, 2)
    box(v, 1, 0, 0, 2, 4, 1, 3)
    write_vox("sword", (3, 16, 1), v, ["#D7DEE6", "#C9A227", "#5A3A22"])

    print("models written to", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
