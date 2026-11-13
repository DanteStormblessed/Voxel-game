# Contenido

Cada carpeta es un **paquete de contenido**. El servidor los carga todos al arrancar (por
orden alfabético, con `base` siempre primero), se los envía a los clientes y los vuelve a
cargar solo cuando cambia algún archivo: no hace falta reiniciar ni recompilar. Un archivo
con errores no rompe nada: se sigue usando el contenido anterior y el error sale en el
registro.

```
content/
  base/
    pack.toml           nombre y descripción del paquete
    blocks/
      grass.toml        un archivo por bloque
    items/
      selection_wand.toml  un archivo por ítem (los bloques ya traen el suyo)
    models/
      flower.vox        modelos de mini-voxeles hechos con MagicaVoxel
    icons/
      star.png          iconos dibujados a mano (opcional: si no, se generan)
```

Un mod es otra carpeta con la misma forma.

## Identificadores

El identificador es `paquete:archivo`: `content/base/blocks/grass.toml` es `base:grass`.
Solo minúsculas, números y `_ - . /`. Con `id = "base:grass"` un archivo usa otro
identificador; así un paquete sustituye un bloque o ítem de otro (gana el paquete cargado
más tarde).

## Bloques

```toml
name = "Hierba"          # nombre visible (obligatorio)
color = "#3ACB3C"        # color plano, #RRGGBB (obligatorio, salvo que haya faces.side)
category = "Terreno"     # pestaña del inventario creativo (por defecto "Blocks")
tags = ["natural"]       # etiquetas libres: búsqueda, generador, mods
```

Todos los campos opcionales:

| Campo | Por defecto | Qué hace |
|---|---|---|
| `parent = "base:stone"` | — | Hereda todos los campos de otro bloque (o plantilla) y cambia solo los que escriba |
| `template = true` | `false` | Es una plantilla: solo sirve de `parent`, no es un bloque (no se hereda) |
| `orientation` | `"none"` | `"axis"`: se tumba según la cara sobre la que se coloca (`axis=x\|y\|z`, troncos). `"horizontal"`: mira hacia quien lo coloca (`facing=north\|east\|south\|west`) |
| `[faces]` `top`, `side`, `bottom`, `front` | `color` | Color de cada cara. `front` es la que mira hacia `facing` |
| `solid` | `true` | Choca. Con `false` se atraviesa (agua, flores) |
| `collision` | `"full"` (o `"model"` si hay modelo) | Forma con la que choca si es sólido: `"full"`, `"model"` (las cajas que ocupa el modelo) o una lista de cajas en mini-voxeles, `[[x0, y0, z0, x1, y1, z1], ...]` de 0 a 8, que se orientan con el bloque |
| `transparent` | `false` | Deja ver a través (agua, cristal); va en un segundo pase con mezcla |
| `opacity` | `0.6` | Opacidad de un bloque `transparent`, de 0 a 1 |
| `light` | `0` | Luz que emite, de 0 a 15: el bloque brilla sin sombra. Aún no ilumina lo de alrededor |
| `model` | — | Modelo de mini-voxeles que dibuja en vez de un cubo (`"base:flower"`, ver abajo) |
| `cell` | — | Dibujar solo esa celda de un modelo grande, como bloque suelto de una celda (normalmente no hace falta: ver "Tamaño") |
| `[icon]` | vista generada | Icono de su ítem (ver "Iconos") |
| `item` | `true` | Genera un ítem con su mismo identificador |

Notas:

- El rayo de edición atraviesa los bloques no sólidos sin modelo (el agua) pero se para
  en las flores, para poder romperlas.
- Un cubo `transparent` no tapa a sus vecinos ni hace sombra; dos iguales juntos no
  tienen cara entre ellos.
- Un bloque con modelo no tapa a sus vecinos (se ve el suelo bajo una losa) y no hace
  sombra.

Ejemplo con herencia (`blocks/_leaves.toml` es una plantilla):

```toml
# blocks/_leaves.toml
template = true
category = "Naturaleza"
tags = ["natural", "hojas"]

# blocks/leaves_green.toml
parent = "base:_leaves"
name = "Hojas verdes"
color = "#1FA42D"
```

### Estados

