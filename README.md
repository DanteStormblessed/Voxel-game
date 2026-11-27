# Tokikura · KuraVoxel

Sandbox de voxeles escrito en Rust, en la línea de Minecraft, Cube World y Veloren.

- **KuraVoxel** es el motor: una biblioteca reutilizable (cliente, servidor, protocolo e
  interfaz) que no sabe nada del juego.
- **Tokikura** es el juego: un crate pequeño que usa el motor y pone las pantallas, la
  barra, el inventario y las herramientas.

Voxeles de 1 m **sin texturas**, de colores vivos y planos, con las aristas suavizadas en
el shader: el mundo se ve como una maqueta de MagicaVoxel. El mundo es una franja finita
inspirada en Chile (5,12 × 32,8 km), con el océano al oeste, una cordillera muy alta al
este y nueve biomas de norte a sur, y se ve hasta 5 km de distancia.

## Características

- **Cliente-servidor autoritativo** desde el principio. Jugar solo arranca un servidor
  integrado en un hilo; el multijugador usa los mismos mensajes sobre **QUIC** (`quinn`).
  El cliente nunca genera terreno.
- **Mundo de chunks cúbicos de 32³** con altura ilimitada, paleta por chunk (`u16` por
  estado de bloque) y mallado greedy multihilo.
- **Terreno lejano** a partir del mapa de alturas: quadtree de 6 niveles de LOD hasta
  5 km, con bruma atmosférica y curvatura sutil en lugar de niebla corta. Las
  construcciones del jugador también se ven a lo lejos.
- **Terreno por datos**: cada bioma es un archivo TOML con un grafo de nodos (ruido,
  celdas, aritmética, curvas) y reglas de superficie y vegetación, con recarga en
  caliente, perfiles por mundo y vista previa en PNG.
- **Editor visual de biomas** en una segunda ventana (`F7`): lienzo de nodos con
  miniaturas, formularios para las reglas, mapa de la zona y aplicación en vivo al mundo.
- **Contenido como datos**: bloques, ítems, modelos `.vox` y prefabs en paquetes de
  archivos (`content/`), con herencia, plantillas, estados orientables y recarga en
  caliente.
- **Modelos de mini-voxeles** (8 por metro) hechos con MagicaVoxel, incluidos objetos de
  varias celdas (una puerta se coloca y se rompe entera).
- **Herramienta de construcción**: vara de selección, copiar / cortar / pegar con vista
  previa y giro, prefabs guardados en disco y deshacer.
- **Día y noche** llevados por el servidor: sol, luna con fases, cielo en degradado y
  **sombras en 3 cascadas** con penumbra suave. Panel para ajustar luz y colores en vivo
  (`F6`).
- **Jugador con física** (escalones, salto, vuelo), tres modos de juego y cámara en 1.ª
  y 3.ª persona, al hombro, más una vista de mapa de hasta 3,2 km.
- **Interfaz con Iced**: portada, mundos, ajustes, pausa, HUD, inventario creativo,
  consola con comandos del cliente y del servidor.
- **Guardado mínimo**: solo se guardan los chunks modificados; el resto se regenera.
- Pensado para rendir en cualquier PC: solo usa funciones *downlevel* de wgpu, para poder
  llevarlo a OpenGL / GLES y móvil más adelante.

## Requisitos

