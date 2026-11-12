# Voxel

Motor y juego sandbox de voxeles en Rust, en la línea de Minecraft / Cube World / Veloren.
Este archivo recoge la dirección acordada del proyecto. Se conversa en español.

**Todo el código va en inglés**: identificadores, mensajes de registro y de error, ayuda
de línea de comandos, textos de la ventana y nombres de archivo. Solo los comentarios (y
esta documentación) pueden ir en español. El contenido del juego (`content/`) es dato, no
código: sus nombres visibles pueden estar en español.

## Visión

- Juego sandbox voxel. Primero el motor, después el juego.
- **Muy optimizado**: cualquier PC debe moverlo con soltura, y más adelante se portará a móvil.
- Estética: voxeles **sin texturas**, colores **vivos y planos**, con las **aristas suavizadas**
  (los cubos se ven ligeramente redondeados). Simple, tipo maqueta de MagicaVoxel.
- Veloren (también en Rust) sirve de referencia técnica, sin copiarlo.

## Cómo trabajar

- **Preguntar antes de desarrollar** cuando haya una decisión de dirección o una duda de
  fondo: escala, formato del mundo, técnica de render, mecánicas, aspecto. Presentar
  opciones con una recomendación.
- Los detalles de implementación se deciden sin preguntar y se explican al terminar.
- Si una idea del usuario tiene un coste técnico importante, decirlo y proponer la
  alternativa antes de implementarla.
- Verificar antes de dar algo por hecho: compilar, pasar tests y mirar una captura.

## Decisiones acordadas

| Tema | Decisión | Motivo |
|---|---|---|
| Lenguaje y stack | Rust + wgpu + winit | Elección del usuario |
| API gráfica | Vulkan ahora; OpenGL y móvil en el futuro | Elección del usuario |
| Tamaño de voxel | 1 m, tipo Minecraft | Lo más barato por distancia de visión |
| Mundo | Chunks cúbicos de 32³, altura ilimitada | Montañas, cuevas e islas sin techo de mundo |
| Bordes suaves | Bisel en el fragment shader + greedy meshing | Ver abajo |
| Larga distancia | Terreno lejano por mapa de alturas, estilo Veloren | Casi gratis; ver abajo |
| Distancias | Voxeles reales hasta 12 chunks, terreno lejano hasta 32 | Acotado de momento a 16-32 |
| Árboles lejanos | Copas simplificadas en su posición real | Los bosques no desaparecen |
| Red | Cliente-servidor autoritativo, multijugador nativo | Como Minecraft / Hytale |
| Un jugador | Servidor integrado en el propio juego | Un único camino de código |
| Transporte | QUIC (quinn) desde el principio | Elección del usuario; como Hytale |
| Terreno lejano en red | Lo genera y envía el servidor | Elección del usuario; como Veloren |
| Contenido | Paquetes de archivos TOML, un archivo por bloque | Como Hytale: el juego da las herramientas |
| Bloques | Color plano, sin texturas | Estilo del juego |
| Guardado | Solo los chunks modificados; el resto se regenera | Partidas diminutas; ver abajo |
| Mundos | Una carpeta por mundo, `worlds/<nombre>/` | Varios mundos de prueba |
| Objetos pequeños | Mini-voxeles a 8 por metro (12,5 cm) | Reservado; aún sin implementar |

### Bordes suaves

La idea original era dar a cada vértice una normal inclinada hacia fuera. Se descartó
porque abomba la cara entera (un suelo plano se ve acolchado) e impide fusionar caras,
con 5-20× más triángulos. En su lugar:

- El mallador fusiona caras coplanares y marca qué lados de cada quad son aristas
  convexas reales.
- El fragment shader inclina la normal solo en una franja fina junto a esas aristas,
  hasta 45° en el borde. La cara vecina hace lo mismo, así la luz es continua y la
  arista se percibe redonda.
