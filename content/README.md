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
    prefabs/
      trees/oak.toml    prefabs guardados desde el juego (+ oak.bin con los voxeles)
    worldgen/
      world.toml        forma del mundo, orden de los biomas y nodos comunes
      biomes/
        desert.toml     un archivo por bioma
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
| `model_front` | `"north"` | Lado del `.vox` que es el frente: `"north"` (+Y de MagicaVoxel), `"south"` (−Y, lo que se ve de frente al abrir MagicaVoxel), `"east"` o `"west"`. El modelo se gira al cargarlo |
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
  para que el frente mire a `facing`; con `"axis"` lo tumba como a un tronco. Si el
  modelo está dibujado con el frente hacia otro lado (lo habitual es −Y, la cara que se
  ve al abrir MagicaVoxel), no hace falta redibujarlo: `model_front = "south"`.
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

### Flujo de trabajo: un bloque detallado

1. En MagicaVoxel, un modelo de **8×8×8** (una celda = un bloque; 16 de alto para dos
   bloques, como la puerta). Z es arriba.
2. Guardarlo en `content/<paquete>/models/<nombre>.vox`.
3. En `blocks/<bloque>.toml`: `model = "<paquete>:<nombre>"`, `orientation = "horizontal"`
   si debe mirar a quien lo coloca y `model_front` si el frente no está en +Y. El horno
   (`blocks/kiln.toml` + `models/furnace.vox`) es el ejemplo.
4. Con el juego abierto se recarga solo al guardar. `give <bloque>` lo pone en la barra.
5. Retocar si hace falta `collision` y el `[icon]`.

## Prefabs

Un prefab es un trozo de mundo guardado (un árbol, una casa) para volver a colocarlo.
**No se escriben a mano**: se construyen en el juego y se guardan con la herramienta de
selección. Solo puede usarla el anfitrión del mundo.

1. Construir la pieza con bloques normales y bloques con modelo.
2. Coger la **vara de selección** (inventario → Herramientas, o `give selection_wand`)
   y dibujar la caja con el clic izquierdo (alcance 96 m):
   - **Clic 1**: una esquina de la base, en el bloque apuntado.
   - **Clic 2**: la esquina opuesta. También vale mantener el primer clic, arrastrar
     hasta la otra esquina y soltar.
   - **Clic 3**: la altura, que sigue a la mira hacia arriba (o hacia abajo) hasta que
     se confirma.

   La caja se ve translúcida, con una rejilla de bloques y **también a través del
   terreno**, y lleva sobre sus aristas el ancho, el largo y el alto. Para retocarla se
   apunta a la **manilla** de una cara, el cuadrado blanco de su centro (se pone verde):
   manteniendo el clic izquierdo la cara sigue a la mira, y la rueda la empuja o tira de
   ella bloque a bloque. Las manillas se ven y se agarran a través de los bloques y de la
   propia caja: la de la cara de abajo se coge mirándola desde arriba. Si dos quedan
   alineadas se agarra la más cercana (con `Ctrl`, la más lejana). Un clic izquierdo en
   cualquier otro sitio empieza otra caja; el derecho o el central la borran.
3. `P` abre el menú de prefabs: escribir un nombre (`trees/oak`; con `/` se ordenan en
   carpetas) y **Save**. Queda en `content/base/prefabs/trees/oak.toml` + `.bin`. Para
   guardarlo en otro paquete: `mimod:trees/oak`.
4. Para colocarlo: en el menú, elegirlo y **Load and place**. Aparece una vista previa
   bajo la mira: clic derecho lo pone (se puede repetir), `R` lo gira 90°, `RePág` /
   `AvPág` lo suben o bajan, clic izquierdo o `Esc` terminan. `Ctrl+Z` deshace.

Atajos con la vara en la mano: `Ctrl+C` copiar, `Ctrl+X` cortar, `Ctrl+V` colocar lo
copiado, `Supr` vaciar la selección, `Ctrl+Z` deshacer (hasta 32 cambios). Todo está
también en la consola: `pos1`, `pos2`, `desel`, `copy`, `cut`, `erase`, `paste [here]`,
`rotate`, `pasteair on|off`, `undo`, `prefab save|load|delete <nombre>`, `prefab list`.

El `.toml` de un prefab se puede leer y retocar:

```toml
size = [5, 8, 5]         # x, y, z en bloques (máximo 128 por lado y 1 048 576 en total)
anchor = [2, 0, 2]       # voxel que cae donde se apunta: por defecto, el centro de la base
palette = ["air", "base:wood", "base:leaves_green"]   # bloques por nombre de estado
```

- Guarda **nombres de estado**, no números: sirve en cualquier mundo. Un nombre que el
  contenido ya no tiene se salta al colocarlo (y se avisa al cargarlo).
- Los huecos (aire) del prefab no borran lo que haya, salvo con "Empty space also
  clears blocks" (`pasteair on`).