- [Rust](https://rustup.rs/) estable (edición 2021).
- Una GPU con soporte de Vulkan (por ahora es el único backend activado).
- Python 3, solo para las herramientas de `tools/` (convertir capturas, generar modelos).

## Compilar y jugar

```bash
cargo run --release                              # abrir el juego en el menú principal
cargo run --release -- --world NOMBRE            # abrir o crear ese mundo directamente
cargo run --release -- --world NOMBRE --host     # además, abrir la partida a la red
cargo run --release -- --connect HOST[:PUERTO]   # unirse a un servidor
cargo run --release --bin tokikura-server        # servidor dedicado (--bind, --seed, --world, --content)
cargo run --release --bin tokikura-worldmap -- --watch   # mapa PNG del terreno, se redibuja al guardar
cargo test --workspace                           # tests
```

Puerto por defecto: **28700/UDP**. `--name NOMBRE` cambia el nombre del jugador.

Los mundos se guardan en `worlds/<nombre>/`, los ajustes del jugador en `settings.toml` y
los del cielo y la luz en `environment.toml` (todos fuera del control de versiones).

## Controles

| Tecla | Acción |
|---|---|
| `WASD` | Moverse |
| `Espacio` | Saltar (volando: subir; `Shift` baja) |
| `Ctrl` | Correr |
| Clic izquierdo / derecho / central | Romper / colocar / copiar el bloque apuntado |
| Rueda o `1`-`9` | Elegir ranura de la barra |
| `Q` | Tirar el ítem activo |
| `E` | Inventario creativo |
| `Esc` | Menú de pausa (y cerrar cualquier pantalla) |
| `º` o `F1` | Consola (`help` lista los comandos) |
| `F3` | Datos de depuración |
| `F5` | Cambiar de modo: supervivencia → creativo → espectador |
| `F6` | Panel de cielo y luz |
| `F7` | Editor de biomas |
| `V` | Alternar 1.ª / 3.ª persona |
| `Alt` + rueda | Acercar o alejar la cámara (más de 32 m: vista de mapa) |
| `P` | Menú de prefabs |

Con la vara de selección en la mano: clic izquierdo marca las esquinas y la altura,
arrastrar la manilla de una cara la mueve, `Ctrl+C` / `Ctrl+X` / `Ctrl+V` copian, cortan
y empiezan a colocar, `Supr` vacía la selección y `Ctrl+Z` deshace. Colocando: clic
derecho pone, `R` gira y `RePág` / `AvPág` suben y bajan.

## Estructura

```
content/                  paquetes de contenido (bloques, ítems, modelos, prefabs, terreno)
crates/
  kuravoxel-common/       datos compartidos: bloques, chunks, contenido, protocolo, QUIC, física
  kuravoxel-server/       servidor autoritativo: generación, mundo, streaming, guardado, comandos
  kuravoxel-client/       cliente: ventana, mallado, render (terreno, sombras, cielo), sesión
  kuravoxel-ui/           puente entre el motor e Iced (IcedLayer, trait Screen)
  tokikura/               el juego: pantallas, barra, herramientas, ajustes y binarios
tools/                    utilidades en Python (ppm2png.py, make_vox.py)
```

## Contenido y modding

Todo lo que define el mundo son archivos: un bloque es `content/<paquete>/blocks/<bloque>.toml`,
un ítem `items/<ítem>.toml`, un modelo `models/<modelo>.vox`, un prefab
`prefabs/<nombre>.toml` + `.bin`, y el terreno vive en `worldgen/` (`world.toml` y un
archivo por bioma). Un mod es otra carpeta dentro de `content/`. El servidor vigila la
carpeta y recarga los cambios sin reiniciar; un archivo con errores no rompe nada.

El formato completo está en [content/README.md](content/README.md).

## Documentación

[CLAUDE.md](CLAUDE.md) recoge la dirección acordada del proyecto, las decisiones de diseño,
la arquitectura en detalle, el estado actual (qué está hecho y verificado, qué falta) y las
variables de entorno de desarrollo (capturas automáticas, cámara fija, medición de GPU...).

## Estado

En desarrollo activo. Hecho: motor cliente-servidor, terreno lejano a 5 km, paquetes de
contenido, edición y guardado, modelos de mini-voxeles, prefabs y herramienta de
construcción, terreno por datos con editor visual de nodos, día y noche con sombras.

Siguiente en la hoja de ruta: prefabs en el generador (árboles, rocas, formaciones),
vaivén por viento, entidades animadas y, más adelante, scripting para mods en el
servidor y el backend OpenGL / móvil.
