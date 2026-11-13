# Tokikura / KuraVoxel

**KuraVoxel** es el motor y **Tokikura** el juego: un sandbox de voxeles en Rust, en la
línea de Minecraft / Cube World / Veloren. El motor es una biblioteca reutilizable; el
juego es un crate pequeño que la usa.
Este archivo recoge la dirección acordada del proyecto. Se conversa en español.

**Todo el código va en inglés**: identificadores, mensajes de registro y de error, ayuda
de línea de comandos, textos de la ventana y nombres de archivo. Solo los comentarios (y
esta documentación) pueden ir en español. El contenido del juego (`content/`) es dato, no
código: sus nombres visibles pueden estar en español.

## Visión

- Juego sandbox voxel. Primero el motor, después el juego.
- **Motor extensible y modular**: cada parte funciona por separado y se puede cambiar sin
  romper las demás. Lo específico de Tokikura nunca va en los crates `kuravoxel-*`.
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
| Distancias | Voxeles reales hasta 12 chunks, terreno lejano hasta 160 (5 km) | Elección del usuario: el mundo debe sentirse inmenso |
| Niebla | Sin niebla corta: bruma atmosférica leve + curvatura sutil; solo se funde con el cielo en el borde | Elección del usuario; el rendimiento lo dan los niveles de LOD |
| Estilo lejano | Cubos cada vez más gruesos (hasta 32 m), mismo mallador y shader | Elección del usuario; estética de maqueta |
| Árboles lejanos | Copas simplificadas en su posición real; desde 16 m por celda, color de bosque | Los bosques no desaparecen |
| Forma del mundo | Finito, tipo Chile: franja norte-sur de 5,12 × 32,8 km, océano al este, cordillera muy alta al oeste | Elección del usuario; provisional hasta el generador por datos |
| Red | Cliente-servidor autoritativo, multijugador nativo | Como Minecraft / Hytale |
| Un jugador | Servidor integrado en el propio juego | Un único camino de código |
| Transporte | QUIC (quinn) desde el principio | Elección del usuario; como Hytale |
| Terreno lejano en red | Lo genera y envía el servidor | Elección del usuario; como Veloren |
| Contenido | Paquetes de archivos TOML, un archivo por bloque | Como Hytale: el juego da las herramientas |
| Bloques | Color plano, sin texturas | Estilo del juego |
| Guardado | Solo los chunks modificados; el resto se regenera | Partidas diminutas; ver abajo |
| Mundos | Una carpeta por mundo, `worlds/<nombre>/` | Varios mundos de prueba |
| Interfaz | Iced para todas las pantallas del juego | Elección del usuario; como Veloren |
| Pantallas | Viven en el crate del juego (`tokikura`), no en el motor | Cambiar un menú no toca el motor |
| Nombres | Motor KuraVoxel, juego Tokikura | Elección del usuario |
| Objetos pequeños | Mini-voxeles a 8 por metro (12,5 cm), modelados en MagicaVoxel (`.vox` en `models/`) | Elección del usuario; encaja con la estética de maqueta |
| Objetos de varias celdas | Un estado por celda, como Minecraft, pero un solo bloque e ítem: la puerta de 8×16×2 se coloca y se rompe entera | Encaja con chunks, colisión y edición |
| Iconos de ítems | Como Hytale: se generan del modelo con `[icon]` (giro, escala, desplazamiento) o son un PNG dibujado a mano (`[icon] image`) | Elección del usuario; fácil de retocar |
| Ítems sueltos | `Q` tira el ítem activo; el servidor lo simula (cae, se posa) y lo da a quien pasa por encima; flota y gira, bloques a 1/4 y modelos a 1/2 | Elección del usuario; como Minecraft |
| Inventario | Paleta creativa en el cliente, sin objetos ni cantidades | Elección del usuario; aún no hay supervivencia |
| Ítems | Definidos por datos (`items/*.toml`); cada bloque genera su ítem; sin cantidades ni inventario en el servidor | Elección del usuario; el formato queda listo para la supervivencia |
| Prefabs | Se crean con una herramienta del juego (seleccionar → guardar, copiar/pegar); `prefabs/<nombre>.toml` (metadatos + paleta por identificador) + `.bin` con los voxeles | Como Hytale: nadie edita voxeles a mano |
| Números de bloque | `u16` global por **estado** de bloque + paleta por chunk (1 byte por voxel salvo chunks con > 256 estados) | Quita el tope de 255 sin gastar memoria |
| Propiedades de bloque | Colisión, categoría y etiquetas, herencia (`parent`), transparencia, colores por cara con orientación, emisivo | Elección del usuario |
| Rotaciones | Colores por cara (arriba / lados / abajo / frente) y modelos de mini-voxeles, orientados por eje o hacia quien coloca | Un cubo de color plano no cambia al girarlo |
| Luz | Bloques emisivos (brillan, no iluminan); la luz por propagación y las cuevas oscuras, en una fase propia | Casi gratis ahora |
| Entidades y enemigos | Más adelante; carpeta `entities/` reservada con el mismo patrón de carga | Necesitan IA, física en el servidor y animación |
| Consola | Mixta: comandos del cliente en el juego, los de autoridad en el servidor | Elección del usuario; encaja con el scripting |
| Menús | Provisionales en código Iced | Se rehacen con la herramienta de edición de menús |
| Física del jugador | La simula el cliente; el servidor solo limita la velocidad | Elección del usuario; el código vive en `common` para que el servidor la re-simule más adelante |
| Movimiento | Tipo Hytale: más ágil que Minecraft, menos que Veloren; sube solo escalones de 1 bloque | Elección del usuario |
| Modos de juego | Supervivencia (a pie), Creativo (vuelo con colisiones), Espectador (vuelo sin colisiones); `F5` los recorre | Elección del usuario |
| Cámara | 1.ª y 3.ª persona hasta 32 m; cerca el personaje mira con la cámara (Hytale), desde 8 m la cámara orbita y el personaje gira hacia donde camina (Veloren) | Elección del usuario |
| Vista de mapa | Pasados los 32 m la cámara salta a 160-3200 m: sin interfaz ni edición, solo moverse y mirar el terreno lejano | Elección del usuario; tipo Cube World; al máximo se ve el ancho entero del mundo |

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

