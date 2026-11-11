# Contenido

Cada carpeta es un **paquete de contenido**. El servidor los carga todos al arrancar (por
orden alfabético, con `base` siempre primero), se los envía a los clientes y los vuelve a
cargar solo cuando cambia algún archivo: no hace falta reiniciar ni recompilar.

```
content/
  base/
    pack.toml           nombre y descripción del paquete
    blocks/
      grass.toml        un archivo por bloque
```

## Bloques

El identificador de un bloque es `paquete:archivo`; `content/base/blocks/grass.toml` es
`base:grass`.

```toml
name = "Hierba"        # nombre visible
color = "#3ACB3C"      # color plano, #RRGGBB
```

Campos opcionales:

- `id = "base:grass"`: usa este identificador en vez del que sale de la ruta. Sirve para
  que un paquete sustituya un bloque de otro (gana el paquete cargado más tarde).
- `model = "nombre"`: reservado para objetos de mini-voxeles (8 por metro, 12,5 cm).
  Todavía no se dibujan; el bloque se muestra como un cubo completo.

Límite actual: 255 bloques en total.

El generador de terreno necesita que existan estos bloques: `base:grass`, `base:dirt`,
`base:stone`, `base:sand`, `base:water`, `base:snow`, `base:wood`, `base:leaves_yellow`,
`base:leaves_lime`, `base:leaves_orange` y `base:leaves_green`.