- Al girarlo giran también los bloques orientables (troncos, hornos, puertas).
- Guardar o borrar prefabs no recarga el resto del contenido.

## Ítems

Cada bloque genera su ítem (mismo identificador, nombre, color y categoría) salvo que diga
`item = false`. Los archivos de `items/` añaden ítems nuevos o sustituyen a los de los
bloques:

```toml
name = "Vara de selección"
color = "#E04CE0"          # color en la interfaz
category = "Herramientas"
tags = ["creativo"]
tool = "selection"         # herramienta: con este ítem en la mano se seleccionan cajas (ver "Prefabs")
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

## Terreno: mundo y biomas

El terreno también es contenido. `worldgen/world.toml` describe el mundo y cada archivo de
`worldgen/biomes/` es un bioma (`base:desert` es `base/worldgen/biomes/desert.toml`). El
servidor los vuelve a leer en cuanto cambian: el mundo se regenera con el juego abierto y
lo que esté construido se conserva. Un archivo con errores no rompe nada: el servidor lo
dice en el registro y sigue con el terreno anterior.

### El mundo

```toml
width = 5120        # metros de oeste a este
length = 32768      # metros de norte a sur
biomes = ["desert", "shattered_plains", "plains"]   # de norte a sur, a partes iguales
blend = 700         # ancho de la mezcla entre dos biomas vecinos, en metros
border_wobble = 700 # cuánto serpentea la frontera, y el tamaño de esas ondas
border_scale = 1700
stone = "base:stone"   # roca bajo el subsuelo
water = "base:water"   # lo que llena todo lo que queda bajo el nivel 0

[nodes]             # nodos comunes: cualquier bioma puede usarlos por su nombre
range = { type = "smoothstep", input = "across", from = 0.58, to = 0.96 }
```

Entre dos biomas vecinos la altura se mezcla poco a poco y la superficie (bloques,
árboles, plantas) se entrelaza en manchas. Los nodos de `world.toml` son los que hacen que
la costa y la cordillera casen de un bioma al siguiente; un bioma puede definir un nodo
con el mismo nombre para cambiarlo solo en él.

### Un bioma

```toml
name = "Planicies"
color = "#B0E66E"        # color en el mapa de biomas de la vista previa
height = "height"        # nodo que da la altura (por defecto, "height")

[nodes]                  # el grafo: cada nodo da un número por columna (x, z)
flat_noise = { type = "noise", scale = 900, octaves = 2 }
flat_part = { type = "mul", inputs = ["flat_noise", 5] }
height = { type = "add", inputs = [22, "flat_part", "cordillera"] }

[[surface]]              # reglas de superficie: vale la primera que se cumple
below = 2
top = "base:sand"

[[surface]]
top = "base:grass"
under = "base:dirt"

[[trees]]
density = 0.2
wood = "base:wood"
leaves = ["base:leaves_green", "base:leaves_lime"]
on = ["base:grass"]