- **Reparto**: `kuravoxel-common` (datos, protocolo, QUIC), `kuravoxel-server`
  (generación, mundo, jugadores, ticks) y `kuravoxel-client` (ventana, mallado, render). El
  cliente no puede acceder al generador de mundo: no está en su código.
- **Servidor integrado**: jugar solo arranca `kuravoxel_server::start` en un hilo y el
  cliente le habla por canales en memoria (sin serializar, chunks compartidos con `Arc`).
  En multijugador los mismos mensajes viajan por QUIC. La lógica del servidor no distingue.
- **Transporte**: un stream bidireccional fiable (mensajes con prefijo de longitud,
  `postcard`) y datagramas para lo que caduca (movimiento, fotos de entidades).
- **Servidor**: atiende eventos en cuanto llegan (conexiones, mensajes, chunks generados)
  y simula a 20 ticks por segundo. Decide qué chunks y teselas lejanas ve cada jugador,
  los envía de cerca a lejos y ordena descargarlos.
- **Movimiento**: el cliente mueve su cámara al instante y envía su posición a ritmo de
  tick; el servidor valida la velocidad y, si no cuadra, responde con `Teleport`.
- **Otros jugadores**: el servidor difunde sus posiciones cada tick; el cliente las suaviza.
- Los chunks viajan comprimidos por tramos (RLE): un chunk denso suele ocupar < 1 KiB.
- **Comandos de consola**: los que necesitan autoridad (`tp`, `setblock`, `fill`, `seed`,
  `save`, `players`)
  viajan como `ClientMsg::Command` y el servidor responde con `CommandOutput`. Solo puede
  usarlos el **operador**: la conexión del anfitrión (`ServerHandle::connect_operator`, la
  que usa el servidor integrado). Por QUIC nadie lo es todavía.

### Jugador, movimiento y cámara

- **Física** (`kuravoxel-common/src/physics.rs`): una caja de 0,6 × 1,8 m (ojos a 1,62)
  contra un `Collider`, que da la forma de cada voxel: nada, cubo entero o cajas en
  mini-voxeles (losas, escaleras; `shape.rs` las deriva del contenido). Una función
  `Fn(IVec3) -> bool` también sirve. No sabe de chunks ni de red. Se mueve eje a eje y
  solo cuentan las cajas que tiene por delante, así que un cuerpo dentro de un bloque
  puede salir; a pie, además, sube solo hasta quedar libre. Una losa es medio escalón.
- **Números** (constantes al principio del archivo): andar 5,6 m/s, correr 8,2, salto de
  1,3 bloques, gravedad 30 m/s². En el suelo acelera y frena en ~0,1 s; en el aire
  conserva la inercia. Vuelo 12 / 32 m/s; espectador 14 / 112.
- **Escalones**: si un bloque corta el paso estando en el suelo, se prueba el mismo
  movimiento desde un bloque más arriba y se acepta si así se avanza más. No frena la
  carrera; el cuerpo sube de golpe y la vista le sigue suavizada (`Body::step_offset`).
- **`MoveMode`** (`Walk`, `Fly`, `Ghost`) es del motor; los **modos de juego** con nombre
  (`game_mode.rs`) son de Tokikura y solo eligen un `MoveMode`.
- **`player.rs`** (cliente): traduce la entrada (WASD siempre relativo a la cámara) a un
  paso de física y coloca la cámara. El cuerpo no se mueve hasta que llega el chunk de
  sus pies, y lo que aún no se ha recibido cuenta como sólido.
- **Cámara en 3.ª persona**: orbita un punto algo por encima de la cabeza y mira justo a
  él, así el punto de mira y el rayo de edición señalan lo mismo que en 1.ª persona. Un
  rayo hacia atrás la acerca de golpe si el terreno estorba y la suelta con suavidad.
- **Vista de mapa**: es la misma cámara muy alejada e inclinada hacia abajo (mínimo ~34°).
  El radio de voxeles reales (`clip`) baja a 0, así que todo lo dibuja el terreno lejano
  ya recibido; un poste marca al jugador. No hay datos ni render nuevos: el mapa llega
  hasta donde llega el terreno lejano (5 km con los ajustes por defecto). A 3,2 km de
  distancia se ve el ancho entero del mundo, de la cordillera al mar.
- El servidor recibe la posición de los ojos y el giro del **personaje** (no el de la
  cámara), y los chunks se piden alrededor del jugador, no de la cámara.

### Motor y juego

- Crates del motor: `kuravoxel-common`, `kuravoxel-server`, `kuravoxel-client` y
  `kuravoxel-ui`. Ninguno sabe nada de Tokikura.
- `tokikura` es el juego: implementa `kuravoxel_client::app::Game` y decide cuándo se
  entra o sale de un mundo y qué pantalla se ve. Da las órdenes al motor con `Context`
  (`start_session`, `end_session`, `exit`, `configure`, `select_block`, `grab_mouse`,
  `server_command`, `set_move_mode`, `zoom_camera`, `set_camera_distance`).
- El juego no toca la partida: cada frame recibe en `Context` una copia (`SessionInfo`:
  contenido, bloque elegido, posición, estado) y los avisos (`SessionEvent`: bloque
  copiado, respuesta a un comando). Por eso la lógica del juego se prueba sin GPU.
- **El motor no tiene barra ni inventario**: solo sabe "qué bloque coloca el clic
  derecho". Qué bloques se ofrecen y con qué teclas se eligen lo decide el juego.
- Capas del cliente, cada una usable sin las de encima: `render` (dibuja lo que le dan) →
  `world` + `mesher` → `net` → `session` (una partida) → `app` (ventana y bucle).

### Interfaz (Iced)

Todas las interfaces del juego se hacen con Iced, y deben ser fáciles de modificar.