- Cero triángulos extra; las superficies grandes siguen planas.

### Cliente-servidor autoritativo

El servidor es la única autoridad sobre el mundo. **El cliente nunca genera terreno**:
pide, predice su propio movimiento y dibuja lo que recibe.

- **Tres crates**: `voxel-common` (datos, protocolo, QUIC), `voxel-server` (generación,
  mundo, jugadores, ticks) y `voxel-client` (ventana, mallado, render). El cliente no puede
  acceder al generador de mundo: no está en su código.
- **Servidor integrado**: jugar solo arranca `voxel_server::start` en un hilo y el cliente
  le habla por canales en memoria (sin serializar, chunks compartidos con `Arc`). En
  multijugador los mismos mensajes viajan por QUIC. La lógica del servidor no distingue.
- **Transporte**: un stream bidireccional fiable (mensajes con prefijo de longitud,
  `postcard`) y datagramas para lo que caduca (movimiento, fotos de entidades).
- **Servidor**: atiende eventos en cuanto llegan (conexiones, mensajes, chunks generados)
  y simula a 20 ticks por segundo. Decide qué chunks y teselas lejanas ve cada jugador,
  los envía de cerca a lejos y ordena descargarlos.
- **Movimiento**: el cliente mueve su cámara al instante y envía su posición a ritmo de
  tick; el servidor valida la velocidad y, si no cuadra, responde con `Teleport`.
- **Otros jugadores**: el servidor difunde sus posiciones cada tick; el cliente las suaviza.
- Los chunks viajan comprimidos por tramos (RLE): un chunk denso suele ocupar < 1 KiB.

### Contenido y herramientas

Meta: como en Hytale, el juego proporciona las herramientas con las que jugadores y
desarrolladores crean su contenido. **El contenido son datos, no código.** Hoja de ruta
acordada, por orden:

1. **Paquetes de contenido** (hecho): bloques en archivos, recarga en caliente.
2. **Edición de bloques y guardado** (hecho): poner/romper validado por el servidor.
3. Herramientas creativas en el juego: selección, pinceles, copiar/pegar, prefabs.
4. Generador de terreno por datos (biomas, ruido, prefabs), con recarga en caliente.
5. Scripting para mods, en el servidor.

Cómo funciona el paso 1 (detalle del formato en `content/README.md`):

- `content/<paquete>/blocks/<bloque>.toml` define un bloque: `name` y `color`. Su
  identificador es `paquete:bloque` (p. ej. `base:grass`). Un mod es otra carpeta.
- Solo el servidor lee los archivos. Asigna a cada bloque un número (el byte que va en los
  chunks) y envía el resultado (`ContentData`) a los clientes al conectar.
- **El código nunca debe fijar números de bloque** (solo `Block::AIR` = 0). Quien necesite
  un bloque lo resuelve por identificador, como hace `TerrainBlocks` en el generador.
- Recarga en caliente: el servidor mira la carpeta cada 0,5 s; si algo cambia, relee todo
  y lo reenvía. Los bloques existentes conservan su número, los nuevos van al final, los
  borrados dejan hueco. Un archivo con errores no rompe nada: sigue el contenido anterior.
- Objetos pequeños: un objeto será una rejilla de mini-voxeles de color (8 por metro)
  dentro del espacio de un bloque. El campo `model` está reservado, pero no hay formato de
  modelo ni se dibujan todavía.

### Edición de bloques y guardado

- **Edición**: el cliente lanza un rayo desde la cámara (alcance 8 m), cambia el voxel en
  su copia al instante (predicción) y envía `SetBlock`. El servidor comprueba que el bloque
  existe y está al alcance, lo aplica y difunde `BlockChange` a quien tenga ese chunk. Si
  lo rechaza, devuelve al cliente el bloque real.
- **Los chunks nunca se modifican in situ**: se comparten con `Arc` entre hilos y, en el
  servidor integrado, entre servidor y cliente. Editar crea una copia (`Chunk::with_block`).