[[plants]]
density = 0.01
block = "base:flower_red"
on = ["base:grass"]
```

**Nodos.** Una entrada (`input`, `inputs`, `a`, `b`, `t`) es un número o el nombre de otro
nodo. Siempre existen `x` y `z` (metros) y `u` y `v` (0-1 de oeste a este y de norte a sur
dentro de la zona jugable). Un parámetro mal escrito es un error, no se ignora.

| `type` | Parámetros | Resultado |
|---|---|---|
| `constant` | `value` | Un número fijo |
| `noise` | `scale` (metros, o `[x, z]`), `octaves` (4), `seed`, `warp`, `warp_scale` | Ruido suave, de -1 a 1 |
| `ridge` | Los de `noise` | Crestas: 1 en la línea de cumbres, 0 en los valles |
| `cells` | `scale`, `mode` (`edge`, `center`, `value`), `jitter` (0,8), `seed`, `warp` | Celdas irregulares: metros hasta el borde, metros hasta el centro, o un valor 0-1 fijo por celda |
| `spots` | `spacing`, `radius`, `chance` (1), `vary` (0), `seed` | Manchas redondas al azar: 1 en el centro, 0 desde el radio |
| `point` | `x`, `z`, `radius` | Una mancha en un punto fijo del mundo |
| `add`, `mul`, `min`, `max` | `inputs` | Suma, producto, mínimo, máximo |
| `sub` | `a`, `b` | `a - b` |
| `mix` | `a`, `b`, `t` | Mezcla: `a` con `t` = 0, `b` con `t` = 1 |
| `clamp` | `input`, `min`, `max` | Limita a un rango |
| `abs`, `pow` | `input` (+ `exponent`) | Valor absoluto, potencia |
| `smoothstep` | `input`, `from`, `to` | 0 antes de `from`, 1 después de `to`, suave entre medias (al revés si `from` > `to`) |
| `curve` | `input`, `points = [[entrada, salida], …]`, `smooth` | Curva por puntos: dibujar un perfil a mano |
| `terrace` | `input`, `step`, `sharpness` (1) | Escalona en terrazas de `step` metros |

Dos nodos de ruido con nombre distinto dan ruido distinto; con el mismo `seed` comparten
el dibujo (así `cells` en modo `edge` y en modo `value` hablan de las mismas celdas).
`warp` deforma las formas demasiado regulares, a costa de dos consultas de ruido más.

El grafo se evalúa de forma perezosa: un `mul` no calcula el resto de factores si uno vale
0, y un `mix` con `t` en 0 o en 1 solo calcula la rama que usa. Poner las máscaras delante
(`inputs = ["range", "ridges", 30]`) hace que en mar abierto no se calcule la tierra, ni
las cumbres en el valle. Un grafo admite hasta 160 operaciones.

**Superficie.** Cada regla puede pedir `above` / `below` (altura en metros), `steeper` /
`flatter` (pendiente: metros de desnivel por metro; 1 = 45°) y `node` con `node_above` /
`node_below` (un nodo del grafo en un rango). `jitter` (2) trama los límites de altura
para que no sean líneas rectas. Pone `top` en la superficie y `depth` (3) capas de `under`
(por defecto, el mismo bloque) debajo. Si ninguna regla se cumple, queda roca.

**Árboles.** `density` es la fracción de casillas de 7 × 7 m con árbol: un número o un
nodo (para hacer arboledas). `shape`: `round` (copa redonda), `pine` (cono), `bush` (solo
copa, a ras de suelo) o `pillar` (solo tronco: un cactus). `trunk = [mín, máx]` es la
altura del tronco y `on`, los bloques de superficie sobre los que crece (vacío: cualquiera
en tierra firme). Con varias reglas, las densidades se suman.

**Plantas.** Un bloque (normalmente un modelo pequeño) sobre la superficie, en la fracción
`density` de las columnas con suelo de `on`.

### El editor de biomas

`F7` dentro del juego (solo en un mundo propio) abre el editor, en una ventana aparte,
sobre estos mismos archivos: el grafo de cada bioma como nodos unidos por cables, con la
miniatura del valor de cada nodo; las reglas de superficie, árboles y plantas en
formularios; los ajustes del mundo; y un mapa de la zona que se está editando. Con "Auto
save" guarda un momento después del último cambio y, con "Live world", el mundo se
regenera alrededor; sin él, los cambios esperan a "Apply to world". Edita el texto en su
sitio: los comentarios se conservan. Dónde se dibuja cada nodo (y el tamaño de su caja,
si se ha estirado) queda al final del archivo:

```toml
[editor.layout]
height = [700, 120]            # x, y en el lienzo; el generador lo ignora
flat_noise = [0, 0, 320, 420]  # con ancho y alto: una caja estirada a mano
```

### Perfiles de un bioma

Un perfil es otra versión entera de un bioma, para probar ideas sin tocar el archivo de
siempre:

```
worldgen/biomes/plains.toml                 el perfil por defecto (el que se comparte)
worldgen/profiles/plains/rivers.toml        un perfil "rivers" de las planicies
```

Tiene el mismo formato que el archivo del bioma. Cada **mundo** elige qué perfil usa cada
bioma, y lo recuerda en su `worlds/<mundo>/world.toml`:

```toml
[profiles]
"base:plains" = "rivers"
```

En el editor, "Profile" elige el perfil que se edita y que usa el mundo abierto, y "Save
as new profile" crea uno nuevo como copia del actual. Desde la consola: `terrain
profiles` los lista y `terrain profile plains rivers` (o `default`) elige uno.

### Verlo rápido

```bash
cargo run --release --bin tokikura-worldmap -- --watch            # mapa del mundo entero
cargo run --release --bin tokikura-worldmap -- --biome fjords --watch
```

Escribe `worldmap.png` (el mundo visto desde arriba, con relieve) y, con `--watch`, lo
redibuja en menos de un segundo cada vez que se guarda un archivo. `--biome` dibuja el
mundo entero con un solo bioma, para trabajarlo por separado; `--center X,Z --size W,H
--scale N` acercan una zona y `--tint` tiñe cada bioma con su `color`.

Dentro del juego, `KURAVOXEL_BIOME=fjords` abre el mundo con ese único bioma, el comando
`biome` dice en cuál se está y dónde quedan los demás, y la vista de mapa (alejar la
cámara) enseña el resultado en 3D.

## Límites y requisitos

- Hasta 65 535 estados de bloque en total.
- El generador de terreno necesita los bloques que nombren `worldgen/world.toml` y los
  biomas (usa su estado por defecto): si falta alguno, el terreno no se carga.
- Solo hay agua al nivel 0: un lago o un río son terreno excavado por debajo de él.
