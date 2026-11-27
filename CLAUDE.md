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
| Forma del mundo | Finito, tipo Chile: franja norte-sur de 5,12 × 32,8 km, océano al oeste, cordillera muy alta al este | Elección del usuario; como el Chile real |
| Biomas | Tres zonas de tres biomas, en franjas fijas de norte a sur: Desértico, Llanuras Quebradas, Desierto Florido / Mediterráneo, Planicies, Volcán y Lagos / Bosque Templado, Fiordos, Pre-Antártico | Elección del usuario: la semilla cambia el detalle, no la geografía |
| Terreno por datos | Cada bioma es un archivo con un grafo de nodos + reglas de superficie y vegetación; recarga en caliente | Elección del usuario; como el editor de nodos de Hytale |
| Herramienta de biomas | Por fases: primero formato, evaluador, recarga y vista previa; después, el editor visual de nodos en Iced sobre ese mismo formato (hecho las dos) | Elección del usuario: resultados pronto, y el editor no condiciona el formato |
| Editor de biomas | En una segunda ventana del juego (`F7`, solo el anfitrión): lienzo de nodos con miniatura en cada nodo y cajas que se estiran, formularios para las reglas, mapa de la zona; el mundo se ve en la ventana del juego. Guarda solo; "Live world" regenera al guardar o espera a "Apply to world" | Elección del usuario; como las herramientas de Hytale |
| Perfiles de bioma | Un bioma = su archivo por defecto (el que se comparte y se trabaja entre los dos) + perfiles opcionales (`worldgen/profiles/<bioma>/<perfil>.toml`, copias enteras); cada mundo elige el suyo en su `world.toml` | Elección del usuario: probar sin tocar lo compartido |
| Relieve | Mapa de alturas + prefabs; sin densidad 3D | Elección del usuario: barato, y lo lejano coincide con lo cercano |
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
| Herramienta de construcción | Vara de selección (dos clics = caja), portapapeles en el servidor, vista previa al colocar, deshacer; solo el anfitrión | Elección del usuario; como Hytale |
| Números de bloque | `u16` global por **estado** de bloque + paleta por chunk (1 byte por voxel salvo chunks con > 256 estados) | Quita el tope de 255 sin gastar memoria |
| Propiedades de bloque | Colisión, categoría y etiquetas, herencia (`parent`), transparencia, colores por cara con orientación, emisivo | Elección del usuario |
| Rotaciones | Colores por cara (arriba / lados / abajo / frente) y modelos de mini-voxeles, orientados por eje o hacia quien coloca | Un cubo de color plano no cambia al girarlo |
| Luz | Bloques emisivos (brillan, no iluminan); la luz por propagación y las cuevas oscuras, en una fase propia | Casi gratis ahora |
| Sombras | Shadow maps en 3 cascadas con penumbra suave (PCF), solo sobre los voxeles reales | Elección del usuario; aspecto de "mundo de juguete" |
| Hora del mundo | La lleva el servidor, se guarda en `world.toml` y todos ven el mismo sol | Elección del usuario; encaja con el diseño autoritativo |
| Cielo | Degradado horizonte → cénit, discos de sol y luna, noche clara y azulada; sin estrellas ni nubes | Elección del usuario |
| Noches | La luna tiene fases (ciclo de 8 noches, sale del día del mundo); cada noche mezcla la paleta de luna llena (`night`) y la de sin luna (`moonless`, casi negra); sin luna no hay luz directa | Elección del usuario: noches variadas y oscuridad de verdad |
| Ritmo de frames | Tope "Max FPS" (por defecto, el del monitor; 30-240 o sin límite) y 30 fps sin foco, en pausa o en los menús (15 minimizada) | Elección del usuario: la gráfica no debe ir al 100 % dibujando frames que no se ven |
| Ajuste del ambiente | Panel de Iced con deslizadores (`F6`) + comandos `env` y `time`; se guarda en `environment.toml` | Elección del usuario: probarlo en vivo |
| Entidades y enemigos | Más adelante; carpeta `entities/` reservada con el mismo patrón de carga | Necesitan IA, física en el servidor y animación |
| Consola | Mixta: comandos del cliente en el juego, los de autoridad en el servidor | Elección del usuario; encaja con el scripting |
| Menús | Provisionales en código Iced | Se rehacen con la herramienta de edición de menús |
| Física del jugador | La simula el cliente; el servidor solo limita la velocidad | Elección del usuario; el código vive en `common` para que el servidor la re-simule más adelante |
| Movimiento | Tipo Hytale: más ágil que Minecraft, menos que Veloren; sube solo escalones de 1 bloque | Elección del usuario |
| Modos de juego | Supervivencia (a pie), Creativo (vuelo con colisiones), Espectador (vuelo sin colisiones); `F5` los recorre | Elección del usuario |
| Cámara | 1.ª y 3.ª persona hasta 32 m; cerca va sobre el hombro derecho y el personaje mira con la cámara (Hytale), desde 8 m la cámara orbita centrada y el personaje gira hacia donde camina (Veloren) | Elección del usuario |
| Construcciones a lo lejos | En el mapa de alturas del terreno lejano: volumen exterior con el color del bloque más alto; una celda con algo construido toma su altura máxima (nunca desaparece); se actualiza en vivo ~1 s después de editar | Elección del usuario |
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
- **Comandos de consola**: los que necesitan autoridad (`tp`, `setblock`, `fill`, `time`,
  `seed`, `biome`, `terrain`, `save`, `players`)
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
- **Cámara en 3.ª persona**: orbita un punto algo por encima de la cabeza y mira en la
  dirección de la mirada desde él; el rayo de edición sale de ese mismo punto, así el
  punto de mira señala justo lo que se edita. Un rayo hacia atrás la acerca de golpe si
  el terreno estorba y la suelta con suavidad.
- **Al hombro**: cerca, ese punto se desplaza 0,7 m a la derecha (`SHOULDER_OFFSET`) y el
  personaje queda a la izquierda de la mira. Entre 6 y 10 m de distancia vuelve al centro
  (`SHOULDER_FADE`). Un rayo lateral lo recoge de golpe si hay una pared a la derecha.
- **Vista de mapa**: es la misma cámara muy alejada e inclinada hacia abajo (mínimo ~34°).
  El radio de voxeles reales (`clip`) baja a 0, así que todo lo dibuja el terreno lejano
  ya recibido; un poste marca al jugador. No hay datos ni render nuevos: el mapa llega
  hasta donde llega el terreno lejano (5 km con los ajustes por defecto). A 3,2 km de
  distancia se ve el ancho entero del mundo, de la cordillera al mar.
- El servidor recibe la posición de los ojos y el giro del **personaje** (no el de la
  cámara), y los chunks se piden alrededor del jugador, no de la cámara.

### Día, noche y sombras

- **Hora** (`WorldTime` en `protocol.rs`): hora del día (0-24), segundos reales que dura
  un día (20 min por defecto; 0 = tiempo parado) y **día** del mundo (sube al pasar la
  medianoche; se guarda en `world.toml`). El servidor la avanza con los ticks, la
  guarda en `world.toml` (al cambiarla, cada minuto y al cerrar) y la envía con
  `ServerMsg::Time` al entrar, al cambiarla y cada 10 s; el cliente la avanza solo entre
  avisos. Se cambia con `time set` / `time cycle` o con `ClientMsg::SetTime` (lo que usan
  los deslizadores: sin respuesta de texto); solo el operador.
- **Luna**: `WorldTime::moon_age` (0 = nueva, 0,5 = llena) sale del número de noche (la
  noche `n` va de la tarde del día `n` a la mañana siguiente), con un ciclo de
  `MOON_CYCLE_NIGHTS` = 8; un mundo nuevo empieza con luna llena. Como el día lo lleva el
  servidor, todos ven la misma fase. `Lighting::at(ambiente, hora, edad)` mezcla los
  colores de la noche entre `night` (llena) y `moonless` (nueva) según la parte
  iluminada, y la luz de la luna (y lo que se ven sus sombras) crece con ella: en luna
  nueva no hay luz directa y no se dibujan sombras. El cielo dibuja la fase en el disco
  (esfera iluminada de lado; la parte oscura apenas se ve, `EARTHSHINE`). Los bloques
  emisivos siguen viéndose en plena oscuridad. `time moon <fase>` y `time day <n>` (solo
  el anfitrión) cambian la luna para probar.