- **Remallado**: una edición sube la versión del chunk y lo vuelve a mallar con prioridad,
  junto con los vecinos que tocan ese voxel (hasta 7 más). Las mallas de versiones
  anteriores se descartan.
- **Guardado**: `worlds/<nombre>/world.toml` (semilla y tabla número → identificador de
  bloque) y `regions/r.X.Y.Z.bin` (regiones de 8³ chunks, solo los modificados). Se guarda
  cada 5 s y al cerrar, escribiendo a un temporal y renombrando. Un chunk guardado
  sustituye al generado.
- La tabla de bloques del mundo hace que cada identificador conserve su número entre
  sesiones aunque cambie el contenido.
- Contrapartida asumida: si cambia el generador, el terreno sin tocar cambia y puede no
  casar con lo construido. El terreno lejano tampoco refleja las ediciones.

### Terreno lejano (LOD)

Objetivo: la sensación de gran escala de Veloren (ver las montañas a lo lejos) sin que
apenas cueste rendimiento.

- Más allá de los voxeles reales, el mundo se dibuja desde el **mapa de alturas** como
  voxeles gruesos: celdas de 2 m (nivel 1) hasta ~20 chunks y de 4 m (nivel 2) hasta 32.
- El servidor genera los datos de cada tesela (alturas y bloques de 34×34 celdas, ~1-2 KiB)
  y el cliente los malla, sin vecinos, con el mismo mallador, formato de quad y shader
  (solo cambia la escala del chunk).
- La altura de cada celda es la **mínima** de las columnas que cubre: el terreno lejano
  nunca sobresale del detallado ni de un nivel más fino, así las uniones no dejan huecos.
- Quadtree de dos niveles. El servidor solo ordena soltar una tesela cuando ya ha enviado
  sus reemplazos, y el cliente espera a tenerlos mallados: nunca queda un hueco.
- El relevo voxeles/lejano es un círculo alrededor de la cámara (`clip`), con 8 m de
  solape. Al ganar altura el círculo se encoge y el terreno lejano lo cubre todo.
- Límite asumido: a lo lejos no se ven cuevas, voladizos ni construcciones del jugador.

## Restricciones técnicas

Para poder llegar a OpenGL / GLES 3 y móvil, **no usar nada fuera de los límites
"downlevel" de wgpu**:

- Sin push constants, sin storage buffers en el vertex shader, sin `first_instance`,
  sin multi-draw-indirect, sin compute.
- El dispositivo se pide con `Limits::downlevel_defaults()` para que el propio wgpu avise.
- Priorizar poca memoria, pocos triángulos y poco sobredibujado.
- El hilo principal nunca genera ni malla chunks.

## Arquitectura

```
content/          paquetes de contenido (ver content/README.md)
crates/common/src/
  block.rs        número de bloque y paleta para la GPU
  content.rs      carga de paquetes, definición de bloques, asignación de números
  chunk.rs        Chunk: Uniform(bloque) o Dense(32 KiB); serialización RLE
  lod.rs          teselas de terreno lejano: datos, quadtree, selección por distancia
  protocol.rs     ClientMsg / ServerMsg, versión, constantes de streaming
  net.rs          QUIC: endpoints, mensajes enmarcados, datagramas
crates/server/src/
  lib.rs          Server: eventos, sesiones, streaming por jugador, ticks, validación
  world.rs        chunks y teselas con autoridad, trabajos de generación, edición
  storage.rs      guardado del mundo: world.toml y archivos de región
  worldgen.rs     terreno procedural, caché por columna, árboles, datos de teselas
  noise.rs        ruido Perlin/fBm propio
  quic.rs         escucha de conexiones QUIC
  main.rs         servidor dedicado (voxel-server)
crates/client/src/
  app.rs          ventana, entrada, argumentos, bucle principal, mensajes del servidor
  net.rs          Connection: local (canales) o remota (QUIC)
  world.rs        copia local del mundo recibido; mallado, edición local y raycast
  mesher.rs       mallado greedy con aristas y oclusión; mallado de teselas (+ tests)
  camera.rs       cámara libre (posición en f64) y frustum
  render/         mod.rs (dispositivo, pase), terrain.rs, entities.rs, shaders/
```

