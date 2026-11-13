//! Cliente del motor KuraVoxel: ventana, render, mundo local, red e interfaz.
//!
//! Esta biblioteca no es un juego. Un juego (como Tokikura) implementa [`app::Game`] y
//! llama a [`app::run`]; el motor pone la ventana, el bucle, el renderer y las partidas.
//!
//! Piezas, de abajo arriba, cada una usable sin las de encima:
//!
//! - [`render`]: dibuja lo que le dan. No sabe de red ni de interfaz.
//! - [`world`] + [`mesher`]: copia local del mundo y su mallado.
//! - [`net`]: conexión con un servidor (integrado o remoto).
//! - [`player`]: el jugador local: movimiento con colisiones y cámara en 1.ª/3.ª persona.
//! - [`session`]: una partida en curso; une las anteriores.
//! - [`app`]: ventana y bucle principal; reparte la entrada entre el juego y la partida.
//!
//! El motor no depende de ninguna biblioteca de interfaz. Ofrece al juego dos ganchos
//! ([`app::Game::window_event`] y [`app::Game::draw_overlay`]) y el acceso al dispositivo
//! gráfico ([`render::Gpu`]); con eso, el crate `kuravoxel-ui` dibuja interfaces de Iced
//! encima de la escena.

pub mod app;
pub mod camera;
pub mod icons;
pub mod mesher;
pub mod net;
pub mod player;
pub mod render;
pub mod session;
pub mod world;

/// Tipos de terceros que aparecen en las firmas públicas del motor, para que quien
/// implemente sus traits use exactamente las mismas versiones.
pub mod reexports {
    pub use wgpu;
    pub use winit;
}