- **El motor no depende de Iced.** `kuravoxel-client` solo ofrece al juego el dispositivo
  gráfico (`render::Gpu`) y tres ganchos opcionales en `Game`: `window_event`,
  `wants_input` y `draw_overlay` (pintar encima de la escena ya dibujada).
- **`kuravoxel-ui`** une esos ganchos con Iced: `IcedLayer` convierte los eventos de
  ventana, guarda el estado de los widgets (por tipo de pantalla) y dibuja sobre la imagen
  final. Define el trait `Screen` (estado + `Message` + `update` + `view`, al estilo Elm).
- **Pantallas apiladas**: en un frame, `IcedLayer::draw` pinta una pantalla sin entrada
  (el HUD) y `IcedLayer::frame` pinta encima la que recibe ratón y teclado. `focus` da el
  foco a una caja de texto y `set_scale` cambia el tamaño de toda la interfaz.
- **Una pantalla es un archivo** en `crates/tokikura/src/screens/`. No toca GPU, ventana
  ni motor: cuando quiere que pase algo fuera (abrir un mundo, salir) lo anota como una
  petición (`Request`) que `game.rs` recoge, y lo que enseña del juego (barra, contenido)
  se lo copia `game.rs` antes de dibujarla. Por eso su lógica se prueba con tests normales.
- Para añadir una pantalla: nuevo archivo en `screens/`, implementar `Screen`, añadir su
  variante a `Shown` en `game.rs`, dibujarla en `draw_overlay` y atender sus peticiones en
  `handle_requests`.
- **Atajos de teclado**: los decide `game.rs` (`key_pressed`) antes de que el evento llegue
  a las pantallas o a la partida. Con una caja de texto abierta, letras y números no son atajos.
- **Aspecto compartido** en `screens/style.rs` (fondo, panel, botón de menú, ranura de
  bloque). Las pantallas actuales son **marcadores de posición**: la meta es una
  herramienta para editar menús sin tocar código, así que lo que debe durar es la lógica
  (mensajes y peticiones), no las vistas.
- wgpu y winit van **a la par de los de Iced** (iced 0.14 → wgpu 27, winit 0.30), porque
  motor e interfaz comparten dispositivo y eventos. Subir Iced implica subir wgpu.

Pantallas que existen (todas provisionales):

| Pantalla | Archivo | Qué hace |
|---|---|---|
| Portada | `main_menu.rs` | Jugar, ajustes, salir |
| Mundos | `world_select.rs` | Lista con semilla; jugar, borrar (dos clics), crear |
| Crear mundo | `world_create.rs` | Nombre y semilla (número, texto o vacía = aleatoria) |
| Ajustes | `settings.rs` | Sobre un borrador; se aplica y guarda al pulsar "Done" |
| Pausa | `pause_menu.rs` | Volver, ajustes, salir al menú, cerrar el juego |
| HUD | `hud.rs` | Punto de mira, barra de 9, aviso de carga, datos de F3 |
| Inventario | `inventory.rs` | Creativo: todos los ítems por categoría y con buscador → barra |
| Consola | `console.rs` | Salida, registro del motor, entrada con historial |

- **Ajustes del jugador** (`settings.rs` → `settings.toml`): nombre, distancia de voxeles
  y lejana, campo de visión, sensibilidad del ratón, vsync y escala de la interfaz. Las
  distancias se aplican al entrar en un mundo; lo demás al momento.
- **Barra de herramientas** (`hotbar.rs`): 9 ranuras con un **ítem** cada una, estado
  del juego y solo del cliente. Se rellena con los primeros ítems que colocan bloque al
  entrar en un mundo y no se guarda. `game.rs` traduce el ítem activo a "qué bloque
  coloca el clic derecho" para el motor; copiar con el clic central busca el ítem del
  bloque apuntado.
- **Inventario creativo** (`inventory.rs`): pestañas "All", una por categoría (las de
  los TOML) y "Search". Clic izquierdo en un ítem → ranura activa; derecho → primera
  ranura libre; clic derecho en una ranura la vacía. En la pestaña de búsqueda la caja
  tiene el foco y `E` y los números se escriben (no son atajos); `Esc` cierra.
- **Iconos** (`icons.rs`): al cambiar el contenido, el juego crea el icono de cada ítem
  (imagen propia o el generado por el motor, `kuravoxel_client::icons`) y lo convierte en
  imagen de Iced; las ranuras (`style::slot`) lo enseñan y, sin icono, el color. Los
  dibujados a mano se amplían sin suavizar.
- **Ítems sueltos**: `Q` (o `drop [ítem]`) tira el ítem activo sin quitarlo de la barra
  (es creativo); al recoger uno (`SessionEvent::PickedUp`) va a la primera ranura libre si
  no estaba, sin cambiar la activa.
- **Consola** (`commands.rs`): los comandos del cliente (`help`, `clear`, `pos`, `stats`,
  `blocks [texto]`, `items [texto]`, `give <ítem> [ranura]`, `block <ítem>`, `look`,
  `drop [ítem]`, `icons`, `mode [nombre]`, `zoom <metros>`, `set [clave valor]`, `menu`,
  `quit`) se resuelven en
  el juego; cualquier otra línea se envía al servidor. Los ítems se pueden escribir
  completos (`base:lamp`), solo por nombre (`lamp`) o como estado de bloque.

### Contenido y herramientas

Meta: como en Hytale, el juego proporciona las herramientas con las que jugadores y
desarrolladores crean su contenido. **El contenido son datos, no código.** Hoja de ruta
acordada, por orden:

1. **Paquetes de contenido** (hecho): bloques en archivos, recarga en caliente.
2. **Edición de bloques y guardado** (hecho): poner/romper validado por el servidor.
3. Bloques, ítems y prefabs (en curso), por fases:
   a. (hecho) Números de bloque `u16` con paleta por chunk (chunk, red, guardado, GPU),
      con conversión de los mundos guardados.
   b. (hecho) Formato de contenido: estados de bloque, propiedades nuevas, `parent`,
      plantillas, ítems; inventario creativo y comandos (`give`, `setblock`, `fill`...).
   c. (hecho) Render y física: colores por cara, emisivo, bloques no sólidos,
      transparencia, y modelos de mini-voxeles (`.vox`) con orientación y colisión.
   d. Prefabs y herramienta de selección: guardar, copiar/pegar, colocar validado por el servidor.
   e. Paleta creativa con categorías e ítems (adelantada a la 3b).
   Después: pinceles y demás herramientas creativas.