Puntos clave:

- **Voxel**: 1 byte (`Block`), 0 = aire, el resto indexa una paleta de 256 colores.
- **Chunk**: los uniformes (cielo, roca maciza) no reservan memoria.
- **Quad**: 8 bytes, una instancia por quad; los 4 vértices los genera el vertex shader.
  ```
  [0]  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
  [1]  bloque:8 | ao:8 | libre:16
  ```
  Las tablas de caras (`FACES` en `mesher.rs`, `face_u`/`face_v` en el shader) deben
  coincidir siempre.
- **Mallado**: sobre una copia acolchada de 34³ (el chunk + una capa de sus 26 vecinos),
  por eso un chunk solo se malla cuando sus 26 vecinos están generados. El anillo exterior
  cargado nunca tiene malla.
- **Radios** (con distancia de voxeles V): el servidor envía V+4 chunks, el cliente malla
  V+2,5 y dibuja voxeles hasta V·32 m. El margen da vecinos y evita huecos al andar.
- **Streaming** (servidor, por jugador): lista de offsets ordenada por cercanía + cursor;
  como mucho 256 chunks esperando generación; margen de 2 chunks antes de descargar.
  El cliente sube mallas con presupuesto por frame.
- **Render**: coordenadas relativas al chunk de la cámara, Z invertida con plano lejano
  infinito, descarte por frustum y por caras que miran en contra, dibujo de cerca a lejos.
- **Origen de cada chunk**: un único buffer de uniforms con offset dinámico.
- **Uniones en T**: cada quad crece 0,75 píxeles (`QUAD_GROW_PIXELS`) para tapar los
  agujeros de un píxel que deja el mallado greedy.

## Estado actual

Hecho y verificado en una RX 570 (Vulkan, Wayland):

- Ventana, cámara libre, terreno procedural (colinas, montañas, playas, agua, árboles).
- Carga y descarga de chunks multihilo.
- Bisel de aristas, oclusión ambiental, luz de sol + ambiente, niebla.
- Terreno lejano hasta 32 chunks (1 km) con árboles simplificados.
- Paquetes de contenido: 11 bloques en `content/base`, recarga en caliente de colores con
  el juego abierto, y errores de contenido que no tumban el servidor.
- Edición y guardado: prueba de punta a punta (`crates/server/tests/editing.rs`) que
  edita, comprueba los rechazos, reinicia el servidor y verifica que el mundo conserva los
  cambios; y captura del mundo guardado con el contorno del bloque apuntado. Sin probar a
  mano el clic en el juego ni la edición con dos clientes a la vez.
- Cliente-servidor: en solitario (servidor integrado) y con servidor dedicado + dos
  clientes por QUIC en local, viéndose entre sí. Sin probar entre máquinas distintas.
- Release sin vsync, cámara fija, 1 km de visión, servidor integrado: carga inicial
  ~0,5 s, ~1 800 fps, 1,04 M de quads. Por QUIC en local: ~0,75 s de carga.

Provisional o pendiente:

- Red: el cliente **no verifica la identidad del servidor** (certificados autofirmados,
  cifrado sí). Sin autenticación de jugadores. Al cerrar, el cliente no avisa al servidor
  (este lo expulsa por inactividad a los 20 s).
- La validación de movimiento es solo un tope de velocidad; cuando haya jugador con física
  debe simularla el servidor. `trust_movement` la desactiva en el servidor integrado si se
  usa `VOXEL_CAMERA`.