- **Ambiente** (`render/environment.rs`): `Environment` son los valores que se ajustan a
  ojo (inclinación del sol, intensidades, sombras y los colores de día, atardecer y noche:
  cénit, horizonte, luz y ambiente). `Lighting::at(ambiente, hora)` da la luz del
  instante. Es cálculo en CPU, con tests.
- **Sol**: sale por el este (+X) a las 6, se pone por el oeste a las 18 y su recorrido se
  inclina hacia el norte (`sun_tilt`), así a mediodía sigue habiendo sombra. La luna está
  justo enfrente. Hay **una sola luz directa**: el sol sobre el horizonte y la luna
  después; las dos se apagan al cruzarlo (un momento solo hay luz ambiente).
- **Cielo** (`sky.wgsl`): un triángulo a pantalla completa en el fondo de la profundidad,
  después de lo opaco. El terreno lejano se funde con el color del horizonte, que al
  amanecer y al atardecer lleva un resplandor hacia el sol (`horizon_color`, la misma
  función para el cielo y para la bruma).
- **Sombras** (`render/shadows.rs`, `shadow.wgsl`): tres mapas de profundidad (una textura
  con 3 capas) vistos desde la luz. Cada cascada es una esfera de radio fijo delante de la
  cámara (8,5 %, 29 % y 100 % de la distancia de sombras, 200 m por defecto) cuyo centro
  se mueve de texel en texel: no tiemblan al moverse. El shader usa la primera cascada
  que contiene el punto y mezcla con la siguiente en el borde.
- **Reutilizar los mapas**: un mapa ya dibujado vale mientras no cambie lo que da sombra
  (versión de las mallas de voxeles, jugadores, chunk de la cámara), su centro no se
  mueva más de un 3 % del radio y la luz no gire más de ~0,05°
  (`Cascade::can_stand_in_for`). Se lee con la matriz con la que se dibujó, así que la
  sombra es exacta. Quieto, no se redibuja ninguno; con el día en marcha, el sol avanza a
  saltos invisibles.
- En los mapas **solo se dibujan las caras que dan la espalda a la luz** (las elige la
  CPU por rangos de cara): lo iluminado nunca se compara consigo mismo, casi no hace falta
  sesgo y la sombra toca lo que la proyecta. Dan sombra los chunks reales (también los
  modelos) y los jugadores; lo translúcido, las marcas y el terreno lejano no.
- **Penumbra**: filtro de 3×3 o 5×5 muestras con comparación bilineal, de ancho constante
  en metros (`shadow_softness`) hasta donde da el filtro (~1,5 texels por muestra).
  Primero se leen las cuatro esquinas del filtro: si coinciden (del todo al sol o del
  todo en sombra) no se lee el resto. Leer los mapas era lo más caro del frame (caché de
  texturas): con esto, `high` a 1080p bajó de ~8 a ~1,3 ms.
- **Calidad** (`ShadowQuality`, ajuste `shadows`): `off`, `low` (1024, 3×3), `medium`
  (2048, 5×5) y `high` (4096, 5×5). Camino de la vista de mapa (sin voxeles reales) las
  sombras se apagan solas.
- **Shaders comunes**: `common.wgsl` (uniforms del frame, luz, sombras, bruma) y
  `faces.wgsl` (tablas de caras) van delante de cada shader de la escena
  (`scene_shader`). Las pasadas de sombras tienen su propio grupo 0 (no pueden usar el de
  la escena, que lleva los mapas como textura).
- **Juego**: `environment.rs` da nombre, límites y archivo (`environment.toml`, colores
  como `#RRGGBB`) a esos valores; `screens/sky.rs` es el panel. Los cambios se aplican al
  momento con `Context::set_environment` y se guardan al pulsar "Save" (o `env save`).

### Motor y juego

- Crates del motor: `kuravoxel-common`, `kuravoxel-server`, `kuravoxel-client` y
  `kuravoxel-ui`. Ninguno sabe nada de Tokikura.
- `tokikura` es el juego: implementa `kuravoxel_client::app::Game` y decide cuándo se
  entra o sale de un mundo y qué pantalla se ve. Da las órdenes al motor con `Context`
  (`start_session`, `end_session`, `exit`, `configure`, `select_block`, `grab_mouse`,
  `server_command`, `set_move_mode`, `zoom_camera`, `set_camera_distance`, `set_tool`,
  `set_overlay`, `prefab`, `set_time`, `set_environment`, `set_scene_region`, `terrain`,
  `open_window`, `close_window`).
- **Ventanas extra** (`app.rs`): `Context::open_window(clave, WindowSpec)` abre otra
  ventana de sistema con su superficie de wgpu sobre el mismo dispositivo
  (`Renderer::create_window_surface`, mismo formato de imagen); no dibuja escena. El
  juego recibe `window_opened` (con un `Host` para montar ahí su interfaz),
  `extra_window_event`, `draw_window` (cada frame, sobre un fondo liso) y
  `window_closed` (la X la cierra el motor solo). Se dibujan en el mismo frame que la
  principal; para el ritmo de frames cuenta el foco de cualquiera de las ventanas, y
  con la principal minimizada el frame lo dispara una extra. Con `KURAVOXEL_SCREENSHOT`
  cada ventana extra se captura junto a la principal (`<captura>-window<clave>.ppm`).
- **Ritmo de frames** (`app.rs`): `ClientSettings::max_fps` (`FrameLimit`: `Display`, la
  frecuencia del monitor; `Fps(n)`; `Unlimited`) y `background_fps` (con la ventana sin
  foco o `Game::idle()`, la mitad minimizada; 0 = sin rebaja). El bucle duerme hasta el
  siguiente frame (`ControlFlow::WaitUntil`, temporizador de alta resolución en Windows)
  y sigue atendiendo los eventos mientras. Tokikura marca `idle` en pausa y en los menús.
  Las capturas de desarrollo no se frenan.
- `set_scene_region` dice qué parte de la ventana deja ver la escena (una pantalla opaca
  tapa el resto): el renderer recorta el pase (scissor) y descarta los chunks que caen
  fuera con la pirámide de esa zona. Ahora no la usa ninguna pantalla (la usaba el editor
  de biomas antes de tener ventana propia).
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
| Prefabs | `prefabs.rs` | Lista de prefabs (filtrar, cargar y colocar, borrar), guardar la selección, copiar / cortar / vaciar, deshacer |
| Cielo y luz | `sky.rs` | Hora, duración del día, sol, sombras y colores del cielo, con el mundo a la vista (no oscurece la escena) |
| Biomas | `biome_editor.rs` (+ `node_graph.rs`) | Editor de biomas, en su propia ventana: nodos, superficie, vegetación, ajustes, mapa, perfiles y recarga; ver "Editor de biomas" |

- **Ajustes del jugador** (`settings.rs` → `settings.toml`): nombre, distancia de voxeles
  y lejana, campo de visión, sensibilidad del ratón, vsync, tope de fps (`max_fps`:
  `monitor`, `unlimited` o 30-240; `set fps`), escala de la interfaz y calidad de las
  sombras. Las
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
  `drop [ítem]`, `icons`, `mode [nombre]`, `zoom <metros>`, `set [clave valor]`, `sky`,
  `env [clave valor]`, `menu`, `quit`) se resuelven en
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
   d. (hecho) Prefabs y herramienta de selección: guardar, copiar/pegar, colocar
      validado por el servidor, deshacer. Falta que el generador los use (paso 4).
   e. Paleta creativa con categorías e ítems (adelantada a la 3b).
   Después: pinceles y demás herramientas creativas.