4. Generador de terreno por datos (biomas, ruido, prefabs), con recarga en caliente.
5. Scripting para mods, en el servidor.

Cómo funciona el contenido (detalle del formato en `content/README.md`):

- `content/<paquete>/blocks/<bloque>.toml` define un bloque y `items/<ítem>.toml` un
  ítem. El identificador es `paquete:archivo` (p. ej. `base:grass`). Un mod es otra carpeta.
- **Herencia**: `parent = "id"` toma todos los campos de otro archivo y cambia los que
  escriba; `template = true` marca una plantilla que solo sirve de padre.
- **Bloque, estado e ítem** (`content.rs`): un bloque (`BlockDef`) es un tipo; cada
  orientación suya es un **estado** con su propio número (`Block`, lo que va en los
  chunks), con nombre `base:wood[axis=x]`; sin corchetes es el estado por defecto. Cada
  bloque genera un ítem con su mismo id (salvo `item = false`); los de `items/` añaden
  más (p. ej. herramientas, `tool` reservado para la 3d) o sustituyen esos.
- Propiedades de bloque: colores por cara (`[faces]` top/side/bottom/front),
  `orientation` (`axis` u `horizontal`), `solid`, `collision`, `transparent` +
  `opacity`, `light` (0-15), `model` + `cell`, `category`, `tags`. Colocar un bloque
  orientable elige el estado (`ContentData::orient`).
- **Tabla de bloques** (`shape.rs`, `BlockTable`): por número de bloque, lo que
  consultan los bucles internos, con la orientación ya aplicada: colores de las 6 caras,
  celda del modelo girada, colisión (cajas), opaco / translúcido, si el rayo lo alcanza y
  su luz. La usan la física y el rayo (`World`) y, convertida en `MeshTable` junto con la
  paleta de GPU (`mesher::build_tables`), el mallado. Se reconstruye al recibir
  contenido; si cambia la geometría, se vuelve a mallar todo.
- Solo el servidor lee los archivos. Numera estados e ítems y envía el resultado
  (`ContentData`) a los clientes al conectar.
- **El código nunca debe fijar números de bloque** (solo `Block::AIR` = 0). Quien necesite
  un bloque lo resuelve por nombre, como hace `TerrainBlocks` en el generador.
- Recarga en caliente: el servidor mira la carpeta cada 0,5 s; si algo cambia, relee todo
  y lo reenvía. Estados e ítems existentes conservan su número (por nombre), los nuevos
  van al final, los borrados dejan hueco; un bloque que pasa a orientarse conserva su
  número en el estado por defecto. Un archivo con errores (o un ciclo de `parent`) no
  rompe nada: sigue el contenido anterior.
- **Modelos** (`model.rs`): `.vox` de MagicaVoxel en `content/<paquete>/models/`, leídos
  por el servidor y enviados con el contenido. X de MagicaVoxel = este, Z = arriba, +Y =
  norte = frente del modelo. Una celda es 8³; un modelo menor se centra en X/Z y se apoya
  abajo. Al cargar el contenido, el cliente fusiona cada celda (ya girada) en quads de
  1/8 de voxel; el mallador los copia en la malla del chunk (sin fusión entre celdas ni
  oclusión). Cada modelo guarda qué casillas ocupa en cada lado de su celda: un quad de
  borde se omite si el vecino (cubo opaco o modelo) las tapa, y un modelo que llena un
  lado entero tapa al vecino como un cubo. Así dos losas juntas no tienen costura. La
  colisión sale de descomponer la celda en ≤ 16 cajas. `tools/make_vox.py` genera los de
  ejemplo.
- **Objetos de varias celdas**: un modelo mayor que 8³ sin `cell` hace del bloque un
  objeto de varias partes (`BlockDef::parts`, la primera es el anclaje): un estado por
  orientación y parte (`base:door[facing=east,part=0-1-0]`), un solo ítem.
  `ContentData::structure` da todas sus celdas desde cualquiera, giradas con el bloque.
  El cliente coloca el anclaje y predice el resto; el servidor (`edit_cells`) solo acepta
  el anclaje, exige todas las celdas libres (aire o agua) y lo pone entero; romper una
  celda quita el objeto entero. `setblock` y `fill` ponen estados sueltos, sin expandir.
- **Ítems sueltos** (`drops.rs` en el servidor): `ClientMsg::DropItem` → el servidor
  crea el ítem delante de los ojos, lo simula a 20 Hz como una caja de 25 cm
  (`physics::step_loose`, la misma colisión por cajas), lo difunde (`DropSpawned`, las
  posiciones de los que se mueven en `Entities`, `DropRemoved`) y, pasados 1,5 s, lo da a
  quien lo toca (`PickedUp`). Caducan a los 5 min; como mucho 512. No se guardan. El
  cliente los dibuja con `render/items.rs`: la forma del ítem (`icons::item_shape`, la
  misma del icono), una cara por instancia y una matriz por ítem, flotando y girando.

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
- **Guardado**: `worlds/<nombre>/world.toml` (semilla, tabla número → nombre de estado
  de bloque y tabla de ítems) y `regions/r.X.Y.Z.bin` (regiones de 8³ chunks, solo los modificados). Se guarda
  cada 5 s y al cerrar, escribiendo a un temporal y renombrando. Un chunk guardado
  sustituye al generado. Las regiones llevan número de formato: el 1 (bloques de 8 bits,
  anterior a la paleta por chunk) se convierte al leerlo y se reescribe en el formato
  actual (2) la próxima vez que se edite esa región.
- La tabla de bloques del mundo hace que cada identificador conserve su número entre
  sesiones aunque cambie el contenido.
- Contrapartida asumida: si cambia el generador, el terreno sin tocar cambia y puede no
  casar con lo construido. El terreno lejano tampoco refleja las ediciones.