- Contenido: máximo 255 bloques (1 byte por voxel). El generador aún no es configurable:
  usa por nombre 11 bloques de `base`, que deben existir.
- Edición: sin interfaz; el bloque elegido se ve en el título de la ventana y el punto de
  mira y el contorno son cajas del renderer de entidades. Romper es instantáneo (modo
  creativo de hecho). Se puede colocar un bloque dentro de un jugador.
- Guardado: las regiones leídas se quedan en memoria (solo contienen chunks modificados).
  No se guardan las posiciones de los jugadores.
- Los jugadores se dibujan como dos cajas; el propio no se dibuja.
- Solo cámara libre: no hay jugador, física ni colisiones.
- El agua es un bloque opaco; la transparencia necesita un segundo pase.
- El terreno y los árboles son de prueba, no diseño definitivo del juego.
- Terreno lejano: si se vuela muy rápido o al arrancar puede verse un instante el cielo
  dentro del círculo de voxeles; los árboles de nivel 2 son columnas toscas de 4 m.
- Un buffer de GPU por chunk (sin pool), chunks densos sin comprimir, sin sombras.
- Backend OpenGL sin empezar.
- wgpu 26 y winit 0.30 están fijados a propósito; subir de versión implica revisar la API.

Siguiente paso de la hoja de ruta: herramientas creativas (paso 3). También pendiente de
decidir por el usuario: jugador con colisiones.

## Comandos

```bash
cargo run --release                              # jugar solo (servidor integrado)
cargo run --release -- --host                    # jugar y abrir la partida a la red
cargo run --release -- --connect HOST[:PUERTO]   # unirse a un servidor
cargo run --release -- --world NOMBRE            # abrir o crear otro mundo
cargo run --release -p voxel-server              # servidor dedicado (--bind, --seed, --world, --content)
cargo test --workspace                           # tests
```

Puerto por defecto: 28700/UDP. `--name NOMBRE` cambia el nombre del jugador.

Controles: clic captura el ratón, `WASD`, `Espacio`/`Shift` subir y bajar, `Ctrl` rápido,
`Esc` suelta el ratón o sale. Con el ratón capturado: clic izquierdo rompe, derecho
coloca, central copia el bloque apuntado; rueda o `1`-`9` eligen bloque.

Opciones de desarrollo (variables de entorno):

| Variable | Efecto |
|---|---|
| `VOXEL_VIEW_DISTANCE=N` | Radio de voxeles reales en chunks (por defecto 12) |
| `VOXEL_FAR_DISTANCE=N` | Radio total con terreno lejano (por defecto 32, máx. 64) |
| `VOXEL_SEED=N` | Semilla del servidor integrado |
| `VOXEL_CONTENT=ruta` | Carpeta de contenido (por defecto `./content`) |
| `VOXEL_VSYNC=0` | Sin vsync, para medir fps |
| `VOXEL_CAMERA="x,y,z,yaw,pitch"` | Vista inicial, ángulos en grados |
| `VOXEL_SCREENSHOT=ruta.ppm` | Guarda un frame al terminar de cargar y sale |
| `RUST_LOG=voxel=debug` | Vuelca fps y contadores por consola |

Registro de carga del cliente: `world loaded in …` sale una sola vez, al terminar la carga
inicial. Después solo avisa (`nearby terrain was incomplete for …`, nivel warn) si la capa
de chunks de la cámara, dentro del radio de voxeles, estuvo más de 0,25 s sin estar lista
para dibujarse; es la señal de que el servidor o el mallado no dan abasto.

Para verificar un cambio visual: capturar con `VOXEL_SCREENSHOT` + `VOXEL_CAMERA`,
convertir el PPM a PNG y mirarlo. Ajustes de aspecto (`BEVEL_RADIUS`, `AO_MIN`, color del
cielo) al principio de `crates/client/src/render/mod.rs`; colores de bloques en
`content/base/blocks/`.