4. Generador de terreno por datos (en curso), por fases:
   a. (hecho) Biomas como grafos de nodos en archivos, mezcla entre biomas, reglas de
      superficie, árboles y plantas por bioma, recarga en caliente y vista previa en PNG.
   b. (hecho) Editor visual de nodos en Iced, con vista previa, sobre el mismo formato.
   c. Prefabs en el generador (árboles, rocas, formaciones).
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
  un bloque lo resuelve por nombre, como hace `Terrain::compile` con los de los biomas.
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
- `model_front` en el bloque dice qué lado del `.vox` es el frente si no es +Y: al
  cargar, el servidor crea una copia girada del modelo (`base:furnace#south`) y el bloque
  usa esa; el resto del motor no nota la diferencia.
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

### Prefabs y herramienta de construcción

- **Formato** (`kuravoxel-common/src/prefab.rs`): una caja de voxeles con paleta de
  **nombres de estado** (no números), más un anclaje (por defecto el centro de la base).
  En disco, `content/<paquete>/prefabs/<nombre>.toml` (tamaño, anclaje, paleta) + `.bin`
  (tramos); el nombre admite carpetas (`trees/oak`) y nunca sale de `prefabs/`. Máximo
  128 por lado y 2²⁰ voxeles. `Prefab::rotated` gira posiciones, anclaje y bloques
  orientables (`ContentData::rotate_y`). La huella del contenido ignora `prefabs/`:
  guardar uno no recarga bloques e ítems.
- **Servidor** (`prefabs.rs`): `ClientMsg::Prefab(PrefabOp)` con `Copy`, `Cut`, `Save`,
  `Load`, `Delete`, `List`, `Paste { pos, turns, air }`, `Fill` y `Undo`; solo para el
  operador. El portapapeles de cada jugador vive en el servidor y se le envía una copia
  (`ServerMsg::Clipboard`) para la vista previa; responde con `PrefabResult` y
  `PrefabList`. Todo cambia el mundo con `World::edit_region` (chunk a chunk, reenvía los
  chunks enteros) y exige que la zona esté cargada. **Deshacer** guarda los `Arc` de los
  chunks de antes (hasta 32 cambios por jugador) y los devuelve: también deshace lo que
  otros hayan editado después en esos chunks.
- **Motor** (`session.rs`): no sabe de prefabs como herramienta. Ofrece un **modo
  herramienta** (`set_tool(alcance)`: los clics no editan, llegan al juego como
  `SessionEvent::ToolClick` con el voxel apuntado y el hueco anterior, y al soltar el
  botón `ToolRelease`), el **rayo de la mira** en `SessionInfo` (`aim_origin`, `aim_dir`)
  y unas **marcas** (`set_overlay`: cajas de aristas, que pueden ser planas y más gruesas,
  y la vista previa de un prefab, que se dibuja con el renderer de cajas: un cubo por
  voxel visible, un pelo mayor que el voxel para que no haya costuras y algo aclarado,
  hasta 12 000).
- **Dibujar la selección** (`builder.rs`, `Stage`): todo con el clic izquierdo. Clic 1 =
  esquina A (manteniéndolo, la base se estira hasta soltar); clic 2 = esquina B; después
  la altura sigue a la mira (recta vertical sobre B) hasta el clic 3. El derecho y el
  central la borran; un clic fuera de la caja empieza otra.
- **Manillas de la selección**: cada cara tiene en su centro un cuadrado (40 % del lado
  corto, entre 0,6 y 3 m). Cada frame se corta el rayo de la mira con las seis
  (`ray_handle`); se resalta la más cercana (con `Ctrl`, la más lejana). Ni los bloques ni
  la caja las tapan: la de abajo se agarra desde arriba. Clic izquierdo sobre una la
  agarra: la cara sigue a la mira (punto de la recta
  perpendicular a la cara más cercano al rayo) hasta soltar; la rueda la mueve de bloque
  en bloque. Nunca cruza la cara de enfrente ni pasa de 128.
- **Marcas a través del terreno**: los contornos de `set_overlay` van en un pase propio
  (`render/entities.rs` con `overlay.wgsl`): mezcla alfa, sin prueba ni escritura de
  profundidad, al final del frame. Un `Outline` con `fill` > 0 dibuja además sus caras
  translúcidas con una rejilla de un bloque calculada en el fragment shader.
- **Medidas**: `SessionInfo::project` (matriz de vista + origen) da la posición en
  pantalla de un punto del mundo; el juego pone con ella tres rótulos en el HUD (ancho,
  largo y alto) sobre las aristas de la esquina superior más cercana a la cámara.
- **Juego** (`builder.rs`): estado de la herramienta (esquinas, portapapeles, giro,
  altura, si se está colocando) y qué orden enviar; se prueba sin motor. Se activa con el
  ítem de `tool = "selection"` en la mano o mientras se coloca algo. `game.rs` pone las
  teclas y `screens/prefabs.rs` el menú.

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
  casar con lo construido.

### Forma del mundo y biomas

- Zona jugable de 5,12 km (X, de oeste a este) por 32,8 km (Z, de norte a sur), centrada
  en el origen (`width` / `length` en `worldgen/world.toml`). El servidor la envía en
  `Welcome` (`WorldBounds`); cliente y servidor no dejan salir de ella a nadie, ni con `tp`.
- Perfil de oeste a este, como Chile: océano, costa, cordillera de la costa, valle central
  y cordillera (picos de ~400-600 m). La costa y la cordillera serpentean a lo largo de Z.
  El sol sale por detrás de la cordillera y se pone en el mar.
- **Nueve biomas en franjas** de ~3,6 km, de norte a sur: Desértico (farellón costero,
  pampa con dunas, salares), Llanuras Quebradas (mesetas separadas por abismos de 60 m),
  Desierto Florido (alfombras de flores), Mediterráneo (pasto seco y matorral), Planicies
  (valle llano con ríos; aquí se aparece), Volcán y Lagos (un cono de 520 m con cráter),
  Bosque Templado (cerros con bosque denso), Fiordos (brazos de mar entre montañas) y
  Pre-Antártico (estepa, canales y campo de hielo). La línea de nieve baja hacia el sur.
- **Fuera de la zona jugable el terreno se sigue generando** (más mar al oeste, más
  cordillera al este, y el primer y el último bioma siguen al norte y al sur): el
  horizonte nunca muestra un corte artificial.

### Terreno por datos

El terreno es contenido: `content/<paquete>/worldgen/world.toml` y un archivo por bioma en
`worldgen/biomes/` (formato completo en `content/README.md`). **Nada del mundo de Tokikura
está en el código del motor**: el motor sabe de franjas de biomas, grafos y reglas.

- **Grafo de nodos** (`graph.rs`): cada nodo da un número por columna (x, z). Hay ruido
  (`noise`, `ridge`), celdas (`cells`: mesetas y grietas), manchas (`spots`, `point`),
  aritmética (`add`, `mul`, `mix`, `min`, `max`...) y formas (`smoothstep`, `curve`,
  `terrace`). Se escribe como una tabla de nodos con nombre y se compila a una lista de
  operaciones con registros. No sabe de biomas ni de bloques.
- **Evaluación perezosa**: un `mul` se detiene en el primer factor que vale 0 y un `mix`
  con `t` en 0 o en 1 solo calcula su rama. Con las máscaras delante, en mar abierto no se
  calcula la tierra ni en el valle las cumbres: una altura cuesta 0,4-0,9 µs.
- **Nodos comunes**: los de `world.toml` (`across`, `range`, `coastal`, `cordillera`,
  `seafloor`, `hills`) los puede usar cualquier bioma. Es lo que hace que la costa y la
  cordillera casen de un bioma al siguiente.
- **Bioma** (`biomes.rs`): un nodo de altura, reglas de superficie (la primera que se
  cumple: por altura, pendiente o el valor de un nodo → bloque de arriba, de debajo y
  cuántas capas), árboles (`round`, `pine`, `bush`, `pillar`; la densidad es un número o
  un nodo) y plantas (un modelo sobre la superficie).