### Forma del mundo

- Zona jugable de 5,12 km (X, de oeste a este) por 32,8 km (Z, de norte a sur), centrada
  en el origen (`WORLD_WIDTH` / `WORLD_LENGTH` en `worldgen.rs`). El servidor la envía en
  `Welcome` (`WorldBounds`); cliente y servidor no dejan salir de ella a nadie, ni con `tp`.
- Perfil de oeste a este: cordillera (picos de ~400-600 m, nieve desde 320, roca desde
  150), valle central (~25 m), cordillera de la costa, costa y océano. La costa y la
  cordillera serpentean a lo largo de Z.
- **Fuera de la zona jugable el terreno se sigue generando** (más cordillera al oeste,
  mar abierto al este, la franja sigue al norte y al sur): el horizonte nunca muestra un
  corte artificial. El mundo es finito para el jugador, no para la vista.
- Es un generador provisional en código, a la espera del generador por datos (paso 4).

### Terreno lejano (LOD)

Objetivo: la sensación de gran escala de Veloren (ver las montañas a lo lejos) sin que
apenas cueste rendimiento y sin niebla que limite la vista.

- Más allá de los voxeles reales, el mundo se dibuja desde el **mapa de alturas** como
  voxeles gruesos. Quadtree de niveles 1 a 6 (`lod.rs`): una tesela de nivel `n` tiene
  32×32 celdas de `2^n` m. Se divide mientras su punto más cercano esté a menos de 5 lados
  de tesela (`SPLIT_DISTANCE`): celdas de 2 m hasta ~640 m, 4 m hasta 1,3 km, 8 m hasta
  2,6 km y 16 m hasta 5 km. El nivel 1 llega siempre algo más allá de los voxeles.
- El servidor genera los datos de cada tesela (alturas y bloques de 34×34 celdas, ~1-2 KiB)
  y el cliente los malla, sin vecinos, con el mismo mallador, formato de quad y shader
  (solo cambia la escala del chunk).
- Niveles 1 y 2: la altura de cada celda es la **mínima** de las columnas que cubre, así
  las uniones con los voxeles no dejan huecos. Desde el nivel 3 se **muestrea** (mínimo de
  3×3 puntos por celda): cada tesela cuesta ~5-10 ms en cualquier nivel, que es lo que
  permite llegar a 5 km. Árboles: en el nivel 3, un bloque de copa por celda con árbol;
  desde el 4, la celda se pinta de hojas si tiene bastantes.
- El servidor solo ordena soltar una tesela cuando ya ha enviado las que cubren su
  terreno en otros niveles (`lod::overlaps`), y el cliente espera a tenerlas malladas:
  nunca queda un hueco.
- **Bruma y curvatura** (`render/mod.rs`): sin niebla corta. La perspectiva aérea tiñe
  lo lejano del color del cielo (`HAZE_STRENGTH`, ~45 % en el borde) y solo el último 14 %
  de la distancia se funde del todo con él (`HAZE_EDGE`). La bruma se mide desde el
  jugador, no desde la cámara, para que el mapa se vea nítido. El terreno baja `d²/2R`
  con la distancia a la cámara (`CURVATURE_RADIUS` = 200 km, ~60 m a 5 km); más curvatura
  escondería el borde, pero el mundo parecería un planeta diminuto.
- El relevo voxeles/lejano es un **cilindro** (`SceneView`): `clip` metros en horizontal
  alrededor de la cámara y, en vertical, las capas de chunks que el servidor envía con
  todos sus vecinos (±3 chunks del de los ojos, es decir, ±96-128 m). El terreno lejano
  se dibuja fuera de él (lejos, muy abajo o muy arriba) con 8 m de solape. Así, desde una
  cumbre a 550 m el valle de 250 m sigue viéndose, y al volar alto el suelo lo cubre el
  terreno lejano sin trucos de altitud.
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
tools/ppm2png.py  convierte las capturas de desarrollo a PNG
tools/make_vox.py genera los modelos .vox de ejemplo de content/base/models
crates/kuravoxel-common/src/     compartido por cliente y servidor
  block.rs        número de bloque y paleta para la GPU
  content.rs      paquetes: bloques, estados, ítems, herencia y numeración estable
  chunk.rs        Chunk: Uniform, Paletted (paleta + 1 byte/voxel) o Wide; serialización RLE
  lod.rs          teselas de terreno lejano: datos, quadtree, selección por distancia
  protocol.rs     ClientMsg / ServerMsg, versión, constantes de streaming
  net.rs          QUIC: endpoints, mensajes enmarcados, datagramas
  physics.rs      física del jugador: caja contra voxeles y cajas, escalones, vuelo (+ tests)
  model.rs        modelos de mini-voxeles: lectura de .vox, celdas, giros, cajas (+ tests)
  icon.rs         datos de iconos ([icon]) y PNG (+ tests)
  shape.rs        BlockTable: propiedades orientadas por número de bloque (+ tests)
crates/kuravoxel-server/src/     servidor autoritativo (biblioteca)
  lib.rs          Server: eventos, sesiones, streaming por jugador, ticks, validación
  commands.rs     comandos de consola del servidor (tp, setblock, fill, seed, save, players)
  drops.rs        ítems sueltos: lanzar, simular, recoger, caducar (+ tests)
  world.rs        chunks y teselas con autoridad, trabajos de generación, edición
  storage.rs      guardado del mundo: world.toml y archivos de región
  worldgen.rs     terreno procedural, caché por columna, árboles, datos de teselas
  noise.rs        ruido Perlin/fBm propio
  quic.rs         escucha de conexiones QUIC