Cada orientación es un **estado** con su propio número de bloque (los chunks guardan
estados). Se nombran como `base:wood[axis=x]`; sin corchetes se refiere al estado por
defecto (`axis=y`, `facing=north`). Ese nombre es el que usan los comandos (`setblock`,
`fill`) y la tabla de `world.toml`. Un bloque sin orientación tiene un solo estado, que se
llama como el bloque.

## Modelos

Los objetos con forma propia (flores, losas, escaleras, puertas, armas) son **modelos de
mini-voxeles**, 8 por metro (12,5 cm), hechos con [MagicaVoxel](https://ephtracy.github.io/)
y guardados como `.vox` en `models/`. El identificador es `paquete:archivo`
(`content/base/models/flower.vox` es `base:flower`). Se usa el primer modelo del archivo y
sus colores salen de su paleta (no tienen nada que ver con `color`, que es el del ítem).

- **Ejes**: la X de MagicaVoxel es el este, su Z es arriba y su +Y es el norte, que es
  también el **frente** del modelo. Un bloque con `orientation = "horizontal"` lo gira
  para que el frente mire a `facing`; con `"axis"` lo tumba como a un tronco.
- **Tamaño**: un bloque es una celda de 8×8×8. Un modelo más pequeño se centra en X y Z y
  se apoya abajo. Uno más grande (la puerta, de 8×16×2) ocupa varias celdas y el bloque
  es **un solo objeto**: un solo ítem, se coloca entero desde la celda de abajo (si todas
  sus celdas están libres) y al romper cualquiera de ellas se quita entero. Cada celda es
  un estado (`base:door[facing=east,part=0-1-0]`). Hasta 64 celdas y 128 mini-voxeles
  por lado.
- **Colisión**: por defecto la del modelo (cajas que lo envuelven); `collision` la cambia.
- Los ítems también pueden tener `model` (la espada): se ve en su icono y al tirarlo al
  suelo, aunque aún no en la mano.

`tools/make_vox.py` genera los modelos de ejemplo de `base`; los de verdad se dibujan en
MagicaVoxel.

## Ítems

Cada bloque genera su ítem (mismo identificador, nombre, color y categoría) salvo que diga
`item = false`. Los archivos de `items/` añaden ítems nuevos o sustituyen a los de los
bloques:

```toml
name = "Vara de selección"
color = "#E04CE0"          # color en la interfaz
category = "Herramientas"
tags = ["creativo"]
tool = "selection"         # herramienta del motor (reservado: llegan en la fase 3d)
model = "base:sword"       # modelo propio: icono y objeto tirado en el suelo
max_stack = 1              # reservado para la supervivencia (por defecto 64)
```

También admiten `parent`, `template` e `id`, y `block = "base:wood"` para que el clic
derecho coloque ese bloque; un ítem con `block` toma de él lo que no diga (nombre, color,
categoría).

Un ítem tirado al suelo (`Q`) se ve con su modelo, o con el de su bloque, flotando y
girando: los bloques a 1/4 de su tamaño y los modelos propios a la mitad.

## Iconos

Como en Hytale, cada ítem tiene un icono que **se genera solo a partir de su modelo** (o
del cubo de su bloque, con los colores de sus caras). Lo que se ajusta es la cámara, en
`[icon]`, en el archivo del bloque o del ítem (el ítem de un bloque usa el del bloque):

```toml
[icon]
rotation = [30, 45, 0]   # grados: inclinación, giro y alabeo (por defecto, tres cuartos)
scale = 1.0              # 1 = el objeto llena el icono; 0.8, algo más pequeño
offset = [0, 0]          # desplazamiento en fracción del icono: [0.1, 0] = 10 % a la derecha
```

Para un icono dibujado a mano, se guarda un PNG (hasta 256×256) en `icons/` y se indica
con `image = "paquete:nombre"`; sustituye al generado y se amplía sin suavizar (pixel art).

El comando de consola `icons` escribe todos los iconos en `icons_export/<paquete>/`: es la
forma más rápida de empezar uno a mano (se retoca el generado, se copia a `icons/` y se
pone `image`). Los iconos se regeneran al cambiar el contenido, sin reiniciar.

## Límites y requisitos

- Hasta 65 535 estados de bloque en total.
- El generador de terreno necesita estos bloques: `base:grass`, `base:dirt`, `base:stone`,
  `base:sand`, `base:water`, `base:snow`, `base:wood`, `base:leaves_yellow`,
  `base:leaves_lime`, `base:leaves_orange` y `base:leaves_green` (usa su estado por defecto).