- **Mezcla**: en una zona de `blend` metros a cada lado de la frontera (que serpentea), la
  altura es la media ponderada de los dos biomas; la superficie y la vegetación las pone
  uno u otro, entrelazados en lenguas y manchas (`Terrain::surface_band`), no en línea.
- **Pendiente**: módulo del gradiente a 2 m (`SLOPE_STEP`); en las teselas muestreadas
  sale de las propias muestras de la celda.
- **Recarga en caliente** (`Server::reload_terrain_if_changed`): el servidor mira
  `worldgen/` cada 0,5 s. Si compila, cambia el generador (`World::set_generator`): los
  chunks sin editar se olvidan, lo encargado al generador anterior se descarta al llegar
  (`epoch`), y a cada jugador se le reenvía todo de cerca a lejos; el cliente sustituye
  cada chunk y cada tesela según llegan. Lo editado se conserva. Un error deja el terreno
  anterior y sale en el registro. La huella del contenido ignora `worldgen/`: cambiar un
  bioma no recarga bloques e ítems.
- **Recarga a mano** (`TerrainOp`, `TerrainStatus` en `protocol.rs`): con la recarga
  automática apagada (`TerrainOp::Auto(false)`, `terrain auto off`), un cambio que da otro
  terreno queda **pendiente** (`TerrainStatus::pending`) hasta `TerrainOp::Apply`
  (`terrain apply`). El servidor compila igual al notar el cambio, así sabe si de verdad
  hay algo que aplicar (mover nodos no cuenta). La recarga automática vuelve a estar
  activada en cada arranque. El estado se envía al anfitrión al entrar y al cambiar.
- **Perfiles** (`biomes::PROFILES`): `worldgen/profiles/<bioma>/<perfil>.toml` es otra
  versión entera de un bioma. El mundo guarda el suyo para cada bioma en su `world.toml`
  (`[profiles]`, `WorldMeta::profiles`) y el servidor lee el terreno con
  `WorldSource::load_with(dir, perfiles)`; un perfil que ya no existe deja el archivo por
  defecto (con un aviso). Se elige con `TerrainOp::Profile` / `terrain profile <bioma>
  <perfil|default>` (respeta la recarga automática) y `terrain profiles` los lista.
- **Vista previa** (`preview.rs`, `tokikura-worldmap`): un mapa PNG visto desde arriba
  (colores de los bloques, relieve sombreado, bosques) que solo consulta alturas y
  superficie: el mundo entero a 8 m por píxel sale en ~1,5 s. Con `--watch` se redibuja al
  guardar un archivo; `--biome` dibuja el mundo con un solo bioma.
- **Un bioma solo**: `KURAVOXEL_BIOME=nombre` (o `ServerConfig::solo_biome`) genera el
  mundo entero con él, para trabajarlo por separado dentro del juego.
- **Aparición**: la columna en tierra firme más cercana al origen (`spawn_column`).
- **Ruido** (`noise.rs`): Perlin sin ramas (tabla de gradientes), ~18 ns por octava.

### Editor de biomas

`F7`, el botón "Biome editor" del menú de pausa (o `KURAVOXEL_UI_OPEN=biomes`) abre el
editor en una **ventana propia** del juego
(1440 × 900, se puede llevar a otro monitor), en el bioma en el que está el jugador. La
ventana del juego se sigue jugando y viendo entera. `F7` en cualquiera de las dos, su
botón "Close" o la X la cierran; `Ctrl+S` guarda. Se cierra sola al salir del mundo. Tiene
su propia capa de Iced (`Tokikura::editor_ui`). Los atajos de las dos ventanas ignoran las
pulsaciones sintéticas de winit (`is_synthetic`: al recibir el foco, winit repite las
teclas que siguen apretadas, y F7 cerraba el editor nada más abrirse).

- **Archivos** (`kuravoxel-server/src/worldgen_doc.rs`): `Workspace` abre `world.toml` y
  todos los biomas con las reglas del generador; `GenDoc` edita uno **sobre su propio
  texto** (`toml_edit`): cambiar un valor no toca el resto, así que comentarios, orden y
  formato se conservan. Renombrar o borrar un nodo actualiza sus usos (nodos, `height`,
  reglas); renombrar uno común de `world.toml` también en los biomas que lo usan. Crear un
  bioma lo añade al final de la lista del mundo. Los valores entran y salen como
  `toml::Value`.
- **Esquema** (`graph::NODE_TYPES`): de cada tipo de nodo, sus parámetros y de qué clase
  son (entrada, lista de entradas, número, entero, semilla, tamaño, opción, puntos), con
  su valor inicial y una ayuda. El editor dibuja puertos y campos a partir de él, sin
  conocer cada tipo; un test comprueba que coincide con `NodeDef`.
- **Lienzo** (`screens/node_graph.rs`, `canvas` de Iced): cajas con un puerto por entrada
  (una lista tiene uno por elemento más uno "+"), una fila por parámetro (`scale: 900`),
  cables, miniatura y rango de valores. Los textos se recortan al ancho de la caja.
  Arrastrar de una salida a una entrada conecta; agarrar la punta de un cable lo suelta;
  arrastrar el fondo o con el botón derecho desplaza, la rueda acerca, `Supr` borra. Un
  arrastre vive en el lienzo y solo al soltar llega a la pantalla. Una conexión que haría
  un bucle se rechaza.
- **Cajas estirables**: la esquina inferior derecha de cada caja la estira (más ancha, se
  leen los textos enteros; más alta, crece la miniatura), nunca por debajo de su
  contenido; doble clic en la esquina o "Normal box size" en el inspector la devuelven a
  su tamaño.
- **Dónde va cada nodo**: en el propio archivo, `[editor.layout]` (`nombre = [x, y]`, o
  `[x, y, ancho, alto]` si se ha estirado), al final. Los nodos sin posición se colocan
  en columnas según su profundidad, debajo de los que ya tienen una. **Antes de cualquier
  cambio en los nodos** (mover, estirar, conectar, añadir, renombrar, un campo) se guarda
  la posición de todos (`pin_layout`): si no, al mover uno los demás de su columna se
  recolocaban. La vista del lienzo se fija al abrir cada archivo (antes se recalculaba en
  cada frame desde el nodo de arriba a la izquierda, y moverlo desplazaba todo). El
  generador ignora esa tabla, y el servidor **no regenera el mundo si solo cambia eso**
  (compara el terreno sin los datos del editor, `WorldSource::without_editor`).
- **Perfiles**: debajo de la barra, "Profile" elige el perfil del bioma (`default` = su
  archivo); el que se edita es el que usa el mundo abierto (envía `TerrainOp::Profile`).
  "Save as new profile" crea y guarda una copia del que se edita y pasa a él; "Delete
  profile" (dos clics) lo borra del disco. Si el servidor dice que el mundo usa otro
  perfil (consola, o no se aceptó), el editor pasa a ese. En `worldgen_doc.rs`, un
  documento se nombra por su clave (`world`, `base:plains`, `base:plains@perfil`);
  `Workspace` tiene los perfiles aparte (`profiles`) y el perfil elegido de cada bioma
  (`active`), y `source()` compila con ellos.
- **Live world / Apply to world**: el interruptor manda `TerrainOp::Auto` al servidor
  (solo el anfitrión). Apagado, guardar no cambia el mundo; "Apply to world" guarda lo
  pendiente (si compila) y envía `TerrainOp::Apply`.
- **Vistas previas** (`biome_preview.rs`, un hilo propio; solo cuenta el último
  encargo): la miniatura de cada nodo (64², en grises de su mínimo a su máximo) y el mapa
  de la zona (280², el bioma solo; en `world.toml`, el mundo teñido por biomas), con
  `preview::node_grids` y `preview::window_map`. Un nodo roto da su error y los demás se
  ven igual. Tarda ~40-50 ms. La zona empieza sobre la franja del bioma; se mueve con un
  clic en el mapa, `−` / `+`, "Me" (sobre el jugador) y "Go there" (`tp` al suelo de ese
  punto).