crates/kuravoxel-client/src/     cliente del motor (biblioteca)
  app.rs          ventana y bucle principal; trait Game, Context, Host
  session.rs      una partida: conexión, mundo local, cámara, entrada, edición;
                  SessionInfo / SessionEvent (lo que ve el juego) y ClientSettings
  net.rs          Connection: local (canales) o remota (QUIC)
  world.rs        copia local del mundo recibido; mallado, edición local y raycast
  mesher.rs       mallado greedy con aristas, oclusión, translúcidos y modelos; paleta
                  de GPU (build_tables); mallado de teselas (+ tests)
  player.rs       jugador local: entrada → física, cámara en 1.ª/3.ª persona y mapa
  camera.rs       punto de vista (posición en f64) y frustum
  icons.rs        forma 3D de un ítem (modelo o cubo) e iconos generados en CPU (+ tests)
  render/         mod.rs (dispositivo, pase, Gpu), terrain.rs, entities.rs, items.rs
                  (ítems sueltos), shaders/
crates/kuravoxel-ui/src/lib.rs   Iced sobre la escena: IcedLayer y trait Screen
crates/tokikura/src/             el juego
  main.rs         argumentos y ajustes; arranca el motor
  game.rs         Tokikura: implementa Game, elige pantalla, atajos, abre y cierra mundos
  game_mode.rs    modos de juego (supervivencia, creativo, espectador)
  settings.rs     ajustes del jugador y su archivo (settings.toml)
  hotbar.rs       barra de herramientas de 9 ranuras (ítems)
  icons.rs        iconos de los ítems para Iced; exportación a PNG
  commands.rs     comandos de consola del cliente
  log_capture.rs  copia del registro para la consola
  worlds.rs       mundos en disco: listar (con semilla), validar nombre, semillas, borrar
  screens/        una pantalla de Iced por archivo, más style.rs (aspecto compartido)
  bin/server.rs   servidor dedicado (tokikura-server)