- **Formularios**: inspector del nodo elegido (nombre, tipo, parámetros: una entrada es un
  número o un nodo), reglas de superficie (condiciones, nodo, bloques con su color, capas;
  subir, bajar, quitar), árboles y plantas (densidad número o nodo, forma, tronco, hojas y
  suelos como fichas) y ajustes (del bioma: nombre, color, nodo de altura; del mundo:
  medidas, mezcla, bloques, orden de los biomas y crear uno). Una caja con un número a
  medias no cambia nada hasta que vale.
- **Guardar**: "Save" (`Ctrl+S`), o solo con "Auto save" 1,2 s después del último
  cambio. Cada cambio compila el terreno entero como el servidor (`Terrain::compile`, con
  los perfiles elegidos) y el error sale arriba; con errores no se guarda solo. "Revert"
  vuelve a los archivos del disco. Al guardar, con "Live world", la recarga en caliente
  del servidor regenera el mundo alrededor.
- **Solo el anfitrión**: el editor escribe en la carpeta de contenido local y usa la
  semilla del mundo abierto (de su `world.toml`). Un invitado puede editar archivos, pero
  su mundo no cambia (se avisa).

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
  3×3 puntos por celda): cada tesela cuesta ~7-10 ms en cualquier nivel, que es lo que
  permite llegar a 5 km. Árboles: en el nivel 3, un bloque de copa por celda con árbol;
  desde el 4, la celda se pinta de hojas con una probabilidad proporcional a la densidad
  de árboles del bioma en ese punto (`FOREST_COVER`).
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
- El recorte por píxel es un `discard`, que impide a la GPU descartar lo tapado antes de
  sombrearlo; por eso solo lo usan las mallas que cruzan el relevo (`Visible::clipped`,
  `fs_clipped`, que recorta antes de sombrear) y se dibujan después del resto (`fs_main`,
  sin `discard`).
- Límite asumido: a lo lejos no se ven cuevas, voladizos ni interiores.

### Construcciones en el terreno lejano

Lo editado se ve a lo lejos y en la vista de mapa, con menos detalle (`builds.rs`).

- **Detección**: tras una edición (1,2 s sin cambios en esa columna de chunks, o 5 s como
  mucho) y al arrancar para todo lo guardado (`Storage::load_all`), un hilo del pool
  compara los chunks modificados de la columna con los que daría el generador
  (`scan_column`). Anota cada columna de voxeles cuyo cubo visible más alto (posición o
  bloque) ya no es el natural: altura, bloque y cuánto sobresale del suelo. Los modelos
  pequeños no cuentan, y lo editado bajo una capa de terreno sin tocar tampoco (minas).
- **Mezcla** (`apply`): el servidor guarda cada tesela tal como sale del generador y con
  las construcciones encima (`LodEntry`), así no la regenera. Por celda: lo que
  sobresale ≥ 2 m va como una columna de bloques (`LodExtra`, como los árboles) hasta su
  altura **máxima** redondeada hacia arriba, de modo que una casa menor que la celda se
  ve como un cubo grueso; lo excavado (≥ 2 m) y lo repintado a ras de suelo solo cuentan
  si ocupan media celda.
- **Envío**: las teselas afectadas (todas las de `tiles_touching`, con sus celdas de
  borde) se reenvían a quien las tenga. El cliente sustituye la tesela sin soltar la
  malla vieja hasta tener la nueva (`Tile::version`): no parpadea.

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
content/          paquetes de contenido (ver content/README.md); el terreno, en worldgen/
tools/ppm2png.py  convierte las capturas de desarrollo a PNG
tools/make_vox.py genera los modelos .vox de ejemplo de content/base/models
crates/kuravoxel-common/src/     compartido por cliente y servidor
  block.rs        número de bloque y paleta para la GPU
  content.rs      paquetes: bloques, estados, ítems, herencia y numeración estable
  chunk.rs        Chunk: Uniform, Paletted (paleta + 1 byte/voxel) o Wide; serialización RLE
  lod.rs          teselas de terreno lejano: datos, quadtree, selección por distancia
  protocol.rs     ClientMsg / ServerMsg, versión, constantes de streaming, hora del mundo
  net.rs          QUIC: endpoints, mensajes enmarcados, datagramas
  physics.rs      física del jugador: caja contra voxeles y cajas, escalones, vuelo (+ tests)
  prefab.rs       prefabs: captura, giro, archivos .toml + .bin, lista (+ tests)
  model.rs        modelos de mini-voxeles: lectura de .vox, celdas, giros, cajas (+ tests)
  icon.rs         datos de iconos ([icon]) y PNG (+ tests)
  shape.rs        BlockTable: propiedades orientadas por número de bloque (+ tests)
crates/kuravoxel-server/src/     servidor autoritativo (biblioteca)
  lib.rs          Server: eventos, sesiones, streaming por jugador, ticks, validación
  commands.rs     comandos de consola del servidor (tp, setblock, fill, time, seed, biome, save, players)
  drops.rs        ítems sueltos: lanzar, simular, recoger, caducar (+ tests)
  builds.rs       construcciones en el terreno lejano: detección y mezcla en teselas (+ tests)
  prefabs.rs      herramientas de construcción: portapapeles, prefabs en disco, deshacer
  world.rs        chunks y teselas con autoridad, trabajos de generación, edición
  storage.rs      guardado del mundo: world.toml y archivos de región
  graph.rs        grafos de nodos del terreno: formato, compilación, evaluación perezosa (+ tests)
  biomes.rs       mundo y biomas por datos: archivos, mezcla, superficie, vegetación (+ tests)
  worldgen.rs     generador: caché por columna, chunks, árboles, datos de teselas (+ tests)
  preview.rs      mapa del terreno visto desde arriba, para la vista previa (+ tests)
  worldgen_doc.rs edición de los archivos del terreno conservando comentarios (+ tests)
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
  render/         mod.rs (dispositivo, pases, Gpu), terrain.rs, entities.rs, items.rs
                  (ítems sueltos), environment.rs (luz y cielo según la hora, + tests),
                  shadows.rs (cascadas y mapas de sombras, + tests), shaders/
crates/kuravoxel-ui/src/lib.rs   Iced sobre la escena: IcedLayer y trait Screen
crates/tokikura/src/             el juego
  main.rs         argumentos y ajustes; arranca el motor
  game.rs         Tokikura: implementa Game, elige pantalla, atajos, abre y cierra mundos
  game_mode.rs    modos de juego (supervivencia, creativo, espectador)
  settings.rs     ajustes del jugador y su archivo (settings.toml)
  environment.rs  cielo, luz y sombras: nombres, límites y archivo (environment.toml) (+ tests)
  hotbar.rs       barra de herramientas de 9 ranuras (ítems)
  builder.rs      herramienta de construcción: selección, portapapeles, colocar (+ tests)
  biome_preview.rs hilo de las vistas previas del editor de biomas
  icons.rs        iconos de los ítems para Iced; exportación a PNG
  commands.rs     comandos de consola del cliente
  log_capture.rs  copia del registro para la consola
  worlds.rs       mundos en disco: listar (con semilla), validar nombre, semillas, borrar
  screens/        una pantalla de Iced por archivo, más style.rs (aspecto compartido)
  bin/server.rs   servidor dedicado (tokikura-server)
  bin/worldmap.rs vista previa del terreno en PNG (tokikura-worldmap)
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
- Editor de biomas, fase 4b (con tests y capturas, en Windows / Vulkan): tests de
  `worldgen_doc.rs` (editar un parámetro conserva los comentarios, renombrar y borrar
  actualizan los usos, nodos nuevos con nombre libre y posición, reglas añadidas, movidas
  y quitadas, y el contenido del juego abierto, editado, guardado y releído igual), del
  esquema (`graph.rs`), de las vistas previas (`preview.rs`), del lienzo (puertos bajo el
  ratón) y de la pantalla (`biome_editor.rs`: abre el bioma del jugador con sus
  miniaturas, conectar y soltar cables, bucles rechazados, campos, nodos y reglas que
  acaban guardados solos con sus comentarios, un archivo roto que no se guarda solo y
  "Revert", ir al punto, crear un bioma). En el servidor, que cambiar solo el lienzo no
  regenera el mundo (`tests/terrain.rs`); en el juego, `F7`. Con capturas: los nodos
  comunes, las Planicies (nodos, inspector, superficie, árboles y plantas, ajustes) y las
  Llanuras Quebradas con el mundo detrás. **Sin probar a mano**: arrastrar nodos y
  cables, la rueda, escribir en las cajas, el guardado automático con el juego abierto y
  ver el mundo regenerarse al editar.
- Editor de biomas en ventana propia, perfiles y recarga a mano (con tests y capturas,
  en Windows / Vulkan): prueba de punta a punta en `tests/terrain.rs` (sin recarga
  automática el cambio espera y `Apply` lo aplica; un perfil elegido por consola, que el
  mundo recuerda al reiniciar y que se puede soltar; perfiles que no existen
  rechazados), tests de `worldgen_doc.rs` (tamaño de caja en el lienzo, perfiles: copia
  guardada que no toca el archivo por defecto, `source()` con el perfil elegido igual que
  el servidor, borrado), del lienzo (estirar desde la esquina, doble clic, mínimo,
  textos recortados) y de la pantalla (mover un nodo, incluido el de arriba a la
  izquierda, y conectar no recolocan los demás ni la vista; cajas estiradas; crear,
  editar, elegir, seguir al servidor y borrar perfiles; "Live world" y "Apply"), y en
  `game.rs` (F7 abre y cierra la ventana, órdenes de terreno al servidor, se cierra al
  salir del mundo). Con capturas de las dos ventanas: el editor sobre las Planicies con
  el juego al lado, y con un perfil elegido por consola, la recarga apagada y una caja
  estirada. **Sin probar a mano**: estirar cajas y arrastrar nodos con el ratón, mover
  la ventana a otro monitor, minimizar el juego con el editor abierto, y ver el mundo
  regenerarse al pulsar "Apply to world".
- Terreno por datos, fase 4a (con tests y capturas, en Windows / Vulkan): los nueve biomas
  en `content/base/worldgen/`. Tests de `graph.rs` (nodos, curvas, celdas, errores),
  `biomes.rs` (orden de las franjas, mezcla sin saltos, nodos comunes, bioma solo, reglas
  de árboles, errores que nombran el bioma), `worldgen.rs` (los chunks siguen el mapa de
  alturas y las reglas; teselas de todos los niveles; el mundo del juego tiene mar al
  oeste y cordillera al este, y cada bioma tiene tierra firme y funciona solo) y
  `preview.rs`. Prueba de punta a punta en `crates/kuravoxel-server/tests/terrain.rs`:
  comando `biome`, cambiar un archivo de bioma regenera y reenvía los chunks, lo editado
  se conserva y un archivo con errores no tumba nada. Con capturas: el mapa del mundo
  entero y, dentro del juego, las Llanuras Quebradas, el volcán sobre el bosque, el
  Desierto Florido, el desierto con su salar y su farellón visto desde el mar, los
  fiordos, los canales del sur y las planicies del punto de aparición. Rendimiento en la
  máquina de desarrollo: 4 columnas de chunks en 4-7 ms y una tesela lejana en 7-10 ms;
  carga inicial a 5 km en ~3,7 s (1,8 M de quads). **Sin probar a mano**: ver la recarga
  en caliente con el juego abierto (el cliente sustituyendo chunks y teselas), andar por
  cada bioma y las fronteras entre biomas a pie.
- Día, noche y sombras (con tests y capturas, en Windows / Vulkan): tests de `environment.rs`
  (sol, luna, relevo en el horizonte), `shadows.rs` (las cascadas cubren lo que hay
  delante y se mueven por texels), del panel (`screens/sky.rs`), del archivo y de `env`
  (`tokikura/src/environment.rs`, `game.rs`), y prueba de punta a punta de la hora en
  `tests/commands.rs` (consola, mensaje de los deslizadores, rechazo a un invitado y que
  el mundo la recuerda al reiniciar). Con capturas: sombras suaves de los árboles a varias
  horas, atardecer con el sol tras la cordillera, noche con luna y el panel abierto.
  Rendimiento en la máquina de desarrollo (64 chunks de terreno lejano, ~2 M de quads):
  ~445 fps sin sombras, ~295 en `medium` y ~185 en `high`; las sombras añaden ~1 100 draw
  calls. **Sin probar a mano**: arrastrar los deslizadores, escribir colores, `F6`, ver
  pasar el día entero y la sombra del propio jugador en 3.ª persona. Sin medir en la RX 570.
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
- Fase 3d (con tests y capturas): prefabs y herramienta de construcción. Prueba de punta
  a punta en `crates/kuravoxel-server/tests/prefabs.rs` (copiar, pegar girado con sus
  bloques orientables, con y sin aire, deshacer, cortar, guardar / listar / cargar /
  borrar en disco, nombres inválidos, límites y rechazo a un invitado) y del flujo del
  juego en `game.rs` (vara → esquinas → `Ctrl+C` → `Ctrl+V` → vista previa → pegar →
  menú). Con capturas: la caja de selección con sus esquinas, la vista previa al colocar,
  el menú de prefabs y un prefab guardado, cargado y pegado girado. El horno es ya un
  modelo de MagicaVoxel (`models/furnace.vox`, con `model_front = "south"`), visto en sus
  cuatro orientaciones y en su icono. Selección: tests en `builder.rs` (tres clics, base
  arrastrando, altura hacia arriba y abajo, caras desde dentro, desde fuera y la del otro
  lado, arrastrar, rueda, topes, medidas) y captura de una caja medio enterrada con su
  rejilla, la cara resaltada y las medidas. **Sin probar a mano**: dibujar la caja y
  arrastrar caras con el ratón, la rueda sobre una cara, los atajos con Ctrl, `R` /
  `RePág` / `AvPág` y escribir en el menú.
- Fase 3c (con tests y capturas): colores por cara orientados (troncos tumbados con los
  anillos en la punta, horno mirando a quien lo coloca), lámpara y farol que brillan,
  agua y cristal translúcidos (segundo pase), y modelos `.vox` (flor, losa, escalera
  orientada, puerta de dos celdas, poste, farol) en una escena montada con `setblock`.
  Física con cajas: tests de subir a una losa como medio escalón y de salir de ella.
  Rendimiento igual que antes: ~780 fps desde el suelo con ~2 M de quads. **Sin probar a
  mano**: andar sobre losas y escaleras, atravesar flores y agua, y verse bajo el agua.
- Cámara al hombro (test en `player.rs` y captura a 3 m) y construcciones en el terreno
  lejano: tests de `builds.rs`, prueba de punta a punta en
  `crates/kuravoxel-server/tests/builds.rs` (construir → tesela reenviada, quitar,
  reiniciar) y captura de la vista de mapa a 400 m con una torre, un muro y una caseta.
  **Sin probar a mano**: cómo se siente la cámara al hombro junto a paredes y ver
  aparecer una construcción en el mapa mientras se edita.
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
- Paquetes de contenido: 45 bloques (60 estados), 47 ítems y 11 modelos en `content/base`,
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
- Rendimiento de GPU (RX 570, 1080p, 12 + 160 chunks, medido con `KURAVOXEL_GPU_TIMING`
  y el contador de uso 3D de Windows): el frame costaba 11,7 ms de GPU con sombras `high`,
  10,5 de ellos en el pase opaco (leer los mapas de sombras + `discard`). Tras el filtro
  con salida rápida y el terreno sin `discard`: 3,6 ms en `high` (~265 fps), 3,0 en
  `medium`, 2,3 en `low` y 1,7 sin sombras. El uso de GPU con fps sin límite es GPU /
  CPU por frame (la CPU tarda ~2,6 ms a 5 km): ~94 % en `high`, ~70 % en `low`, ~67 % sin
  sombras; con el editor de biomas abierto, ~37 %. Probado sin éxito: mapas `Depth16`
  (pase de sombras más lento) y quads sin instancias (−20 % de geometría a cambio de 4×
  memoria y sombras más lentas). Lo que queda: ~1,2 ms de geometría (0,7 ms por millón
  de quads, sobre todo terreno lejano), ~1,3 ms de leer sombras y 0,6 de dibujarlas.
  Reutilizando los mapas, con la escena quieta: 3,0 ms (dibujarlos no cuesta nada). Con
  "Max FPS" en `monitor` (144 Hz): ~54 % de uso; a 60, ~47 % porque la gráfica baja su
  reloj (el frame pasa a costar 7,9 ms); en pausa, 30 fps y ~38 %.
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
  aparece siempre junto al origen (no se guarda la posición).