```

Puntos clave:

- **Voxel**: `Block` es un `u16` (0 = aire, hasta 65 535 bloques), pero en memoria cada
  chunk guarda su paleta y 1 byte por voxel.
- **Chunk**: `Uniform` (cielo, roca maciza) no reserva memoria; `Paletted` ocupa 32 KiB
  + paleta (hasta 256 bloques distintos); `Wide` (64 KiB, `u16` por voxel) solo si un
  chunk mezcla más de 256. Se rellena un `ChunkVoxels` y `Chunk::from_dense` elige la forma.
- **Paleta de la GPU**: textura RGBA8 sRGB de 2048 × N, leída en el vertex shader.
  8 entradas por número de bloque (`bloque · 8 + cara`, color ya orientado; el alfa es
  la opacidad) y detrás los colores de los modelos. Crece si el contenido no cabe.
- **Quad**: 8 bytes, una instancia por quad; los 4 vértices los genera el vertex shader.
  El bit 31 de `[1]` distingue un cubo de un quad de mini-voxeles (modelos):
  ```
  cubo [0]  x:5 | y:5 | z:5 | ancho-1:5 | alto-1:5 | cara:3 | aristas:4
       [1]  bloque:16 | ao:8 | luz:4 | libre:3 | 0
  mini [0]  celda xyz:5·3 | desplazamiento xyz:3·3 | ancho-1:3 | alto-1:3 | libre:2
       [1]  color:20 | cara:3 | libre:1 | luz:4 | libre:3 | 1
  ```
- **Translúcido**: el mallador deja los quads de agua y cristal al final de la malla y
  el renderer los dibuja en un segundo pase (mezcla, sin escribir profundidad, sin
  descartar caras traseras), chunk a chunk de lejos a cerca y después de las entidades.
  No crecen contra las uniones en T (el solape se mezclaría dos veces). A lo lejos todo
  es opaco (`mesh_lod_tile` trata cualquier bloque como cubo).
- **Emisivo**: los 4 bits de luz del quad mezclan el color iluminado con el propio color
  aclarado (`EMISSIVE_BOOST` en el shader).
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

- Ventana, terreno procedural (colinas, montañas, playas, agua, árboles).
- Jugador con física y colisiones, tres modos de juego y cámara en 1.ª/3.ª persona y
  vista de mapa. La física y la cámara tienen tests (`physics.rs`, `player.rs`) y con
  capturas se comprobó aparecer en el suelo, la 3.ª persona a 8 y 32 m y la vista de
  mapa. **Sin probar a mano**: cómo se siente andar, saltar y subir escalones, `F5`, `V`,
  Alt + rueda y la cámara rozando paredes.
- Carga y descarga de chunks multihilo.
- Bisel de aristas, oclusión ambiental, luz de sol + ambiente, niebla.
- Iconos, puerta entera e ítems sueltos (con tests y capturas): inventario y barra con
  los iconos generados (cubos en tres cuartos, puerta, flor, escalera, espada en diagonal),
  `icons` exportando los PNG; puerta colocada y rota entera (prueba de punta a punta en
  `tests/editing.rs`); ítems tirados que caen y se recogen (`tests/drops.rs`) y que se
  ven flotando con su modelo. Dos losas juntas ya no tienen costura. **Sin probar a
  mano**: `Q`, recoger andando, ver girar los ítems, colocar y romper la puerta con el ratón.
- Fase 3c (con tests y capturas): colores por cara orientados (troncos tumbados con los
  anillos en la punta, horno mirando a quien lo coloca), lámpara y farol que brillan,
  agua y cristal translúcidos (segundo pase), y modelos `.vox` (flor, losa, escalera
  orientada, puerta de dos celdas, poste, farol) en una escena montada con `setblock`.
  Física con cajas: tests de subir a una losa como medio escalón y de salir de ella.
  Rendimiento igual que antes: ~780 fps desde el suelo con ~2 M de quads. **Sin probar a
  mano**: andar sobre losas y escaleras, atravesar flores y agua, y verse bajo el agua.
- Terreno lejano hasta 160 chunks (5 km) con 5 niveles de LOD, bruma y curvatura, en el
  mundo tipo Chile. Con capturas: desde el suelo se ve la cordillera nevada en el
  horizonte, y la vista de mapa a 3,2 km enseña el ancho entero, de la cordillera al mar.
- Interfaz con Iced: portada, mundos, crear mundo, ajustes, pausa, HUD, inventario y
  consola. Con capturas se comprobó cada pantalla, el recorrido portada → mundos → jugar
  con clics guionizados, y la consola de punta a punta (`pos`, `block`, `set fov`, `tp`
  contra el servidor integrado). **Sin probar a mano**: teclas (Escape, E, F1, F3, 1-9),
  rueda, escribir en las cajas de texto y la recaptura del ratón al cerrar una pantalla;
  eso solo lo cubren los tests de lógica de `game.rs`.
- Comandos del servidor: prueba de punta a punta
  (`crates/kuravoxel-server/tests/commands.rs`): el anfitrión los ejecuta y a un
  invitado se le rechazan.
- Paquetes de contenido: 23 bloques (37 estados), 25 ítems y 7 modelos en `content/base`,
  con plantillas, herencia y bloques orientables; recarga en caliente de colores con el juego
  abierto, y errores de contenido que no tumban el servidor. Tests de herencia, estados,
  numeración estable y errores (`content.rs`). Con capturas: el inventario creativo (todo
  y filtrado por categoría), y `give`, `items`, `blocks`, `setblock`, `fill` y sus errores
  en la consola, con el resultado guardado tras reiniciar. Un mundo anterior a la 3b
  (`base:wood` sin orientación) carga con su madera en `base:wood[axis=y]`. **Sin probar a
  mano**: el buscador del inventario escribiendo de verdad y el clic derecho en el juego.
- Comandos `setblock` y `fill`: prueba de punta a punta en
  `crates/kuravoxel-server/tests/commands.rs` (por nombre de estado, `~` relativo,
  reenvío de chunks enteros tras `fill`, errores).
- Edición y guardado: prueba de punta a punta (`crates/kuravoxel-server/tests/editing.rs`) que
  edita, comprueba los rechazos, reinicia el servidor y verifica que el mundo conserva los
  cambios; y captura del mundo guardado con el contorno del bloque apuntado. Sin probar a
  mano el clic en el juego ni la edición con dos clientes a la vez.
- Cliente-servidor: en solitario (servidor integrado) y con servidor dedicado + dos
  clientes por QUIC en local, viéndose entre sí. Sin probar entre máquinas distintas.
- Release sin vsync, servidor integrado, 5 km de visión: carga inicial ~1,7 s, ~2 M de
  quads (1 222 teselas lejanas), 600-800 fps desde el suelo y ~385 fps en la vista de
  mapa a 3,2 km (casi 3 000 draw calls). Antes, con 1 km: ~0,5 s, ~1 800 fps y 1,04 M de
  quads. Con el HUD de Iced en pantalla el coste extra es de unos cientos de fps.

Provisional o pendiente:

- Red: el cliente **no verifica la identidad del servidor** (certificados autofirmados,
  cifrado sí). Sin autenticación de jugadores. Al cerrar, el cliente no avisa al servidor
  (este lo expulsa por inactividad a los 20 s).
- La validación de movimiento es solo un tope de velocidad (160 m/s): un cliente
  modificado puede volar o atravesar bloques. El modo de juego es cosa del cliente; el
  servidor no lo conoce. `trust_movement` desactiva el tope en el servidor integrado si
  se usa `KURAVOXEL_CAMERA`.
- Jugador: sin agacharse, nadar ni daño de caída; el agua se atraviesa como el aire (se
  cae al fondo, sin flotar ni frenar). Los demás jugadores no colisionan entre sí. Se
  aparece siempre en (0, 0).
- Cámara: en 3.ª persona puede asomar un poco a través de una pared lateral (solo se
  comprueba el rayo central). La vista de mapa no enseña cuevas ni construcciones y no
  se puede desplazar más allá del terreno lejano cargado.
- Contenido: el generador aún no es configurable:
  usa por nombre 11 bloques de `base`, que deben existir.
- Interfaz: todas las pantallas son provisionales y sin estilo propio, a la espera de la
  herramienta de edición de menús. El menú no permite aún unirse a un servidor (sí por
  línea de comandos). El contorno del bloque apuntado sigue siendo cajas del renderer de
  entidades. El HUD se redibuja entero cada frame (sin caché).
- Inventario: es creativo; no hay cantidades ni inventario en el servidor. La barra no
  se guarda entre sesiones. Los ítems sin bloque (la vara de selección) aún no hacen
  nada. Iconos = color plano del ítem.
- Contenido: `setblock`/`fill` solo actúan sobre chunks cargados; `fill` admite hasta
  64³ voxeles.
- Bloques y modelos: la luz solo hace brillar el bloque (no ilumina alrededor). El rayo
  de edición trata un modelo como su celda entera. Los modelos no hacen sombra (AO) ni
  la reciben y no tienen bisel. La espada no se dibuja en la mano y la puerta aún no se
  abre.
- Ítems sueltos: se difunden a todos los jugadores (no solo a los cercanos) y no se
  guardan. Se dibujan como mucho 256, sin límite de distancia ni orden por cercanía, y
  sin sombra ni bruma. Romper bloques no suelta ítems (aún no hay supervivencia).
- Translúcido: sin orden dentro de un chunk (dos capas de cristal en el mismo chunk
  pueden mezclarse mal); el agua lejana es opaca y el relevo con la cercana se nota algo
  en mar abierto.
- Consola: en un servidor dedicado nadie es operador, así que nadie puede usar los
  comandos del servidor. Sin autocompletado. Las distancias de visión cambiadas con el
  mundo abierto no se aplican hasta volver a entrar.
- Edición: Romper es instantáneo en todos los modos. No se puede colocar un bloque
  dentro de uno mismo, pero sí dentro de otro jugador.
- Guardado: las regiones leídas se quedan en memoria (solo contienen chunks modificados).
  No se guardan las posiciones de los jugadores.
- Los jugadores se dibujan como dos cajas, sin animación; el propio solo en 3.ª persona.
- El terreno y los árboles son de prueba, no diseño definitivo del juego.
- Terreno lejano: si se vuela muy rápido o al arrancar puede verse un instante el cielo
  dentro del círculo de voxeles; los árboles de nivel 2 son columnas toscas de 4 m. Al
  cambiar de nivel la densidad aparente del bosque cambia algo (se nota en el mapa). Las
  teselas muestreadas pueden sobresalir un poco de las vecinas más finas. El cielo es un
  color plano, sin degradado hacia el horizonte. Sin cola de prioridad: todas las
  teselas se piden a la vez al entrar o al moverse mucho.
- Rendimiento a 5 km: el coste lo marcan los ~2 M de quads y las draw calls (una por
  malla y grupo de caras). Para móvil habrá que bajar `SPLIT_DISTANCE` o la distancia.
- Un buffer de GPU por chunk (sin pool), chunks densos sin comprimir, sin sombras.
- Backend OpenGL sin empezar.
- wgpu 27 y winit 0.30 están fijados por Iced 0.14; subir de versión implica revisar la
  API del renderer y subir Iced a la vez.

Siguiente paso de la hoja de ruta: bloques, ítems y prefabs (paso 3), fase 3d (prefabs y
herramienta de selección).

## Comandos

```bash
cargo run --release                              # abrir el juego en el menú principal
cargo run --release -- --world NOMBRE            # saltar el menú: abrir o crear ese mundo
cargo run --release -- --world NOMBRE --host     # además, abrir la partida a la red
cargo run --release -- --connect HOST[:PUERTO]   # saltar el menú: unirse a un servidor
cargo run --release --bin tokikura-server        # servidor dedicado (--bind, --seed, --world, --content)
cargo test --workspace                           # tests
```

Puerto por defecto: 28700/UDP. `--name NOMBRE` cambia el nombre del jugador.

Controles: `WASD` moverse, `Espacio` saltar (volando: `Espacio`/`Shift` subir y bajar),
`Ctrl` correr. `F5` cambia de modo (supervivencia → creativo → espectador), `V` alterna
1.ª/3.ª persona y `Alt` + rueda acerca o aleja la cámara (más allá de 32 m, vista de
mapa; ahí la rueda sola ya hace zoom). `Esc` abre el menú de
pausa (y cierra cualquier pantalla), `E` el inventario, `º` (la tecla a la izquierda del
1) o `F1` la consola, `F3` los datos de depuración. Con el ratón capturado: clic izquierdo
rompe, derecho coloca, central copia el bloque apuntado a la barra; rueda o `1`-`9` eligen
ranura; `Q` tira el ítem activo al suelo (pasar por encima lo recoge). Al entrar desde el menú o cerrar una pantalla el ratón se captura solo; si no, un clic.

Consola: `help` lista los comandos. Del cliente: `clear`, `pos`, `stats`,
`blocks [texto]`, `items [texto]`, `give <ítem> [ranura]`, `block <ítem>`, `look`,
`drop [ítem]`, `icons` (exporta los iconos a `icons_export/`),
`mode [survival|creative|spectator]`, `zoom <metros>`, `set [clave valor]`, `menu`,
`quit`. Del servidor (solo anfitrión): `tp <x> <y> <z>`, `setblock <x> <y> <z> <bloque>`,
`fill <x1> <y1> <z1> <x2> <y2> <z2> <bloque>` (máx. 64³; `air` para vaciar), `seed`,
`save`, `players`. Las coordenadas admiten `~` / `~n`, relativas al jugador (a los pies
en los comandos de bloque, a los ojos en `tp`); los bloques se nombran por estado
(`base:wood[axis=x]`).

Opciones de desarrollo (variables de entorno):

| Variable | Efecto |
|---|---|
| `KURAVOXEL_VIEW_DISTANCE=N` | Radio de voxeles reales en chunks; manda sobre `settings.toml` |
| `KURAVOXEL_FAR_DISTANCE=N` | Radio total con terreno lejano (máx. 160); manda sobre `settings.toml` |
| `KURAVOXEL_SETTINGS=ruta` | Archivo de ajustes del jugador (por defecto `./settings.toml`) |
| `KURAVOXEL_SEED=N` | Semilla del servidor integrado |
| `KURAVOXEL_CONTENT=ruta` | Carpeta de contenido (por defecto `./content`) |
| `KURAVOXEL_WORLDS=ruta` | Carpeta de mundos (por defecto `./worlds`) |
| `KURAVOXEL_VSYNC=0` | Sin vsync, para medir fps |
| `KURAVOXEL_CAMERA="x,y,z,yaw,pitch"` | Vista inicial (ojos), ángulos en grados; entra en modo espectador |
| `KURAVOXEL_SCREENSHOT=ruta.ppm` | Guarda un frame (mundo cargado, o menú asentado) y sale |
| `KURAVOXEL_UI_CLICKS="x,y;x,y"` | Inyecta clics en esos píxeles antes de la captura |
| `KURAVOXEL_UI_OPEN=pantalla` | Al entrar en un mundo abre `pause`, `inventory`, `console` o `debug` |
| `KURAVOXEL_CONSOLE="cmd;cmd"` | Al entrar en un mundo ejecuta esas líneas de consola |
| `RUST_LOG=kuravoxel_client=debug` | Vuelca fps y contadores por consola |

Registro de carga del cliente: `world loaded in …` sale una sola vez, al terminar la carga
inicial. Después solo avisa (`nearby terrain was incomplete for …`, nivel warn) si la capa
de chunks de la cámara, dentro del radio de voxeles, estuvo más de 0,25 s sin estar lista
para dibujarse; es la señal de que el servidor o el mallado no dan abasto.

Para verificar un cambio visual: capturar con `KURAVOXEL_SCREENSHOT` (+ `KURAVOXEL_CAMERA`
en un mundo, `KURAVOXEL_UI_CLICKS` en un menú, o `KURAVOXEL_UI_OPEN` / `KURAVOXEL_CONSOLE`
para las pantallas que se abren con teclas), convertir con `tools/ppm2png.py` y mirarlo.
Conviene apuntar `KURAVOXEL_WORLDS` y `KURAVOXEL_SETTINGS` a una carpeta temporal. Ajustes de aspecto (`BEVEL_RADIUS`, `AO_MIN`, color del
cielo) al principio de `crates/kuravoxel-client/src/render/mod.rs`; colores de bloques en
`content/base/blocks/`.