- Cámara: en 3.ª persona puede asomar un poco a través de una pared lateral (solo se
  comprueban el rayo central y el del hombro). El hombro es siempre el derecho (sin
  tecla para cambiarlo). La vista de mapa no enseña cuevas y no se puede desplazar más
  allá del terreno lejano cargado.
- Construcciones a lo lejos: son un mapa de alturas (un puente es un muro, una
  plataforma flotante es una columna hasta el suelo) de un solo color por celda. Las
  más finas pueden sobresalir un poco de los voxeles reales en el relevo. Un árbol
  talado sigue viéndose a lo lejos. Al arrancar se leen todas las regiones y se
  revisan todas las columnas modificadas (sin caché en disco). Si cambia el generador,
  todo lo guardado cuenta como construido.
- Editor de biomas: los nodos se ven en el orden del archivo y sin agrupar; no hay
  copiar y pegar, deshacer ni búsqueda de nodos; la miniatura de un nodo es en grises
  (sin escala de colores ni valores al pasar el ratón). Los nodos comunes que usa un
  bioma no se dibujan en su lienzo (la entrada dice `← nombre`). Al reordenar biomas o
  cambiar su número, sus franjas se mueven y lo construido puede quedar en otro bioma.
  Los archivos cambiados fuera del editor mientras está abierto no se releen hasta
  "Revert" o volver a abrirlo sin cambios pendientes. Los perfiles son solo de los
  biomas (no de `world.toml`), no se pueden renombrar ni pasar a ser el archivo por
  defecto desde el editor (se copia a mano), y borrar uno que use otro mundo deja a ese
  mundo con el archivo por defecto. La recarga automática no se guarda: cada arranque
  vuelve a estar activada. Un invitado no puede cambiar de perfil ni aplicar. Con el
  editor abierto, el juego dibuja a ritmo normal aunque su ventana no tenga el foco (las
  dos van en el mismo frame). Un texto recortado no se puede leer entero sin estirar la
  caja (no hay ayuda al pasar el ratón).
- Terreno por datos: Solo hay agua al nivel 0: lagos y ríos son terreno excavado bajo él, así
  que no hay lagos de altura ni ríos que bajen de la cordillera. Los ríos son líneas de
  nivel de un ruido: a veces se cierran sobre sí mismos o acaban de golpe. El volcán está
  en un punto fijo del mundo (no depende de la semilla). Los árboles siguen siendo formas
  en código (el generador aún no usa prefabs) y un árbol no sabe de la pendiente. Las
  plantas no se ven a lo lejos (el Desierto Florido sí, porque el color está en el suelo).
  Al recargar el terreno un jugador puede quedar enterrado o en el aire, y los límites del
  mundo no se reenvían si cambian `width` / `length`. Los biomas son una primera versión
  para ajustar a ojo, no diseño definitivo.
- Interfaz: todas las pantallas son provisionales y sin estilo propio, a la espera de la
  herramienta de edición de menús. El menú no permite aún unirse a un servidor (sí por
  línea de comandos). El contorno del bloque apuntado sigue siendo cajas del renderer de
  entidades. El HUD se redibuja entero cada frame (sin caché).
- Inventario: es creativo; no hay cantidades ni inventario en el servidor. La barra no
  se guarda entre sesiones.
- Prefabs: el generador aún no los usa (los árboles siguen en código). La vista previa
  dibuja cada voxel como un cubo de color plano, también los que tienen modelo. La
  selección se dibuja siempre encima de todo (sin pista de qué queda delante o detrás) y
  sus rótulos pueden salirse un poco de la arista. Una
  selección que corta un objeto de varias celdas (media puerta) copia solo esa mitad.
  Sin espejo, sin mover la selección con teclas y sin importar `.vox` como prefab.
  Deshacer devuelve chunks enteros. En un servidor dedicado nadie puede usar la
  herramienta (no hay operadores). Un prefab enorme y muy irregular puede no caber en un
  mensaje de QUIC (4 MiB).
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
- Terreno lejano: si se vuela muy rápido o al arrancar puede verse un instante el cielo
  dentro del círculo de voxeles; los árboles de nivel 2 son columnas toscas de 4 m. Al
  cambiar de nivel la densidad aparente del bosque cambia algo (se nota en el mapa). Las
  teselas muestreadas pueden sobresalir un poco de las vecinas más finas. El cielo es un
  color plano, sin degradado hacia el horizonte. Sin cola de prioridad: todas las
  teselas se piden a la vez al entrar o al moverse mucho.
- Rendimiento a 5 km: el coste lo marcan los ~2 M de quads y las draw calls (una por
  malla y grupo de caras). Para móvil habrá que bajar `SPLIT_DISTANCE` o la distancia.
- Un buffer de GPU por chunk (sin pool), chunks densos sin comprimir.
- Sombras: al moverse, los mapas se redibujan enteros (cada cascada cuando su centro se
  aleja un 3 % del radio; la pequeña, casi cada frame). Más allá de la distancia de sombras (200 m) y en el
  terreno lejano no hay; el corte se funde pero se nota en laderas. Las caras iluminadas
  pueden tener una línea algo más oscura en el borde que da la espalda a la luz. Con el
  tiempo en marcha los bordes de las sombras se arrastran un poco (el sol se mueve de
  forma continua). Los ítems sueltos reciben sombra pero no la dan; en 1.ª persona el
  jugador no proyecta la suya. Bajo techo solo oscurece la sombra del sol: el ambiente
  sigue igual (no hay luz por propagación).
- Cielo: sin estrellas ni nubes (en luna nueva el cielo es casi negro); la luna sale
  siempre justo enfrente del sol (no hay luna de día) y su lado iluminado no apunta al
  sol de verdad; el ciclo de 8 noches es una constante del protocolo; los discos se deforman algo en los
  bordes de la pantalla. El ambiente (`environment.toml`) es del cliente: cada jugador
  puede ver colores distintos; debería acabar siendo dato del mundo o del contenido. Un
  invitado ve los controles de hora del panel, pero el servidor no le hace caso.
- Backend OpenGL sin empezar.
- wgpu 27 y winit 0.30 están fijados por Iced 0.14; subir de versión implica revisar la
  API del renderer y subir Iced a la vez.

Siguiente paso de la hoja de ruta: los prefabs en el generador (4c), empezando por los
árboles. Acordado además con el usuario, por este orden: vaivén por
viento en el vertex shader (`sway` en el bloque) y, después, entidades con partes rígidas
articuladas y animaciones por fotogramas clave (alas, puertas que se abren).

## Comandos

```bash
cargo run --release                              # abrir el juego en el menú principal
cargo run --release -- --world NOMBRE            # saltar el menú: abrir o crear ese mundo
cargo run --release -- --world NOMBRE --host     # además, abrir la partida a la red
cargo run --release -- --connect HOST[:PUERTO]   # saltar el menú: unirse a un servidor
cargo run --release --bin tokikura-server        # servidor dedicado (--bind, --seed, --world, --content)
cargo run --release --bin tokikura-worldmap -- --watch   # mapa PNG del terreno; se redibuja al guardar un bioma
cargo run --release --bin tokikura-worldmap -- --help    # --biome, --scale, --center, --size, --tint, --out
cargo test --workspace                           # tests
```

Puerto por defecto: 28700/UDP. `--name NOMBRE` cambia el nombre del jugador.

Controles: `WASD` moverse, `Espacio` saltar (volando: `Espacio`/`Shift` subir y bajar),
`Ctrl` correr. `F7` abre el editor de biomas. `F5` cambia de modo (supervivencia → creativo → espectador), `V` alterna
1.ª/3.ª persona y `Alt` + rueda acerca o aleja la cámara (más allá de 32 m, vista de
mapa; ahí la rueda sola ya hace zoom). `Esc` abre el menú de
pausa (y cierra cualquier pantalla), `E` el inventario, `º` (la tecla a la izquierda del
1) o `F1` la consola, `F3` los datos de depuración, `F6` el panel de cielo y luz. Con el ratón capturado: clic izquierdo
rompe, derecho coloca, central copia el bloque apuntado a la barra; rueda o `1`-`9` eligen
ranura; `Q` tira el ítem activo al suelo (pasar por encima lo recoge). `P` abre el menú de
prefabs. Con la vara de selección en la mano, clic izquierdo: esquina de la base, esquina
opuesta (o mantener y arrastrar) y altura; el derecho o el central borran la caja;
apuntando a la manilla de una cara (el cuadrado de su centro), mantener el clic izquierdo
la arrastra y la rueda la empuja o tira de ella; `Ctrl+C` copia, `Ctrl+X` corta, `Ctrl+V` empieza a
colocar, `Supr` vacía la selección y `Ctrl+Z` deshace. Colocando: clic derecho pone, `R`
gira, `RePág` / `AvPág` suben y bajan, clic izquierdo o `Esc` terminan. Al entrar desde el menú o cerrar una pantalla el ratón se captura solo; si no, un clic.

Consola: `help` lista los comandos. Del cliente: `clear`, `pos`, `stats`,
`blocks [texto]`, `items [texto]`, `give <ítem> [ranura]`, `block <ítem>`, `look`,
`drop [ítem]`, `icons` (exporta los iconos a `icons_export/`),
`mode [survival|creative|spectator]`, `zoom <metros>`, `set [clave valor]`, `sky` (abre
el panel), `env` (lista los valores del ambiente), `env <clave> <valor>` (p. ej.
`env shadow_softness 0.2`, `env night.zenith #101830`), `env reset`, `env save`, `menu`,
`quit`. De construcción (los ejecuta el servidor, solo anfitrión): `pos1` / `pos2 [x y z]`,
`desel`, `copy`, `cut`, `erase`, `paste [here]`, `rotate`, `pasteair [on|off]`, `undo`,
`prefab save|load|delete <nombre>`, `prefab list`, `prefabs`. Del servidor (solo anfitrión): `tp <x> <y> <z>`, `setblock <x> <y> <z> <bloque>`,
`fill <x1> <y1> <z1> <x2> <y2> <z2> <bloque>` (máx. 64³; `air` para vaciar), `time`,
`time set <hora>` (0-24, `14:30`, o `dawn` / `noon` / `dusk` / `midnight`),
`time cycle <minutos|off>` (minutos reales por día), `time day <n>`,
`time moon <new|waxing_crescent|first_quarter|waxing_gibbous|full|waning_gibbous|last_quarter|waning_crescent>`
(esta noche con esa luna), `terrain` (recarga y perfiles), `terrain auto on|off`,
`terrain apply`, `terrain profiles`, `terrain profile <bioma> <perfil|default>`, `seed`, `biome` (en qué bioma se está
y dónde quedan los demás), `save`, `players`. Las coordenadas admiten `~` / `~n`, relativas al jugador (a los pies
en los comandos de bloque, a los ojos en `tp`); los bloques se nombran por estado
(`base:wood[axis=x]`).

Opciones de desarrollo (variables de entorno):

| Variable | Efecto |
|---|---|
| `KURAVOXEL_VIEW_DISTANCE=N` | Radio de voxeles reales en chunks; manda sobre `settings.toml` |
| `KURAVOXEL_FAR_DISTANCE=N` | Radio total con terreno lejano (máx. 160); manda sobre `settings.toml` |
| `KURAVOXEL_SETTINGS=ruta` | Archivo de ajustes del jugador (por defecto `./settings.toml`) |
| `KURAVOXEL_ENVIRONMENT=ruta` | Archivo del ambiente: cielo, luz y sombras (por defecto `./environment.toml`) |
| `KURAVOXEL_SEED=N` | Semilla del servidor integrado |
| `KURAVOXEL_CONTENT=ruta` | Carpeta de contenido (por defecto `./content`) |
| `KURAVOXEL_BIOME=nombre` | Genera el mundo entero con ese único bioma (p. ej. `fjords`) |
| `KURAVOXEL_WORLDS=ruta` | Carpeta de mundos (por defecto `./worlds`) |
| `KURAVOXEL_VSYNC=0` | Sin vsync, para medir fps |
| `KURAVOXEL_CAMERA="x,y,z,yaw,pitch"` | Vista inicial (ojos), ángulos en grados; entra en modo espectador |
| `KURAVOXEL_SCREENSHOT=ruta.ppm` | Guarda un frame (mundo cargado, o menú asentado) y sale |
| `KURAVOXEL_UI_CLICKS="x,y;x,y"` | Inyecta clics en esos píxeles antes de la captura |
| `KURAVOXEL_UI_OPEN=pantalla` | Al entrar en un mundo abre `pause`, `inventory`, `console`, `prefabs`, `sky`, `biomes` o `debug` |
| `KURAVOXEL_CONSOLE="cmd;cmd"` | Al entrar en un mundo ejecuta esas líneas de consola (antes de que acabe la carga: lo que toque chunks aún sin llegar se salta o se rechaza) |
| `RUST_LOG=kuravoxel_client=debug` | Vuelca fps y contadores por consola |
| `KURAVOXEL_MAX_FPS=valor` | Tope de fps (`monitor`, `unlimited` o un número); manda sobre `settings.toml` |
| `KURAVOXEL_BACKGROUND_FPS=N` | Fps sin foco o en pausa (por defecto 30); 0 = sin rebaja, para medir |
| `KURAVOXEL_GPU_TIMING=1` | Registra cada 2 s el tiempo de GPU por pase (sombras, opaco, cielo, translúcido, interfaz) con marcas de tiempo |
| `KURAVOXEL_WINDOW_SIZE=1920x1080` | Tamaño inicial de la ventana (píxeles lógicos), para medir a una resolución fija |

Registro de carga del cliente: `world loaded in …` sale una sola vez, al terminar la carga
inicial. Después solo avisa (`nearby terrain was incomplete for …`, nivel warn) si la capa
de chunks de la cámara, dentro del radio de voxeles, estuvo más de 0,25 s sin estar lista
para dibujarse; es la señal de que el servidor o el mallado no dan abasto.

Para verificar un cambio visual: capturar con `KURAVOXEL_SCREENSHOT` (+ `KURAVOXEL_CAMERA`
en un mundo, `KURAVOXEL_UI_CLICKS` en un menú, o `KURAVOXEL_UI_OPEN` / `KURAVOXEL_CONSOLE`
para las pantallas que se abren con teclas), convertir con `tools/ppm2png.py` y mirarlo.
Conviene apuntar `KURAVOXEL_WORLDS` y `KURAVOXEL_SETTINGS` a una carpeta temporal. Para fijar la hora de una captura: `KURAVOXEL_CONSOLE="time set 17.4;time cycle off"`. Ajustes de aspecto (`BEVEL_RADIUS`, `AO_MIN`)
al principio de `crates/kuravoxel-client/src/render/mod.rs`; los valores por defecto del cielo, la luz y las sombras, en `Environment::default()` (`render/environment.rs`); colores de bloques en
`content/base/blocks/`.
